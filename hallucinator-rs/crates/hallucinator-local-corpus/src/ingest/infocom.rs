//! Parse the IEEE INFOCOM "Accepted Paper List" page into corpus records.
//!
//! Unlike every other venue here, INFOCOM's ComSoc "minisite" theme
//! doesn't wrap each paper in its own element at all — the entire list
//! lives inside one giant `<p>`, formatted as repeated
//! `<strong>N. Title</strong><br>Author, Author and Author
//! (Affiliation); Author (Affiliation)&nbsp;<br><br>` runs with no
//! per-paper container to select. So instead of a CSS selector per
//! paper, this walks the DOM directly: select every `<strong>` (each one
//! *is* its own real element, just a sibling of all the others rather
//! than a child of a per-paper wrapper), then for each one walks forward
//! through [`ElementRef::next_siblings`] — collecting text nodes,
//! stopping at the next `<strong>` — to gather that paper's author text
//! before the following `<strong>` starts the next entry.
//!
//! Authors are grouped per-affiliation and semicolon-separated —
//! `"Name, Name and Name (Affiliation); Name (Affiliation); ..."` — the
//! same shape [`parse_paren_grouped_names`] already handles.

use scraper::{Html, Selector};
use std::ops::Deref;

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Strip a leading `"12. "`-style ordinal prefix from a title.
fn strip_ordinal_prefix(s: &str) -> &str {
    let trimmed = s.trim();
    match trimmed.split_once(". ") {
        Some((digits, rest))
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) =>
        {
            rest.trim()
        }
        _ => trimmed,
    }
}

/// Parse an INFOCOM accepted-paper-list page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"infocom2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let strong_sel = Selector::parse("strong").unwrap();

    let mut out = Vec::new();
    for strong in document.select(&strong_sel) {
        let raw_title: String = strong.text().collect();
        let title = strip_ordinal_prefix(&raw_title).to_string();
        if title.is_empty() {
            continue;
        }

        // Walk forward through this <strong>'s siblings, collecting text
        // (skipping element nodes like <br>), until the next <strong>
        // starts the following paper.
        let mut authors_text = String::new();
        for sibling in strong.next_siblings() {
            let is_next_strong = sibling
                .value()
                .as_element()
                .is_some_and(|e| e.name() == "strong");
            if is_next_strong {
                break;
            }
            if let Some(t) = sibling.value().as_text() {
                authors_text.push_str(t.deref());
            }
        }

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
        <div class="text field field--name-field-text field__item"><p>
        <strong>1. P2C-MUX: Multiplexing with Power and Polarity Coding</strong><br>Zhao Li, Lijuan Zhang and Zhangbo Gao (Xidian University, China); Kang G. Shin (University of Michigan, USA)&nbsp;<br><br>
        <strong>2. Generative Covert Communication</strong><br>Zhao Li (Xidian University, China); Kang G. Shin (University of Michigan, USA)&nbsp;<br><br>
        <strong>3. Solo-Author Paper</strong><br>Weiyi Qin (Hong Kong Baptist University, Hong Kong)&nbsp;
        </p></div>
    "#;

    #[test]
    fn test_strips_ordinal_prefix_from_title() {
        let out = parse_accepted_papers(SAMPLE, "infocom2026");
        assert_eq!(
            out[0].title,
            "P2C-MUX: Multiplexing with Power and Polarity Coding"
        );
        assert_eq!(out[0].source, "infocom2026");
    }

    #[test]
    fn test_authors_stop_at_next_strong_not_bleeding_into_next_paper() {
        let out = parse_accepted_papers(SAMPLE, "infocom2026");
        assert_eq!(
            out[0].authors,
            vec!["Zhao Li", "Lijuan Zhang", "Zhangbo Gao", "Kang G. Shin"]
        );
        assert_eq!(out[1].title, "Generative Covert Communication");
        assert_eq!(out[1].authors, vec!["Zhao Li", "Kang G. Shin"]);
    }

    #[test]
    fn test_last_entry_with_no_trailing_strong() {
        let out = parse_accepted_papers(SAMPLE, "infocom2026");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].title, "Solo-Author Paper");
        assert_eq!(out[2].authors, vec!["Weiyi Qin"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "infocom2026").is_empty());
    }
}
