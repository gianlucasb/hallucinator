//! Parse ACM SIGCOMM's accepted-papers page into corpus records.
//!
//! One page per year at `conferences.sigcomm.org/sigcomm/<year>/accepted-papers/`,
//! covering both "Full papers" and "Short papers" sections identically.
//! Structure: `li` → `p > span.text-color-primary` (title) +
//! `p.style_italic` (authors, `"Name (Affiliation); Name, Name
//! (Affiliation); ..."` — the same shape [`parse_paren_grouped_names`]
//! already handles).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse a SIGCOMM accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"sigcomm2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
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
}
