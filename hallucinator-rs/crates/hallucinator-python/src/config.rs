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
///     config.max_concurrent_refs = 8
///     config.disabled_dbs = ["openalex"]
///
#[pyclass(name = "ValidatorConfig")]
#[derive(Debug, Clone)]
pub struct PyValidatorConfig {
    pub(crate) openalex_key: Option<String>,
    pub(crate) s2_api_key: Option<String>,
    pub(crate) dblp_offline_path: Option<String>,
    pub(crate) acl_offline_path: Option<String>,
    pub(crate) max_concurrent_refs: usize,
    pub(crate) db_timeout_secs: u64,
    pub(crate) db_timeout_short_secs: u64,
    pub(crate) disabled_dbs: Vec<String>,
    pub(crate) check_openalex_authors: bool,
    pub(crate) crossref_mailto: Option<String>,
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

        Ok(Config {
            openalex_key: self.openalex_key.clone(),
            s2_api_key: self.s2_api_key.clone(),
            dblp_offline_path: self.dblp_offline_path.as_ref().map(PathBuf::from),
            dblp_offline_db,
            acl_offline_path: self.acl_offline_path.as_ref().map(PathBuf::from),
            acl_offline_db,
            max_concurrent_refs: self.max_concurrent_refs,
            db_timeout_secs: self.db_timeout_secs,
            db_timeout_short_secs: self.db_timeout_short_secs,
            disabled_dbs: self.disabled_dbs.clone(),
            check_openalex_authors: self.check_openalex_authors,
            crossref_mailto: self.crossref_mailto.clone(),
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
            max_concurrent_refs: 4,
            db_timeout_secs: 10,
            db_timeout_short_secs: 5,
            disabled_dbs: vec![],
            check_openalex_authors: false,
            crossref_mailto: None,
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

    /// Maximum number of references checked concurrently (default: 4).
    #[getter]
    fn get_max_concurrent_refs(&self) -> usize {
        self.max_concurrent_refs
    }

    #[setter]
    fn set_max_concurrent_refs(&mut self, value: usize) {
        self.max_concurrent_refs = value;
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

    fn __repr__(&self) -> String {
        format!(
            "ValidatorConfig(max_concurrent_refs={}, db_timeout={}s, disabled_dbs={:?})",
            self.max_concurrent_refs, self.db_timeout_secs, self.disabled_dbs,
        )
    }
}
