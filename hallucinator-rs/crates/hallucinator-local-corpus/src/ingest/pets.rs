//! Parse the PETS/PoPETs accepted-papers page into corpus records.
//!
//! PoPETs (Proceedings on Privacy Enhancing Technologies) publishes
//! quarterly, and `petsymposium.org/<year>/paperlist.php` lists every
//! issue's accepted papers on one page. Structure: `div.accepted-list`
//! (one per issue) → `ul` → `li`, where the title is the `<li>`'s own
//! direct text (its first non-empty text-node child — everything after
//! that, an optional `artifact-*` badge link and an italic `<span>` of
//! authors, comes from child *elements*, not direct text, so collecting
//! only direct text nodes cleanly isolates the title without needing to
//! know whether an artifact badge is present). Authors are
//! `"Name (Affiliation), Name (Affiliation), and Name (Affiliation)"` —
//! the same shape [`parse_paren_grouped_names`] already handles.

use scraper::Selector;

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;
use crate::ingest::dom_text::direct_text;

/// Parse a PETS/PoPETs paper-list page into publication records.
///
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"pets2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let document = scraper::Html::parse_document(html);
    let item_sel = Selector::parse("div.accepted-list li").unwrap();
    let authors_sel = Selector::parse("span").unwrap();

    let mut out = Vec::new();
    for item in document.select(&item_sel) {
        let title = direct_text(&item);
        if title.is_empty() {
            continue;
        }

        let Some(authors_el) = item.select(&authors_sel).next() else {
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
        <h3 class="side-headings">Issue 1</h3>
        <div class="accepted-list">
        <ul>
        <li>How We Define Privacy Literacy: Teaching Experiences &amp; Challenges<br />
        <span style="font-style: italic;">Tanisha Afnan (University of Michigan), Sheza Naveed (University of Michigan), and Florian Schaub (University of Michigan)</span></li>

        <li>Obscura: Enabling Ephemeral Proxies <a href="https://github.com/example" target="_blank" class="artifact-available artifact-functional">Artifact: Available, Functional</a><br />
        <span style="font-style: italic;">Afonso Vilalonga (NOVA LINCS), Kevin Gallagher (NOVA LINCS)</span></li>
        </ul>
        </div>
        <h3 class="side-headings">Issue 2</h3>
        <div class="accepted-list">
        <ul>
        <li>A Second-Issue Paper<br />
        <span style="font-style: italic;">Solo Author (Some University)</span></li>
        </ul>
        </div>
    "#;

    #[test]
    fn test_title_without_artifact_badge() {
        let out = parse_accepted_papers(SAMPLE, "pets2026");
        assert_eq!(
            out[0].title,
            "How We Define Privacy Literacy: Teaching Experiences & Challenges"
        );
        assert_eq!(
            out[0].authors,
            vec!["Tanisha Afnan", "Sheza Naveed", "Florian Schaub"]
        );
        assert_eq!(out[0].source, "pets2026");
    }

    #[test]
    fn test_title_ignores_artifact_badge_link_text() {
        let out = parse_accepted_papers(SAMPLE, "pets2026");
        // Direct-text extraction must not pick up "Artifact: Available, Functional"
        // from the badge <a>, which is a child *element*, not direct text.
        assert_eq!(out[1].title, "Obscura: Enabling Ephemeral Proxies");
        assert_eq!(out[1].authors, vec!["Afonso Vilalonga", "Kevin Gallagher"]);
    }

    #[test]
    fn test_second_issue_picked_up() {
        let out = parse_accepted_papers(SAMPLE, "pets2026");
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].title, "A Second-Issue Paper");
        assert_eq!(out[2].authors, vec!["Solo Author"]);
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "pets2026").is_empty());
    }
}
