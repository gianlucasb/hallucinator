//! Parse RAID's accepted-papers page into corpus records.
//!
//! `raidYYYY.github.io` publishes no BibTeX/CSV/JSON export. 2024 and
//! 2025 share one identical format (this module) — the cleanest of the
//! five newest venues added here, no bot-blocking, no per-year format
//! churn between those two. 2023 used a different, ACM-DL-embedded page
//! and 2026 (not yet held) has switched to a CSV data file
//! (`data/accepted_papers.csv`) — both out of scope for now, same
//! current-edition-only pattern as everywhere else.
//!
//! Structure: `<p><b> Title <a>[PDF]</a></b><br/> Name, Affiliation; Name,
//! Affiliation; </p>` — note the title's trailing `[PDF]` link has real
//! text (unlike ICSE's icon badges) and must be excluded explicitly, and
//! authors use `"Name, Affiliation"` with **no parentheses**, semicolon-
//! separated — a different shape from the `parse_paren_grouped_names`
//! pattern used elsewhere, handled by [`split_semicolon_name_affiliation`].

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse a RAID accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"raid2025"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

    let document = Html::parse_document(html);
    let p_sel = Selector::parse("p").unwrap();
    let bold_sel = Selector::parse("b").unwrap();

    let mut out = Vec::new();
    for p in document.select(&p_sel) {
        let Some(title_el) = p.select(&bold_sel).next() else {
            continue;
        };
        // Direct text only — excludes the nested "[PDF]" link, which
        // (unlike ICSE's icon-only badges) carries real visible text.
        let title = direct_and_nested_text_excluding_links(&title_el);
        if title.is_empty() {
            continue;
        }

        // Authors are whatever comes after the title's closing </b> in
        // this paragraph's HTML — string-based rather than a text-prefix
        // match, since the title's flattened text (with "[PDF]" already
        // excluded) won't line up character-for-character against the
        // paragraph's raw concatenated text nodes.
        let p_html = p.html();
        let Some(idx) = p_html.find("</b>") else {
            continue;
        };
        let after_title = &p_html[idx + 4..];
        let plain = TAG_RE.replace_all(after_title, " ");
        let authors = split_semicolon_name_affiliation(&plain);
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

/// The `<b>` title element's text, walking its subtree but skipping any
/// nested `<a>` (the `[PDF]` link, which has real visible text unlike an
/// icon-font badge — can't just take direct children here since the
/// title text itself is a direct child alongside the link).
fn direct_and_nested_text_excluding_links(el: &scraper::ElementRef) -> String {
    use scraper::Node;
    let mut out = String::new();
    for node in el.children() {
        match node.value() {
            Node::Text(t) => out.push_str(t),
            Node::Element(e) if e.name() != "a" => {
                if let Some(child) = scraper::ElementRef::wrap(node) {
                    out.push_str(&child.text().collect::<String>());
                }
            }
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Split `"Name, Affiliation; Name, Affiliation; "` into just the names
/// — comma separates name from affiliation (no parens), semicolon
/// separates people.
fn split_semicolon_name_affiliation(text: &str) -> Vec<String> {
    text.split(';')
        .filter_map(|entry| entry.split(',').next())
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r##"
        <p><b> A Comprehensive Quantification of Inconsistencies in Memory Dumps <a href="#" style="display:none" data-pid="10">[PDF]</a></b></br>
          Andrea Oliveri, EURECOM; Davide Balzarotti, EURECOM;
          </p>
        <p><b> KernJC: Automated Vulnerable Environment Generation for Linux Kernel Vulnerabilities <a href="papers/raid2024-1.pdf">[PDF]</a></b></br>
          Bonan Ruan, National University of Singapore; Jiahao Liu, National University of Singapore;
          </p>
    "##;

    #[test]
    fn test_parse_two_papers_title_excludes_pdf_link() {
        let out = parse_accepted_papers(SAMPLE, "raid2025");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "A Comprehensive Quantification of Inconsistencies in Memory Dumps"
        );
        assert!(!out[0].title.contains("PDF"));
        assert_eq!(out[0].source, "raid2025");
    }

    #[test]
    fn test_semicolon_separated_name_comma_affiliation() {
        let out = parse_accepted_papers(SAMPLE, "raid2025");
        assert_eq!(out[0].authors, vec!["Andrea Oliveri", "Davide Balzarotti"]);
        assert_eq!(out[1].authors, vec!["Bonan Ruan", "Jiahao Liu"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "raid2025").is_empty());
    }
}
