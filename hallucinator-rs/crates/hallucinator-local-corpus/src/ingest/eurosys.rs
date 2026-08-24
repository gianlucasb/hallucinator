//! Parse the EuroSys accepted-papers page into corpus records.
//!
//! One page per year at `<year>.eurosys.org/papers.html`. Structure:
//! `table.papers` → `tr` → first `td`'s `a` (title, a DOI link) + second
//! `td` (authors, `"Name (Affiliation), Name (Affiliation), ..."` — the
//! same shape [`parse_paren_grouped_names`] already handles). The header
//! row uses `<th>`, not `<td>`, so it's naturally skipped.

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse a EuroSys accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"eurosys2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let row_sel = Selector::parse("table.papers tr").unwrap();
    let title_sel = Selector::parse("td:nth-child(1) a").unwrap();
    let authors_sel = Selector::parse("td:nth-child(2)").unwrap();

    let mut out = Vec::new();
    for row in document.select(&row_sel) {
        let Some(title_el) = row.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let Some(authors_el) = row.select(&authors_sel).next() else {
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
        <table class="papers">
          <thead class="pap">
            <tr><th class="bord" title="Field #2">Title</th><th title="Field #3">Authors</th></tr>
          </thead>
          <tbody>
            <tr>
              <td><a href="https://dl.acm.org/doi/10.1145/example">AdaServe: Accelerating Multi-SLO LLM Serving</a></td>
              <td>Zikun Li (Carnegie Mellon University), Zhuofu Chen (Princeton University), Zhihao Jia (Carnegie Mellon University and Amazon Web Services)</td>
            </tr>
            <tr>
              <td><a href="https://dl.acm.org/doi/10.1145/example2">A Second EuroSys Paper</a></td>
              <td>Solo Author (Some University)</td>
            </tr>
          </tbody>
        </table>
    "#;

    #[test]
    fn test_header_row_skipped() {
        let out = parse_accepted_papers(SAMPLE, "eurosys2026");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_title_and_authors() {
        let out = parse_accepted_papers(SAMPLE, "eurosys2026");
        assert_eq!(out[0].title, "AdaServe: Accelerating Multi-SLO LLM Serving");
        assert_eq!(
            out[0].authors,
            vec!["Zikun Li", "Zhuofu Chen", "Zhihao Jia"]
        );
        assert_eq!(out[0].source, "eurosys2026");
    }

    #[test]
    fn test_second_row() {
        let out = parse_accepted_papers(SAMPLE, "eurosys2026");
        assert_eq!(out[1].title, "A Second EuroSys Paper");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "eurosys2026").is_empty());
    }
}
