//! Parse a WWW ("The Web Conference") accepted-papers track page into
//! corpus records.
//!
//! One page per track per year, e.g.
//! `www<year>.thewebconf.org/accepted/research-tracks.html` (separate
//! pages exist for industry/short-papers/etc — call this once per page
//! you want indexed). Structure: `li` → `span.paper-id` (a submission ID
//! we don't need, e.g. `"(rfp0110)"`) + direct text (the title, followed
//! by a trailing em dash separator) + `span.paper-authors` (a flat
//! comma-and-"and"-separated name list — `"Name, Name and Name"`, **no**
//! parenthesized affiliations, unlike most venues here, so this splits
//! on comma/" and " directly rather than using
//! [`super::author_parsing::parse_paren_grouped_names`], the same
//! approach [`super::icml`] and [`super::imc`] use for the same reason).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::dom_text::direct_text;

/// Parse a WWW accepted-papers track page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"www2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("li").unwrap();
    let authors_sel = Selector::parse("span.paper-authors").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(authors_el) = item.select(&authors_sel).next() else {
            continue;
        };

        let raw_title = direct_text(&item);
        let title = raw_title
            .trim_end_matches(['\u{2014}', ' ']) // trailing " — " separator
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }

        let authors_text: String = authors_el.text().collect();
        let authors: Vec<String> = authors_text
            .replace(" and ", ", ")
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
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
        <ul>
        <li><span class="paper-id">(rfp0110)</span> Opinion Dynamics with Multiple Adversaries — <span class="paper-authors">Akhil Jalan and Marios Papachristou</span></li>
        <li><span class="paper-id">(rfp0366)</span> Auto-bidding under Return-on-Spend Constraints — <span class="paper-authors">Jiale Han, Chun Gan, Zhangang Lin, Ching Law and Xiaowu Dai</span></li>
        <li><span class="paper-id">(rfp0434)</span> Deterring A Small Collusion is All You Need — <span class="paper-authors">Yotam Gafni</span></li>
        </ul>
    "#;

    #[test]
    fn test_title_excludes_paper_id_and_trailing_dash() {
        let out = parse_accepted_papers(SAMPLE, "www2026");
        assert_eq!(out[0].title, "Opinion Dynamics with Multiple Adversaries");
        assert_eq!(out[0].authors, vec!["Akhil Jalan", "Marios Papachristou"]);
        assert_eq!(out[0].source, "www2026");
    }

    #[test]
    fn test_oxford_less_and_before_last_author() {
        let out = parse_accepted_papers(SAMPLE, "www2026");
        assert_eq!(
            out[1].authors,
            vec![
                "Jiale Han",
                "Chun Gan",
                "Zhangang Lin",
                "Ching Law",
                "Xiaowu Dai"
            ]
        );
    }

    #[test]
    fn test_solo_author() {
        let out = parse_accepted_papers(SAMPLE, "www2026");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].authors, vec!["Yotam Gafni"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "www2026").is_empty());
    }
}
