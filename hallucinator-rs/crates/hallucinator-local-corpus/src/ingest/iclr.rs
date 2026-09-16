//! Parse a Paper Digest "<Venue> Papers with Code & Data" page into corpus
//! records — built for ICLR specifically (no other clean, ethically-usable
//! source exists: OpenReview's venue-scoped query endpoint 403s with a
//! challenge page, and its one reachable endpoint serves peer-review
//! content rather than paper metadata; ICLR's own `iclr.cc/virtual` site
//! requires ~5,500 individual per-paper page fetches for author data and
//! explicitly disallows GPTBot from that path in robots.txt; Papers With
//! Code itself no longer exists, redirecting to a generic Hugging Face
//! feed with no venue listing).
//!
//! `paperdigest.org` publishes one such page per venue/year at URLs like
//! `paperdigest.org/<year>/<month>/<venue>-<year>-papers-with-code-data/`.
//! Structure: one `<table>` on the page, `<tr>` → `<td>` (index) +
//! `<td>` (title, as `<a><b>Title</b></a>` followed by unrelated
//! "Related Papers/Patents/.../Highlight" clutter we ignore — selecting
//! the first `b` inside the cell rather than the cell's full text avoids
//! all of it) + `<td>` (authors, as `<a>Name</a>; <a>Name</a>; ...` — the
//! name is the link *text*, not the `?name=` slug in its href, which is
//! lowercased/underscored) + `<td>` (an external code-repo link, unused
//! here). The header row uses `<th>`, not `<td>`, so it's naturally
//! skipped by matching on `<td>`.
//!
//! Caveat this importer's caller should know: Paper Digest's "with code"
//! pages only cover accepted papers that have an associated public code
//! or data repository — a meaningful but incomplete subset of the full
//! accepted-papers list, not the whole venue.

use scraper::{Html, Selector};

use crate::db::NewPublication;

/// Parse a Paper Digest "papers with code & data" page into publication
/// records. `source_tag` is the provenance string to store on each
/// record, e.g. `"iclr2026"`.
pub fn parse_papers_with_code(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = Html::parse_document(html);
    let row_sel = Selector::parse("tr").unwrap();
    let title_cell_sel = Selector::parse("td:nth-child(2)").unwrap();
    let title_bold_sel = Selector::parse("b").unwrap();
    let authors_cell_sel = Selector::parse("td:nth-child(3)").unwrap();
    let author_link_sel = Selector::parse("a").unwrap();

    let mut out = Vec::new();
    for row in document.select(&row_sel) {
        let Some(title_cell) = row.select(&title_cell_sel).next() else {
            continue;
        };
        let Some(title_el) = title_cell.select(&title_bold_sel).next() else {
            continue;
        };
        let title: String = title_el.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }

        let Some(authors_cell) = row.select(&authors_cell_sel).next() else {
            continue;
        };
        let authors: Vec<String> = authors_cell
            .select(&author_link_sel)
            .map(|a| a.text().collect::<String>().trim().to_string())
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

    const SAMPLE: &str = r#"
        <table>
        <col width="5%"/><col width="65%"/><col width="25%"/>
        <tr><th></th><th>Paper</th><th>Author(s)</th><th>Code</th></tr>
        <tr>
        <td>1</td>
        <td><a href=https://www.paperdigest.org/reader/?paper_id=x><b>Cosmos Policy: Fine-Tuning Video Models for Visuomotor Control and Planning</b></a><br />
        <small><a href=https://www.paperdigest.org/review/?paper_id=x>Related Papers</a></small>
        <i><u>Highlight</u>: In this work, we introduce Cosmos Policy...</i></td>
        <td><a href=https://www.paperdigest.org/isearch/?name=moo_jin_kim>Moo Jin Kim</a>; <a href=https://www.paperdigest.org/isearch/?name=chelsea_finn>Chelsea Finn</a>;</td>
        <td><a href='https://github.com/example/repo' target='_blank'>code</a></td>
        </tr>
        <tr>
        <td>2</td>
        <td><a href=https://www.paperdigest.org/reader/?paper_id=y><b>A Second Paper</b></a><br />
        <i><u>Highlight</u>: Some other summary.</i></td>
        <td><a href=https://www.paperdigest.org/isearch/?name=solo_author>Solo Author</a>;</td>
        <td><a href='https://arxiv.org/abs/1234.5678' target='_blank'>code</a></td>
        </tr>
        </table>
    "#;

    #[test]
    fn test_header_row_skipped() {
        // The header row has <th>, not <td> — must not produce a bogus record.
        let out = parse_papers_with_code(SAMPLE, "iclr2026");
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn test_title_ignores_highlight_and_related_links_clutter() {
        let out = parse_papers_with_code(SAMPLE, "iclr2026");
        assert_eq!(
            out[0].title,
            "Cosmos Policy: Fine-Tuning Video Models for Visuomotor Control and Planning"
        );
    }

    #[test]
    fn test_authors_use_link_text_not_href_slug() {
        let out = parse_papers_with_code(SAMPLE, "iclr2026");
        // Names, not the lowercased/underscored `?name=` slug from the href.
        assert_eq!(out[0].authors, vec!["Moo Jin Kim", "Chelsea Finn"]);
        assert_eq!(out[0].source, "iclr2026");
    }

    #[test]
    fn test_solo_author_second_row() {
        let out = parse_papers_with_code(SAMPLE, "iclr2026");
        assert_eq!(out[1].title, "A Second Paper");
        assert_eq!(out[1].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_papers_with_code("<html></html>", "iclr2026").is_empty());
    }
}
