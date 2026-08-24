//! Parse the DSN (Dependable Systems and Networks) accepted-papers page
//! into corpus records.
//!
//! One page per year at `dsn<year>.github.io/cpaccepted.html`.
//! Structure: `li.w3-padding-large` → `b` (title, an element — so it's
//! excluded from the `<li>`'s own direct text) followed by direct text
//! (authors: `"Name, Name (Affiliation); Name (Affiliation); ..."` — the
//! same shape [`parse_paren_grouped_names`] already handles).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;
use crate::ingest::dom_text::direct_text;

/// Parse a DSN accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"dsn2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("li.w3-padding-large").unwrap();
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

        let authors_text = direct_text(&item);
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
        <ul>
        <li class="w3-padding-large"><b>5G-STREAM: 5G Service Mesh Tailored for Reliable, Efficient and Authorized Microservices in the Cloud</b><br>Tolga Atalay, Alireza Famili, Sudip Maitra (Virginia Tech); Dragoslav Stojadinovic (Kryptowire LLC); Angelos Stavrou, Haining Wang (Virginia Tech)<br></li>
        <li class="w3-padding-large"><b>A Solo-Author DSN Paper</b><br>Solo Author (Some University)<br></li>
        </ul>
    "#;

    #[test]
    fn test_title_and_multi_group_authors() {
        let out = parse_accepted_papers(SAMPLE, "dsn2026");
        assert_eq!(
            out[0].title,
            "5G-STREAM: 5G Service Mesh Tailored for Reliable, Efficient and Authorized Microservices in the Cloud"
        );
        assert_eq!(
            out[0].authors,
            vec![
                "Tolga Atalay",
                "Alireza Famili",
                "Sudip Maitra",
                "Dragoslav Stojadinovic",
                "Angelos Stavrou",
                "Haining Wang"
            ]
        );
        assert_eq!(out[0].source, "dsn2026");
    }

    #[test]
    fn test_second_entry() {
        let out = parse_accepted_papers(SAMPLE, "dsn2026");
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].title, "A Solo-Author DSN Paper");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "dsn2026").is_empty());
    }
}
