use std::path::Path;

use thiserror::Error;

pub mod authors;
pub mod backend;
pub mod config;
pub mod extractor;
pub mod identifiers;
pub mod scoring;
pub mod section;
pub mod text_processing;
pub mod title;

pub use backend::PdfBackend;
pub use config::{ListOverride, PdfParsingConfig, PdfParsingConfigBuilder};
pub use extractor::PdfExtractor;
pub use scoring::{score_segmentation, select_best_segmentation, ScoringWeights};
pub use section::{SegmentationResult, SegmentationStrategy};
// Re-export domain types from core (canonical definitions live there)
pub use hallucinator_core::{ExtractionResult, Reference, SkipStats};

#[derive(Error, Debug)]
pub enum PdfError {
    #[error("failed to open PDF: {0}")]
    OpenError(String),
    #[error("failed to extract text: {0}")]
    ExtractionError(String),
    #[error("no references section found")]
    NoReferencesSection,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
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
) -> Result<ExtractionResult, PdfError> {
    PdfExtractor::new().extract_references_via_backend(pdf_path, backend)
}
