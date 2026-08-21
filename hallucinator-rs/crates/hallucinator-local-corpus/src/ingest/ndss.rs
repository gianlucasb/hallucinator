//! Parse NDSS's "Accepted Papers" listing page into corpus records.
//!
//! NDSS publishes no BibTeX/CSV/JSON export in any era checked — the only
//! machine-readable source is the HTML listing at
//! `ndss-symposium.org/ndssYYYY/accepted-papers/`. That page has gone
//! through three structurally distinct eras over the last decade (checked
//! 2016–2026); this module handles two of them:
//!
//! - **2020–2026** (`parse_modern`): WordPress "Content Views" plugin —
//!   `div.pt-cv-content-item`, title + link in `h2.pt-cv-title a`, authors
//!   with inline affiliations in `.pt-cv-ctf-value p`.
//! - **2016–2018** (`parse_legacy`): Gutenberg block markup —
//!   `div.single-paper`, title (no link) in `h3.wp-block-heading`, plain
//!   comma-separated author names with no affiliations, and the only link
//!   present goes straight to the paper's PDF rather than an abstract page.
//!
//! **2019 is intentionally not supported.** That year uses a third format
//! (`div.tag-box.rel-paper`) where most titles are truncated with "…" in
//! the listing itself — the full title only exists on each paper's own
//! detail page, requiring a second fetch per paper (~89 extra requests
//! for one year). Storing the truncated titles as-is would mostly defeat
//! the point: they're too short to clear the 95% fuzzy-match threshold
//! against a real citation's full title, so they'd sit in the corpus
//! contributing nothing. Given that cost for one year's ~89 papers,
//! skipping it was the agreed tradeoff — see conversation history, not
//! implemented here.
//!
//! [`parse_accepted_papers`] tries the modern selector first and falls
//! back to the legacy one only if that yields nothing, so callers don't
//! need to know which era a given year's page uses.

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse an NDSS accepted-papers page into publication records, trying
/// the 2020–2026 format first and falling back to the 2016–2018 format.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"ndss2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let modern = parse_modern(html, source_tag);
    if !modern.is_empty() {
        return modern;
    }
    parse_legacy(html, source_tag)
}

/// 2020–2026: WordPress "Content Views" plugin markup.
fn parse_modern(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("div.pt-cv-content-item").unwrap();
    let title_sel = Selector::parse("h2.pt-cv-title a").unwrap();
    let authors_sel = Selector::parse(".pt-cv-ctf-value p").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = title_el.value().attr("href").map(|s| s.to_string());

        let authors_text: String = item
            .select(&authors_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default();
        let authors = parse_paren_grouped_names(&authors_text);

        out.push(NewPublication {
            title,
            authors,
            url,
            source: source_tag.to_string(),
        });
    }
    out
}

/// 2016–2018: Gutenberg block markup. No affiliations (plain comma-
/// separated names) and the only link goes straight to the PDF.
fn parse_legacy(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("div.single-paper").unwrap();
    let title_sel = Selector::parse("h3.wp-block-heading").unwrap();
    let authors_sel = Selector::parse("p.wp-block-paragraph").unwrap();
    let link_sel = Selector::parse("p.paper-link-abs a").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let url = item
            .select(&link_sel)
            .next()
            .and_then(|a| a.value().attr("href"))
            .map(|s| s.to_string());

        let authors: Vec<String> = item
            .select(&authors_sel)
            .next()
            .map(|el| el.text().collect::<String>())
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();

        out.push(NewPublication {
            title,
            authors,
            url,
            source: source_tag.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODERN_SAMPLE: &str = r#"
        <div class="pt-cv-wrapper">
        <div class=" pt-cv-content-item pt-cv-2-col" data-pid="23756">
            <h2 class="pt-cv-title"><a href="https://www.ndss-symposium.org/ndss-paper/a-causal-perspective/" class="_self" target="_self">A Causal Perspective for Enhancing Jailbreak Attack and Defense</a></h2>
            <div class="pt-cv-ctf-list"><div class="pt-cv-custom-fields pt-cv-ctf-display_authors"><div class="pt-cv-ctf-value"><p>Licheng Pan (Zhejiang University), Yunsheng Lu (University of Chicago), Jiexi Liu (Alibaba Group)</p></div></div></div>
        </div>
        <div class=" pt-cv-content-item pt-cv-2-col" data-pid="23637">
            <h2 class="pt-cv-title"><a href="https://www.ndss-symposium.org/ndss-paper/a-hard-label-attack/" class="_self" target="_self">A Hard-Label Black-Box Evasion Attack against ML-based Malicious Traffic Detection Systems</a></h2>
            <div class="pt-cv-ctf-list"><div class="pt-cv-custom-fields pt-cv-ctf-display_authors"><div class="pt-cv-ctf-value"><p>Zixuan Liu (Tsinghua University), Qi Li (Tsinghua University and Zhongguancun Lab)</p></div></div></div>
        </div>
        </div>
    "#;

    const LEGACY_SAMPLE: &str = r#"
        <div class="wp-block-group single-paper is-layout-flow wp-block-group-is-layout-flow advgb-dyn-0151a3e4"><div class="wp-block-group__inner-container">
        <div class="wp-block-group inner-wrap is-layout-flow wp-block-group-is-layout-flow advgb-dyn-8e22d13c"><div class="wp-block-group__inner-container">
        <h3 class="wp-block-heading advgb-dyn-5d3a12df">IoTFuzzer: Discovering Memory Corruptions in IoT Through App-based Fuzzing</h3>
        <p class="wp-block-paragraph">Jiongyi Chen, Wenrui Diao, Qingchuan Zhao</p>
        </div></div>
        <p class="paper-link-abs wp-block-paragraph"><strong><a href="https://www.ndss-symposium.org/wp-content/uploads/2018/02/ndss2018_01A-1_Chen_paper.pdf">Read More</a></strong></p>
        </div></div>
    "#;

    #[test]
    fn test_parse_modern_two_papers() {
        let out = parse_accepted_papers(MODERN_SAMPLE, "ndss2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "A Causal Perspective for Enhancing Jailbreak Attack and Defense"
        );
        assert_eq!(
            out[0].authors,
            vec!["Licheng Pan", "Yunsheng Lu", "Jiexi Liu"]
        );
    }

    #[test]
    fn test_parse_legacy_falls_back_when_modern_selector_finds_nothing() {
        let out = parse_accepted_papers(LEGACY_SAMPLE, "ndss2018");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "IoTFuzzer: Discovering Memory Corruptions in IoT Through App-based Fuzzing"
        );
        assert_eq!(
            out[0].authors,
            vec!["Jiongyi Chen", "Wenrui Diao", "Qingchuan Zhao"]
        );
        assert_eq!(
            out[0].url.as_deref(),
            Some(
                "https://www.ndss-symposium.org/wp-content/uploads/2018/02/ndss2018_01A-1_Chen_paper.pdf"
            )
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "ndss2026").is_empty());
    }
}
