use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use pyo3::exceptions::{PyRuntimeError, PyStopIteration};
use pyo3::prelude::*;

use hallucinator_pdf::archive::{self, ArchiveItem};
use hallucinator_pdf::PdfExtractor;

use crate::types::PyExtractionResult;

/// A single entry yielded from archive extraction.
///
/// For PDF files, ``result`` contains the extraction result and ``content`` is ``None``.
/// For BBL/BIB files, ``content`` contains the raw text and ``result`` is ``None``.
#[pyclass(name = "ArchiveEntry")]
#[derive(Clone)]
pub struct PyArchiveEntry {
    #[pyo3(get)]
    filename: String,
    #[pyo3(get)]
    file_type: String,
    result: Option<PyExtractionResult>,
    content: Option<String>,
}

#[pymethods]
impl PyArchiveEntry {
    /// The extraction result (populated for PDF files).
    #[getter]
    fn result(&self) -> Option<PyExtractionResult> {
        self.result.clone()
    }

    /// The raw text content (populated for BBL/BIB files).
    #[getter]
    fn content(&self) -> Option<String> {
        self.content.clone()
    }

    fn __repr__(&self) -> String {
        match &self.result {
            Some(r) => format!(
                "ArchiveEntry(filename={:?}, file_type={:?}, refs={})",
                self.filename,
                self.file_type,
                r.len(),
            ),
            None => format!(
                "ArchiveEntry(filename={:?}, file_type={:?}, content_len={})",
                self.filename,
                self.file_type,
                self.content.as_ref().map_or(0, |c| c.len()),
            ),
        }
    }
}

/// Iterator over archive entries, yielding results as each file is processed.
///
/// Wraps a streaming Rust archive extractor. PDF files get full reference
/// extraction; BBL/BIB files yield their raw text content.
///
/// Access ``warnings`` after (or during) iteration for any size-limit warnings.
#[pyclass(name = "ArchiveIterator")]
pub struct PyArchiveIterator {
    rx: Arc<Mutex<mpsc::Receiver<ArchiveItem>>>,
    extractor: PdfExtractor,
    // Keep temp_dir alive so extracted files aren't deleted during iteration.
    _temp_dir: tempfile::TempDir,
    warnings: Vec<String>,
    thread_handle: Option<JoinHandle<Result<(), String>>>,
}

impl PyArchiveIterator {
    pub fn new(
        rx: mpsc::Receiver<ArchiveItem>,
        extractor: PdfExtractor,
        temp_dir: tempfile::TempDir,
        handle: JoinHandle<Result<(), String>>,
    ) -> Self {
        Self {
            rx: Arc::new(Mutex::new(rx)),
            extractor,
            _temp_dir: temp_dir,
            warnings: Vec::new(),
            thread_handle: Some(handle),
        }
    }

    /// Join the background thread, propagating any errors as Python exceptions.
    fn join_thread(&mut self) -> PyResult<()> {
        if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(PyRuntimeError::new_err(format!(
                    "Archive extraction failed: {}",
                    e
                ))),
                Err(_) => Err(PyRuntimeError::new_err(
                    "Archive extraction thread panicked",
                )),
            }
        } else {
            Ok(())
        }
    }
}

#[pymethods]
impl PyArchiveIterator {
    /// Warnings emitted during extraction (e.g. size limit reached).
    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.warnings.clone()
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<PyArchiveEntry> {
        loop {
            // Receive the next item from the channel, releasing the GIL while waiting.
            let rx = Arc::clone(&self.rx);
            let item = py.allow_threads(move || {
                let rx = rx.lock().expect("archive receiver lock poisoned");
                rx.recv()
            });

            match item {
                Ok(ArchiveItem::Pdf(extracted)) => {
                    let lower = extracted.filename.to_lowercase();
                    let file_type = if lower.ends_with(".bbl") {
                        "bbl"
                    } else if lower.ends_with(".bib") {
                        "bib"
                    } else {
                        "pdf"
                    };

                    if file_type == "pdf" {
                        // Run full reference extraction on the PDF.
                        let result =
                            self.extractor
                                .extract_references(&extracted.path)
                                .map_err(|e| {
                                    PyRuntimeError::new_err(format!(
                                        "Failed to extract {}: {}",
                                        extracted.filename, e
                                    ))
                                })?;
                        return Ok(PyArchiveEntry {
                            filename: extracted.filename,
                            file_type: file_type.to_string(),
                            result: Some(PyExtractionResult::from(result)),
                            content: None,
                        });
                    } else {
                        // Read BBL/BIB file content as text.
                        let content = std::fs::read_to_string(&extracted.path).map_err(|e| {
                            PyRuntimeError::new_err(format!(
                                "Failed to read {}: {}",
                                extracted.filename, e
                            ))
                        })?;
                        return Ok(PyArchiveEntry {
                            filename: extracted.filename,
                            file_type: file_type.to_string(),
                            result: None,
                            content: Some(content),
                        });
                    }
                }
                Ok(ArchiveItem::Warning(msg)) => {
                    self.warnings.push(msg);
                    continue;
                }
                Ok(ArchiveItem::Done { .. }) => {
                    self.join_thread()?;
                    return Err(PyStopIteration::new_err(()));
                }
                Err(_) => {
                    // Channel closed without Done — check thread for errors.
                    self.join_thread()?;
                    return Err(PyStopIteration::new_err(()));
                }
            }
        }
    }
}

/// Returns true if the given path looks like a supported archive (ZIP or tar.gz).
#[pyfunction]
pub fn is_archive_path(path: &str) -> bool {
    archive::is_archive_path(Path::new(path))
}
