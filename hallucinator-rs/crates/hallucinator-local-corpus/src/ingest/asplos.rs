//! Parse ASPLOS's (ACM International Conference on Architectural Support
//! for Programming Languages and Operating Systems) program page into
//! corpus records.
//!
//! One page per year at `asplos-conference.org/asplos<year>/program/`,
//! WordPress-hosted with a session-by-session schedule. Structure:
//! `div.paper` → `div.paper-title` (plain text) + `div.paper-authors`
//! (plain `"Name (Affiliation), Name (Affiliation)"` text — the same
//! shape [`parse_paren_grouped_names`] already handles). Papers appear
//! nested inside collapsible per-session panels, but selecting `div.paper`
//! directly (rather than walking the session structure) picks up every
//! paper regardless of which session panel it's under.
//!
//! ISCA reuses this same template (`iscaconf.org/isca<year>/program/`),
//! but its `div.paper-authors` text drops affiliations entirely on the
//! 2023-2025 pages (`"Name, Name, Name"`, no parens at all — only the
//! 2026 page includes them), so authors are parsed with
//! [`parse_names_maybe_with_affiliations`], which falls back to a plain
//! comma split when no `(` is present.
//!
//! ASPLOS 2024's page (`asplos-conference.org/asplos2024/main-program/`)
//! predates this `div.paper` template and uses a WordPress Gutenberg
//! table instead: `table tbody td` → one direct-child `<strong>` (title —
//! a `[Best Paper]` badge, if present, is wrapped in its own `<span>` so
//! it isn't a direct child and is skipped automatically) → raw HTML up to
//! the next `<a` link (the "Paper . Abstract . Lightning Talk" row) holds
//! `"Name (Affiliation); Name (Affiliation)"` text, tags-and-all — same
//! shape [`parse_paren_grouped_names`] handles once stripped. Tried as a
//! fallback in [`parse_wp_table_template`] when the `div.paper` template
//! finds nothing.

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::{
    parse_names_maybe_with_affiliations, parse_paren_grouped_names,
};

/// Parse an ASPLOS program page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"asplos2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let out = parse_paper_div_template(html, source_tag);
    if !out.is_empty() {
        return out;
    }
    parse_wp_table_template(html, source_tag)
}

/// Current/ISCA-shared template: `div.paper` → `div.paper-title` +
/// `div.paper-authors`.
fn parse_paper_div_template(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let paper_sel = Selector::parse("div.paper").unwrap();
    let title_sel = Selector::parse("div.paper-title").unwrap();
    let authors_sel = Selector::parse("div.paper-authors").unwrap();

    let mut out = Vec::new();
    for paper in document.select(&paper_sel) {
        let Some(title_el) = paper.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let Some(authors_el) = paper.select(&authors_sel).next() else {
            continue;
        };
        let authors_text: String = authors_el.text().collect();
        let authors = parse_names_maybe_with_affiliations(&authors_text);
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

/// ASPLOS 2024's WordPress-table template: `table tbody td` → direct-child
/// `<strong>` (title) → `"Name (Affiliation); ..."` text up to the next
/// link.
fn parse_wp_table_template(html: &str, source_tag: &str) -> Vec<NewPublication> {
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

    let document = Html::parse_document(html);
    let td_sel = Selector::parse("td").unwrap();
    let title_sel = Selector::parse("td > strong").unwrap();

    let mut out = Vec::new();
    for td in document.select(&td_sel) {
        let Some(title_el) = td.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        // Locate the title element's own serialized HTML within the
        // td's HTML (rather than the first "</strong>", which could
        // belong to a preceding `[Best Paper]` badge) to correctly find
        // where the author text starts, then cut it off at the first
        // link — the "Paper . Abstract . Lightning Talk" row that
        // follows, which has no analog before this point since author
        // names here are never themselves hyperlinked.
        let td_html = td.html();
        let title_html = title_el.html();
        let Some(pos) = td_html.find(title_html.as_str()) else {
            continue;
        };
        let after_title = &td_html[pos + title_html.len()..];
        let authors_html = after_title
            .find("<a")
            .map_or(after_title, |i| &after_title[..i]);
        let plain = TAG_RE.replace_all(authors_html, " ");
        let authors = parse_paren_grouped_names(&plain);
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <div class="panel-body">
          <div class="paper">
            <div class="paper-title">
              Towards High-Goodput LLM Serving with Prefill-decode Multiplexing
            </div>
            <div class="paper-authors">
              Weihao Cui (Shanghai Jiao Tong University), Yukang Chen (Shanghai Jiao Tong University)
            </div>
          </div>
          <hr />
          <div class="paper">
            <div class="paper-title">
              Bullet: Boosting GPU Utilization for LLM Serving via Dynamic Spatial-Temporal Orchestration
            </div>
            <div class="paper-authors">
              Zejia Lin (Sun Yat-sen University), Hongxin Xu (Sun Yat-sen University)
            </div>
          </div>
        </div>
    "#;

    #[test]
    fn test_parse_two_papers() {
        let out = parse_accepted_papers(SAMPLE, "asplos2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Towards High-Goodput LLM Serving with Prefill-decode Multiplexing"
        );
        assert_eq!(out[0].authors, vec!["Weihao Cui", "Yukang Chen"]);
        assert_eq!(out[0].source, "asplos2026");
    }

    #[test]
    fn test_second_paper() {
        let out = parse_accepted_papers(SAMPLE, "asplos2026");
        assert_eq!(
            out[1].title,
            "Bullet: Boosting GPU Utilization for LLM Serving via Dynamic Spatial-Temporal Orchestration"
        );
        assert_eq!(out[1].authors, vec!["Zejia Lin", "Hongxin Xu"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "asplos2026").is_empty());
    }

    const ISCA_NO_AFFILIATIONS_SAMPLE: &str = r#"
        <div class="panel-body">
          <div class="paper">
            <div class="paper-title">
              A Systolic Array-Based Accelerator
            </div>
            <div class="paper-authors">
              Cong Guo, Jiaming Tang, Weiming Hu, Yuhao Zhu
            </div>
          </div>
        </div>
    "#;

    #[test]
    fn test_parse_isca_style_authors_without_affiliations() {
        // ISCA 2023-2025 pages reuse this template but drop affiliations
        // from div.paper-authors entirely.
        let out = parse_accepted_papers(ISCA_NO_AFFILIATIONS_SAMPLE, "isca2024");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].authors,
            vec!["Cong Guo", "Jiaming Tang", "Weiming Hu", "Yuhao Zhu"]
        );
    }

    const WP_TABLE_SAMPLE: &str = r##"
        <figure class="wp-block-table"><table><thead><tr><th>1A: Synthesis</th></tr></thead><tbody>
        <tr><td><strong>Explainable Port Mapping Inference with Sparse Performance Counters</strong><br><br>
        <span class="has-inline-color has-danger-color">Fabian Ritter</span> and Sebastian Hack <span style="color: gray; font-style: italic;">(Saarland University)</span><br><br>
        <a href="https://doi.org/10.1145/3620666.3651363">Paper</a> <strong>.</strong> <a href="abstracts/index.html#1A">Abstract</a></td></tr>
        <tr><td><span class="has-inline-color has-success-color"><strong>[Best Paper] </strong></span><strong>Centauri: Enabling Efficient Scheduling</strong><br><br>
        <span class="has-inline-color has-danger-color">Chang Chen</span>, Xiuhong Li, and <span class="has-inline-color has-danger-color">Qianchao Zhu</span> <span style="color: gray; font-style: italic;">(Peking University)</span>; Jiangfei Duan <span style="color: gray; font-style: italic;">(Chinese University of Hong Kong)</span><br><br>
        <a href="https://doi.org/10.1145/3620666.3651379">Paper</a> <strong>.</strong> <a href="abstracts/index.html#1B">Abstract</a></td></tr>
        </tbody></table></figure>
    "##;

    #[test]
    fn test_parse_wp_table_fallback_plain_title() {
        let out = parse_accepted_papers(WP_TABLE_SAMPLE, "asplos2024");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Explainable Port Mapping Inference with Sparse Performance Counters"
        );
        assert_eq!(out[0].authors, vec!["Fabian Ritter", "Sebastian Hack"]);
        assert_eq!(out[0].source, "asplos2024");
    }

    #[test]
    fn test_parse_wp_table_fallback_best_paper_badge_excluded_from_title() {
        let out = parse_accepted_papers(WP_TABLE_SAMPLE, "asplos2024");
        assert_eq!(out[1].title, "Centauri: Enabling Efficient Scheduling");
        assert!(!out[1].title.contains("Best Paper"));
        assert_eq!(
            out[1].authors,
            vec!["Chang Chen", "Xiuhong Li", "Qianchao Zhu", "Jiangfei Duan"]
        );
    }
}
