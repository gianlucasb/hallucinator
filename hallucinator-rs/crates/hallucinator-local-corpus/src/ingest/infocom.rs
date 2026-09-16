//! Parse the IEEE INFOCOM "Accepted Paper List" page into corpus records.
//!
//! INFOCOM's ComSoc "minisite" theme doesn't wrap each paper in a
//! consistent per-paper container with a stable class name, and the exact
//! markup has churned release to release:
//!
//! - **2026, 2019, 2022-2024**: flat — the whole list lives inside one
//!   shared container, formatted as repeated `<strong>N. Title</strong>
//!   <br>Author, Author and Author (Affiliation); Author (Affiliation)
//!   &nbsp;<br><br>` runs (or, for 2019/2022-2024, the same shape one
//!   level down inside a shared `<li>` per paper). The author text sits
//!   as plain sibling text nodes of the `<strong>` itself.
//! - **2025**: each paper is `<li><p><strong>Title</strong></p>
//!   <p>Authors</p></li>` — the title is wrapped in its own `<p>`, and
//!   the author text is a *sibling of that wrapping `<p>`*, not of the
//!   `<strong>` directly.
//! - **2020, 2021**: each paper is `<ol><li><strong>Title</strong></li>
//!   </ol><p class="rteindent1">Authors</p>` — title and authors aren't
//!   even in the same list item; the author `<p>` is a sibling of the
//!   whole single-item `<ol>`, two levels up from the `<strong>`.
//!
//! Rather than a CSS selector per paper, this walks the DOM directly:
//! select every `<strong>` (each one *is* its own real element — a title
//! — regardless of how deep it's nested for a given year), then look for
//! that paper's author text starting from the `<strong>` itself and
//! climbing up to two ancestor levels ([`collect_following_text`]),
//! stopping as soon as one level's forward-sibling text actually parses
//! into real author names. Each level's sibling walk stops at the next
//! `<strong>` (however deeply it's nested in *its* wrapper) so a later
//! paper's title/authors never bleed into an earlier one's.
//!
//! Authors are grouped per-affiliation and semicolon-separated —
//! `"Name, Name and Name (Affiliation); Name (Affiliation); ..."` — the
//! same shape [`parse_paren_grouped_names`] already handles.

use scraper::{ElementRef, Html, Selector};
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

/// True if `el` is a `<strong>` itself or has one somewhere among its
/// descendants — i.e. it looks like it starts (or wraps) the next
/// paper's title, and the forward sibling walk should stop before it.
fn is_or_wraps_strong(el: ElementRef) -> bool {
    el.value().name() == "strong"
        || el
            .descendent_elements()
            .any(|d| d.value().name() == "strong")
}

/// Walk forward through `start`'s siblings, collecting visible text,
/// stopping at the first sibling that is (or contains) a `<strong>` —
/// that's the next paper's title, however deeply nested in its own
/// wrapper. Handles both plain text siblings (the flat, single-shared-
/// container layout) and element siblings that wrap the author text in
/// their own tag (a `<p>`, e.g.).
fn collect_following_text(start: ElementRef) -> String {
    let mut text = String::new();
    for sibling in start.next_siblings() {
        if let Some(el) = ElementRef::wrap(sibling) {
            if is_or_wraps_strong(el) {
                break;
            }
            text.push_str(&el.text().collect::<String>());
        } else if let Some(t) = sibling.value().as_text() {
            text.push_str(t.deref());
        }
    }
    text
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

        // Author placement relative to the title varies by year (see
        // module docs). Try the <strong> itself, then climb up to two
        // ancestor levels, stopping as soon as a level's forward text
        // actually parses into real author names — a heading like "List
        // of Accepted Papers..." never does, at any level, and is
        // dropped below.
        let mut authors = Vec::new();
        let mut node = strong;
        for _ in 0..3 {
            let candidate = parse_paren_grouped_names(&collect_following_text(node));
            if !candidate.is_empty() {
                authors = candidate;
                break;
            }
            node = match node.parent().and_then(ElementRef::wrap) {
                Some(parent) => parent,
                None => break,
            };
        }
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

    /// 2025's shape: title wrapped in its own `<p>`, authors a sibling
    /// `<p>` of that wrapper (not of the `<strong>` directly).
    const SAMPLE_2025: &str = r#"
        <div class="field-item"><div class="rtecenter"><strong>List of Accepted Papers in IEEE INFOCOM 2025&nbsp;Main Conference</strong></div>
        <p class="rtecenter">&nbsp;</p>
        <ol>
        <li>
        <p><strong>Achieving Efficient Multipath Validation in Software-Defined Networks </strong></p>
        <p>Bing Hu and Yuanguo Bi (Northeastern University, China); Kui Wu (University of Victoria, Canada)</p>
        </li>
        <br />
        <li>
        <p><strong>Oracle: QoS-aware Online Service Provisioning</strong></p>
        <p>Shengyu Zhang (Singapore University of Technology and Design, Singapore)</p>
        </li>
        </ol>
        </div>
    "#;

    #[test]
    fn test_2025_shape_title_wrapped_in_own_paragraph() {
        let out = parse_accepted_papers(SAMPLE_2025, "infocom2025");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "Achieving Efficient Multipath Validation in Software-Defined Networks"
        );
        assert_eq!(out[0].authors, vec!["Bing Hu", "Yuanguo Bi", "Kui Wu"]);
        assert_eq!(
            out[1].title,
            "Oracle: QoS-aware Online Service Provisioning"
        );
        assert_eq!(out[1].authors, vec!["Shengyu Zhang"]);
    }

    /// 2020/2021's shape: title inside its own single-item `<ol><li>`,
    /// authors a sibling `<p>` of the whole `<ol>` — two levels up from
    /// the `<strong>`.
    const SAMPLE_2020: &str = r#"
        <div class="field-item"><p align="center"><strong>List of Accepted Papers in IEEE INFOCOM 2020 Main Conference</strong></p>
        <ol>
        <li><strong>(How Much) Does a Private WAN Improve Cloud Performance?</strong></li>
        </ol>
        <p class="rteindent1">Ege Gurmericliler (Columbia University, USA); Arpit Gupta (Columbia University)</p>
        <ol>
        <li value="2"><strong>A Converse Result on Convergence Time</strong></li>
        </ol>
        <p class="rteindent1">Michael Neely (University of Southern California, USA)</p>
        </div>
    "#;

    #[test]
    fn test_2020_shape_authors_sibling_of_whole_ol() {
        let out = parse_accepted_papers(SAMPLE_2020, "infocom2020");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "(How Much) Does a Private WAN Improve Cloud Performance?"
        );
        assert_eq!(out[0].authors, vec!["Ege Gurmericliler", "Arpit Gupta"]);
        assert_eq!(out[1].title, "A Converse Result on Convergence Time");
        assert_eq!(out[1].authors, vec!["Michael Neely"]);
    }
}
