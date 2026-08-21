//! Parse IEEE S&P's ("Oakland") accepted-papers page into corpus records.
//!
//! Like NDSS, S&P publishes no BibTeX/CSV/JSON export in any era checked.
//! The accepted-papers page has gone through three eras over the last
//! decade (checked 2016–2026), all sharing the same `div.list-group-item`
//! container but differing in how authors are presented:
//!
//! - **2025–2026** (`parse_footnote_authorlist`): title is a
//!   `data-toggle="collapse"` link that expands a `div.collapse.authorlist`
//!   holding the actual author list, with superscript footnote markers
//!   (`<sup>1,2</sup>`) tying names to an affiliation key on a second line.
//! - **2020–2024** (`parse_plain_inline`, Era B): no collapse at all —
//!   title is plain `<b>` text, authors are inline right after a `<br>` as
//!   `"Name (Affiliation), Name (Affiliation), ..."`.
//! - **2016–2019** (`parse_plain_inline`, Era C): *also* uses
//!   `data-toggle="collapse"` — but there it expands the paper's
//!   *abstract*, not an author list. Authors are still plain inline text
//!   after a `<br>`, just with affiliations sometimes shared across
//!   several authors (`"Name and Name (Aff) and Name (Aff)"`). The
//!   abstract's `div.panel-collapse` lives as a *sibling* of
//!   `div.list-group-item`, not a descendant, so scoping extraction to
//!   the item element already excludes it — no special-casing needed.
//!
//! [`parse_accepted_papers`] tries the footnote-authorlist format first
//! (2025–2026) and falls back to the plain-inline format (which covers
//! *both* 2020–2024 and 2016–2019 with one code path — the only
//! structural difference between those two, the abstract collapse, is
//! irrelevant to title/author extraction) if that finds nothing.

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

/// Parse an IEEE S&P accepted-papers page into publication records,
/// trying the 2025–2026 format first and falling back to the 2016–2024
/// plain-inline format.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"ieeesp2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let modern = parse_footnote_authorlist(html, source_tag);
    if !modern.is_empty() {
        return modern;
    }
    parse_plain_inline(html, source_tag)
}

/// 2025–2026: superscript-footnote author list inside a collapse panel.
fn parse_footnote_authorlist(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let item_sel = Selector::parse("div.list-group-item").unwrap();
    let title_sel = Selector::parse("a[data-toggle='collapse']").unwrap();
    let authors_sel = Selector::parse("div.collapse.authorlist").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let authors_html: String = item
            .select(&authors_sel)
            .next()
            .map(|el| el.html())
            .unwrap_or_default();
        let authors = extract_footnote_authors(&authors_html);
        if authors.is_empty() {
            // No authorlist collapse found for this item at all — this
            // isn't the footnote-authorlist era (e.g. we've been handed
            // a 2016-2019 page, whose `data-toggle="collapse"` targets an
            // abstract, not an author list). Skip rather than emit a
            // title-only entry; the caller's fallback to
            // `parse_plain_inline` handles those eras properly.
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            // No detail-page link on this listing — the href is just an
            // in-page collapse-toggle anchor, not a real URL.
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

/// Extract author names from the `div.collapse.authorlist` fragment.
///
/// The fragment looks like:
/// `"Name<sup>1,2</sup>, Name<sup>3</sup>, ...<br/>
///   <sup>1</sup>: Affiliation, <sup>2</sup>: Affiliation, ..."` —
/// the author line comes first, then a `<br>`, then the affiliation key
/// (which this discards entirely, not just tag-strips, since keeping it
/// would leak affiliation text in as fake "authors").
fn extract_footnote_authors(authors_html: &str) -> Vec<String> {
    static SUP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<sup>.*?</sup>").unwrap());
    static BR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

    // Cut everything from the first <br> onward — that's the affiliation
    // key line, not authors.
    let author_line = match BR_RE.splitn(authors_html, 2).next() {
        Some(line) => line,
        None => authors_html,
    };
    let without_footnotes = SUP_RE.replace_all(author_line, "");
    let plain = TAG_RE.replace_all(&without_footnotes, "");

    plain
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// 2016–2024: plain `div.list-group-item`, title in `<b>` (no author
/// collapse — either no collapse at all, or one that expands an abstract
/// rather than an author list), authors as plain inline text after the
/// first `<br>` in the item.
fn parse_plain_inline(html: &str, source_tag: &str) -> Vec<NewPublication> {
    static BR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<br\s*/?>").unwrap());
    static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

    let document = Html::parse_document(html);
    let item_sel = Selector::parse("div.list-group-item").unwrap();
    let title_sel = Selector::parse("b").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let Some(title_el) = item.select(&title_sel).next() else {
            continue;
        };
        // `.text()` already skips the icon-font `<i>`/`<a data-toggle>`
        // sibling some years nest inside `<b>` (2016-2019's abstract
        // toggle) — icon fonts render via CSS, carrying no text node.
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let item_html = item.html();
        let Some(after_title) = BR_RE.splitn(&item_html, 2).nth(1) else {
            continue;
        };
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

    const FOOTNOTE_SAMPLE: &str = r##"
        <div class="list-group-item">
            <b><a data-toggle="collapse" href="#collapse-0" aria-expanded="false" aria-controls="collapse-0">Bridge: High-Order Taint Vulnerabilities Detection in Linux-based IoT Firmware <span class="glyphicon glyphicon-chevron-down" aria-hidden="true" style="position: relative; top: 3px;"></span></a></b><br />
            <div class="collapse authorlist" id="collapse-0">
                Jiaqian Peng<sup>1,2</sup>, Puzhuo Liu<sup>3</sup>, Yicheng Zeng<sup>1,2</sup>, Kai Cheng<sup>1</sup><br />
                <sup>1</sup>: Institute of Information Engineering, Chinese Academy of Sciences, China, <sup>2</sup>: School of Cyber Security, University of Chinese Academy of Sciences, China, <sup>3</sup>: Ant Group &amp; Tsinghua University, China
            </div>
        </div>
        <div class="list-group-item">
            <b><a data-toggle="collapse" href="#collapse-1" aria-expanded="false" aria-controls="collapse-1">A Second Paper Title For Testing</a></b><br />
            <div class="collapse authorlist" id="collapse-1">
                Solo Author<sup>1</sup><br />
                <sup>1</sup>: Some University
            </div>
        </div>
    "##;

    const PLAIN_INLINE_ERA_B_SAMPLE: &str = r##"
        <div class="list-group-item">
          <b>A Security Analysis of the Facebook Ad Library</b>
          <br>
          Laura Edelson (New York University), Tobias Lauinger (New York University), Damon McCoy (New York University)
        </div>
    "##;

    const PLAIN_INLINE_ERA_C_SAMPLE: &str = r##"
        <div class="list-group-item">
          <b>A Method for Verifying Privacy-Type Properties: The Unbounded Case
            <a data-toggle="collapse" data-parent="#papers24-3" href="#oakland16-188"><i class="fa fa-info-circle"></i></a>
          </b>
          <a href="https://www.youtube.com/watch?v=x"><span class="fa fa-youtube"></span></a><br>
          Lucca Hirschi and David Baelde (LSV, ENS Cachan) and Stéphanie Delaune (LSV, ENS Cachan &amp; CNRS)
        </div>
        <div id="oakland16-188" class="panel-collapse collapse">
          <div class="panel-body">In this paper, we consider the problem of verifying anonymity...</div>
        </div>
    "##;

    #[test]
    fn test_parse_footnote_era() {
        let out = parse_accepted_papers(FOOTNOTE_SAMPLE, "ieeesp2026");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Bridge: High-Order Taint Vulnerabilities Detection in Linux-based IoT Firmware"
        );
        assert_eq!(
            out[0].authors,
            vec!["Jiaqian Peng", "Puzhuo Liu", "Yicheng Zeng", "Kai Cheng"]
        );
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_plain_inline_era_b_falls_back_from_footnote() {
        let out = parse_accepted_papers(PLAIN_INLINE_ERA_B_SAMPLE, "ieeesp2022");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "A Security Analysis of the Facebook Ad Library"
        );
        assert_eq!(
            out[0].authors,
            vec!["Laura Edelson", "Tobias Lauinger", "Damon McCoy"]
        );
    }

    #[test]
    fn test_parse_plain_inline_era_c_ignores_sibling_abstract_collapse() {
        let out = parse_accepted_papers(PLAIN_INLINE_ERA_C_SAMPLE, "ieeesp2016");
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].title,
            "A Method for Verifying Privacy-Type Properties: The Unbounded Case"
        );
        assert_eq!(
            out[0].authors,
            vec!["Lucca Hirschi", "David Baelde", "Stéphanie Delaune"]
        );
        // The sibling abstract text must never leak into the authors list.
        assert!(
            !out[0]
                .authors
                .iter()
                .any(|a| a.contains("consider the problem"))
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "ieeesp2026").is_empty());
    }
}
