//! Parse ACM CCS's accepted-papers page into corpus records.
//!
//! CCS publishes no BibTeX/CSV/JSON export in any era checked. Its page
//! format changes almost every year — checked 2016–2026, and barely two
//! consecutive years match exactly (different CSS classes, different
//! author separators: `<br>`, `;`, or `,`, sometimes with per-author
//! affiliations and sometimes shared across a group). Two years are
//! deliberately **not** supported here: 2017 (no accepted-papers list
//! recoverable from sigsac.org in any form, checked including Wayback's
//! full CDX index of that path) and 2023/2025 (data lives behind a JS
//! data-fetch or client-rendered SPA respectively — out of scope for a
//! plain-HTML scraper; see conversation history for the tradeoff).
//!
//! What actually matters for extraction turns out to be simpler than the
//! format churn suggests, because [`parse_paren_grouped_names`] finds
//! `"Name (Affiliation)"` pairs by regex rather than splitting on a fixed
//! separator — so it doesn't care whether entries are `<br>`-separated,
//! `;`-separated, or `,`-separated. That collapses five of this crate's
//! six supported years (2026, 2024, 2022, 2019, 2018 — every year that
//! uses *some* `<table>`, classed or not) into **one** code path:
//! [`parse_table_based`] selects every `<table>` on the page (not scoped
//! to any specific class, so it doesn't matter which year's class name —
//! or lack of one — is in play) and treats any row with exactly two
//! `<td>` cells as a paper (header rows use `<th>`, so they're skipped
//! for free). Only 2016 needs a distinct path: no table at all, just
//! `<p><strong>Title</strong><br/>Author (Aff), Author (Aff) and Author
//! (Aff)</p>` — see [`parse_paragraph_based`].

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse an ACM CCS accepted-papers page into publication records, trying
/// the table-based format (2026, 2024, 2022, 2019, 2018) first and
/// falling back to the paragraph-based format (2016) if that finds
/// nothing.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"ccs2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let table_based = parse_table_based(html, source_tag);
    if !table_based.is_empty() {
        return table_based;
    }
    parse_paragraph_based(html, source_tag)
}

/// 2026, 2024, 2022, 2019, 2018: any `<table>` with two-`<td>` rows,
/// regardless of CSS class (or lack of one) — see module docs for why
/// author-separator style doesn't matter here.
fn parse_table_based(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let table_sel = Selector::parse("table").unwrap();
    let row_sel = Selector::parse("tr").unwrap();
    let cell_sel = Selector::parse("td").unwrap();

    let mut out = Vec::new();
    for table in document.select(&table_sel) {
        for row in table.select(&row_sel) {
            let cells: Vec<_> = row.select(&cell_sel).collect();
            // Header rows use <th>, not <td> — `cells` is empty for them.
            let [title_cell, authors_cell] = cells.as_slice() else {
                continue;
            };

            let title: String = title_cell.text().collect::<String>().trim().to_string();
            if title.is_empty() {
                continue;
            }

            let authors_text: String = authors_cell.text().collect();
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

/// 2016: no table — `<p><strong>Title</strong><br/>Author (Aff), Author
/// (Aff) and Author (Aff)</p>`.
fn parse_paragraph_based(html: &str, source_tag: &str) -> Vec<NewPublication> {
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
    const CURLY_AND_STRAIGHT_QUOTES: [char; 3] = ['\u{201c}', '\u{201d}', '"'];

    let document = Html::parse_document(html);
    let p_sel = Selector::parse("p").unwrap();
    let strong_sel = Selector::parse("strong").unwrap();

    let mut out = Vec::new();
    for p in document.select(&p_sel) {
        let Some(strong_el) = p.select(&strong_sel).next() else {
            continue;
        };
        let title: String = strong_el
            .text()
            .collect::<String>()
            .trim()
            .trim_matches(CURLY_AND_STRAIGHT_QUOTES.as_slice())
            .to_string();
        if title.is_empty() {
            continue;
        }

        // Authors are whatever comes after the title's closing </strong>
        // in this paragraph — everything else in the <p>.
        let p_html = p.html();
        let Some(idx) = p_html.to_lowercase().find("</strong>") else {
            continue;
        };
        let after_title = &p_html[idx + "</strong>".len()..];
        let plain = TAG_RE.replace_all(after_title, " ");
        let authors = parse_paren_grouped_names(&plain);
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

    const BR_SEPARATED_SAMPLE: &str = r#"
        <table class="accepted-papers-table" border="10" frame="void" rules="rows">
          <tr>
            <th style="width:40%">Title</th>
            <th>Author</th>
          </tr>
          <tr>
            <td>Exploring the Exposure of IPv6 End-Host Networks</td>
            <td>Hugo Hue (Georgia Institute of Technology)<br />Abhishek Bhaskar (Georgia Institute of Technology)<br />Frank Li (Georgia Institute of Technology)</td>
          </tr>
          <tr>
            <td>A Second Paper For Testing</td>
            <td>Solo Author (Some University)</td>
          </tr>
        </table>
        <!--
        <h3>Second Cycle</h3>
        <table class="accepted-papers-table" border="10" frame="void" rules="rows">
          <tr><th>Title</th><th>Author</th></tr>
          <tr><td>A Placeholder Future Paper</td><td>Nobody Yet (Nowhere University)</td></tr>
        </table>
        -->
    "#;

    const SEMICOLON_SEPARATED_SAMPLE: &str = r#"
        <table border="10" frame="void" rules="rows">
          <tr><th>Title</th><th>Author</th></tr>
          <tr>
            <td>A Run a Day Won't Keep the Hacker Away</td>
            <td>Karel Dhondt (imec-DistriNet; KU Leuven); Victor Le Pochat (imec-DistriNet; KU Leuven)</td>
          </tr>
        </table>
    "#;

    const COMMA_SEPARATED_THEAD_SAMPLE: &str = r#"
        <table><thead><tr><th>Title</th><th>Authors</th></tr></thead>
        <tbody>
        <tr>
        <td>PrinTracker: Fingerprinting 3D Printers using Commodity Scanners</td>
        <td>Zhengxiong Li (SUNY University at Buffalo), Aditya Singh Rathore (SUNY University at Buffalo)</td>
        </tr>
        </tbody></table>
    "#;

    const PARAGRAPH_SAMPLE: &str = r#"
        <p><strong>&#8220;Make Sure DSA Signing Exponentiations Really are Constant-Time&#8221;</strong><br />
         <em>Cesar Pereida Garcia (Aalto University), Billy Bob Brumley (Tampere University of Technology) and Yuval Yarom (The University of Adelaide)</em></p>
    "#;

    #[test]
    fn test_br_separated_table_skips_header_and_comment() {
        let out = parse_accepted_papers(BR_SEPARATED_SAMPLE, "ccs2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Exploring the Exposure of IPv6 End-Host Networks"
        );
        assert_eq!(
            out[0].authors,
            vec!["Hugo Hue", "Abhishek Bhaskar", "Frank Li"]
        );
        assert_eq!(out[1].authors, vec!["Solo Author"]);
        // The commented-out "Second Cycle" table must never surface.
        assert!(!out.iter().any(|r| r.title.contains("Placeholder")));
    }

    #[test]
    fn test_semicolon_separated_table_no_class() {
        let out = parse_accepted_papers(SEMICOLON_SEPARATED_SAMPLE, "ccs2022");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].authors, vec!["Karel Dhondt", "Victor Le Pochat"]);
    }

    #[test]
    fn test_comma_separated_table_with_thead() {
        let out = parse_accepted_papers(COMMA_SEPARATED_THEAD_SAMPLE, "ccs2018");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "PrinTracker: Fingerprinting 3D Printers using Commodity Scanners"
        );
        assert_eq!(
            out[0].authors,
            vec!["Zhengxiong Li", "Aditya Singh Rathore"]
        );
    }

    #[test]
    fn test_paragraph_based_falls_back_when_no_table() {
        let out = parse_accepted_papers(PARAGRAPH_SAMPLE, "ccs2016");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "Make Sure DSA Signing Exponentiations Really are Constant-Time"
        );
        assert_eq!(
            out[0].authors,
            vec!["Cesar Pereida Garcia", "Billy Bob Brumley", "Yuval Yarom"]
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "ccs2026").is_empty());
    }
}
