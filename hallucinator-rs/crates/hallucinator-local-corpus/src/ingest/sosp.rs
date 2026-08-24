//! Parse SOSP's (ACM SIGOPS Symposium on Operating Systems Principles)
//! accepted-papers page into corpus records.
//!
//! Hosted on sigops.org (not ACM's own site), one static page per year at
//! `sigops.org/s/conferences/sosp/<year>/accepted.html`. Structure:
//! `ul.paperlist` → `li` → `b` (title) → `br` → `em` (authors, the same
//! `"Name (Affiliation), Name (Affiliation)"` shape
//! [`parse_paren_grouped_names`] already handles — trailing entries in a
//! shared affiliation group without their own parenthesized affiliation,
//! e.g. `"Yanyan Shen, Linpeng Huang, Hong Mei (Shanghai Jiao Tong
//! University)"`, still parse fine since the helper doesn't require every
//! name to have its own affiliation).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse a SOSP accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"sosp2025"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let list_sel = Selector::parse("ul.paperlist").unwrap();
    let item_sel = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("b").unwrap();
    let authors_sel = Selector::parse("em").unwrap();

    let mut out = Vec::new();
    for list in document.select(&list_sel) {
        for item in list.select(&item_sel) {
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
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <ul class="paperlist">
        <li><b>Rearchitecting the Thread Model of In-Memory Key-Value Stores with &mu;TPS</b><br>
        <em>Youmin Chen (Shanghai Jiao Tong University), Jiwu Shu (Tsinghua University), Yanyan Shen, Linpeng Huang, Hong Mei (Shanghai Jiao Tong University) </em></li>

        <li><b>Device-Assisted Live Migration of RDMA Devices</b><br>
        <em>Artem Y. Polyakov, Gal Shalom, Asaf Schwartz, Aviad Yehezkel, Omri Ben David, Omri Kahalon, Ariel Shahar, Liran Liss (NVIDIA Corporation) </em></li>
        </ul>
    "#;

    #[test]
    fn test_parse_two_entries() {
        let out = parse_accepted_papers(SAMPLE, "sosp2025");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Rearchitecting the Thread Model of In-Memory Key-Value Stores with μTPS"
        );
        assert_eq!(out[0].source, "sosp2025");
        assert!(out[0].authors.contains(&"Youmin Chen".to_string()));
        assert!(out[0].authors.contains(&"Hong Mei".to_string()));
    }

    #[test]
    fn test_shared_affiliation_group_all_captured() {
        let out = parse_accepted_papers(SAMPLE, "sosp2025");
        // Trailing names in the first entry share the last parenthesized
        // affiliation rather than each having their own — all 5 names
        // must still come through.
        assert_eq!(out[0].authors.len(), 5);
    }

    #[test]
    fn test_second_entry() {
        let out = parse_accepted_papers(SAMPLE, "sosp2025");
        assert_eq!(
            out[1].title,
            "Device-Assisted Live Migration of RDMA Devices"
        );
        assert_eq!(out[1].authors.len(), 8);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "sosp2025").is_empty());
    }
}
