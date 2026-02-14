#[cfg(feature = "pdf")]
use std::path::Path;

use thiserror::Error;

#[cfg(feature = "pdf")]
pub mod archive;
pub mod authors;
pub mod config;
#[cfg(feature = "pdf")]
pub mod extract;
pub mod extractor;
pub mod identifiers;
pub mod section;
pub mod text_processing;
pub mod title;

pub use config::{ListOverride, PdfParsingConfig, PdfParsingConfigBuilder};
pub use extractor::PdfExtractor;

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

/// A parsed reference extracted from a PDF.
#[derive(Debug, Clone)]
pub struct Reference {
    pub raw_citation: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    /// 1-based position in the original PDF reference list (before skip filtering).
    pub original_number: usize,
    /// If set, this reference was skipped during extraction (e.g. "url_only", "short_title").
    pub skip_reason: Option<String>,
}

/// Statistics about references that were skipped during extraction.
#[derive(Debug, Clone, Default)]
pub struct SkipStats {
    pub url_only: usize,
    pub short_title: usize,
    pub no_title: usize,
    pub no_authors: usize,
    pub total_raw: usize,
}

/// Result of extracting references from a PDF.
#[derive(Debug, Clone)]
pub struct ExtractionResult {
    pub references: Vec<Reference>,
    pub skip_stats: SkipStats,
}

/// Extract references from a PDF file.
///
/// Pipeline:
/// 1. Extract text from the PDF using MuPDF
/// 2. Locate the References/Bibliography section
/// 3. Segment individual references
/// 4. For each reference, extract DOI, arXiv ID, title, and authors
/// 5. Handle em-dash "same authors" convention
/// 6. Skip non-academic URL-only refs and short/missing titles
///
/// This is a convenience wrapper around [`PdfExtractor::new().extract_references()`].
#[cfg(feature = "pdf")]
pub fn extract_references(pdf_path: &Path) -> Result<ExtractionResult, PdfError> {
    PdfExtractor::new().extract_references(pdf_path)
}
