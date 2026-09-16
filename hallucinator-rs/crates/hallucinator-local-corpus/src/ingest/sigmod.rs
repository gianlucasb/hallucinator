//! Parse the SIGMOD accepted-papers page into corpus records.
//!
//! One page per year at `<year>.sigmod.org/sigmod_papers.shtml`, listing
//! every PACMMOD submission round's papers directly (no need to follow
//! out to the separate ACM DL table-of-contents page per round).
//! Structure: `li` → `b` (title) followed by direct text (authors:
//! `"Name (Affiliation); Name (Affiliation)*; ..."` — the same shape
//! [`parse_paren_grouped_names`] already handles, except for the
//! trailing `*` SIGMOD marks the corresponding/presenting author with,
//! which sits *outside* the parenthesized affiliation and would
//! otherwise leak into the next name — stripped before parsing).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;
use crate::ingest::dom_text::direct_text;

/// Parse a SIGMOD accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"sigmod2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("b").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let authors_text = direct_text(&item).replace('*', "");
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
        <h3>Round 1</h3>
        <ul>
        <li><b>SWIFT: Enabling Large-Scale Temporal Graph Learning on a Single Machine</b><br>Rui Guo (University of Science and Technology of China); Zezhong Ding (University of Science and Technology of China); Xike Xie (University of Science and Technology of China)*; Jianliang Xu (Hong Kong Baptist University)</li>
        <li><b>A Solo-Author Paper</b><br>Solo Author (Some University)*</li>
        </ul>
    "#;

    #[test]
    fn test_trailing_asterisk_does_not_leak_into_next_name() {
        let out = parse_accepted_papers(SAMPLE, "sigmod2026");
        assert_eq!(
            out[0].title,
            "SWIFT: Enabling Large-Scale Temporal Graph Learning on a Single Machine"
        );
        assert_eq!(
            out[0].authors,
            vec!["Rui Guo", "Zezhong Ding", "Xike Xie", "Jianliang Xu"]
        );
        assert_eq!(out[0].source, "sigmod2026");
    }

    #[test]
    fn test_trailing_asterisk_on_last_author_stripped() {
        let out = parse_accepted_papers(SAMPLE, "sigmod2026");
        assert_eq!(out[1].title, "A Solo-Author Paper");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "sigmod2026").is_empty());
    }
}
