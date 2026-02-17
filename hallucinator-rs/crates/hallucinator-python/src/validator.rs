use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use tokio_util::sync::CancellationToken;

use hallucinator_core::Reference;

use crate::config::PyValidatorConfig;
use crate::types::PyReference;
use crate::validation_types::{PyCheckStats, PyProgressEvent, PyValidationResult};

/// Validates references against academic databases.
///
/// Example::
///
///     config = ValidatorConfig()
///     config.s2_api_key = "..."
///
///     validator = Validator(config)
///     results = validator.check(references, progress=lambda e: print(e))
///
#[pyclass(name = "Validator")]
pub struct PyValidator {
    config: hallucinator_core::Config,
    runtime: tokio::runtime::Runtime,
    cancel: CancellationToken,
}

#[pymethods]
impl PyValidator {
    /// Create a new Validator with the given configuration.
    #[new]
    fn new(config: &PyValidatorConfig) -> PyResult<Self> {
        let core_config = config.to_core_config()?;
        let runtime = tokio::runtime::Runtime::new().map_err(|e| {
            PyRuntimeError::new_err(format!("Failed to create tokio runtime: {}", e))
        })?;
        Ok(Self {
            config: core_config,
            runtime,
            cancel: CancellationToken::new(),
        })
    }

    /// Check references against academic databases.
    ///
    /// Blocks until all checks complete. Releases the GIL so other
    /// Python threads can run concurrently.
    ///
    /// Args:
    ///     references: List of ``Reference`` objects from PDF extraction.
    ///     progress: Optional callable receiving ``ProgressEvent`` objects.
    ///
    /// Returns:
    ///     List of ``ValidationResult`` objects.
    #[pyo3(signature = (references, progress=None))]
    fn check(
        &self,
        py: Python<'_>,
        references: Vec<PyReference>,
        progress: Option<PyObject>,
    ) -> PyResult<Vec<PyValidationResult>> {
        // Convert PyReference -> Reference
        let refs: Vec<Reference> = references.into_iter().map(|r| r.into_inner()).collect();

        let config = self.config.clone();
        let cancel = self.cancel.clone();

        // Reset cancellation token for a fresh run
        let cancel = if cancel.is_cancelled() {
            // Token was previously cancelled; create a fresh one
            // (We can't un-cancel a CancellationToken)
            CancellationToken::new()
        } else {
            cancel
        };

        let runtime = &self.runtime;

        let results = py.allow_threads(move || {
            runtime.block_on(async move {
                if let Some(cb) = progress {
                    hallucinator_core::check_references(
                        refs,
                        config,
                        move |event| {
                            let py_event = PyProgressEvent::from(event);
                            Python::with_gil(|py| {
                                if let Err(e) = cb.call1(py, (py_event,)) {
                                    eprintln!("hallucinator: progress callback error: {}", e);
                                }
                            });
                        },
                        cancel,
                    )
                    .await
                } else {
                    hallucinator_core::check_references(refs, config, |_| {}, cancel).await
                }
            })
        });

        Ok(results.into_iter().map(PyValidationResult::from).collect())
    }

    /// Compute summary statistics from validation results.
    ///
    /// Args:
    ///     results: List of ``ValidationResult`` objects.
    ///
    /// Returns:
    ///     A ``CheckStats`` summary.
    #[staticmethod]
    fn stats(results: Vec<PyValidationResult>) -> PyCheckStats {
        let inners: Vec<&hallucinator_core::ValidationResult> =
            results.iter().map(|r| r.inner()).collect();
        PyCheckStats::compute(&inners)
    }

    /// Cancel an in-progress check from another thread.
    fn cancel(&self) {
        self.cancel.cancel();
    }

    fn __repr__(&self) -> String {
        format!("Validator(num_workers={})", self.config.num_workers,)
    }
}
