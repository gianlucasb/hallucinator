//! Parse an ICML `proceedings.mlr.press` volume page into corpus records.
//!
//! ICML's own site (`icml.cc/virtual/<year>/papers.html`) renders its
//! paper list client-side via JavaScript, so it can't be scraped directly
//! — and per this project's policy (see MANIFESTO.md) OpenReview is off
//! the table too. PMLR (Proceedings of Machine Learning Research) is the
//! canonical static-HTML archive instead, same role NeurIPS's
//! `papers.nips.cc` plays for that venue — but it only gets a volume
//! *after* proceedings are formally published, typically weeks after the
//! conference (same lag documented for NeurIPS's importer). Find the
//! current ICML volume number at <https://proceedings.mlr.press/> (look
//! for "Proceedings of ICML <year>") before importing.
//!
//! Structure: `div.paper` → `p.title` (plain text) + `span.authors`
//! (comma-separated plain names, **no** parenthesized affiliations —
//! unlike every venue using [`super::author_parsing::parse_paren_grouped_names`],
//! so this module splits on comma directly instead).

use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse a PMLR ICML volume page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"icml2026"`.
pub fn parse_volume(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let paper_sel = Selector::parse("div.paper").unwrap();
    let title_sel = Selector::parse("p.title").unwrap();
    let authors_sel = Selector::parse("span.authors").unwrap();

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
        let authors: Vec<String> = authors_text
            .split(',')
            // PMLR's markup uses `&nbsp;` after the comma, decoded by
            // the HTML parser to U+00A0 — an ordinary `trim()` (which
            // only strips ASCII/Unicode whitespace it recognizes, and
            // U+00A0 *is* Unicode whitespace per `char::is_whitespace`,
            // so plain `.trim()` already covers it) is enough here.
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

    const SAMPLE: &str = "
        <div class=\"paper\">
          <p class=\"title\">Aggregation of Dependent Expert Distributions in Multimodal Variational Autoencoders</p>
          <p class=\"details\">
            <span class=\"authors\">Rogelio A. Mancisidor,&nbsp;Robert Jenssen,&nbsp;Shujian Yu,&nbsp;Michael Kampffmeyer</span>;
            <span class=\"info\"><i>Proceedings of the 42nd International Conference on Machine Learning</i>, PMLR 267:1-26</span>
          </p>
        </div>
        <div class=\"paper\">
          <p class=\"title\">A Second Paper Title</p>
          <p class=\"details\">
            <span class=\"authors\">Solo Author</span>;
          </p>
        </div>
    ";

    #[test]
    fn test_parse_multi_author_no_affiliations() {
        let out = parse_volume(SAMPLE, "icml2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Aggregation of Dependent Expert Distributions in Multimodal Variational Autoencoders"
        );
        assert_eq!(
            out[0].authors,
            vec![
                "Rogelio A. Mancisidor",
                "Robert Jenssen",
                "Shujian Yu",
                "Michael Kampffmeyer"
            ]
        );
        assert_eq!(out[0].source, "icml2026");
    }

    #[test]
    fn test_solo_author() {
        let out = parse_volume(SAMPLE, "icml2026");
        assert_eq!(out[1].title, "A Second Paper Title");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_volume("<html></html>", "icml2026").is_empty());
    }
}
