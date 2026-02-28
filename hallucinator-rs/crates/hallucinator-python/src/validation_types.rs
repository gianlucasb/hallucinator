use pyo3::prelude::*;

use hallucinator_core::{
    ArxivInfo, CheckStats, DbResult, DbStatus, DoiInfo, MismatchKind, ProgressEvent,
    RetractionInfo, Status, ValidationResult,
};

// ── PyValidationResult ──

/// The result of validating a single reference against academic databases.
#[pyclass(name = "ValidationResult")]
#[derive(Debug, Clone)]
pub struct PyValidationResult {
    inner: ValidationResult,
}

impl From<ValidationResult> for PyValidationResult {
    fn from(r: ValidationResult) -> Self {
        Self { inner: r }
    }
}

impl PyValidationResult {
    pub(crate) fn inner(&self) -> &ValidationResult {
        &self.inner
    }
}

#[pymethods]
impl PyValidationResult {
    /// The reference title.
    #[getter]
    fn title(&self) -> &str {
        &self.inner.title
    }

    /// The raw citation text.
    #[getter]
    fn raw_citation(&self) -> &str {
        &self.inner.raw_citation
    }

    /// Authors from the parsed reference.
    #[getter]
    fn ref_authors(&self) -> Vec<String> {
        self.inner.ref_authors.clone()
    }

    /// Validation status: "verified", "not_found", or "mismatch".
    #[getter]
    fn status(&self) -> &str {
        match &self.inner.status {
            Status::Verified => "verified",
            Status::NotFound => "not_found",
            Status::Mismatch(_) => "mismatch",
        }
    }

    /// Mismatch kind description (if status is "mismatch").
    #[getter]
    fn mismatch_kind(&self) -> Option<String> {
        match &self.inner.status {
            Status::Mismatch(kind) => Some(kind.description()),
            _ => None,
        }
    }

    /// The database source that verified this reference, if any.
    #[getter]
    fn source(&self) -> Option<&str> {
        self.inner.source.as_deref()
    }

    /// Authors found in the matching database record.
    #[getter]
    fn found_authors(&self) -> Vec<String> {
        self.inner.found_authors.clone()
    }

    /// URL of the paper in the matching database, if any.
    #[getter]
    fn paper_url(&self) -> Option<&str> {
        self.inner.paper_url.as_deref()
    }

    /// List of database names that failed/timed out.
    #[getter]
    fn failed_dbs(&self) -> Vec<String> {
        self.inner.failed_dbs.clone()
    }

    /// Per-database query results.
    #[getter]
    fn db_results(&self) -> Vec<PyDbResult> {
        self.inner
            .db_results
            .iter()
            .cloned()
            .map(PyDbResult::from)
            .collect()
    }

    /// DOI lookup information, if a DOI was found.
    #[getter]
    fn doi_info(&self) -> Option<PyDoiInfo> {
        self.inner.doi_info.clone().map(PyDoiInfo::from)
    }

    /// arXiv lookup information, if an arXiv ID was found.
    #[getter]
    fn arxiv_info(&self) -> Option<PyArxivInfo> {
        self.inner.arxiv_info.clone().map(PyArxivInfo::from)
    }

    /// Retraction check information, if applicable.
    #[getter]
    fn retraction_info(&self) -> Option<PyRetractionInfo> {
        self.inner
            .retraction_info
            .clone()
            .map(PyRetractionInfo::from)
    }

    fn __repr__(&self) -> String {
        format!(
            "ValidationResult(title={:?}, status={:?}, source={:?})",
            self.inner.title,
            self.status(),
            self.inner.source,
        )
    }
}

// ── PyDbResult ──

/// Result from querying a single database backend.
#[pyclass(name = "DbResult")]
#[derive(Debug, Clone)]
pub struct PyDbResult {
    inner: DbResult,
}

impl From<DbResult> for PyDbResult {
    fn from(r: DbResult) -> Self {
        Self { inner: r }
    }
}

fn db_status_str(s: &DbStatus) -> &'static str {
    match s {
        DbStatus::Match => "match",
        DbStatus::NoMatch => "no_match",
        DbStatus::AuthorMismatch => "author_mismatch",
        DbStatus::Timeout => "timeout",
        DbStatus::RateLimited => "rate_limited",
        DbStatus::Error => "error",
        DbStatus::Skipped => "skipped",
    }
}

#[pymethods]
impl PyDbResult {
    /// Name of the database (e.g. "crossref", "arxiv").
    #[getter]
    fn db_name(&self) -> &str {
        &self.inner.db_name
    }

    /// Query status: "match", "no_match", "author_mismatch", "timeout", "error", "skipped".
    #[getter]
    fn status(&self) -> &str {
        db_status_str(&self.inner.status)
    }

    /// Query elapsed time in milliseconds, or None.
    #[getter]
    fn elapsed_ms(&self) -> Option<f64> {
        self.inner.elapsed.map(|d| d.as_secs_f64() * 1000.0)
    }

    /// Authors found in this database's record.
    #[getter]
    fn found_authors(&self) -> Vec<String> {
        self.inner.found_authors.clone()
    }

    /// URL of the paper in this database, if found.
    #[getter]
    fn paper_url(&self) -> Option<&str> {
        self.inner.paper_url.as_deref()
    }

    /// Error message if the query failed, or None.
    #[getter]
    fn error_message(&self) -> Option<&str> {
        self.inner.error_message.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "DbResult(db={:?}, status={:?}, elapsed_ms={:?})",
            self.inner.db_name,
            self.status(),
            self.elapsed_ms(),
        )
    }
}

// ── PyDoiInfo ──

/// Information about a DOI lookup.
#[pyclass(name = "DoiInfo")]
#[derive(Debug, Clone)]
pub struct PyDoiInfo {
    inner: DoiInfo,
}

impl From<DoiInfo> for PyDoiInfo {
    fn from(d: DoiInfo) -> Self {
        Self { inner: d }
    }
}

#[pymethods]
impl PyDoiInfo {
    /// The DOI string.
    #[getter]
    fn doi(&self) -> &str {
        &self.inner.doi
    }

    /// Whether the DOI resolved successfully.
    #[getter]
    fn valid(&self) -> bool {
        self.inner.valid
    }

    /// The title returned by the DOI resolver, if any.
    #[getter]
    fn title(&self) -> Option<&str> {
        self.inner.title.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "DoiInfo(doi={:?}, valid={})",
            self.inner.doi, self.inner.valid
        )
    }
}

// ── PyArxivInfo ──

/// Information about an arXiv lookup.
#[pyclass(name = "ArxivInfo")]
#[derive(Debug, Clone)]
pub struct PyArxivInfo {
    inner: ArxivInfo,
}

impl From<ArxivInfo> for PyArxivInfo {
    fn from(a: ArxivInfo) -> Self {
        Self { inner: a }
    }
}

#[pymethods]
impl PyArxivInfo {
    /// The arXiv identifier.
    #[getter]
    fn arxiv_id(&self) -> &str {
        &self.inner.arxiv_id
    }

    /// Whether the arXiv ID resolved successfully.
    #[getter]
    fn valid(&self) -> bool {
        self.inner.valid
    }

    /// The title returned by arXiv, if any.
    #[getter]
    fn title(&self) -> Option<&str> {
        self.inner.title.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "ArxivInfo(arxiv_id={:?}, valid={})",
            self.inner.arxiv_id, self.inner.valid
        )
    }
}

// ── PyRetractionInfo ──

/// Information about a retraction check.
#[pyclass(name = "RetractionInfo")]
#[derive(Debug, Clone)]
pub struct PyRetractionInfo {
    inner: RetractionInfo,
}

impl From<RetractionInfo> for PyRetractionInfo {
    fn from(r: RetractionInfo) -> Self {
        Self { inner: r }
    }
}

#[pymethods]
impl PyRetractionInfo {
    /// Whether the paper has been retracted.
    #[getter]
    fn is_retracted(&self) -> bool {
        self.inner.is_retracted
    }

    /// DOI of the retraction notice, if available.
    #[getter]
    fn retraction_doi(&self) -> Option<&str> {
        self.inner.retraction_doi.as_deref()
    }

    /// Source of the retraction information.
    #[getter]
    fn retraction_source(&self) -> Option<&str> {
        self.inner.retraction_source.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "RetractionInfo(is_retracted={}, doi={:?})",
            self.inner.is_retracted, self.inner.retraction_doi,
        )
    }
}

// ── PyProgressEvent ──

/// A progress event emitted during validation.
///
/// Properties vary by ``event_type``:
///
/// - ``"checking"`` — ``index``, ``total``, ``title``
/// - ``"result"`` — ``index``, ``total``, ``result`` (a ``ValidationResult``)
/// - ``"warning"`` — ``index``, ``total``, ``title``, ``failed_dbs``, ``message``
/// - ``"retry_pass"`` — ``count``
/// - ``"db_query_complete"`` — ``paper_index``, ``ref_index``, ``db_name``, ``status``, ``elapsed_ms``
/// - ``"rate_limit_wait"`` — ``db_name``, waiting for rate limiter
/// - ``"rate_limit_retry"`` — ``ref_index``, ``db_name``, ``attempt``, retrying after 429
#[pyclass(name = "ProgressEvent")]
#[derive(Debug, Clone)]
pub struct PyProgressEvent {
    inner: ProgressEvent,
}

impl From<ProgressEvent> for PyProgressEvent {
    fn from(e: ProgressEvent) -> Self {
        Self { inner: e }
    }
}

#[pymethods]
impl PyProgressEvent {
    /// The event type string.
    #[getter]
    fn event_type(&self) -> &str {
        match &self.inner {
            ProgressEvent::Checking { .. } => "checking",
            ProgressEvent::Result { .. } => "result",
            ProgressEvent::Warning { .. } => "warning",
            ProgressEvent::Retrying { .. } => "retrying",
            ProgressEvent::RetryPass { .. } => "retry_pass",
            ProgressEvent::DatabaseQueryComplete { .. } => "db_query_complete",
            ProgressEvent::RateLimitWait { .. } => "rate_limit_wait",
            ProgressEvent::RateLimitRetry { .. } => "rate_limit_retry",
        }
    }

    /// Index of the reference (for checking/result/warning/retrying events).
    #[getter]
    fn index(&self) -> Option<usize> {
        match &self.inner {
            ProgressEvent::Checking { index, .. }
            | ProgressEvent::Result { index, .. }
            | ProgressEvent::Warning { index, .. }
            | ProgressEvent::Retrying { index, .. } => Some(*index),
            _ => None,
        }
    }

    /// Total number of references (for checking/result/warning/retrying events).
    #[getter]
    fn total(&self) -> Option<usize> {
        match &self.inner {
            ProgressEvent::Checking { total, .. }
            | ProgressEvent::Result { total, .. }
            | ProgressEvent::Warning { total, .. }
            | ProgressEvent::Retrying { total, .. } => Some(*total),
            _ => None,
        }
    }

    /// Reference title (for checking/warning/retrying events).
    #[getter]
    fn title(&self) -> Option<&str> {
        match &self.inner {
            ProgressEvent::Checking { title, .. }
            | ProgressEvent::Warning { title, .. }
            | ProgressEvent::Retrying { title, .. } => Some(title),
            _ => None,
        }
    }

    /// The validation result (for result events).
    #[getter]
    fn result(&self) -> Option<PyValidationResult> {
        match &self.inner {
            ProgressEvent::Result { result, .. } => Some(PyValidationResult::from(*result.clone())),
            _ => None,
        }
    }

    /// List of failed databases (for warning/retrying events).
    #[getter]
    fn failed_dbs(&self) -> Option<Vec<String>> {
        match &self.inner {
            ProgressEvent::Warning { failed_dbs, .. }
            | ProgressEvent::Retrying { failed_dbs, .. } => Some(failed_dbs.clone()),
            _ => None,
        }
    }

    /// Warning message (for warning events).
    #[getter]
    fn message(&self) -> Option<&str> {
        match &self.inner {
            ProgressEvent::Warning { message, .. } => Some(message),
            _ => None,
        }
    }

    /// Number of references being retried (for retry_pass events).
    #[getter]
    fn count(&self) -> Option<usize> {
        match &self.inner {
            ProgressEvent::RetryPass { count } => Some(*count),
            _ => None,
        }
    }

    /// Paper index (for db_query_complete events).
    #[getter]
    fn paper_index(&self) -> Option<usize> {
        match &self.inner {
            ProgressEvent::DatabaseQueryComplete { paper_index, .. } => Some(*paper_index),
            _ => None,
        }
    }

    /// Reference index within the paper (for db_query_complete, rate_limit_retry events).
    #[getter]
    fn ref_index(&self) -> Option<usize> {
        match &self.inner {
            ProgressEvent::DatabaseQueryComplete { ref_index, .. }
            | ProgressEvent::RateLimitRetry { ref_index, .. } => Some(*ref_index),
            _ => None,
        }
    }

    /// Database name (for db_query_complete, rate_limit_wait, rate_limit_retry events).
    #[getter]
    fn db_name(&self) -> Option<&str> {
        match &self.inner {
            ProgressEvent::DatabaseQueryComplete { db_name, .. }
            | ProgressEvent::RateLimitWait { db_name, .. }
            | ProgressEvent::RateLimitRetry { db_name, .. } => Some(db_name),
            _ => None,
        }
    }

    /// Database query status string (for db_query_complete events).
    #[getter]
    fn db_status(&self) -> Option<&str> {
        match &self.inner {
            ProgressEvent::DatabaseQueryComplete { status, .. } => Some(db_status_str(status)),
            _ => None,
        }
    }

    /// Elapsed time in milliseconds (for db_query_complete events).
    #[getter]
    fn elapsed_ms(&self) -> Option<f64> {
        match &self.inner {
            ProgressEvent::DatabaseQueryComplete { elapsed, .. } => {
                Some(elapsed.as_secs_f64() * 1000.0)
            }
            _ => None,
        }
    }

    /// Retry attempt number (for rate_limit_retry events).
    #[getter]
    fn attempt(&self) -> Option<u32> {
        match &self.inner {
            ProgressEvent::RateLimitRetry { attempt, .. } => Some(*attempt),
            _ => None,
        }
    }

    /// Wait duration in milliseconds (for rate_limit_wait events).
    #[getter]
    fn wait_ms(&self) -> Option<f64> {
        match &self.inner {
            ProgressEvent::RateLimitWait { wait_duration, .. } => {
                Some(wait_duration.as_secs_f64() * 1000.0)
            }
            _ => None,
        }
    }

    /// Backoff duration in milliseconds (for rate_limit_retry events).
    #[getter]
    fn backoff_ms(&self) -> Option<f64> {
        match &self.inner {
            ProgressEvent::RateLimitRetry { backoff, .. } => {
                Some(backoff.as_secs_f64() * 1000.0)
            }
            _ => None,
        }
    }

    fn __repr__(&self) -> String {
        match &self.inner {
            ProgressEvent::Checking {
                index,
                total,
                title,
            } => format!(
                "ProgressEvent(type='checking', index={}, total={}, title={:?})",
                index, total, title,
            ),
            ProgressEvent::Result {
                index,
                total,
                result,
            } => format!(
                "ProgressEvent(type='result', index={}, total={}, status={:?})",
                index,
                total,
                match &result.status {
                    Status::Verified => "verified",
                    Status::NotFound => "not_found",
                    Status::Mismatch(_) => "mismatch",
                },
            ),
            ProgressEvent::Warning {
                index,
                total,
                title,
                ..
            } => format!(
                "ProgressEvent(type='warning', index={}, total={}, title={:?})",
                index, total, title,
            ),
            ProgressEvent::Retrying {
                index,
                total,
                title,
                failed_dbs,
            } => format!(
                "ProgressEvent(type='retrying', index={}, total={}, title={:?}, failed_dbs={:?})",
                index, total, title, failed_dbs,
            ),
            ProgressEvent::RetryPass { count } => {
                format!("ProgressEvent(type='retry_pass', count={})", count)
            }
            ProgressEvent::DatabaseQueryComplete {
                db_name, status, ..
            } => format!(
                "ProgressEvent(type='db_query_complete', db={:?}, status={:?})",
                db_name,
                db_status_str(status),
            ),
            ProgressEvent::RateLimitWait {
                db_name,
                wait_duration,
            } => format!(
                "ProgressEvent(type='rate_limit_wait', db={:?}, wait_ms={:.0})",
                db_name,
                wait_duration.as_secs_f64() * 1000.0,
            ),
            ProgressEvent::RateLimitRetry {
                ref_index,
                db_name,
                attempt,
                backoff,
            } => format!(
                "ProgressEvent(type='rate_limit_retry', ref={}, db={:?}, attempt={}, backoff_ms={:.0})",
                ref_index,
                db_name,
                attempt,
                backoff.as_secs_f64() * 1000.0,
            ),
        }
    }
}

// ── PyCheckStats ──

/// Summary statistics for a validation run.
#[pyclass(name = "CheckStats")]
#[derive(Debug, Clone)]
pub struct PyCheckStats {
    inner: CheckStats,
}

impl From<CheckStats> for PyCheckStats {
    fn from(s: CheckStats) -> Self {
        Self { inner: s }
    }
}

impl PyCheckStats {
    pub(crate) fn compute(results: &[&ValidationResult]) -> Self {
        let mut stats = CheckStats {
            total: results.len(),
            ..Default::default()
        };
        for r in results {
            match &r.status {
                Status::Verified => stats.verified += 1,
                Status::NotFound => stats.not_found += 1,
                Status::Mismatch(kind) => {
                    stats.mismatch += 1;
                    if kind.contains(MismatchKind::AUTHOR) {
                        stats.author_mismatch += 1;
                    }
                    if kind.contains(MismatchKind::DOI) {
                        stats.doi_mismatch += 1;
                    }
                    if kind.contains(MismatchKind::ARXIV_ID) {
                        stats.arxiv_mismatch += 1;
                    }
                }
            }
            if r.retraction_info.as_ref().is_some_and(|ri| ri.is_retracted) {
                stats.retracted += 1;
            }
        }
        Self { inner: stats }
    }
}

#[pymethods]
impl PyCheckStats {
    /// Total number of references checked.
    #[getter]
    fn total(&self) -> usize {
        self.inner.total
    }

    /// Number of verified references.
    #[getter]
    fn verified(&self) -> usize {
        self.inner.verified
    }

    /// Number of references not found in any database.
    #[getter]
    fn not_found(&self) -> usize {
        self.inner.not_found
    }

    /// Number of references with author mismatches.
    #[getter]
    fn author_mismatch(&self) -> usize {
        self.inner.author_mismatch
    }

    /// Number of retracted references.
    #[getter]
    fn retracted(&self) -> usize {
        self.inner.retracted
    }

    /// Number of skipped references.
    #[getter]
    fn skipped(&self) -> usize {
        self.inner.skipped
    }

    fn __repr__(&self) -> String {
        format!(
            "CheckStats(total={}, verified={}, not_found={}, author_mismatch={}, retracted={}, skipped={})",
            self.inner.total,
            self.inner.verified,
            self.inner.not_found,
            self.inner.author_mismatch,
            self.inner.retracted,
            self.inner.skipped,
        )
    }
}
