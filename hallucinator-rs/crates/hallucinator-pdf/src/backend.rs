use std::path::Path;

use crate::PdfError;

/// Trait for PDF text extraction backends.
///
/// Implementors provide the low-level text extraction step; the parsing
/// pipeline (section detection, reference segmentation, title/author extraction)
/// lives in [`crate::extractor::PdfExtractor`].
pub trait PdfBackend: Send + Sync {
    /// Extract the full text content of a PDF file.
    fn extract_text(&self, path: &Path) -> Result<String, PdfError>;
}
