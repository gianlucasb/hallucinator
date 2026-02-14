use pyo3::PyErr;
use pyo3::exceptions::PyRuntimeError;

use hallucinator_pdf::PdfError;

/// Convert a `PdfError` into a Python exception.
pub fn pdf_error_to_py(e: PdfError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}
