//! Parse IEEE EuroS&P's accepted-papers page into corpus records.
//!
//! Despite being run by the same IEEE Technical Committee as US IEEE
//! S&P, EuroS&P uses its own, unrelated template — none of the three
//! `hallucinator_local_corpus::ingest::ieee_sp` eras (footnote-authorlist
//! collapse, plain-inline, abstract-collapse) apply here. EuroS&P's own
//! format changed once (2023-2024 inline-`style=` era vs. 2025-2026
//! `class="paper_title"`/`class="paper_authors"` era) — this module
//! covers only the current era, same current-edition-only scope as
//! every other venue added in this round.
//!
//! Structure: `table.papers` → `tr` → `td.paper_title` (plain text) +
//! `td.paper_authors` (`<strong>Name</strong> (Affiliation), ...` — the
//! same `"Name (Affiliation)"` shape [`parse_paren_grouped_names`]
//! already handles). The page has separate `table.papers` blocks for an
//! "Awards" section and the main "Accepted Papers" list, with award
//! winners repeated in both — selecting every `table.papers` (not just
//! the first) picks up both, and any resulting duplicate title is a
//! no-op at import time (dedup-checked against what's already in the
//! corpus).

use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse a EuroS&P accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"eurosp2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let table_sel = Selector::parse("table.papers").unwrap();
    let row_sel = Selector::parse("tr").unwrap();
    let title_sel = Selector::parse("td.paper_title").unwrap();
    let authors_sel = Selector::parse("td.paper_authors").unwrap();

    let mut out = Vec::new();
    for table in document.select(&table_sel) {
        for row in table.select(&row_sel) {
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
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <div class="container-box">
          <h1>Awards</h1>
          <table class="papers">
            <tr>
              <td class="paper_title">Bandwidth Efficient Partial Authorized PSI</td>
              <td class="paper_authors"><strong>Tjitske Ollie Koster</strong> (TU Delft), <strong>Francesca Falzon</strong> (ETH Zurich)</td>
            </tr>
          </table>
        </div>
        <div class="container-box">
          <h1>Accepted Papers</h1>
          <table class="papers">
            <tr>
              <td class="paper_title">Bandwidth Efficient Partial Authorized PSI</td>
              <td class="paper_authors"><strong>Tjitske Ollie Koster</strong> (TU Delft), <strong>Francesca Falzon</strong> (ETH Zurich)</td>
            </tr>
            <tr>
              <td class="paper_title">Helltrap: Transforming physical machines into UEFI rootkit trap</td>
              <td class="paper_authors"><strong>Darius Suciu</strong> (Private Machines Inc.), <strong>Radu Sion</strong> (Private Machines Inc.)</td>
            </tr>
          </table>
        </div>
    "#;

    #[test]
    fn test_parse_across_multiple_tables() {
        // 3 rows total (1 award + 2 accepted), one of them a duplicate —
        // the parser itself doesn't dedupe (that's insert_if_new's job
        // at import time), so all 3 come through here.
        let out = parse_accepted_papers(SAMPLE, "eurosp2026");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].title, "Bandwidth Efficient Partial Authorized PSI");
        assert_eq!(
            out[0].authors,
            vec!["Tjitske Ollie Koster", "Francesca Falzon"]
        );
        assert_eq!(out[0].source, "eurosp2026");
    }

    #[test]
    fn test_second_table_entry() {
        let out = parse_accepted_papers(SAMPLE, "eurosp2026");
        assert_eq!(
            out[2].title,
            "Helltrap: Transforming physical machines into UEFI rootkit trap"
        );
        assert_eq!(out[2].authors, vec!["Darius Suciu", "Radu Sion"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "eurosp2026").is_empty());
    }
}
