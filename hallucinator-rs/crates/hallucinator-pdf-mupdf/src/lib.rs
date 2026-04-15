use std::path::Path;

use mupdf::{Document, TextPageFlags};

use hallucinator_core::{BackendError, PdfBackend};

/// MuPDF-based implementation of [`PdfBackend`].
///
/// This crate is the sole AGPL island — it isolates the mupdf dependency
/// (which is AGPL-3.0) so that non-PDF code paths do not transitively
/// depend on it.
pub struct MupdfBackend;

impl PdfBackend for MupdfBackend {
    fn extract_text(&self, path: &Path) -> Result<String, BackendError> {
        let path_str = path
            .to_str()
            .ok_or_else(|| BackendError::OpenError("invalid path encoding".into()))?;

        let document =
            Document::open(path_str).map_err(|e| BackendError::OpenError(e.to_string()))?;

        let mut pages_text = Vec::new();

        for page_result in document
            .pages()
            .map_err(|e| BackendError::ExtractionError(e.to_string()))?
        {
            let page = page_result.map_err(|e| BackendError::ExtractionError(e.to_string()))?;
            let text_page = page
                .to_text_page(TextPageFlags::empty())
                .map_err(|e| BackendError::ExtractionError(e.to_string()))?;

            // Use to_text() for proper text extraction that handles column layouts
            // This uses mupdf's internal text extraction which properly handles
            // two-column PDFs without truncating characters at column boundaries
            let page_text = text_page
                .to_text()
                .map_err(|e| BackendError::ExtractionError(e.to_string()))?;
            pages_text.push(page_text);
        }

        Ok(pages_text.join("\n"))
    }
}
