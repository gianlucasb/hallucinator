use axum::response::sse::Event;
use hallucinator_core::{SkipStats, Status, ValidationResult};
use serde::{Deserialize, Serialize};

// ── Result JSON (matches Python's per-reference JSON shape) ─────────────

#[derive(Debug, Clone, Serialize)]
pub struct ResultJson {
    pub title: String,
    pub raw_citation: String,
    pub ref_authors: Vec<String>,
    pub status: String,
    pub source: Option<String>,
    pub found_authors: Vec<String>,
    pub paper_url: Option<String>,
    pub error_type: Option<String>,
    pub failed_dbs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi_info: Option<DoiInfoJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arxiv_info: Option<ArxivInfoJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retraction_info: Option<RetractionInfoJson>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoiInfoJson {
    pub doi: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi_title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArxivInfoJson {
    pub arxiv_id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arxiv_title: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RetractionInfoJson {
    pub retracted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retraction_doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retraction_type: Option<String>,
}

impl From<&ValidationResult> for ResultJson {
    fn from(r: &ValidationResult) -> Self {
        let status_str = match r.status {
            Status::Verified => "verified",
            Status::NotFound => "not_found",
            Status::AuthorMismatch => "author_mismatch",
        };

        let error_type = match r.status {
            Status::NotFound => Some("not_found".to_string()),
            Status::AuthorMismatch => Some("author_mismatch".to_string()),
            Status::Verified => None,
        };

        let doi_info = r.doi_info.as_ref().map(|d| DoiInfoJson {
            doi: d.doi.clone(),
            status: if d.valid { "verified" } else { "invalid" }.to_string(),
            doi_title: d.title.clone(),
        });

        let arxiv_info = r.arxiv_info.as_ref().map(|a| ArxivInfoJson {
            arxiv_id: a.arxiv_id.clone(),
            status: if a.valid { "verified" } else { "invalid" }.to_string(),
            arxiv_title: a.title.clone(),
        });

        let retraction_info = r.retraction_info.as_ref().map(|ri| RetractionInfoJson {
            retracted: ri.is_retracted,
            retraction_doi: ri.retraction_doi.clone(),
            retraction_type: ri.retraction_source.clone(),
        });

        ResultJson {
            title: r.title.clone(),
            raw_citation: r.raw_citation.clone(),
            ref_authors: r.ref_authors.clone(),
            status: status_str.to_string(),
            source: r.source.clone(),
            found_authors: r.found_authors.clone(),
            paper_url: r.paper_url.clone(),
            error_type,
            failed_dbs: r.failed_dbs.clone(),
            doi_info,
            arxiv_info,
            retraction_info,
        }
    }
}

// ── Summary JSON ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Default)]
pub struct SummaryJson {
    pub total_raw: usize,
    pub total: usize,
    pub verified: usize,
    pub not_found: usize,
    pub mismatched: usize,
    pub skipped: usize,
    pub skipped_url: usize,
    pub skipped_short_title: usize,
    pub title_only: usize,
}

impl SummaryJson {
    pub fn from_results(results: &[ValidationResult], skip_stats: &SkipStats) -> Self {
        let verified = results
            .iter()
            .filter(|r| r.status == Status::Verified)
            .count();
        let not_found = results
            .iter()
            .filter(|r| r.status == Status::NotFound)
            .count();
        let mismatched = results
            .iter()
            .filter(|r| r.status == Status::AuthorMismatch)
            .count();

        SummaryJson {
            total_raw: skip_stats.total_raw,
            total: results.len(),
            verified,
            not_found,
            mismatched,
            skipped: skip_stats.url_only + skip_stats.short_title,
            skipped_url: skip_stats.url_only,
            skipped_short_title: skip_stats.short_title,
            title_only: skip_stats.no_authors,
        }
    }
}

// ── SSE Event Structs ───────────────────────────────────────────────────

#[derive(Serialize)]
pub struct ExtractionCompleteEvent {
    pub total_refs: usize,
    pub skip_stats: SkipStatsJson,
}

#[derive(Serialize)]
pub struct SkipStatsJson {
    pub total_raw: usize,
    pub skipped_url: usize,
    pub skipped_short_title: usize,
    pub skipped_no_authors: usize,
}

#[derive(Serialize)]
pub struct CheckingEvent {
    pub index: usize,
    pub total: usize,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Serialize)]
pub struct ResultEvent {
    #[serde(flatten)]
    pub result: ResultJson,
    pub index: usize,
    pub total: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Serialize)]
pub struct WarningEvent {
    pub index: usize,
    pub total: usize,
    pub title: String,
    pub failed_dbs: Vec<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Serialize)]
pub struct RetryPassEvent {
    pub count: usize,
}

#[derive(Serialize)]
pub struct ArchiveStartEvent {
    pub file_count: usize,
}

#[derive(Serialize)]
pub struct FileStartEvent {
    pub file_index: usize,
    pub file_count: usize,
    pub filename: String,
}

#[derive(Serialize)]
pub struct FileCompleteEvent {
    pub filename: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryJson>,
    pub results: Vec<ResultJson>,
}

#[derive(Serialize)]
pub struct CompleteEvent {
    pub summary: SummaryJson,
    pub results: Vec<ResultJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileResultJson>>,
}

#[derive(Serialize)]
pub struct FileResultJson {
    pub filename: String,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<SummaryJson>,
    pub results: Vec<ResultJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Sent when analysis is cancelled (e.g. server-initiated cancellation).
/// Currently only used client-side via AbortController; kept for future use.
#[derive(Serialize)]
#[allow(dead_code)]
pub struct CancelledEvent {
    pub summary: SummaryJson,
    pub results: Vec<ResultJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files: Option<Vec<FileResultJson>>,
    pub message: String,
}

#[derive(Serialize)]
pub struct ErrorEvent {
    pub message: String,
}

// ── Retry DTOs ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RetryRequest {
    pub title: String,
    #[serde(default)]
    pub ref_authors: Vec<String>,
    pub failed_dbs: Vec<String>,
    pub openalex_key: Option<String>,
    pub s2_api_key: Option<String>,
    #[serde(default)]
    pub check_openalex_authors: bool,
}

#[derive(Serialize)]
pub struct RetryResponse {
    pub success: bool,
    pub status: String,
    pub source: Option<String>,
    pub found_authors: Vec<String>,
    pub paper_url: Option<String>,
    pub error_type: Option<String>,
    pub failed_dbs: Vec<String>,
}

// ── SSE Helper ──────────────────────────────────────────────────────────

pub fn sse_event<T: Serialize>(event_type: &str, data: &T) -> Event {
    Event::default()
        .event(event_type)
        .data(serde_json::to_string(data).unwrap_or_default())
}
