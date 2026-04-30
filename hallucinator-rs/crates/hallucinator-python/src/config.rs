use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use hallucinator_core::Config;

/// Configuration for the reference validator.
///
/// Example::
///
///     config = ValidatorConfig()
///     config.s2_api_key = "your-key"
///     config.num_workers = 8
///     config.disabled_dbs = ["openalex"]
///
#[pyclass(name = "ValidatorConfig")]
#[derive(Debug, Clone)]
pub struct PyValidatorConfig {
    pub(crate) openalex_key: Option<String>,
    pub(crate) s2_api_key: Option<String>,
    pub(crate) dblp_offline_path: Option<String>,
    pub(crate) acl_offline_path: Option<String>,
    pub(crate) arxiv_offline_path: Option<String>,
    pub(crate) iacr_eprint_offline_path: Option<String>,
    pub(crate) openalex_offline_path: Option<String>,
    pub(crate) cache_path: Option<String>,
    pub(crate) cache_positive_ttl_secs: u64,
    pub(crate) cache_negative_ttl_secs: u64,
    pub(crate) searxng_url: Option<String>,
    pub(crate) num_workers: usize,
    pub(crate) max_rate_limit_retries: u32,
    pub(crate) db_timeout_secs: u64,
    pub(crate) db_timeout_short_secs: u64,
    pub(crate) disabled_dbs: Vec<String>,
    pub(crate) check_openalex_authors: bool,
    pub(crate) crossref_mailto: Option<String>,
    pub(crate) url_match: bool,
}

impl PyValidatorConfig {
    /// Build a `hallucinator_core::Config` from this Python config.
    ///
    /// Opens offline databases if paths are provided.
    pub(crate) fn to_core_config(&self) -> PyResult<Config> {
        let dblp_offline_db = match &self.dblp_offline_path {
            Some(path) => {
                let db = hallucinator_dblp::DblpDatabase::open(std::path::Path::new(path))
                    .map_err(|e| {
                        PyRuntimeError::new_err(format!("Failed to open DBLP database: {}", e))
                    })?;
                Some(Arc::new(Mutex::new(db)))
            }
            None => None,
        };

        let acl_offline_db = match &self.acl_offline_path {
            Some(path) => {
                let db = hallucinator_acl::AclDatabase::open(std::path::Path::new(path)).map_err(
                    |e| PyRuntimeError::new_err(format!("Failed to open ACL database: {}", e)),
                )?;
                Some(Arc::new(Mutex::new(db)))
            }
            None => None,
        };

        let arxiv_offline_db = match &self.arxiv_offline_path {
            Some(path) => {
                let db = hallucinator_arxiv_offline::ArxivDatabase::open(std::path::Path::new(
                    path,
                ))
                .map_err(|e| {
                    PyRuntimeError::new_err(format!("Failed to open arXiv database: {}", e))
                })?;
                Some(Arc::new(Mutex::new(db)))
            }
            None => None,
        };

        let iacr_eprint_offline_db = match &self.iacr_eprint_offline_path {
            Some(path) => {
                let db = hallucinator_iacr_eprint::IacrDatabase::open(std::path::Path::new(path))
                    .map_err(|e| {
                        PyRuntimeError::new_err(format!(
                            "Failed to open IACR ePrint database: {}",
                            e
                        ))
                    })?;
                Some(Arc::new(Mutex::new(db)))
            }
            None => None,
        };

        let openalex_offline_db = match &self.openalex_offline_path {
            Some(path) => {
                let db = hallucinator_openalex::OpenAlexDatabase::open(std::path::Path::new(path))
                    .map_err(|e| {
                        PyRuntimeError::new_err(format!(
                            "Failed to open OpenAlex database: {}",
                            e
                        ))
                    })?;
                Some(Arc::new(Mutex::new(db)))
            }
            None => None,
        };

        let rate_limiters = std::sync::Arc::new(hallucinator_core::RateLimiters::new(
            self.crossref_mailto.is_some(),
            self.s2_api_key.is_some(),
        ));

        Ok(Config {
            openalex_key: self.openalex_key.clone(),
            s2_api_key: self.s2_api_key.clone(),
            dblp_offline_path: self.dblp_offline_path.as_ref().map(PathBuf::from),
            dblp_offline_db,
            acl_offline_path: self.acl_offline_path.as_ref().map(PathBuf::from),
            acl_offline_db,
            arxiv_offline_path: self.arxiv_offline_path.as_ref().map(PathBuf::from),
            arxiv_offline_db,
            iacr_eprint_offline_path: self.iacr_eprint_offline_path.as_ref().map(PathBuf::from),
            iacr_eprint_offline_db,
            openalex_offline_path: self.openalex_offline_path.as_ref().map(PathBuf::from),
            openalex_offline_db,
            num_workers: self.num_workers,
            db_timeout_secs: self.db_timeout_secs,
            db_timeout_short_secs: self.db_timeout_short_secs,
            disabled_dbs: self.disabled_dbs.clone(),
            check_openalex_authors: self.check_openalex_authors,
            crossref_mailto: self.crossref_mailto.clone(),
            max_rate_limit_retries: self.max_rate_limit_retries,
            rate_limiters,
            cache_path: self.cache_path.as_ref().map(PathBuf::from),
            cache_positive_ttl_secs: self.cache_positive_ttl_secs,
            cache_negative_ttl_secs: self.cache_negative_ttl_secs,
            searxng_url: self.searxng_url.clone(),
            govinfo_key: None,
            patentsview_key: None,
            query_cache: Some(hallucinator_core::build_query_cache(
                self.cache_path.as_ref().map(std::path::Path::new),
                self.cache_positive_ttl_secs,
                self.cache_negative_ttl_secs,
            )),
            url_match: self.url_match,
        })
    }
}

#[pymethods]
impl PyValidatorConfig {
    #[new]
    fn new() -> Self {
        Self {
            openalex_key: None,
            s2_api_key: None,
            dblp_offline_path: None,
            acl_offline_path: None,
            arxiv_offline_path: None,
            iacr_eprint_offline_path: None,
            openalex_offline_path: None,
            cache_path: None,
            cache_positive_ttl_secs: hallucinator_core::DEFAULT_POSITIVE_TTL.as_secs(),
            cache_negative_ttl_secs: hallucinator_core::DEFAULT_NEGATIVE_TTL.as_secs(),
            searxng_url: None,
            num_workers: 4,
            max_rate_limit_retries: 3,
            db_timeout_secs: 10,
            db_timeout_short_secs: 5,
            disabled_dbs: vec![],
            check_openalex_authors: false,
            crossref_mailto: None,
            url_match: false,
        }
    }

    /// OpenAlex API key (optional).
    #[getter]
    fn get_openalex_key(&self) -> Option<&str> {
        self.openalex_key.as_deref()
    }

    #[setter]
    fn set_openalex_key(&mut self, value: Option<String>) {
        self.openalex_key = value;
    }

    /// Semantic Scholar API key (optional).
    #[getter]
    fn get_s2_api_key(&self) -> Option<&str> {
        self.s2_api_key.as_deref()
    }

    #[setter]
    fn set_s2_api_key(&mut self, value: Option<String>) {
        self.s2_api_key = value;
    }

    /// Path to offline DBLP SQLite database (optional).
    #[getter]
    fn get_dblp_offline_path(&self) -> Option<&str> {
        self.dblp_offline_path.as_deref()
    }

    #[setter]
    fn set_dblp_offline_path(&mut self, value: Option<String>) {
        self.dblp_offline_path = value;
    }

    /// Path to offline ACL Anthology SQLite database (optional).
    #[getter]
    fn get_acl_offline_path(&self) -> Option<&str> {
        self.acl_offline_path.as_deref()
    }

    #[setter]
    fn set_acl_offline_path(&mut self, value: Option<String>) {
        self.acl_offline_path = value;
    }

    /// Path to offline arXiv SQLite database (Kaggle snapshot, optional).
    #[getter]
    fn get_arxiv_offline_path(&self) -> Option<&str> {
        self.arxiv_offline_path.as_deref()
    }

    #[setter]
    fn set_arxiv_offline_path(&mut self, value: Option<String>) {
        self.arxiv_offline_path = value;
    }

    /// Path to offline IACR Cryptology ePrint Archive SQLite database
    /// (optional, no online counterpart — the backend is offline-only).
    #[getter]
    fn get_iacr_eprint_offline_path(&self) -> Option<&str> {
        self.iacr_eprint_offline_path.as_deref()
    }

    #[setter]
    fn set_iacr_eprint_offline_path(&mut self, value: Option<String>) {
        self.iacr_eprint_offline_path = value;
    }

    /// Path to offline OpenAlex index directory (optional).
    #[getter]
    fn get_openalex_offline_path(&self) -> Option<&str> {
        self.openalex_offline_path.as_deref()
    }

    #[setter]
    fn set_openalex_offline_path(&mut self, value: Option<String>) {
        self.openalex_offline_path = value;
    }

    /// Path to persistent query cache SQLite database (optional).
    #[getter]
    fn get_cache_path(&self) -> Option<&str> {
        self.cache_path.as_deref()
    }

    #[setter]
    fn set_cache_path(&mut self, value: Option<String>) {
        self.cache_path = value;
    }

    /// TTL in seconds for positive (verified) cache entries (default: 604800 = 7 days).
    #[getter]
    fn get_cache_positive_ttl_secs(&self) -> u64 {
        self.cache_positive_ttl_secs
    }

    #[setter]
    fn set_cache_positive_ttl_secs(&mut self, value: u64) {
        self.cache_positive_ttl_secs = value;
    }

    /// TTL in seconds for negative (not-found) cache entries (default: 86400 = 24 hours).
    #[getter]
    fn get_cache_negative_ttl_secs(&self) -> u64 {
        self.cache_negative_ttl_secs
    }

    #[setter]
    fn set_cache_negative_ttl_secs(&mut self, value: u64) {
        self.cache_negative_ttl_secs = value;
    }

    /// SearxNG instance base URL for web search fallback (optional).
    #[getter]
    fn get_searxng_url(&self) -> Option<&str> {
        self.searxng_url.as_deref()
    }

    #[setter]
    fn set_searxng_url(&mut self, value: Option<String>) {
        self.searxng_url = value;
    }

    /// Number of concurrent reference checks (default: 4).
    #[getter]
    fn get_num_workers(&self) -> usize {
        self.num_workers
    }

    #[setter]
    fn set_num_workers(&mut self, value: usize) {
        self.num_workers = value;
    }

    /// Maximum 429 retries per database query (default: 3).
    #[getter]
    fn get_max_rate_limit_retries(&self) -> u32 {
        self.max_rate_limit_retries
    }

    #[setter]
    fn set_max_rate_limit_retries(&mut self, value: u32) {
        self.max_rate_limit_retries = value;
    }

    /// Timeout in seconds for database queries (default: 10).
    #[getter]
    fn get_db_timeout_secs(&self) -> u64 {
        self.db_timeout_secs
    }

    #[setter]
    fn set_db_timeout_secs(&mut self, value: u64) {
        self.db_timeout_secs = value;
    }

    /// Short timeout in seconds for fast database queries (default: 5).
    #[getter]
    fn get_db_timeout_short_secs(&self) -> u64 {
        self.db_timeout_short_secs
    }

    #[setter]
    fn set_db_timeout_short_secs(&mut self, value: u64) {
        self.db_timeout_short_secs = value;
    }

    /// List of database names to skip (e.g. ``["openalex"]``).
    #[getter]
    fn get_disabled_dbs(&self) -> Vec<String> {
        self.disabled_dbs.clone()
    }

    #[setter]
    fn set_disabled_dbs(&mut self, value: Vec<String>) {
        self.disabled_dbs = value;
    }

    /// Whether to verify authors for OpenAlex matches (default: False).
    #[getter]
    fn get_check_openalex_authors(&self) -> bool {
        self.check_openalex_authors
    }

    #[setter]
    fn set_check_openalex_authors(&mut self, value: bool) {
        self.check_openalex_authors = value;
    }

    /// CrossRef mailto address for polite pool (optional).
    #[getter]
    fn get_crossref_mailto(&self) -> Option<&str> {
        self.crossref_mailto.as_deref()
    }

    #[setter]
    fn set_crossref_mailto(&mut self, value: Option<String>) {
        self.crossref_mailto = value;
    }

    /// Cross-check unverified references against their raw URLs via URL
    /// liveness checks and the Wayback Machine. When False (default),
    /// non-academic-URL refs that miss every database land as "skipped"
    /// rather than "not_found".
    #[getter]
    fn get_url_match(&self) -> bool {
        self.url_match
    }

    #[setter]
    fn set_url_match(&mut self, value: bool) {
        self.url_match = value;
    }

    fn __repr__(&self) -> String {
        format!(
            "ValidatorConfig(num_workers={}, db_timeout={}s, disabled_dbs={:?})",
            self.num_workers, self.db_timeout_secs, self.disabled_dbs,
        )
    }
}
