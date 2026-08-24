//! Parse the KDD accepted-papers page into corpus records.
//!
//! Unlike every other venue here, KDD's paper list isn't in the HTML at
//! all — the page renders it client-side from data embedded as plain
//! JavaScript array literals: `const cycle1Papers = [{"track": ...,
//! "title": ..., "url": ..., "authors": "Name (Affiliation); Name
//! (Affiliation)"}, ...];` (and a second `cycle2Papers` array, same
//! shape). Since each array literal *is* valid JSON, this locates each
//! `const cycleNPapers = ` marker and hands the text starting at that
//! array straight to `serde_json`'s streaming deserializer, which parses
//! just the one JSON value and stops — ignoring the trailing `;` and the
//! rest of the `<script>` block — rather than trying to scrape any HTML
//! structure.
//!
//! `authors` is `"Name (Affiliation); Name (Affiliation); ..."` — the
//! same shape [`parse_paren_grouped_names`] already handles.

use serde::Deserialize;

use crate::db::NewPublication;
use crate::ingest::author_parsing::parse_paren_grouped_names;

#[derive(Deserialize)]
struct KddPaper {
    title: String,
    authors: String,
}

/// Every `const cycleNPapers = [...]` variable name this page has used so
/// far. New cycles (if KDD ever adds a `cycle3Papers`) just need adding
/// here.
const CYCLE_MARKERS: &[&str] = &["const cycle1Papers = ", "const cycle2Papers = "];

/// Parse a KDD accepted-papers page (its embedded JS data, not its HTML)
/// into publication records. `source_tag` is the provenance string to
/// store on each record, e.g. `"kdd2026"`.
pub fn parse_accepted_papers(html: &str, source_tag: &str) -> Vec<NewPublication> {
    let mut out = Vec::new();
    for marker in CYCLE_MARKERS {
        let Some(start) = html.find(marker) else {
            continue;
        };
        let array_start = &html[start + marker.len()..];
        let mut stream =
            serde_json::Deserializer::from_str(array_start).into_iter::<Vec<KddPaper>>();
        let Some(Ok(papers)) = stream.next() else {
            continue;
        };

        for paper in papers {
            let title = paper.title.trim().to_string();
            if title.is_empty() {
                continue;
            }
            let authors = parse_paren_grouped_names(&paper.authors);
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
        <script>
        const cycle1Papers = [{"track": "rtp", "title": "Spatiotemporal Graph Learning", "url": "https://doi.org/10.1145/example1", "authors": "Yuan Mi (Renmin University of China); Qi Wang (Renmin University of China)"}, {"track": "rtp", "title": "Towards Self-cognitive Exploration", "url": "https://doi.org/10.1145/example2", "authors": "Xujie Yuan (Sun Yat-Sen University)"}];
        const cycle2Papers = [{"track": "dtb", "title": "RELIANCE: Curating Reproductive Health Information", "url": "https://doi.org/10.1145/example3", "authors": "Vaibhav Balloli (University of Michigan); Laura Peyton Ellis (University of Connecticut Health)"}];
        </script>
    "#;

    #[test]
    fn test_both_cycles_parsed() {
        let out = parse_accepted_papers(SAMPLE, "kdd2026");
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].source, "kdd2026");
    }

    #[test]
    fn test_cycle1_titles_and_authors() {
        let out = parse_accepted_papers(SAMPLE, "kdd2026");
        assert_eq!(out[0].title, "Spatiotemporal Graph Learning");
        assert_eq!(out[0].authors, vec!["Yuan Mi", "Qi Wang"]);
        assert_eq!(out[1].authors, vec!["Xujie Yuan"]);
    }

    #[test]
    fn test_cycle2_trailing_script_content_ignored() {
        // The streaming deserializer must stop at the array's closing `]`
        // and not choke on the trailing `;` / </script> that follows.
        let out = parse_accepted_papers(SAMPLE, "kdd2026");
        assert_eq!(
            out[2].title,
            "RELIANCE: Curating Reproductive Health Information"
        );
        assert_eq!(
            out[2].authors,
            vec!["Vaibhav Balloli", "Laura Peyton Ellis"]
        );
    }

    #[test]
    fn test_parse_empty_html() {
        assert!(parse_accepted_papers("<html></html>", "kdd2026").is_empty());
    }
}
