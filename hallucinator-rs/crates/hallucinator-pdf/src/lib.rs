#[cfg(feature = "pdf")]
use std::path::Path;

#[cfg(feature = "pdf")]
use once_cell::sync::Lazy;
#[cfg(feature = "pdf")]
use regex::Regex;
use thiserror::Error;

#[cfg(feature = "pdf")]
pub mod archive;
pub mod authors;
#[cfg(feature = "pdf")]
pub mod extract;
pub mod identifiers;
pub mod section;
pub mod text_processing;
pub mod title;

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
#[cfg(feature = "pdf")]
pub fn extract_references(pdf_path: &Path) -> Result<ExtractionResult, PdfError> {
    let text = extract::extract_text_from_pdf(pdf_path)?;

    let ref_section =
        section::find_references_section(&text).ok_or(PdfError::NoReferencesSection)?;

    let raw_refs = section::segment_references(&ref_section);

    let mut stats = SkipStats {
        total_raw: raw_refs.len(),
        ..Default::default()
    };

    let mut references = Vec::new();
    let mut previous_authors: Vec<String> = Vec::new();

    for ref_text in &raw_refs {
        // Extract DOI and arXiv ID BEFORE fixing hyphenation
        let doi = identifiers::extract_doi(ref_text);
        let arxiv_id = identifiers::extract_arxiv_id(ref_text);

        // Remove standalone page/column numbers on their own lines
        static PAGE_NUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\d{1,4}\n").unwrap());
        let ref_text = PAGE_NUM_RE.replace_all(ref_text, "\n");

        // Fix hyphenation
        let ref_text = text_processing::fix_hyphenation(&ref_text);

        // Skip entries with non-academic URLs
        static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?\s*:\s*//").unwrap());
        static BROKEN_URL_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"ht\s*tps?\s*:\s*//").unwrap());
        static ACADEMIC_URL_RE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r"(?i)(acm\.org|ieee\.org|usenix\.org|arxiv\.org|doi\.org)").unwrap()
        });

        if (URL_RE.is_match(&ref_text) || BROKEN_URL_RE.is_match(&ref_text))
            && !ACADEMIC_URL_RE.is_match(&ref_text)
        {
            stats.url_only += 1;
            continue;
        }

        // Extract title
        let (extracted_title, from_quotes) = title::extract_title_from_reference(&ref_text);
        let cleaned_title = title::clean_title(&extracted_title, from_quotes);

        if cleaned_title.is_empty() || cleaned_title.split_whitespace().count() < 4 {
            stats.short_title += 1;
            continue;
        }

        // Extract authors
        let mut ref_authors = authors::extract_authors_from_reference(&ref_text);

        // Handle em-dash "same authors as previous"
        if ref_authors.len() == 1 && ref_authors[0] == authors::SAME_AS_PREVIOUS {
            if !previous_authors.is_empty() {
                ref_authors = previous_authors.clone();
            } else {
                ref_authors = vec![];
            }
        }

        if ref_authors.is_empty() {
            stats.no_authors += 1;
            // Still include the reference (just track it)
        }

        // Update previous_authors
        if !ref_authors.is_empty() {
            previous_authors = ref_authors.clone();
        }

        // Clean up raw citation for display
        static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
        let raw_citation = WS_RE.replace_all(&ref_text, " ").trim().to_string();
        static IEEE_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[\d+\]\s*").unwrap());
        let raw_citation = IEEE_PREFIX.replace(&raw_citation, "").to_string();
        static NUM_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+\.\s*").unwrap());
        let raw_citation = NUM_PREFIX.replace(&raw_citation, "").to_string();

        references.push(Reference {
            raw_citation,
            title: Some(cleaned_title),
            authors: ref_authors,
            doi,
            arxiv_id,
        });
    }

    Ok(ExtractionResult {
        references,
        skip_stats: stats,
    })
}
