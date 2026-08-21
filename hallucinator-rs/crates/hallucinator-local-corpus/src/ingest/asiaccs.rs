//! Parse ASIACCS's accepted-papers page into corpus records.
//!
//! Like ESORICS, ASIACCS has no permanent conference domain — a fresh
//! site per year, run by that edition's local host institution, and past
//! years' domains go dead or get squatted within a couple of years
//! (confirmed: 2023's domain is now spam-squatted, 2024's no longer
//! resolves at all). This module covers only the current edition (2026,
//! IIT Kharagpur): two consecutive `<p>` tags per paper — the first
//! containing the bolded title (sometimes split across multiple `<b>`
//! spans from copy-paste artifacts in the source), the next holding
//! `"Name, Name (Affiliation); Name (Affiliation)"` — the same
//! multi-name-per-affiliation-group shape [`parse_paren_grouped_names`]
//! already handles for other venues.
//!
//! ASIACCS runs two submission cycles per year on separate pages
//! (`cycle-1-papers/`, `cycle-2-papers/`) — import both with the same
//! `source_tag` (or different tags per cycle, caller's choice).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse an ASIACCS accepted-papers page (one submission cycle) into
/// publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"asiaccs2026-cycle1"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let p_sel = Selector::parse("p").unwrap();
    let bold_sel = Selector::parse("b, strong").unwrap();

    let mut out = Vec::new();
    let mut paragraphs = document.select(&p_sel).peekable();
    while let Some(p) = paragraphs.next() {
        // A title paragraph has a bold child; concatenate every bold
        // span's text (a title occasionally splits across `<b>Foo</b>
        // <b>Bar</b>` from a copy-paste artifact in the source).
        let bold_text: String = p
            .select(&bold_sel)
            .map(|b| b.text().collect::<String>())
            .collect::<Vec<_>>()
            .join(" ");
        let title = bold_text.trim();
        if title.is_empty() {
            continue;
        }

        // The next paragraph holds the author/affiliation text.
        let Some(&authors_p) = paragraphs.peek() else {
            continue;
        };
        let authors_text: String = authors_p.text().collect();
        let authors = parse_paren_grouped_names(&authors_text);
        if authors.is_empty() {
            continue;
        }
        paragraphs.next(); // consume the authors paragraph

        out.push(NewPublication {
            title: title.to_string(),
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
        <p><b>Kitten or Panda? Measuring the Specificity of Threat Group Behaviors in Public CTI Knowledge</b> <b>Bases</b></p>
        <p><span>Aakanksha Saha, Martina Lindorfer (TU Wien); Juan Caballero (IMDEA Software Institute)</span></p>
        <p><b>MYao: Efficient Multiparty "Yao" Garbled Circuits with Row Reduction and Half Gates</b></p>
        <p><span>Aner Ben-Efraim, Lior Breitman (Ariel University)</span></p>
    "#;

    #[test]
    fn test_title_split_across_bold_spans_is_joined() {
        let out = parse_accepted_papers(SAMPLE, "asiaccs2026-cycle1");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Kitten or Panda? Measuring the Specificity of Threat Group Behaviors in Public CTI Knowledge Bases"
        );
        assert_eq!(out[0].source, "asiaccs2026-cycle1");
    }

    #[test]
    fn test_multi_name_affiliation_group() {
        let out = parse_accepted_papers(SAMPLE, "asiaccs2026-cycle1");
        assert_eq!(
            out[0].authors,
            vec!["Aakanksha Saha", "Martina Lindorfer", "Juan Caballero"]
        );
        assert_eq!(out[1].authors, vec!["Aner Ben-Efraim", "Lior Breitman"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "asiaccs2026-cycle1").is_empty());
    }
}
