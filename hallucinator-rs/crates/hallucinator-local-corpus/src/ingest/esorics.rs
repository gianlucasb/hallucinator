//! Parse ESORICS's accepted-papers page into corpus records.
//!
//! ESORICS has no permanent conference domain — each year is a fresh
//! site run by that edition's local organizers, on a different platform
//! every time (checked 2023-2025: Hugo list, a WYSIWYG drag-and-drop
//! builder, and Sciencesconf's own table widget — zero shared markup
//! across any two years). Worse, at least one past year's domain
//! (`esorics2024.org`) has since been squatted by unrelated spam
//! content — old ESORICS URLs are not safe to fetch directly without
//! going through Wayback first. This module covers only the current
//! format (ESORICS 2025, Sciencesconf): a plain two-column
//! `<table>` — **Authors, then Title** (opposite column order from
//! every other venue here) — with plain comma/"and"-joined names and no
//! affiliations, inside `div#page` (scoped there to skip the site's own
//! unrelated login/nav table elsewhere on the page).

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse an ESORICS accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"esorics2025"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let row_sel = Selector::parse("div#page table tr").unwrap();
    let cell_sel = Selector::parse("td").unwrap();

    let mut out = Vec::new();
    for row in document.select(&row_sel) {
        let cells: Vec<_> = row.select(&cell_sel).collect();
        let [authors_cell, title_cell] = cells.as_slice() else {
            continue;
        };

        let title: String = title_cell.text().collect::<String>().trim().to_string();
        // Header row ("Authors" / "Title" column labels) uses the same
        // <td> shape as data rows — filter it out by content instead.
        if title.is_empty() || title.eq_ignore_ascii_case("title") {
            continue;
        }

        let authors_text: String = authors_cell.text().collect();
        let authors = split_plain_names(&authors_text);
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

/// Split `"Name, Name and Name"` (no affiliations, no parens) into
/// individual names.
fn split_plain_names(text: &str) -> Vec<String> {
    static AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s+and\s+").unwrap());
    let normalized = AND_RE.replace_all(text.trim(), ", ");
    normalized
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <div id="page">
        <table border="0" cellspacing="0">
        <tbody>
        <tr>
        <td><strong>Authors</strong></td>
        <td><strong>Title</strong></td>
        </tr>
        <tr>
        <td>Sarat Chandra Prasad Gingupalli</td>
        <td>Hardening HSM Clusters: Resolving Key Sync Vulnerabilities for Robust CU Isolation</td>
        </tr>
        <tr>
        <td>Ankit Gangwal, Mauro Conti and Tommaso Pauselli</td>
        <td>KeTS: Kernel-based Trust Segmentation against Model Poisoning Attacks</td>
        </tr>
        </tbody>
        </table>
        </div>
    "#;

    #[test]
    fn test_parse_two_papers_skips_header() {
        let out = parse_accepted_papers(SAMPLE, "esorics2025");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].authors, vec!["Sarat Chandra Prasad Gingupalli"]);
        assert_eq!(
            out[0].title,
            "Hardening HSM Clusters: Resolving Key Sync Vulnerabilities for Robust CU Isolation"
        );
        assert_eq!(out[0].source, "esorics2025");
    }

    #[test]
    fn test_and_joined_authors_split() {
        let out = parse_accepted_papers(SAMPLE, "esorics2025");
        assert_eq!(
            out[1].authors,
            vec!["Ankit Gangwal", "Mauro Conti", "Tommaso Pauselli"]
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "esorics2025").is_empty());
    }
}
