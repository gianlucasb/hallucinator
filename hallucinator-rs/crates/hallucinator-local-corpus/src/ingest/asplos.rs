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

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse an ASPLOS program page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"asplos2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
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
}
