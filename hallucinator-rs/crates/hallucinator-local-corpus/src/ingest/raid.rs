//! Parse RAID's accepted-papers page into corpus records.
//!
//! `raidYYYY.github.io` publishes no BibTeX/CSV/JSON export. 2024 and
//! 2025 share one identical format ([`parse_accepted_papers`]) — the
//! cleanest of the five newest venues added here, no bot-blocking, no
//! per-year format churn between those two. 2026 (not yet held) has
//! switched to a CSV data file (`data/accepted_papers.csv`) — out of
//! scope for now, same current-edition-only pattern as everywhere else.
//!
//! Structure: `<p><b> Title <a>[PDF]</a></b><br/> Name, Affiliation; Name,
//! Affiliation; </p>` — note the title's trailing `[PDF]` link has real
//! text (unlike ICSE's icon badges) and must be excluded explicitly, and
//! authors use `"Name, Affiliation"` with **no parentheses**, semicolon-
//! separated — a different shape from the `parse_paren_grouped_names`
//! pattern used elsewhere, handled by [`split_semicolon_name_affiliation`].
//!
//! 2022 and 2023 instead embed the standard ACM Digital Library
//! "Open Access" TOC widget directly into the page (rather than a
//! hand-rolled listing): `h3 > a.DLtitleLink` (title) followed by a
//! sibling `ul.DLauthors > li.nameList` (one `<li>` per author, plain
//! name text, no affiliations at all — nothing to strip). Tried as a
//! fallback in [`parse_dl_widget_papers`] when the primary format finds
//! nothing.

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse a RAID accepted-papers page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"raid2025"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let out = parse_hand_rolled_papers(html, source_tag);
    if !out.is_empty() {
        return out;
    }
    parse_dl_widget_papers(html, source_tag)
}

/// 2024/2025 hand-rolled template: `p > b` (title) + plain text after it
/// (authors).
fn parse_hand_rolled_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
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

/// 2022/2023 ACM Digital Library "Open Access" TOC widget: `h3 >
/// a.DLtitleLink` (title) + the following sibling `ul.DLauthors >
/// li.nameList` (authors, one per `<li>`, plain names — no affiliations
/// to strip).
fn parse_dl_widget_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let title_link_sel = Selector::parse("h3 > a.DLtitleLink").unwrap();
    let authors_list_sel = Selector::parse("ul.DLauthors").unwrap();
    let name_sel = Selector::parse("li.nameList").unwrap();

    let mut out = Vec::new();
    for title_link in document.select(&title_link_sel) {
        let title = title_link.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        // The authors list is the h3's next-sibling ul.DLauthors, not a
        // descendant of anything title-related — walk sibling elements
        // from the enclosing <h3> until one matches.
        let Some(h3) = title_link.parent().and_then(scraper::ElementRef::wrap) else {
            continue;
        };
        let authors_list = h3
            .next_siblings()
            .filter_map(scraper::ElementRef::wrap)
            .find(|el| authors_list_sel.matches(el));
        let Some(authors_list) = authors_list else {
            continue;
        };

        let authors: Vec<String> = authors_list
            .select(&name_sel)
            .map(|li| li.text().collect::<String>().trim().to_string())
            .filter(|name| !name.is_empty())
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

    const DL_WIDGET_SAMPLE: &str = r##"
        <div id="DLcontent"><div class="text-center"><h2>SESSION: IoT / Firmware / Binaries</h2></div>
            <h3><a class="DLtitleLink" href="https://dl.acm.org/doi/10.1145/3607199.3607200">Black-box Attacks Against Neural Binary Function Detection</a></h3>
            <ul class="DLauthors"><li class="nameList">Joshua Bundt</li><li class="nameList">Michael Davinroy</li><li class="nameList Last">William Robertson</li></ul>
            <div class="DLabstract"><div style="display:inline"><p>Some abstract text.</p></div></div>

            <h3><a class="DLtitleLink" href="https://dl.acm.org/doi/10.1145/3607199.3607211">Extracting Threat Intelligence From Cheat Binaries</a></h3>
            <ul class="DLauthors"><li class="nameList">Md Sakib Anwar</li><li class="nameList Last">Zhiqiang Lin</li></ul>
            <div class="DLabstract"><div style="display:inline"><p>Some abstract text.</p></div></div>
        </div>
    "##;

    #[test]
    fn test_parse_dl_widget_fallback_two_papers() {
        let out = parse_accepted_papers(DL_WIDGET_SAMPLE, "raid2023");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Black-box Attacks Against Neural Binary Function Detection"
        );
        assert_eq!(
            out[0].authors,
            vec!["Joshua Bundt", "Michael Davinroy", "William Robertson"]
        );
        assert_eq!(out[0].source, "raid2023");
        assert_eq!(
            out[1].title,
            "Extracting Threat Intelligence From Cheat Binaries"
        );
        assert_eq!(out[1].authors, vec!["Md Sakib Anwar", "Zhiqiang Lin"]);
    }
}
