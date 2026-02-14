use std::path::Path;

use mupdf::{Document, TextPageFlags};

use crate::PdfError;
use crate::text_processing::expand_ligatures;

/// Extract text from a PDF file using MuPDF.
///
/// Opens the PDF, iterates all pages, extracts text from each,
/// joins with newlines, and expands typographic ligatures.
pub fn extract_text_from_pdf(pdf_path: &Path) -> Result<String, PdfError> {
    let path_str = pdf_path
        .to_str()
        .ok_or_else(|| PdfError::OpenError("invalid path encoding".into()))?;

    let document = Document::open(path_str).map_err(|e| PdfError::OpenError(e.to_string()))?;

    let mut pages_text = Vec::new();

    for page_result in document
        .pages()
        .map_err(|e| PdfError::ExtractionError(e.to_string()))?
    {
        let page = page_result.map_err(|e| PdfError::ExtractionError(e.to_string()))?;
        let text_page = page
            .to_text_page(TextPageFlags::empty())
            .map_err(|e| PdfError::ExtractionError(e.to_string()))?;

        // Use block/line iteration to match PyMuPDF's get_text() behavior
        let mut page_text = String::new();
        for block in text_page.blocks() {
            for line in block.lines() {
                let line_text: String = line
                    .chars()
                    .map(|c| c.char().unwrap_or('\u{FFFD}'))
                    .collect();
                page_text.push_str(&line_text);
                page_text.push('\n');
            }
        }
        pages_text.push(page_text);
    }

    let text = pages_text.join("\n");

    // Expand typographic ligatures (ﬁ → fi, ﬂ → fl, etc.)
    Ok(expand_ligatures(&text))
}
