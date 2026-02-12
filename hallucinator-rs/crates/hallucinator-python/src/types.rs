use pyo3::prelude::*;

use hallucinator_pdf::{ExtractionResult, Reference, SkipStats};

/// A parsed reference extracted from a PDF.
#[pyclass(name = "Reference")]
#[derive(Debug, Clone)]
pub struct PyReference {
    inner: Reference,
}

impl From<Reference> for PyReference {
    fn from(r: Reference) -> Self {
        Self { inner: r }
    }
}

impl PyReference {
    pub fn into_inner(self) -> Reference {
        self.inner
    }
}

#[pymethods]
impl PyReference {
    /// The raw citation text (cleaned up for display).
    #[getter]
    fn raw_citation(&self) -> &str {
        &self.inner.raw_citation
    }

    /// The extracted title, or `None` if no title could be parsed.
    #[getter]
    fn title(&self) -> Option<&str> {
        self.inner.title.as_deref()
    }

    /// List of author names.
    #[getter]
    fn authors(&self) -> Vec<String> {
        self.inner.authors.clone()
    }

    /// The DOI, if found.
    #[getter]
    fn doi(&self) -> Option<&str> {
        self.inner.doi.as_deref()
    }

    /// The arXiv ID, if found.
    #[getter]
    fn arxiv_id(&self) -> Option<&str> {
        self.inner.arxiv_id.as_deref()
    }

    fn __repr__(&self) -> String {
        format!(
            "Reference(title={:?}, authors={}, doi={:?})",
            self.inner.title.as_deref().unwrap_or(""),
            self.inner.authors.len(),
            self.inner.doi.as_deref().unwrap_or("None"),
        )
    }
}

/// Statistics about references that were skipped during extraction.
#[pyclass(name = "SkipStats")]
#[derive(Debug, Clone)]
pub struct PySkipStats {
    inner: SkipStats,
}

impl From<SkipStats> for PySkipStats {
    fn from(s: SkipStats) -> Self {
        Self { inner: s }
    }
}

#[pymethods]
impl PySkipStats {
    /// Number of references skipped because they only contained non-academic URLs.
    #[getter]
    fn url_only(&self) -> usize {
        self.inner.url_only
    }

    /// Number of references skipped because the title was too short.
    #[getter]
    fn short_title(&self) -> usize {
        self.inner.short_title
    }

    /// Number of references where no title could be extracted.
    #[getter]
    fn no_title(&self) -> usize {
        self.inner.no_title
    }

    /// Number of references where no authors could be extracted.
    #[getter]
    fn no_authors(&self) -> usize {
        self.inner.no_authors
    }

    /// Total number of raw reference segments before filtering.
    #[getter]
    fn total_raw(&self) -> usize {
        self.inner.total_raw
    }

    fn __repr__(&self) -> String {
        format!(
            "SkipStats(total_raw={}, url_only={}, short_title={}, no_title={}, no_authors={})",
            self.inner.total_raw,
            self.inner.url_only,
            self.inner.short_title,
            self.inner.no_title,
            self.inner.no_authors,
        )
    }
}

/// Result of extracting references from a PDF.
#[pyclass(name = "ExtractionResult")]
#[derive(Debug, Clone)]
pub struct PyExtractionResult {
    inner: ExtractionResult,
}

impl From<ExtractionResult> for PyExtractionResult {
    fn from(r: ExtractionResult) -> Self {
        Self { inner: r }
    }
}

#[pymethods]
impl PyExtractionResult {
    /// List of parsed references.
    #[getter]
    fn references(&self) -> Vec<PyReference> {
        self.inner
            .references
            .iter()
            .cloned()
            .map(PyReference::from)
            .collect()
    }

    /// Skip statistics.
    #[getter]
    fn skip_stats(&self) -> PySkipStats {
        PySkipStats::from(self.inner.skip_stats.clone())
    }

    /// Construct an ExtractionResult from parts (used by the Python wrapper).
    #[staticmethod]
    fn _from_parts(
        refs: Vec<PyReference>,
        total_raw: usize,
        url_only: usize,
        short_title: usize,
        no_title: usize,
        no_authors: usize,
    ) -> Self {
        let references = refs.into_iter().map(|r| r.into_inner()).collect();
        let skip_stats = SkipStats {
            total_raw,
            url_only,
            short_title,
            no_title,
            no_authors,
        };
        Self {
            inner: ExtractionResult {
                references,
                skip_stats,
            },
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "ExtractionResult(references={}, total_raw={})",
            self.inner.references.len(),
            self.inner.skip_stats.total_raw,
        )
    }

    fn __len__(&self) -> usize {
        self.inner.references.len()
    }
}
