use std::path::Path;

use thiserror::Error;

pub mod authors;
pub mod config;
pub mod dictionary;
pub mod extractor;
pub mod identifiers;
pub mod scoring;
pub mod section;
pub mod text_processing;
pub mod title;

pub use config::{ListOverride, ParsingConfig, ParsingConfigBuilder};
pub use dictionary::Dictionary;
pub use extractor::ReferenceExtractor;
pub use scoring::{ScoringWeights, score_segmentation, select_best_segmentation};
pub use section::{SegmentationResult, SegmentationStrategy};
// Re-export domain types from core (canonical definitions live there)
pub use hallucinator_core::{BackendError, ExtractionResult, PdfBackend, Reference, SkipStats};

#[derive(Error, Debug)]
pub enum ParsingError {
    #[error("no references section found")]
    NoReferencesSection,
    #[error("backend error: {0}")]
    Backend(#[from] hallucinator_core::BackendError),
}

/// Extract references from a PDF file using the given backend for text extraction.
///
/// Pipeline:
/// 1. Extract text from the PDF via `backend`
/// 2. Locate the References/Bibliography section
/// 3. Segment individual references
/// 4. For each reference, extract DOI, arXiv ID, title, and authors
/// 5. Handle em-dash "same authors" convention
/// 6. Skip non-academic URL-only refs and short/missing titles
pub fn extract_references(
    pdf_path: &Path,
    backend: &dyn PdfBackend,
) -> Result<ExtractionResult, ParsingError> {
    ReferenceExtractor::new().extract_references_via_backend(pdf_path, backend)
}
