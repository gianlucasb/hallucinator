use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

pub mod authors;
pub mod checker;
pub mod db;
pub mod doi;
pub mod matching;
pub mod orchestrator;
pub mod retraction;

// Re-export for convenience
pub use hallucinator_pdf::{ExtractionResult, Reference, SkipStats};

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("PDF extraction error: {0}")]
    Pdf(#[from] hallucinator_pdf::PdfError),
    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("DBLP error: {0}")]
    Dblp(#[from] hallucinator_dblp::DblpError),
    #[error("validation error: {0}")]
    Validation(String),
}

/// The validation status of a reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Verified,
    NotFound,
    AuthorMismatch,
}

/// Information about a DOI lookup.
#[derive(Debug, Clone)]
pub struct DoiInfo {
    pub doi: String,
    pub valid: bool,
    pub title: Option<String>,
}

/// Information about an arXiv lookup.
#[derive(Debug, Clone)]
pub struct ArxivInfo {
    pub arxiv_id: String,
    pub valid: bool,
    pub title: Option<String>,
}

/// Information about a retraction check.
#[derive(Debug, Clone)]
pub struct RetractionInfo {
    pub is_retracted: bool,
    pub retraction_doi: Option<String>,
    pub retraction_source: Option<String>,
}

/// The result of validating a single reference.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub title: String,
    pub raw_citation: String,
    pub status: Status,
    pub source: Option<String>,
    pub found_authors: Vec<String>,
    pub paper_url: Option<String>,
    pub failed_dbs: Vec<String>,
    pub doi_info: Option<DoiInfo>,
    pub arxiv_info: Option<ArxivInfo>,
    pub retraction_info: Option<RetractionInfo>,
}

/// Progress events emitted during validation.
#[derive(Debug, Clone)]
pub enum ProgressEvent {
    Checking {
        index: usize,
        total: usize,
        title: String,
    },
    Result {
        index: usize,
        total: usize,
        result: ValidationResult,
    },
    Warning {
        message: String,
    },
    RetryPass {
        count: usize,
    },
}

/// Summary statistics for a complete check run.
#[derive(Debug, Clone, Default)]
pub struct CheckStats {
    pub total: usize,
    pub verified: usize,
    pub not_found: usize,
    pub author_mismatch: usize,
    pub retracted: usize,
    pub skipped: usize,
}

/// Configuration for the reference checker.
#[derive(Clone)]
pub struct Config {
    pub openalex_key: Option<String>,
    pub s2_api_key: Option<String>,
    pub dblp_offline_path: Option<PathBuf>,
    pub dblp_offline_db: Option<Arc<Mutex<hallucinator_dblp::DblpDatabase>>>,
    pub max_concurrent_refs: usize,
    pub db_timeout_secs: u64,
    pub db_timeout_short_secs: u64,
    pub disabled_dbs: Vec<String>,
    pub check_openalex_authors: bool,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("openalex_key", &self.openalex_key.as_ref().map(|_| "***"))
            .field("s2_api_key", &self.s2_api_key.as_ref().map(|_| "***"))
            .field("dblp_offline_path", &self.dblp_offline_path)
            .field("dblp_offline_db", &self.dblp_offline_db.as_ref().map(|_| "<open>"))
            .field("max_concurrent_refs", &self.max_concurrent_refs)
            .field("db_timeout_secs", &self.db_timeout_secs)
            .field("db_timeout_short_secs", &self.db_timeout_short_secs)
            .field("disabled_dbs", &self.disabled_dbs)
            .field("check_openalex_authors", &self.check_openalex_authors)
            .finish()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            openalex_key: None,
            s2_api_key: None,
            dblp_offline_path: None,
            dblp_offline_db: None,
            max_concurrent_refs: 4,
            db_timeout_secs: 10,
            db_timeout_short_secs: 5,
            disabled_dbs: vec![],
            check_openalex_authors: false,
        }
    }
}

/// Check a list of references against academic databases.
///
/// Validates each reference concurrently, querying multiple databases in parallel.
/// Progress events are emitted via the callback. The operation can be cancelled
/// via the CancellationToken.
pub async fn check_references(
    refs: Vec<Reference>,
    config: Config,
    progress: impl Fn(ProgressEvent) + Send + Sync + 'static,
    cancel: CancellationToken,
) -> Vec<ValidationResult> {
    checker::check_references(refs, config, progress, cancel).await
}
