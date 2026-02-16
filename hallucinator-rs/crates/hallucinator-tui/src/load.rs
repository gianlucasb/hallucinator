use std::path::Path;
use std::time::Duration;

use serde::Deserialize;

use hallucinator_core::{
    ArxivInfo, DbResult, DbStatus, DoiInfo, Reference, RetractionInfo, Status, ValidationResult,
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
        "author_mismatch" => Some(Status::AuthorMismatch),
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

fn convert_loaded(loaded: LoadedFile) -> (PaperState, Vec<RefState>, Vec<Reference>) {
    let ref_count = loaded.references.len();
    let mut paper = PaperState::new(loaded.filename);
    paper.phase = PaperPhase::Complete;
    paper.total_refs = ref_count;
    paper.init_results(ref_count);
    paper.verdict = loaded.verdict.as_deref().and_then(parse_verdict);

    let mut ref_states = Vec::with_capacity(ref_count);
    let mut references = Vec::with_capacity(ref_count);

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
                title: title.clone(),
                phase: RefPhase::Skipped(reason.clone()),
                result: None,
                fp_reason,
                raw_citation: raw_cit.clone(),
                authors: authors.clone(),
                doi: None,
                arxiv_id: None,
            });
            references.push(Reference {
                raw_citation: raw_cit,
                title: if title.is_empty() { None } else { Some(title) },
                authors,
                doi: None,
                arxiv_id: None,
                original_number: orig_num,
                skip_reason: Some(reason),
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
                    title: title.clone(),
                    phase: RefPhase::Done,
                    result: None,
                    fp_reason,
                    raw_citation: raw_cit.clone(),
                    authors: authors.clone(),
                    doi: doi.clone(),
                    arxiv_id: arxiv_id.clone(),
                });
                references.push(Reference {
                    raw_citation: raw_cit,
                    title: if title.is_empty() { None } else { Some(title) },
                    authors,
                    doi,
                    arxiv_id,
                    original_number: orig_num,
                    skip_reason: None,
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
        };

        paper.record_result(loaded_ref.index, result.clone());

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
            raw_citation: raw_cit.clone(),
            authors: ref_authors.clone(),
            doi: ref_doi.clone(),
            arxiv_id: ref_arxiv.clone(),
        });

        references.push(Reference {
            raw_citation: raw_cit,
            title: if title.is_empty() { None } else { Some(title) },
            authors: ref_authors,
            doi: ref_doi,
            arxiv_id: ref_arxiv,
            original_number: orig_num,
            skip_reason: None,
        });
    }

    // Sort ref_states and references by original position so they align
    // with paper.results (which is indexed by original position).
    // This handles JSON files where entries are sorted by severity.
    let mut pairs: Vec<(RefState, Reference)> = ref_states.into_iter().zip(references).collect();
    pairs.sort_by_key(|(rs, _)| rs.index);
    let (sorted_states, sorted_refs): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
    ref_states = sorted_states;
    references = sorted_refs;

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

    (paper, ref_states, references)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load previously saved results from a JSON file.
///
/// Handles both formats:
/// - **Export format**: JSON array of paper objects (from TUI export or `--load`)
/// - **Persistence format**: Single JSON object (from auto-save in `~/.cache/hallucinator/runs/`)
#[allow(clippy::type_complexity)]
pub fn load_results_file(
    path: &Path,
) -> Result<Vec<(PaperState, Vec<RefState>, Vec<Reference>)>, String> {
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
