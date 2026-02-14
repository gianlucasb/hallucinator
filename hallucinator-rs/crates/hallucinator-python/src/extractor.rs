#[cfg(feature = "pdf")]
use std::path::PathBuf;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;

use hallucinator_pdf::{PdfExtractor, PdfParsingConfigBuilder};

#[cfg(feature = "pdf")]
use crate::archive::PyArchiveIterator;
use crate::errors::pdf_error_to_py;
use crate::types::{PyExtractionResult, PyReference};

/// A configurable PDF reference extractor.
///
/// Set properties to customize regex patterns and thresholds, then call
/// extraction methods. The config is compiled lazily on first use after
/// any change (dirty flag pattern).
///
/// Example::
///
///     ext = PdfExtractor()
///     ext.section_header_regex = r"(?i)\n\s*Bibliografía\s*\n"
///     ext.min_title_words = 3
///     result = ext.extract("paper.pdf")
///
#[pyclass(name = "NativePdfExtractor")]
pub struct PyPdfExtractor {
    builder: PdfParsingConfigBuilder,
    cached: Option<PdfExtractor>,
}

impl PyPdfExtractor {
    /// Get or rebuild the underlying Rust extractor.
    fn extractor(&mut self) -> PyResult<&PdfExtractor> {
        if self.cached.is_none() {
            let config = self
                .builder
                .clone()
                .build()
                .map_err(|e| PyValueError::new_err(format!("Invalid regex: {}", e)))?;
            self.cached = Some(PdfExtractor::with_config(config));
        }
        Ok(self.cached.as_ref().unwrap())
    }

    /// Mark config as dirty so the extractor is rebuilt on next use.
    fn invalidate(&mut self) {
        self.cached = None;
    }
}

#[pymethods]
impl PyPdfExtractor {
    #[new]
    fn new() -> Self {
        Self {
            builder: PdfParsingConfigBuilder::new(),
            cached: None,
        }
    }

    // ── Config setters ──

    /// Set the regex for locating the references section header.
    #[setter]
    fn set_section_header_regex(&mut self, pattern: &str) {
        self.builder = self.builder.clone().section_header_regex(pattern);
        self.invalidate();
    }

    /// Set the regex for finding section end markers.
    #[setter]
    fn set_section_end_regex(&mut self, pattern: &str) {
        self.builder = self.builder.clone().section_end_regex(pattern);
        self.invalidate();
    }

    /// Set the fallback fraction (0.0–1.0) for when no header is found.
    #[setter]
    fn set_fallback_fraction(&mut self, fraction: f64) {
        self.builder = self.builder.clone().fallback_fraction(fraction);
        self.invalidate();
    }

    /// Set the regex for IEEE-style segmentation.
    #[setter]
    fn set_ieee_segment_regex(&mut self, pattern: &str) {
        self.builder = self.builder.clone().ieee_segment_regex(pattern);
        self.invalidate();
    }

    /// Set the regex for numbered-list segmentation.
    #[setter]
    fn set_numbered_segment_regex(&mut self, pattern: &str) {
        self.builder = self.builder.clone().numbered_segment_regex(pattern);
        self.invalidate();
    }

    /// Set the regex for fallback double-newline segmentation.
    #[setter]
    fn set_fallback_segment_regex(&mut self, pattern: &str) {
        self.builder = self.builder.clone().fallback_segment_regex(pattern);
        self.invalidate();
    }

    /// Set the minimum number of words a title must have.
    #[setter]
    fn set_min_title_words(&mut self, n: usize) {
        self.builder = self.builder.clone().min_title_words(n);
        self.invalidate();
    }

    /// Set the maximum number of authors to retain per reference.
    #[setter]
    fn set_max_authors(&mut self, n: usize) {
        self.builder = self.builder.clone().max_authors(n);
        self.invalidate();
    }

    /// Add an extra venue cutoff pattern (appended to defaults).
    fn add_venue_cutoff_pattern(&mut self, pattern: &str) {
        self.builder = self
            .builder
            .clone()
            .add_venue_cutoff_pattern(pattern.to_string());
        self.invalidate();
    }

    /// Replace all venue cutoff patterns with the given list.
    fn set_venue_cutoff_patterns(&mut self, patterns: Vec<String>) {
        self.builder = self.builder.clone().set_venue_cutoff_patterns(patterns);
        self.invalidate();
    }

    /// Add an extra quote detection pattern (appended to defaults).
    fn add_quote_pattern(&mut self, pattern: &str) {
        self.builder = self.builder.clone().add_quote_pattern(pattern.to_string());
        self.invalidate();
    }

    /// Replace all quote patterns with the given list.
    fn set_quote_patterns(&mut self, patterns: Vec<String>) {
        self.builder = self.builder.clone().set_quote_patterns(patterns);
        self.invalidate();
    }

    /// Add an extra compound suffix (appended to defaults).
    fn add_compound_suffix(&mut self, suffix: &str) {
        self.builder = self.builder.clone().add_compound_suffix(suffix.to_string());
        self.invalidate();
    }

    /// Replace all compound suffixes with the given list.
    fn set_compound_suffixes(&mut self, suffixes: Vec<String>) {
        self.builder = self.builder.clone().set_compound_suffixes(suffixes);
        self.invalidate();
    }

    // ── Extraction methods ──

    /// Run the full extraction pipeline on a PDF file.
    ///
    /// Returns an `ExtractionResult` with `.references` and `.skip_stats`.
    #[cfg(feature = "pdf")]
    fn extract(&mut self, path: &str) -> PyResult<PyExtractionResult> {
        let ext = self.extractor()?;
        let result = ext
            .extract_references(&PathBuf::from(path))
            .map_err(pdf_error_to_py)?;
        Ok(PyExtractionResult::from(result))
    }

    /// Extract and parse references from a ZIP or tar.gz archive.
    ///
    /// Returns an iterator that yields ``ArchiveEntry`` items as each file
    /// is processed. PDFs get full reference extraction; BBL/BIB files yield
    /// raw text content. Access ``.warnings`` on the iterator for any
    /// size-limit warnings.
    #[cfg(feature = "pdf")]
    #[pyo3(signature = (path, max_size_bytes=0))]
    fn extract_archive(&mut self, path: &str, max_size_bytes: u64) -> PyResult<PyArchiveIterator> {
        let archive_path = PathBuf::from(path);

        if !hallucinator_pdf::archive::is_archive_path(&archive_path) {
            return Err(PyValueError::new_err(format!(
                "Unsupported archive format: {}. Expected .zip, .tar.gz, or .tgz",
                path
            )));
        }

        let config = self
            .builder
            .clone()
            .build()
            .map_err(|e| PyValueError::new_err(format!("Invalid regex: {}", e)))?;
        let extractor = PdfExtractor::with_config(config);

        let temp_dir = tempfile::tempdir()
            .map_err(|e| PyValueError::new_err(format!("Failed to create temp dir: {}", e)))?;
        let dir = temp_dir.path().to_path_buf();

        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            hallucinator_pdf::archive::extract_archive_streaming(
                &archive_path,
                &dir,
                max_size_bytes,
                &tx,
            )
        });

        Ok(PyArchiveIterator::new(rx, extractor, temp_dir, handle))
    }

    /// Extract raw text from a PDF file (step 1).
    #[cfg(feature = "pdf")]
    fn extract_text(&mut self, path: &str) -> PyResult<String> {
        let ext = self.extractor()?;
        ext.extract_text(&PathBuf::from(path))
            .map_err(pdf_error_to_py)
    }

    /// Locate the references section in document text (step 2).
    fn find_section(&mut self, text: &str) -> PyResult<Option<String>> {
        let ext = self.extractor()?;
        Ok(ext.find_references_section(text))
    }

    /// Segment a references section into individual reference strings (step 3).
    fn segment(&mut self, text: &str) -> PyResult<Vec<String>> {
        let ext = self.extractor()?;
        Ok(ext.segment_references(text))
    }

    /// Parse a single reference string (step 4).
    ///
    /// Returns a `Reference` or `None` if the reference was skipped.
    /// `prev_authors` is used for em-dash "same authors" handling.
    #[pyo3(signature = (text, prev_authors=None))]
    fn parse_reference(
        &mut self,
        text: &str,
        prev_authors: Option<Vec<String>>,
    ) -> PyResult<Option<PyReference>> {
        let ext = self.extractor()?;
        let prev = prev_authors.unwrap_or_default();
        let parsed = ext.parse_reference(text, &prev);
        match parsed {
            hallucinator_pdf::extractor::ParsedRef::Ref(r) => Ok(Some(PyReference::from(r))),
            hallucinator_pdf::extractor::ParsedRef::Skip(..) => Ok(None),
        }
    }

    /// Parse a single reference string, returning skip reason if skipped.
    ///
    /// Returns `(Reference, None)` on success or `(None, reason)` on skip.
    /// `reason` is `"url_only"` or `"short_title"`.
    #[pyo3(signature = (text, prev_authors=None))]
    fn parse_reference_detailed(
        &mut self,
        text: &str,
        prev_authors: Option<Vec<String>>,
    ) -> PyResult<(Option<PyReference>, Option<String>)> {
        let ext = self.extractor()?;
        let prev = prev_authors.unwrap_or_default();
        let parsed = ext.parse_reference(text, &prev);
        match parsed {
            hallucinator_pdf::extractor::ParsedRef::Ref(r) => {
                Ok((Some(PyReference::from(r)), None))
            }
            hallucinator_pdf::extractor::ParsedRef::Skip(reason, _, _) => {
                let reason_str = match reason {
                    hallucinator_pdf::extractor::SkipReason::UrlOnly => "url_only",
                    hallucinator_pdf::extractor::SkipReason::ShortTitle => "short_title",
                };
                Ok((None, Some(reason_str.to_string())))
            }
        }
    }

    /// Run extraction on already-extracted text (steps 2–4).
    ///
    /// Useful when you've already extracted text and want to re-parse
    /// with different config.
    fn extract_from_text(&mut self, text: &str) -> PyResult<PyExtractionResult> {
        let ext = self.extractor()?;
        let result = ext
            .extract_references_from_text(text)
            .map_err(pdf_error_to_py)?;
        Ok(PyExtractionResult::from(result))
    }

    fn __repr__(&self) -> String {
        "NativePdfExtractor(...)".to_string()
    }
}
