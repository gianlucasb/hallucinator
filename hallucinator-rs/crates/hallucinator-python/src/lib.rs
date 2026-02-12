use pyo3::prelude::*;

mod config;
mod errors;
mod extractor;
mod types;
mod validation_types;
mod validator;

/// The native extension module for the `hallucinator` Python package.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // PDF extraction (Phase 1/2A)
    m.add_class::<extractor::PyPdfExtractor>()?; // exported as NativePdfExtractor
    m.add_class::<types::PyReference>()?;
    m.add_class::<types::PyExtractionResult>()?;
    m.add_class::<types::PySkipStats>()?;

    // Validation pipeline (Phase 2B)
    m.add_class::<config::PyValidatorConfig>()?;
    m.add_class::<validator::PyValidator>()?;
    m.add_class::<validation_types::PyValidationResult>()?;
    m.add_class::<validation_types::PyDbResult>()?;
    m.add_class::<validation_types::PyDoiInfo>()?;
    m.add_class::<validation_types::PyArxivInfo>()?;
    m.add_class::<validation_types::PyRetractionInfo>()?;
    m.add_class::<validation_types::PyProgressEvent>()?;
    m.add_class::<validation_types::PyCheckStats>()?;
    Ok(())
}
