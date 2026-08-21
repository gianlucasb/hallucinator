//! Parse this tool's own report JSON export (`export_json` in
//! `hallucinator-reporting`) and pull out references the user marked safe
//! (`fp_reason` set) as corpus records.
//!
//! This is the fix for fp_overrides' exact-identity matching being too
//! brittle for citation-style drift (see `query` module docs): once a
//! marked-safe reference is in the corpus, any later citation of the same
//! paper — worded slightly differently, with a truncated author list — is
//! found via the same fuzzy FTS5 path used for every other database.

use serde::Deserialize;

use crate::db::NewPublication;

/// Prefix for the provenance tag on records imported this way. The specific
/// `fp_reason` is appended (e.g. `"marked_safe:known_good"`) so entries
/// stay auditable by why they were trusted, not just that they were.
const SOURCE_PREFIX: &str = "marked_safe";

#[derive(Debug, Deserialize)]
struct ReportFile {
    #[serde(default)]
    references: Vec<ReportRefJson>,
}

#[derive(Debug, Deserialize)]
struct ReportRefJson {
    title: String,
    #[serde(default)]
    fp_reason: Option<String>,
    #[serde(default)]
    ref_authors: Vec<String>,
    #[serde(default)]
    paper_url: Option<String>,
}

/// Parse one report JSON file's content (the tool's own `export_json`
/// output — an array of per-paper report objects) and return every
/// reference that was marked safe (`fp_reason` is set).
///
/// Titles with fewer than 3 words are skipped — these are the same class
/// of over-eager-fuzzy-match risk the rest of the pipeline already avoids
/// for short titles, worse here since a bad corpus entry keeps silently
/// matching future references forever.
pub fn parse_marked_safe(content: &str) -> Result<Vec<NewPublication>, serde_json::Error> {
    let papers: Vec<ReportFile> = serde_json::from_str(content)?;

    Ok(papers
        .into_iter()
        .flat_map(|p| p.references)
        .filter_map(|r| {
            let reason = r.fp_reason?;
            let title = r.title.trim().to_string();
            if title.split_whitespace().count() < 3 {
                return None;
            }
            Some(NewPublication {
                title,
                authors: r.ref_authors,
                url: r.paper_url,
                source: format!("{SOURCE_PREFIX}:{reason}"),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
        {
            "filename": "submission1.pdf",
            "verdict": "clean",
            "stats": {"total": 2, "verified": 1, "not_found": 0, "author_mismatch": 0, "retracted": 0, "skipped": 0, "problematic_pct": 0.0},
            "references": [
                {
                    "index": 0,
                    "original_number": 1,
                    "title": "A Paper That Is Real But Not Yet Indexed",
                    "raw_citation": "...",
                    "status": "not_found",
                    "effective_status": "verified",
                    "url_check_skipped": false,
                    "fp_reason": "known_good",
                    "source": null,
                    "ref_authors": ["Alice Smith", "Bob Jones"],
                    "found_authors": [],
                    "paper_url": "https://example.org/paper",
                    "failed_dbs": [],
                    "doi_info": null,
                    "arxiv_info": null,
                    "retraction_info": null,
                    "db_results": []
                },
                {
                    "index": 1,
                    "original_number": 2,
                    "title": "A Normally Verified Reference",
                    "raw_citation": "...",
                    "status": "verified",
                    "effective_status": "verified",
                    "url_check_skipped": false,
                    "fp_reason": null,
                    "source": "CrossRef",
                    "ref_authors": ["Carol White"],
                    "found_authors": ["Carol White"],
                    "paper_url": null,
                    "failed_dbs": [],
                    "doi_info": null,
                    "arxiv_info": null,
                    "retraction_info": null,
                    "db_results": []
                }
            ]
        }
    ]"#;

    #[test]
    fn test_only_marked_safe_refs_are_extracted() {
        let result = parse_marked_safe(SAMPLE).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].title, "A Paper That Is Real But Not Yet Indexed");
        assert_eq!(result[0].authors, vec!["Alice Smith", "Bob Jones"]);
        assert_eq!(result[0].url.as_deref(), Some("https://example.org/paper"));
        assert_eq!(result[0].source, "marked_safe:known_good");
    }

    #[test]
    fn test_short_title_skipped() {
        let json = r#"[{
            "filename": "x.pdf", "verdict": null,
            "stats": {"total": 1, "verified": 0, "not_found": 0, "author_mismatch": 0, "retracted": 0, "skipped": 0, "problematic_pct": 0.0},
            "references": [{
                "index": 0, "original_number": 1, "title": "Foo Bar",
                "raw_citation": "", "status": "not_found", "effective_status": "verified",
                "url_check_skipped": false, "fp_reason": "known_good", "source": null,
                "ref_authors": [], "found_authors": [], "paper_url": null, "failed_dbs": [],
                "doi_info": null, "arxiv_info": null, "retraction_info": null, "db_results": []
            }]
        }]"#;
        assert!(parse_marked_safe(json).unwrap().is_empty());
    }

    #[test]
    fn test_multiple_papers_flattened() {
        let json = r#"[
            {"filename": "a.pdf", "verdict": null,
             "stats": {"total": 0, "verified": 0, "not_found": 0, "author_mismatch": 0, "retracted": 0, "skipped": 0, "problematic_pct": 0.0},
             "references": []},
            {"filename": "b.pdf", "verdict": null,
             "stats": {"total": 0, "verified": 0, "not_found": 0, "author_mismatch": 0, "retracted": 0, "skipped": 0, "problematic_pct": 0.0},
             "references": []}
        ]"#;
        assert!(parse_marked_safe(json).unwrap().is_empty());
    }

    #[test]
    fn test_invalid_json_errors() {
        assert!(parse_marked_safe("not json").is_err());
    }
}
