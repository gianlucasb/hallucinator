use std::path::Path;

use mupdf::{Document, TextPage, TextPageFlags};

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
            // two-column PDFs without truncating characters at column boundaries.
            //
            // mupdf's to_text() internally calls `read_to_string` on the raw
            // output buffer, which enforces UTF-8. Some PDFs (including two in
            // the NDSS 2026 corpus: 2026-f29, 2026-f808) contain bytes that
            // aren't valid UTF-8, causing the whole extraction to abort with
            // "stream did not contain valid UTF-8". When that happens, fall
            // back to iterating the TextPage block/line/char structure
            // directly — invalid codepoints become `None` via
            // `char::from_u32` and we simply skip them. This yields
            // best-effort text for an otherwise-unusable paper.
            let page_text = match text_page.to_text() {
                Ok(t) => t,
                Err(_) => extract_text_lossy(&text_page),
            };
            pages_text.push(page_text);
        }

        Ok(pages_text.join("\n"))
    }
}

/// Fallback: walk the TextPage block/line/char structure and build a string
/// while skipping bytes that don't map to a valid Unicode scalar.
///
/// Block and line boundaries are emitted as newlines (matching mupdf's
/// default `to_text()` layout) so that downstream reference-section
/// detection still sees reasonable line breaks.
fn extract_text_lossy(text_page: &TextPage) -> String {
    let mut out = String::new();
    for block in text_page.blocks() {
        for line in block.lines() {
            for ch in line.chars() {
                if let Some(c) = ch.char() {
                    out.push(c);
                }
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}
