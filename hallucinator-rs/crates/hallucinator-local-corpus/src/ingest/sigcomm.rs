//! Parse ACM SIGCOMM's accepted-papers page into corpus records.
//!
//! Three templates, tried in order:
//!
//! - **Current (2025+)**, one page per year at
//!   `conferences.sigcomm.org/sigcomm/<year>/accepted-papers/`, covering
//!   both "Full papers" and "Short papers" sections identically.
//!   Structure: `li` → `p > span.text-color-primary` (title) +
//!   `p.style_italic` (authors, `"Name (Affiliation); Name, Name
//!   (Affiliation); ..."` — the same shape [`parse_paren_grouped_names`]
//!   already handles).
//! - **2024's Next.js template**, `.../2024/accepted-papers/`: `li` →
//!   `p.font-bold` (title as a direct text node, followed by a nested
//!   `<span>` track badge like "Research Track" that must *not* be
//!   included) + `p.text-base` (authors, same `"Name (Affiliation); ..."`
//!   shape as the current template).
//! - **Legacy (pre-2024) jQuery-Mobile listing**, e.g.
//!   `conferences.sigcomm.org/sigcomm/2018/accepted-papers.html` or
//!   `.../2023/list-accepted.html` — these are dedicated
//!   accepted-papers-only listings (not the full multi-day program page,
//!   which mixes in workshops/breaks/keynotes too unpredictably to parse
//!   safely). Structure: `li.prog-item` → `p.paper-header` or `h2`
//!   (title) + a plain sibling `p` containing `"Name, Name
//!   (Affiliation), ..."` (again handled by
//!   [`parse_paren_grouped_names`]). Note some years (2019's
//!   `accepted-papers.html` and all of 2020-2022) don't statically render
//!   a papers list at all — server response has an empty/absent list and
//!   this parser correctly yields nothing for those, rather than
//!   guessing.

use scraper::{ElementRef, Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;
use crate::ingest::dom_text::direct_text;

/// Parse a SIGCOMM accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"sigcomm2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let mut out = parse_current_template(&document, source_tag);
    if out.is_empty() {
        out = parse_2024_template(&document, source_tag);
    }
    if out.is_empty() {
        out = parse_legacy_template(&document, source_tag);
    }
    out
}

/// Current (2025+) template: `li` → `span.text-color-primary` (title) +
/// `p.style_italic` (authors).
fn parse_current_template(document: &Html, source_tag: &str) -> Vec<NewPublication> {
    let item_sel = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("span.text-color-primary").unwrap();
    let authors_sel = Selector::parse("p.style_italic").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let Some(authors_el) = item.select(&authors_sel).next() else {
            continue;
        };
        let authors_text: String = authors_el.text().collect();
        let authors = parse_paren_grouped_names(&authors_text);
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

/// 2024's Next.js template: `li` → `p.font-bold` (title as a direct text
/// node — a sibling `<span>` track badge like "Research Track" must be
/// excluded) + `p.text-base` (authors).
fn parse_2024_template(document: &Html, source_tag: &str) -> Vec<NewPublication> {
    let item_sel = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("p.font-bold").unwrap();
    let authors_sel = Selector::parse("p.text-base").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title = direct_text(&title_el);
        if title.is_empty() {
            continue;
        }

        let Some(authors_el) = item.select(&authors_sel).next() else {
            continue;
        };
        let authors_text: String = authors_el.text().collect();
        let authors = parse_paren_grouped_names(&authors_text);
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

/// Legacy (pre-2024) jQuery-Mobile listing template: `li.prog-item` →
/// `p.paper-header` or `h2` (title) + a plain sibling `p` (authors).
fn parse_legacy_template(document: &Html, source_tag: &str) -> Vec<NewPublication> {
    let item_sel = Selector::parse("li.prog-item").unwrap();
    let paper_header_sel = Selector::parse("p.paper-header").unwrap();
    let h2_sel = Selector::parse("h2").unwrap();
    let p_sel = Selector::parse("p").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let title = item
            .select(&paper_header_sel)
            .next()
            .or_else(|| item.select(&h2_sel).next())
            .map(|el| normalize_whitespace(&el.text().collect::<String>()))
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }

        // The author line is the first plain (unclassed) `p` in the item
        // that looks like a `"Name (Affiliation), ..."` list — skipping
        // `p.paper-header` itself and anything without a parenthesized
        // affiliation (badges, links, session metadata, etc.). These
        // pages hard-wrap long entries across lines, so the raw text can
        // carry newlines/indentation mid-name; collapse it before
        // splitting on names.
        let authors_text = item
            .select(&p_sel)
            .find(|el| !is_paper_header(el) && el.text().collect::<String>().contains('('))
            .map(|el| normalize_whitespace(&el.text().collect::<String>()));
        let Some(authors_text) = authors_text else {
            continue;
        };
        let authors = parse_paren_grouped_names(&authors_text);
        if authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

fn is_paper_header(el: &ElementRef) -> bool {
    el.value()
        .attr("class")
        .is_some_and(|c| c.split_whitespace().any(|t| t == "paper-header"))
}

/// Collapse runs of whitespace (including the newlines/indentation these
/// hard-wrapped legacy pages embed mid-title/mid-name) into single spaces,
/// and trim the ends.
fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <h2>Full papers</h2>
        <ul>
            <li>
                <p><span class="text-color-primary">Nezha: SmartNIC-based Virtual Switch Load Sharing</span></p>
                <p class="style_italic">Xing Li (Zhejiang University and Alibaba Cloud); Enge Song, Bowen Yang (Alibaba Cloud)</p>
            </li>
        </ul>
        <h2>Short papers</h2>
        <ul>
            <li>
                <p><span class="text-color-primary">A Short Paper Title</span></p>
                <p class="style_italic">Solo Author (Some University)</p>
            </li>
        </ul>
    "#;

    // 2024's Next.js template: `p.font-bold` title with a nested track
    // badge `<span>` that must not leak into the title, plus a React
    // hydration comment between the title text and the badge.
    const NEXTJS_2024_SAMPLE: &str = r#"
        <ul class="list-disc pb-2 pl-4 text-left">
            <li>
                <p class="font-bold mb-0 text-lg">In-Network Address Caching for Virtual Networks<!-- --> <span class="font-semibold inline-flex items-center rounded-md text-base bg-green-100 px-1 text-green-700">Research Track</span></p>
                <p class="text-base">Lior Zeno (Technion); Ang Chen (University of Michigan); Mark Silberstein (Technion)</p>
            </li>
        </ul>
    "#;

    #[test]
    fn test_2024_nextjs_template() {
        let out = parse_accepted_papers(NEXTJS_2024_SAMPLE, "sigcomm2024");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "In-Network Address Caching for Virtual Networks"
        );
        assert_eq!(
            out[0].authors,
            vec!["Lior Zeno", "Ang Chen", "Mark Silberstein"]
        );
        assert_eq!(out[0].source, "sigcomm2024");
    }

    #[test]
    fn test_full_papers_section() {
        let out = parse_accepted_papers(SAMPLE, "sigcomm2026");
        assert_eq!(
            out[0].title,
            "Nezha: SmartNIC-based Virtual Switch Load Sharing"
        );
        assert_eq!(out[0].authors, vec!["Xing Li", "Enge Song", "Bowen Yang"]);
        assert_eq!(out[0].source, "sigcomm2026");
    }

    #[test]
    fn test_short_papers_section_also_picked_up() {
        let out = parse_accepted_papers(SAMPLE, "sigcomm2026");
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].title, "A Short Paper Title");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "sigcomm2026").is_empty());
    }

    // 2018-style: `p.paper-header` title + plain `p` authors with `<em>`
    // affiliations.
    const LEGACY_PAPER_HEADER_SAMPLE: &str = r#"
        <ul>
            <li data-icon="false" class="prog-item ">
                <table><tr><td style="width:100%;text-align:left;">
                    <p class="paper-header">
                        Elastic Sketch: Adaptive and Fast Network-wide Measurements
                    </p>
                    <p>Tong Yang, Jie Jiang, Peng Liu <em>(PKU, China)</em>, Qun Huang <em>(CAS, China)</em></p>
                </td></tr></table>
            </li>
        </ul>
    "#;

    // 2023-style: `h2` title + plain `p` authors, no `<em>`.
    const LEGACY_H2_SAMPLE: &str = r#"
        <ul>
            <li data-icon="false" class="prog-item prog-friday">
                <div style="width: 100%">
                    <p class="keynote-header"></p>
                    <table><tr><td style="width: 100%; text-align: left">
                        <h2>A Formal Framework for End-to-End DNS Resolution</h2>
                        <p>Si Liu, Huayi Duan, and David Basin (ETH Zurich)</p>
                    </td></tr></table>
                </div>
            </li>
        </ul>
    "#;

    #[test]
    fn test_legacy_paper_header_template() {
        let out = parse_accepted_papers(LEGACY_PAPER_HEADER_SAMPLE, "sigcomm2018");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "Elastic Sketch: Adaptive and Fast Network-wide Measurements"
        );
        assert_eq!(
            out[0].authors,
            vec!["Tong Yang", "Jie Jiang", "Peng Liu", "Qun Huang"]
        );
        assert_eq!(out[0].source, "sigcomm2018");
    }

    #[test]
    fn test_legacy_h2_template() {
        let out = parse_accepted_papers(LEGACY_H2_SAMPLE, "sigcomm2023");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "A Formal Framework for End-to-End DNS Resolution"
        );
        assert_eq!(out[0].authors, vec!["Si Liu", "Huayi Duan", "David Basin"]);
    }

    #[test]
    fn test_legacy_template_skips_non_paper_prog_items() {
        // A session/workshop-announcement item with no author line at all
        // shouldn't be picked up as a paper.
        let html = r#"
            <ul>
                <li class="prog-item ">
                    <p class="paper-header"><strong>NAI'21: Workshop on Network-Application Integration</strong></p>
                    <p>(<a href="details.html">Details</a>)</p>
                </li>
            </ul>
        "#;
        assert!(parse_accepted_papers(html, "sigcomm2021").is_empty());
    }

    #[test]
    fn test_legacy_h2_template_collapses_hard_wrapped_whitespace() {
        // Regression: 2023's page hard-wraps long titles/names across
        // lines with deep indentation, which must collapse to single
        // spaces rather than leaking newlines/runs of spaces into the
        // stored title/author names.
        let html = "
            <ul>
                <li class=\"prog-item prog-friday\">
                    <h2>A Millimeter Wave Backscatter Network for Two-Way
                    Communication and Localization</h2>
                    <p>Haofan Lu, Mohammad Hossein Mazaheri, Reza Rezvani, Omid
                    Abari (UCLA)</p>
                </li>
            </ul>
        ";
        let out = parse_accepted_papers(html, "sigcomm2023");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "A Millimeter Wave Backscatter Network for Two-Way Communication and Localization"
        );
        assert_eq!(
            out[0].authors,
            vec![
                "Haofan Lu",
                "Mohammad Hossein Mazaheri",
                "Reza Rezvani",
                "Omid Abari"
            ]
        );
    }
}
