use std::path::Path;

use thiserror::Error;

pub mod archive;

// Re-export domain types for convenience
pub use hallucinator_core::{ExtractionResult, Reference, SkipStats};
// Re-export archive API
pub use archive::{ArchiveItem, ExtractedPdf, extract_archive_streaming, is_archive_path};

#[derive(Error, Debug)]
pub enum IngestError {
    #[error("PDF extraction error: {0}")]
    Pdf(#[from] hallucinator_parsing::ParsingError),
    #[error("BBL/BIB extraction error: {0}")]
    Bbl(#[from] hallucinator_bbl::BblError),
    #[cfg(not(feature = "pdf"))]
    #[error("PDF support not compiled in (enable the `pdf` feature of hallucinator-ingest)")]
    NoPdfSupport,
}

/// Extract references from a PDF, BBL, or BIB file.
///
/// Dispatches to the appropriate parser based on file extension:
/// - `.bbl` → BBL parser
/// - `.bib` → BibTeX parser
/// - anything else → PDF parser (requires `pdf` feature / mupdf)
pub fn extract_references(path: &Path) -> Result<ExtractionResult, IngestError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "bbl" => hallucinator_bbl::extract_references_from_bbl(path).map_err(IngestError::Bbl),
        "bib" => hallucinator_bbl::extract_references_from_bib(path).map_err(IngestError::Bbl),
        _ => extract_pdf(path),
    }
}

#[cfg(feature = "pdf")]
fn extract_pdf(path: &Path) -> Result<ExtractionResult, IngestError> {
    let backend = hallucinator_pdf_mupdf::MupdfBackend::default();
    hallucinator_parsing::extract_references(path, &backend).map_err(IngestError::Pdf)
}

#[cfg(not(feature = "pdf"))]
fn extract_pdf(_path: &Path) -> Result<ExtractionResult, IngestError> {
    Err(IngestError::NoPdfSupport)
}
