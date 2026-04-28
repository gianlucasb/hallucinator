use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use hallucinator_core::{
    ArxivInfo, DbResult, DbStatus, DoiInfo, MismatchKind, RetractionInfo, Status, ValidationResult,
};

use crate::model::paper::{FpReason, RefPhase, RefState};
use crate::model::queue::{PaperPhase, PaperState, PaperVerdict};

// ---------------------------------------------------------------------------
// Deserialization structs — mirrors export.rs JSON schema.
// All non-essential fields are Option so we gracefully handle both the rich
// export format and the simplified persistence format.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct LoadedFile {
    filename: String,
    verdict: Option<String>,
    stats: Option<LoadedStats>,
    references: Vec<LoadedRef>,
}

#[derive(Deserialize)]
struct LoadedStats {
    total: Option<usize>,
    skipped: Option<usize>,
    // remaining fields are recomputed from results
}

#[derive(Deserialize)]
struct LoadedRef {
    index: usize,
    /// 1-based original reference number from the PDF (before skip filtering).
    original_number: Option<usize>,
    title: Option<String>,
    raw_citation: Option<String>,
    status: String,
    source: Option<String>,
    ref_authors: Option<Vec<String>>,
    found_authors: Option<Vec<String>>,
    paper_url: Option<String>,
    failed_dbs: Option<Vec<String>>,
    /// Simplified persistence format field (rich format uses retraction_info).
    retracted: Option<bool>,
    doi_info: Option<LoadedDoiInfo>,
    arxiv_info: Option<LoadedArxivInfo>,
    retraction_info: Option<LoadedRetractionInfo>,
    db_results: Option<Vec<LoadedDbResult>>,
    /// FP reason string (new format).
    fp_reason: Option<String>,
    /// Legacy boolean field — if true and no fp_reason, maps to KnownGood.
    marked_safe: Option<bool>,
    /// Skip reason (e.g. "url_only", "short_title") — present when status is "skipped".
    skip_reason: Option<String>,
    /// True iff the ref was rendered as "skipped" because the run had
    /// `--url-match` disabled and the ref was a NotFound with a
    /// non-academic URL still on hand. Present in exports from
    /// sessions where URL matching was opt-in.
    #[serde(default)]
    url_check_skipped: bool,
}

#[derive(Deserialize)]
struct LoadedDoiInfo {
    doi: String,
    valid: bool,
    title: Option<String>,
}

#[derive(Deserialize)]
struct LoadedArxivInfo {
    arxiv_id: String,
    valid: bool,
    title: Option<String>,
}

#[derive(Deserialize)]
struct LoadedRetractionInfo {
    is_retracted: bool,
    retraction_doi: Option<String>,
    retraction_source: Option<String>,
}

#[derive(Deserialize)]
struct LoadedDbResult {
    db: String,
    status: String,
    elapsed_ms: Option<u64>,
    authors: Option<Vec<String>>,
    url: Option<String>,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

fn parse_status(s: &str) -> Option<Status> {
    match s {
        "verified" => Some(Status::Verified),
        "not_found" => Some(Status::NotFound),
        // Handle both old and new mismatch formats
        "author_mismatch" => Some(Status::Mismatch(MismatchKind::AUTHOR)),
        "mismatch" => Some(Status::Mismatch(MismatchKind::AUTHOR)), // Default to author for generic mismatch
        s if s.starts_with("mismatch_") => {
            // Parse "mismatch_author", "mismatch_doi", "mismatch_arxiv_id", etc.
            let mut kind = MismatchKind::empty();
            if s.contains("author") {
                kind |= MismatchKind::AUTHOR;
            }
            if s.contains("doi") {
                kind |= MismatchKind::DOI;
            }
            if s.contains("arxiv") {
                kind |= MismatchKind::ARXIV_ID;
            }
            if kind.is_empty() {
                kind = MismatchKind::AUTHOR; // Default
            }
            Some(Status::Mismatch(kind))
        }
        _ => None, // "pending", "skipped", or unknown
    }
}

fn parse_verdict(s: &str) -> Option<PaperVerdict> {
    match s {
        "safe" | "SAFE" => Some(PaperVerdict::Safe),
        "questionable" | "?!" => Some(PaperVerdict::Questionable),
        _ => None,
    }
}

fn convert_db_status(s: &str) -> DbStatus {
    match s {
        "match" => DbStatus::Match,
        "no_match" => DbStatus::NoMatch,
        "author_mismatch" => DbStatus::AuthorMismatch,
        "timeout" => DbStatus::Timeout,
        "error" => DbStatus::Error,
        "skipped" => DbStatus::Skipped,
        _ => DbStatus::Error,
    }
}

/// Parse fp_reason from loaded JSON fields, with backward compat for marked_safe bool.
fn parse_fp_reason(loaded_ref: &LoadedRef) -> Option<FpReason> {
    if let Some(reason_str) = &loaded_ref.fp_reason {
        reason_str.parse().ok()
    } else if loaded_ref.marked_safe == Some(true) {
        // Legacy backward compat: marked_safe: true → KnownGood
        Some(FpReason::KnownGood)
    } else {
        None
    }
}

fn convert_loaded(loaded: LoadedFile) -> (PaperState, Vec<RefState>) {
    let ref_count = loaded.references.len();
    let mut paper = PaperState::new(loaded.filename);
    paper.phase = PaperPhase::Complete;
    paper.total_refs = ref_count;
    paper.init_results(ref_count);
    paper.verdict = loaded.verdict.as_deref().and_then(parse_verdict);

    let mut ref_states = Vec::with_capacity(ref_count);

    for loaded_ref in &loaded.references {
        let title = loaded_ref.title.clone().unwrap_or_default();
        let fp_reason = parse_fp_reason(loaded_ref);

        // Parse status — skip pending/unknown entries (no result to reconstruct)
        // original_number: use saved value, or fall back to index+1 for older exports
        let orig_num = loaded_ref.original_number.unwrap_or(loaded_ref.index + 1);

        // Handle skipped refs
        if loaded_ref.status == "skipped" {
            let reason = loaded_ref
                .skip_reason
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            let raw_cit = loaded_ref.raw_citation.clone().unwrap_or_default();
            let authors = loaded_ref.ref_authors.clone().unwrap_or_default();
            ref_states.push(RefState {
                index: orig_num.saturating_sub(1),
                title,
                phase: RefPhase::Skipped(reason),
                result: None,
                fp_reason,
                raw_citation: raw_cit,
                authors,
                doi: None,
                arxiv_id: None,
                urls: vec![],
            });
            continue;
        }

        let status = match parse_status(&loaded_ref.status) {
            Some(s) => s,
            None => {
                let raw_cit = loaded_ref.raw_citation.clone().unwrap_or_default();
                let authors = loaded_ref.ref_authors.clone().unwrap_or_default();
                let doi = loaded_ref.doi_info.as_ref().map(|d| d.doi.clone());
                let arxiv_id = loaded_ref.arxiv_info.as_ref().map(|a| a.arxiv_id.clone());
                ref_states.push(RefState {
                    index: orig_num.saturating_sub(1),
                    title,
                    phase: RefPhase::Done,
                    result: None,
                    fp_reason,
                    raw_citation: raw_cit,
                    authors,
                    doi,
                    arxiv_id,
                    urls: vec![],
                });
                continue;
            }
        };

        // Build DOI info
        let doi_info = loaded_ref.doi_info.as_ref().map(|d| DoiInfo {
            doi: d.doi.clone(),
            valid: d.valid,
            title: d.title.clone(),
        });

        // Build arXiv info
        let arxiv_info = loaded_ref.arxiv_info.as_ref().map(|a| ArxivInfo {
            arxiv_id: a.arxiv_id.clone(),
            valid: a.valid,
            title: a.title.clone(),
        });

        // Build retraction info — prefer rich retraction_info, fall back to bool flag
        let retraction_info = if let Some(ret) = &loaded_ref.retraction_info {
            Some(RetractionInfo {
                is_retracted: ret.is_retracted,
                retraction_doi: ret.retraction_doi.clone(),
                retraction_source: ret.retraction_source.clone(),
            })
        } else if loaded_ref.retracted == Some(true) {
            Some(RetractionInfo {
                is_retracted: true,
                retraction_doi: None,
                retraction_source: None,
            })
        } else {
            None
        };

        // Build per-DB results
        let db_results = loaded_ref
            .db_results
            .as_ref()
            .map(|dbs| {
                dbs.iter()
                    .map(|db| DbResult {
                        db_name: db.db.clone(),
                        status: convert_db_status(&db.status),
                        elapsed: db.elapsed_ms.map(Duration::from_millis),
                        found_authors: db.authors.clone().unwrap_or_default(),
                        paper_url: db.url.clone(),
                        error_message: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Normalize source: empty string → None
        let source = loaded_ref
            .source
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned();

        let result = ValidationResult {
            title: title.clone(),
            raw_citation: loaded_ref.raw_citation.clone().unwrap_or_default(),
            ref_authors: loaded_ref.ref_authors.clone().unwrap_or_default(),
            status,
            source,
            found_authors: loaded_ref.found_authors.clone().unwrap_or_default(),
            paper_url: loaded_ref.paper_url.clone(),
            failed_dbs: loaded_ref.failed_dbs.clone().unwrap_or_default(),
            db_results,
            doi_info: doi_info.clone(),
            arxiv_info: arxiv_info.clone(),
            retraction_info,
            url_check_skipped: loaded_ref.url_check_skipped,
        };

        let is_retracted = result
            .retraction_info
            .as_ref()
            .is_some_and(|r| r.is_retracted);
        paper.record_status(
            loaded_ref.index,
            result.status.clone(),
            result.url_check_skipped,
            is_retracted,
        );
        // If this ref was persisted with an fp_reason, carry the
        // mark-safe adjustment into the paper stats so the queue table
        // and totals line reflect the prior user decision on load.
        if fp_reason.is_some() {
            paper.apply_fp_delta(&result.status, result.url_check_skipped, is_retracted, 1);
        }

        let raw_cit = loaded_ref.raw_citation.clone().unwrap_or_default();
        let ref_authors = loaded_ref.ref_authors.clone().unwrap_or_default();
        let ref_doi = doi_info.as_ref().map(|d| d.doi.clone());
        let ref_arxiv = arxiv_info.as_ref().map(|a| a.arxiv_id.clone());
        ref_states.push(RefState {
            index: orig_num.saturating_sub(1),
            title: title.clone(),
            phase: RefPhase::Done,
            result: Some(result),
            fp_reason,
            raw_citation: raw_cit,
            authors: ref_authors,
            doi: ref_doi,
            arxiv_id: ref_arxiv,
            urls: vec![],
        });
    }

    // Sort ref_states by original position so they align
    // with paper.results (which is indexed by original position).
    // This handles JSON files where entries are sorted by severity.
    ref_states.sort_by_key(|rs| rs.index);

    // Set total and skipped from loaded stats if available
    if let Some(stats) = &loaded.stats {
        let total = stats.total.filter(|&t| t > 0).unwrap_or(ref_count);
        paper.stats.total = total;
        paper.total_refs = total;
        paper.stats.skipped = stats.skipped.unwrap_or(0);
    } else {
        paper.stats.total = ref_count;
        paper.total_refs = ref_count;
    }

    // `parse_skipped` is only the parse-time subset of skipped refs —
    // those that were marked "skipped" AND did NOT carry the URL-gate
    // marker. Needed by the gauge denominator; must NOT include URL-
    // gated refs because those would have entered validation on a
    // live run and contribute to `done`.
    paper.parse_skipped = loaded
        .references
        .iter()
        .filter(|r| r.status == "skipped" && !r.url_check_skipped)
        .count();

    (paper, ref_states)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load previously saved results from a JSON file.
///
/// Handles both formats:
/// - **Export format**: JSON array of paper objects (from TUI export or `--load`)
/// - **Persistence format**: Single JSON object (from auto-save in `~/.cache/hallucinator/runs/`)
pub fn load_results_file(path: &Path) -> Result<Vec<(PaperState, Vec<RefState>)>, String> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    let loaded_files: Vec<LoadedFile> =
        if let Ok(arr) = serde_json::from_str::<Vec<LoadedFile>>(&content) {
            arr
        } else if let Ok(single) = serde_json::from_str::<LoadedFile>(&content) {
            vec![single]
        } else {
            return Err(
                "Invalid JSON: expected export format (array) or persistence format (object)"
                    .to_string(),
            );
        };

    if loaded_files.is_empty() {
        return Err("JSON file contains no papers".to_string());
    }

    Ok(loaded_files.into_iter().map(convert_loaded).collect())
}

#[cfg(test)]
mod export_regression_tests {
    use super::*;
    use hallucinator_reporting::{ExportFormat, ReportPaper, ReportRef, SkipInfo, export_results};
    use std::io::Write;

    /// Synthetic-fixture version of the regression test for CI / fresh
    /// checkouts that don't have the v3 JSON.
    #[test]
    fn html_export_after_json_load_marks_fp_refs_as_verified() {
        // Minimal JSON in severity-sorted order (not_found ref first,
        // even though its original_number is 3) — the same shape as
        // usenix2026-v3.json.
        let json = r#"[
  {
    "filename": "fixture.pdf",
    "verdict": null,
    "stats": {"total": 3, "skipped": 0},
    "references": [
      {
        "index": 2,
        "original_number": 3,
        "title": "Marked Safe Paper",
        "raw_citation": "Some Author. Marked Safe Paper. 2024",
        "status": "not_found",
        "effective_status": "not_found",
        "url_check_skipped": false,
        "fp_reason": "non_academic",
        "source": null,
        "ref_authors": ["Some Author"],
        "found_authors": [],
        "paper_url": null,
        "failed_dbs": [],
        "doi_info": null,
        "arxiv_info": null,
        "retraction_info": null,
        "db_results": []
      },
      {
        "index": 0,
        "original_number": 1,
        "title": "Verified Paper",
        "raw_citation": "Author 1. Verified Paper. 2024",
        "status": "verified",
        "effective_status": "verified",
        "url_check_skipped": false,
        "fp_reason": null,
        "source": "arxiv",
        "ref_authors": ["Author 1"],
        "found_authors": ["Author 1"],
        "paper_url": "https://arxiv.org/abs/0001",
        "failed_dbs": [],
        "doi_info": null,
        "arxiv_info": null,
        "retraction_info": null,
        "db_results": []
      },
      {
        "index": 1,
        "original_number": 2,
        "title": "Plain Not Found",
        "raw_citation": "Author 2. Plain Not Found. 2024",
        "status": "not_found",
        "effective_status": "not_found",
        "url_check_skipped": false,
        "fp_reason": null,
        "source": null,
        "ref_authors": ["Author 2"],
        "found_authors": [],
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
        let dir = tempfile::tempdir().expect("tempdir");
        let json_path = dir.path().join("fixture.json");
        std::fs::File::create(&json_path)
            .and_then(|mut f| f.write_all(json.as_bytes()))
            .expect("write json");

        let loaded = load_results_file(&json_path).expect("load");
        assert_eq!(loaded.len(), 1);
        let (paper, ref_states) = &loaded[0];

        // Build the export structures EXACTLY the way app/update.rs does.
        let results_vec: Vec<Option<hallucinator_core::ValidationResult>> =
            ref_states.iter().map(|rs| rs.result.clone()).collect();
        let report_paper = ReportPaper {
            filename: &paper.filename,
            stats: &paper.stats,
            results: &results_vec,
            verdict: paper.verdict,
        };
        let report_refs: Vec<ReportRef> = ref_states
            .iter()
            .map(|rs| ReportRef {
                index: rs.index,
                title: rs.title.clone(),
                skip_info: if let RefPhase::Skipped(reason) = &rs.phase {
                    Some(SkipInfo {
                        reason: reason.clone(),
                    })
                } else {
                    None
                },
                fp_reason: rs.fp_reason,
            })
            .collect();
        let ref_slices: &[&[ReportRef]] = &[&report_refs];

        let html_path = dir.path().join("fixture.html");
        export_results(
            &[report_paper],
            ref_slices,
            ExportFormat::Html,
            &html_path,
            false,
        )
        .expect("export html");

        let html = std::fs::read_to_string(&html_path).expect("read html");

        // The FP-marked ref ("Marked Safe Paper") MUST show as Verified
        // — its badge cannot be "Not Found". The ref-card spans from
        // an opening `<div class="ref-card" data-status="...">` through
        // a closing `</div>` matched at the same depth. Slice between
        // the title and the next ref-card to get this card's body.
        let marked_idx = html
            .find("Marked Safe Paper")
            .expect("FP-marked ref title should appear in HTML");
        let after_title = &html[marked_idx..];
        // The next ref-card or the end of the papers block bounds us.
        let card_end = after_title
            .find("<div class=\"ref-card\"")
            .or_else(|| after_title.find("</div>\n</details>"))
            .unwrap_or(after_title.len());
        let card = &after_title[..card_end];
        assert!(
            !card.contains("badge not-found"),
            "FP-marked ref's badge is 'not-found' — bug reproduced. Card body:\n{card}"
        );
        assert!(
            card.contains("badge verified"),
            "FP-marked ref's badge should be 'verified'. Card body:\n{card}"
        );

        // Sanity: the unmarked NotFound ref ("Plain Not Found") MUST
        // still render with the not-found badge — we shouldn't have
        // accidentally promoted everything.
        let plain_idx = html
            .find("Plain Not Found")
            .expect("Plain not-found title should appear in HTML");
        let plain_after = &html[plain_idx..];
        let plain_end = plain_after
            .find("<div class=\"ref-card\"")
            .or_else(|| plain_after.find("</div>\n</details>"))
            .unwrap_or(plain_after.len());
        let plain_card = &plain_after[..plain_end];
        assert!(
            plain_card.contains("badge not-found"),
            "Unmarked NotFound ref must still render the not-found badge"
        );
    }
}
