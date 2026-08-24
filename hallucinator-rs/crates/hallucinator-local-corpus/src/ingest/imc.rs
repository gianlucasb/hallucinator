//! Parse ACM IMC's (Internet Measurement Conference) accepted-papers page
//! into corpus records.
//!
//! One page per year at `conferences.sigcomm.org/imc/<year>/accepted-papers/`
//! — same host as SIGCOMM's own accepted-papers page, but a different,
//! much plainer template: `li` → `strong` (title) + direct text following
//! it (authors). Unlike every other venue here, IMC's author line carries
//! **no parenthesized affiliations at all** — just a flat comma-separated
//! name list (`"Name, Name, Name"`), so [`parse_paren_grouped_names`]
//! (which requires at least one `(...)` group to anchor on) would return
//! nothing; this splits on comma directly instead, the same approach
//! [`super::icml`] uses for the same reason.

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::dom_text::direct_text;

/// Parse an IMC accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"imc2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("li").unwrap();
    let title_sel = Selector::parse("strong").unwrap();

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
        let authors: Vec<String> = authors_text
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
        <h2 id=cycle-1>Cycle 1</h2>
        <ul>
        <li><strong>Exploration of the Dynamics of Buy and Sale of Social Media Accounts</strong><br>Mario Beluri, Bhupendra Acharya, Thorsten Holz</li>
        <li><strong>Sibling Prefixes: Identifying Similarities in IPv4 and IPv6 Prefixes</strong><br>Fariba Osali, Khwaja Zubair Sediqi, Oliver Gasser</li>
        </ul>
        <h2 id=cycle-2>Cycle 2</h2>
        <ul>
        <li><strong>A Cycle-2 Paper</strong><br>Solo Author</li>
        </ul>
    "#;

    #[test]
    fn test_title_and_flat_author_list_no_affiliations() {
        let out = parse_accepted_papers(SAMPLE, "imc2026");
        assert_eq!(
            out[0].title,
            "Exploration of the Dynamics of Buy and Sale of Social Media Accounts"
        );
        assert_eq!(
            out[0].authors,
            vec!["Mario Beluri", "Bhupendra Acharya", "Thorsten Holz"]
        );
        assert_eq!(out[0].source, "imc2026");
    }

    #[test]
    fn test_both_cycles_picked_up() {
        let out = parse_accepted_papers(SAMPLE, "imc2026");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].title, "A Cycle-2 Paper");
        assert_eq!(out[2].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "imc2026").is_empty());
    }
}
