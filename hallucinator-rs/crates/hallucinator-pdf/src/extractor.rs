use once_cell::sync::Lazy;
use regex::Regex;
#[cfg(feature = "pdf")]
use std::path::Path;

use crate::config::PdfParsingConfig;
use crate::{ExtractionResult, PdfError, Reference, SkipStats};
use crate::{authors, identifiers, section, text_processing, title};

/// A configurable PDF reference extraction pipeline.
///
/// Holds a [`PdfParsingConfig`] and exposes each pipeline step as a method.
/// The default constructor uses built-in defaults; use [`PdfExtractor::with_config`]
/// to supply custom regex patterns and thresholds.
pub struct PdfExtractor {
    config: PdfParsingConfig,
}

impl Default for PdfExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfExtractor {
    /// Create an extractor with default configuration.
    pub fn new() -> Self {
        Self {
            config: PdfParsingConfig::default(),
        }
    }

    /// Create an extractor with a custom configuration.
    pub fn with_config(config: PdfParsingConfig) -> Self {
        Self { config }
    }

    /// Get a reference to the current config.
    pub fn config(&self) -> &PdfParsingConfig {
        &self.config
    }

    /// Extract raw text from a PDF file (step 1).
    #[cfg(feature = "pdf")]
    pub fn extract_text(&self, path: &Path) -> Result<String, PdfError> {
        crate::extract::extract_text_from_pdf(path)
    }

    /// Locate the references section in document text (step 2).
    pub fn find_references_section(&self, text: &str) -> Option<String> {
        section::find_references_section_with_config(text, &self.config)
    }

    /// Segment a references section into individual reference strings (step 3).
    pub fn segment_references(&self, text: &str) -> Vec<String> {
        section::segment_references_with_config(text, &self.config)
    }

    /// Parse a single reference string into a [`Reference`] (step 4).
    ///
    /// `prev_authors` is used for em-dash "same authors" handling.
    pub fn parse_reference(&self, ref_text: &str, prev_authors: &[String]) -> ParsedRef {
        parse_single_reference(ref_text, prev_authors, &self.config)
    }

    /// Run the full extraction pipeline on a PDF file.
    #[cfg(feature = "pdf")]
    pub fn extract_references(&self, pdf_path: &Path) -> Result<ExtractionResult, PdfError> {
        let text = self.extract_text(pdf_path)?;
        self.extract_references_from_text(&text)
    }

    /// Run the extraction pipeline on already-extracted text.
    pub fn extract_references_from_text(&self, text: &str) -> Result<ExtractionResult, PdfError> {
        let ref_section = self
            .find_references_section(text)
            .ok_or(PdfError::NoReferencesSection)?;

        let raw_refs = self.segment_references(&ref_section);

        let mut stats = SkipStats {
            total_raw: raw_refs.len(),
            ..Default::default()
        };

        let mut references = Vec::new();
        let mut previous_authors: Vec<String> = Vec::new();

        for (raw_idx, ref_text) in raw_refs.iter().enumerate() {
            let parsed = parse_single_reference(ref_text, &previous_authors, &self.config);
            match parsed {
                ParsedRef::Skip(reason, raw_citation, title) => {
                    match reason {
                        SkipReason::UrlOnly => stats.url_only += 1,
                        SkipReason::ShortTitle => stats.short_title += 1,
                    }
                    references.push(Reference {
                        raw_citation,
                        title,
                        authors: vec![],
                        doi: None,
                        arxiv_id: None,
                        original_number: raw_idx + 1,
                        skip_reason: Some(match reason {
                            SkipReason::UrlOnly => "url_only".to_string(),
                            SkipReason::ShortTitle => "short_title".to_string(),
                        }),
                    });
                }
                ParsedRef::Ref(mut r) => {
                    r.original_number = raw_idx + 1; // 1-based
                    if r.authors.is_empty() {
                        stats.no_authors += 1;
                    } else {
                        previous_authors = r.authors.clone();
                    }
                    references.push(r);
                }
            }
        }

        Ok(ExtractionResult {
            references,
            skip_stats: stats,
        })
    }
}

/// Result of parsing a single reference.
pub enum ParsedRef {
    Ref(Reference),
    /// A skipped reference: reason, raw_citation, and optional title.
    Skip(SkipReason, String, Option<String>),
}

/// Reason a reference was skipped.
pub enum SkipReason {
    UrlOnly,
    ShortTitle,
}

/// Parse a single reference string, applying config overrides.
fn parse_single_reference(
    ref_text: &str,
    prev_authors: &[String],
    config: &PdfParsingConfig,
) -> ParsedRef {
    // Extract DOI and arXiv ID BEFORE fixing hyphenation
    let doi = identifiers::extract_doi(ref_text);
    let arxiv_id = identifiers::extract_arxiv_id(ref_text);

    // Remove standalone page/column numbers on their own lines
    static PAGE_NUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\d{1,4}\n").unwrap());
    let ref_text = PAGE_NUM_RE.replace_all(ref_text, "\n");

    // Fix hyphenation (config-aware for custom compound suffixes)
    let ref_text = text_processing::fix_hyphenation_with_config(&ref_text, config);

    // Skip entries with non-academic URLs
    static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?\s*:\s*//").unwrap());
    static BROKEN_URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"ht\s*tps?\s*:\s*//").unwrap());
    static ACADEMIC_URL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(acm\.org|ieee\.org|usenix\.org|arxiv\.org|doi\.org)").unwrap()
    });

    if (URL_RE.is_match(&ref_text) || BROKEN_URL_RE.is_match(&ref_text))
        && !ACADEMIC_URL_RE.is_match(&ref_text)
    {
        // Still extract a title for display purposes even though we're skipping
        let (extracted_title, from_quotes) =
            title::extract_title_from_reference_with_config(&ref_text, config);
        let cleaned_title = title::clean_title_with_config(&extracted_title, from_quotes, config);
        let title = if cleaned_title.is_empty() {
            None
        } else {
            Some(cleaned_title)
        };

        static WS_SKIP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
        let raw = WS_SKIP_RE.replace_all(&ref_text, " ").trim().to_string();
        return ParsedRef::Skip(SkipReason::UrlOnly, raw, title);
    }

    // Extract title
    let (extracted_title, from_quotes) =
        title::extract_title_from_reference_with_config(&ref_text, config);
    let cleaned_title = title::clean_title_with_config(&extracted_title, from_quotes, config);

    if cleaned_title.is_empty() || cleaned_title.split_whitespace().count() < config.min_title_words
    {
        // Short titles can still be real citations if we have strong signals:
        // DOI, arXiv ID, quoted title, or venue/year markers in the raw text.
        let has_strong_signal = !cleaned_title.is_empty()
            && (doi.is_some()
                || arxiv_id.is_some()
                || from_quotes
                || looks_like_citation(&ref_text));

        if !has_strong_signal {
            static WS_SKIP_RE2: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
            let raw = WS_SKIP_RE2.replace_all(&ref_text, " ").trim().to_string();
            let title = if cleaned_title.is_empty() {
                None
            } else {
                Some(cleaned_title)
            };
            return ParsedRef::Skip(SkipReason::ShortTitle, raw, title);
        }
    }

    // Extract authors
    let mut ref_authors = authors::extract_authors_from_reference_with_config(&ref_text, config);

    // Handle em-dash "same authors as previous"
    if ref_authors.len() == 1 && ref_authors[0] == authors::SAME_AS_PREVIOUS {
        if !prev_authors.is_empty() {
            ref_authors = prev_authors.to_vec();
        } else {
            ref_authors = vec![];
        }
    }

    // Clean up raw citation for display
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    let raw_citation = WS_RE.replace_all(&ref_text, " ").trim().to_string();
    static IEEE_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[\d+\]\s*").unwrap());
    let raw_citation = IEEE_PREFIX.replace(&raw_citation, "").to_string();
    static NUM_PREFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+\.\s*").unwrap());
    let raw_citation = NUM_PREFIX.replace(&raw_citation, "").to_string();

    ParsedRef::Ref(Reference {
        raw_citation,
        title: Some(cleaned_title),
        authors: ref_authors,
        doi,
        arxiv_id,
        original_number: 0, // placeholder; overwritten by caller
        skip_reason: None,
    })
}

/// Check whether raw citation text has structural signals of a real reference
/// (venue markers, author-year patterns, journal metadata) even when the
/// extracted title is very short.
fn looks_like_citation(ref_text: &str) -> bool {
    static VENUE_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?:In\s+Proceedings|Proc\.|Conference|Workshop|Symposium|IEEE|ACM|USENIX|AAAI|ICML|NeurIPS|ICLR|arXiv\s+preprint|Journal\s+of|Transactions\s+on)\b").unwrap()
    });
    static YEAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?:19|20)\d{2}").unwrap());
    static AUTHOR_YEAR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"[A-Z][a-z]+.*(?:19|20)\d{2}").unwrap());
    static VOL_ISSUE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\d+\s*\(\d+\)").unwrap());

    let has_venue = VENUE_RE.is_match(ref_text);
    let has_year = YEAR_RE.is_match(ref_text);
    let has_author_year = AUTHOR_YEAR_RE.is_match(ref_text);
    let has_vol_issue = VOL_ISSUE_RE.is_match(ref_text);

    // Need at least two signals: venue+year, author+year+volume, etc.
    (has_venue && has_year) || (has_author_year && has_vol_issue)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PdfParsingConfigBuilder;

    // ── PdfExtractor with default config ──

    #[test]
    fn test_extractor_default_find_section() {
        let ext = PdfExtractor::new();
        let text = "Body text.\n\nReferences\n\n[1] First ref.\n[2] Second ref.\n";
        let section = ext.find_references_section(text).unwrap();
        assert!(section.contains("[1] First ref."));
    }

    #[test]
    fn test_extractor_default_segment() {
        let ext = PdfExtractor::new();
        let text = "\n[1] First reference text here.\n[2] Second reference text here.\n[3] Third reference.\n";
        let refs = ext.segment_references(text);
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_extractor_default_parse_reference() {
        let ext = PdfExtractor::new();
        let ref_text = r#"J. Smith, A. Jones, and C. Williams, "Detecting Fake References in Academic Papers," in Proc. IEEE Conf., 2023."#;
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(r.title.unwrap().contains("Detecting Fake References"));
                assert!(!r.authors.is_empty());
            }
            ParsedRef::Skip(..) => panic!("Expected a reference, got skip"),
        }
    }

    #[test]
    fn test_extractor_full_pipeline_from_text() {
        let ext = PdfExtractor::new();

        // In real PDFs, there's typically page content (page number, header text)
        // between the "References" header and the first [1] marker, providing
        // the \n that the IEEE segmentation regex requires.
        let mut text = String::new();
        text.push_str("Body text.\n\nReferences\n");
        // Simulate a page number line between header and first ref (common in real PDFs)
        text.push_str("42\n");
        text.push_str("[1] J. Smith, A. Jones, \"Detecting Fake References in Academic Papers,\" in Proc. IEEE Conf., 2023.\n");
        text.push_str("[2] A. Brown, B. Davis, \"Another Important Paper on Machine Learning Approaches,\" in Proc. AAAI, 2022.\n");
        text.push_str("[3] C. Wilson, \"A Third Paper About Natural Language Processing Systems,\" in Proc. ACL, 2021.\n");
        let result = ext.extract_references_from_text(&text).unwrap();
        assert_eq!(
            result.skip_stats.total_raw, 3,
            "Expected 3 raw refs, got {}",
            result.skip_stats.total_raw,
        );
        assert_eq!(result.references.len(), 3);
    }

    #[test]
    fn test_extractor_skips_url_only_refs() {
        let ext = PdfExtractor::new();
        // Test URL skipping via parse_reference directly (avoids segmentation complexity)
        let ref_text = "See https://github.com/some/repo for details about the implementation.";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Skip(SkipReason::UrlOnly, _, _) => {} // expected
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // also acceptable
            ParsedRef::Ref(r) => panic!("URL-only ref should be skipped, got: {:?}", r.title),
        }

        // Academic URLs should NOT be skipped
        let academic_ref = r#"J. Smith, "A Paper Title About Reference Detection Systems," https://doi.org/10.1234/test, 2023."#;
        let parsed2 = ext.parse_reference(academic_ref, &[]);
        match parsed2 {
            ParsedRef::Ref(_) => {} // expected — doi.org is academic
            ParsedRef::Skip(..) => panic!("Academic URL ref should not be skipped"),
        }
    }

    #[test]
    fn test_extractor_no_references_section() {
        let ext = PdfExtractor::new();
        // Very short text with no references header — fallback will kick in but
        // there won't be meaningful references to parse
        let text = "Short.";
        let result = ext.extract_references_from_text(text);
        // Fallback returns empty section text, which is still Ok but with 0 refs
        assert!(result.is_ok());
    }

    // ── Custom config actually takes effect ──

    #[test]
    fn test_custom_section_header_regex() {
        let config = PdfParsingConfigBuilder::new()
            .section_header_regex(r"(?i)\n\s*Bibliografía\s*\n")
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        // Should find Spanish header
        let text = "Body.\n\nBibliografía\n\n[1] Primer ref.\n[2] Segundo ref.\n[3] Tercer ref.\n";
        let section = ext.find_references_section(text).unwrap();
        assert!(section.contains("[1] Primer ref."));

        // Default "References" header should NOT match with this custom regex —
        // the extractor falls back to the last 30% of the document.
        // Make the text long enough so fallback doesn't include the header.
        let padding = "X ".repeat(200);
        let text2 = format!("{}.\n\nReferences\n\nSome refs here.\n", padding);
        let section2 = ext.find_references_section(&text2).unwrap();
        // Fallback grabs the tail — shouldn't start cleanly after "References"
        assert!(
            !section2.starts_with("\n["),
            "Should be fallback, not header-matched"
        );
    }

    #[test]
    fn test_custom_section_end_regex() {
        let config = PdfParsingConfigBuilder::new()
            .section_end_regex(r"(?i)\n\s*Apéndice")
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        let text = "Body.\n\nReferences\n\n[1] Ref one.\n\nApéndice A\n\nExtra stuff.";
        let section = ext.find_references_section(text).unwrap();
        assert!(section.contains("[1] Ref one."));
        assert!(!section.contains("Extra stuff"));
    }

    #[test]
    fn test_custom_fallback_fraction() {
        let config = PdfParsingConfigBuilder::new()
            .fallback_fraction(0.9) // only last 10%
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        // No references header, so fallback kicks in
        let text = "A".repeat(100) + " last ten percent here";
        let section = ext.find_references_section(&text).unwrap();
        // With 0.9 fraction, we get the last ~10%
        assert!(section.len() < text.len() / 2);
    }

    #[test]
    fn test_custom_min_title_words() {
        // A reference with a 3-word title and no strong citation signals
        // (no DOI, no arXiv, no quotes, no venue/year combo)
        let ref_text = "Smith, J. Three Word Title";

        // Default min_title_words=4 → should be SKIPPED (3 < 4)
        let ext_default = PdfExtractor::new();
        let parsed_default = ext_default.parse_reference(ref_text, &[]);
        match parsed_default {
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // expected
            _ => panic!("3-word title should be skipped with default min_title_words=4"),
        }

        // With min_title_words = 3, same reference should PASS
        let config = PdfParsingConfigBuilder::new()
            .min_title_words(3)
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(r.title.as_ref().unwrap().contains("Three Word Title"));
            }
            ParsedRef::Skip(..) => panic!("3-word title should pass with min_title_words=3"),
        }

        // With min_title_words = 10, a normal title should be skipped
        // (no strong signals to override the threshold)
        let config_strict = PdfParsingConfigBuilder::new()
            .min_title_words(10)
            .build()
            .unwrap();
        let ext_strict = PdfExtractor::with_config(config_strict);
        let long_ref = "Smith, J. A Five Word Paper Title Here";
        let parsed2 = ext_strict.parse_reference(long_ref, &[]);
        match parsed2 {
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // expected
            _ => panic!("5-word title should be skipped with min_title_words=10"),
        }
    }

    #[test]
    fn test_custom_max_authors() {
        let config = PdfParsingConfigBuilder::new()
            .max_authors(2)
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        let ref_text = r#"A. Smith, B. Jones, C. Williams, and D. Brown, "A Paper About Testing Maximum Author Limits in Reference Parsing," in Proc. IEEE, 2023."#;
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(
                    r.authors.len() <= 2,
                    "Expected at most 2 authors, got {}",
                    r.authors.len()
                );
            }
            ParsedRef::Skip(..) => panic!("Expected a reference"),
        }
    }

    #[test]
    fn test_custom_ieee_segment_regex() {
        // Custom pattern that matches {1}, {2}, etc. instead of [1], [2]
        let config = PdfParsingConfigBuilder::new()
            .ieee_segment_regex(r"\n\s*\{(\d+)\}\s*")
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        let text = "\n{1} First ref text here.\n{2} Second ref text here.\n{3} Third ref.\n";
        let refs = ext.segment_references(text);
        assert_eq!(refs.len(), 3);
        assert!(refs[0].starts_with("First"));
    }

    #[test]
    fn test_custom_compound_suffix() {
        let config = PdfParsingConfigBuilder::new()
            .add_compound_suffix("powered".to_string())
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        // "AI- powered" should become "AI-powered" with the custom suffix
        let ref_text = r#"J. Smith, "An AI- powered Approach to Detecting Hallucinated References," in Proc. IEEE, 2023."#;
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(
                    r.title.as_ref().unwrap().contains("AI-powered"),
                    "Expected 'AI-powered', got: {}",
                    r.title.unwrap(),
                );
            }
            ParsedRef::Skip(..) => panic!("Expected a reference"),
        }
    }

    #[test]
    fn test_em_dash_same_authors() {
        let ext = PdfExtractor::new();
        let prev_authors = vec!["J. Smith".to_string(), "A. Jones".to_string()];

        // Em-dash pattern followed by a quoted title (so extraction works reliably)
        let ref_text = "\u{2014}\u{2014}\u{2014}, \"Another Important Paper on Machine Learning Systems,\" in Proc. IEEE, 2023.";
        let parsed = ext.parse_reference(ref_text, &prev_authors);
        match parsed {
            ParsedRef::Ref(r) => {
                assert_eq!(r.authors, prev_authors);
            }
            ParsedRef::Skip(..) => panic!("Expected a reference"),
        }
    }

    // ── looks_like_citation tests ──

    #[test]
    fn test_looks_like_citation_venue_and_year() {
        // Venue + year → true
        assert!(looks_like_citation(
            "Smith, J. 2020. XYZ. In Proceedings of ACM CHI."
        ));
        assert!(looks_like_citation("Jones, K. Foo. Proc. IEEE, 2019."));
    }

    #[test]
    fn test_looks_like_citation_author_year_vol_issue() {
        // Author-year + volume(issue) → true
        assert!(looks_like_citation("Smith 2020. Bar. 15(3), pp. 1-10."));
    }

    #[test]
    fn test_looks_like_citation_not_enough_signals() {
        // Only a year, no venue or vol/issue → false
        assert!(!looks_like_citation("Smith 2020. Some text here."));
        // No year at all → false
        assert!(!looks_like_citation("Smith. Some random text."));
    }

    // ── Strong signal rescue for short titles ──

    #[test]
    fn test_short_title_rescued_by_doi() {
        let ext = PdfExtractor::new();
        // 3-word quoted title with DOI → should NOT be skipped despite short title
        let ref_text = r#"Smith, J. "Word Affect Intensities." doi:10.1234/test.2020"#;
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(r.doi.is_some(), "Should have extracted DOI");
            }
            ParsedRef::Skip(..) => panic!("Short title with DOI should be rescued"),
        }
    }

    #[test]
    fn test_short_title_rescued_by_arxiv() {
        let ext = PdfExtractor::new();
        // 3-word title but has arXiv ID → should NOT be skipped
        let ref_text = "Smith, J. Word Affect Intensities. arXiv:1704.08798";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(r.arxiv_id.is_some(), "Should have extracted arXiv ID");
            }
            ParsedRef::Skip(..) => panic!("Short title with arXiv should be rescued"),
        }
    }

    #[test]
    fn test_short_title_rescued_by_venue() {
        let ext = PdfExtractor::new();
        // 3-word title with venue + year signals → should NOT be skipped
        let ref_text = "Smith, J. 2020. Three Word Title. In Proceedings of ACM CHI. New York.";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(_) => {} // expected
            ParsedRef::Skip(..) => {
                panic!("Short title with venue+year signals should be rescued")
            }
        }
    }

    #[test]
    fn test_short_title_not_rescued_without_signals() {
        let ext = PdfExtractor::new();
        // 3-word title with no strong signals → should be skipped
        let ref_text = "Smith, J. Three Word Title";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // expected
            _ => panic!("Short title without signals should be skipped"),
        }
    }

    // ── URL-only skip with title extraction ──

    #[test]
    fn test_url_only_skip_preserves_title() {
        let ext = PdfExtractor::new();
        // A reference with a non-academic URL that also has a parseable title
        let ref_text =
            "Smith, J. 2023. Some Interesting Report About Software. https://example.com/report";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Skip(SkipReason::UrlOnly, _, title) => {
                assert!(
                    title.is_some(),
                    "URL-only skip should still extract a title"
                );
            }
            ParsedRef::Ref(_) => panic!("Non-academic URL should be skipped"),
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {
                panic!("Should be UrlOnly skip, not ShortTitle")
            }
        }
    }

    #[test]
    fn test_two_word_title_rescued_by_venue() {
        // "Translation-based Recommendation" is 2 words — below min_title_words=4.
        // But it has venue ("Proceedings", "ACM", "Conference") + year → should be rescued.
        let ext = PdfExtractor::new();
        let ref_text = "He, R.; Kang, W.-C.; and McAuley, J. 2017. Translation-based Recommendation. Proceedings of the Eleventh ACM Conference on Recommender Systems";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert_eq!(r.title.as_deref(), Some("Translation-based Recommendation"));
            }
            ParsedRef::Skip(..) => {
                panic!("2-word title with venue+year signals should be rescued")
            }
        }
    }

    #[test]
    fn test_disambiguated_year_suffix() {
        // AAAI year "2022b" — letter suffix for multiple papers by same author in one year
        let ext = PdfExtractor::new();
        let ref_text = "Feng, S.; and Luo, M. 2022b. TwiBot-22: Towards Graph-Based Twitter Bot Detection. In Proceedings of NeurIPS, 35254-35269";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                let title = r.title.unwrap();
                assert!(
                    title.contains("TwiBot-22"),
                    "Title should be the paper title, not the year. Got: {}",
                    title
                );
            }
            ParsedRef::Skip(..) => panic!("Should not be skipped"),
        }
    }

    #[test]
    fn test_add_venue_cutoff_pattern() {
        // Add a custom cutoff pattern for a niche journal
        let config = PdfParsingConfigBuilder::new()
            .add_venue_cutoff_pattern(r"(?i)\.\s*My Niche Journal\b.*$".to_string())
            .build()
            .unwrap();
        let ext = PdfExtractor::with_config(config);

        let ref_text = "Smith, J. and Jones, A. 2022. A Novel Approach to Reference Detection. My Niche Journal, vol 5.";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                let title = r.title.unwrap();
                assert!(
                    !title.contains("My Niche Journal"),
                    "Custom cutoff should remove journal name, got: {}",
                    title,
                );
            }
            ParsedRef::Skip(..) => panic!("Expected a reference"),
        }
    }
}
