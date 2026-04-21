use once_cell::sync::Lazy;
use regex::Regex;
use std::path::Path;
use std::sync::Arc;

use crate::config::ParsingConfig;
use crate::dictionary::Dictionary;
use crate::{ExtractionResult, ParsingError, PdfBackend, Reference, SkipStats};
use crate::{authors, identifiers, section, text_processing, title};

/// A configurable reference extraction pipeline.
///
/// Holds a [`ParsingConfig`] and exposes each pipeline step as a method.
/// The default constructor uses built-in defaults; use [`ReferenceExtractor::with_config`]
/// to supply custom regex patterns and thresholds.
///
/// Optionally accepts a [`Dictionary`] for dictionary-based hyphenation fixing.
/// When a dictionary is provided, merged words are validated against the dictionary;
/// otherwise, heuristic-based hyphenation is used as a fallback.
pub struct ReferenceExtractor {
    config: ParsingConfig,
    dictionary: Option<Arc<dyn Dictionary>>,
}

impl Default for ReferenceExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl ReferenceExtractor {
    /// Create an extractor with default configuration.
    pub fn new() -> Self {
        Self {
            config: ParsingConfig::default(),
            dictionary: None,
        }
    }

    /// Create an extractor with a custom configuration.
    pub fn with_config(config: ParsingConfig) -> Self {
        Self {
            config,
            dictionary: None,
        }
    }

    /// Create an extractor with a dictionary for hyphenation fixing.
    ///
    /// When a dictionary is provided, words are validated against it to determine
    /// whether to merge hyphenated parts (if the merged word exists) or keep the
    /// hyphen (if it doesn't). This is more accurate than heuristic-based hyphenation.
    pub fn with_dictionary<D: Dictionary + 'static>(mut self, dict: D) -> Self {
        self.dictionary = Some(Arc::new(dict));
        self
    }

    /// Create an extractor with a shared dictionary reference.
    ///
    /// Use this when you want to share a dictionary across multiple extractors.
    pub fn with_shared_dictionary(mut self, dict: Arc<dyn Dictionary>) -> Self {
        self.dictionary = Some(dict);
        self
    }

    /// Get a reference to the current config.
    pub fn config(&self) -> &ParsingConfig {
        &self.config
    }

    /// Run the full extraction pipeline on a PDF file using the provided backend.
    pub fn extract_references_via_backend(
        &self,
        pdf_path: &Path,
        backend: &dyn PdfBackend,
    ) -> Result<ExtractionResult, ParsingError> {
        let text = backend.extract_text(pdf_path)?;
        self.extract_references_from_text(&text)
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
        parse_single_reference(
            ref_text,
            prev_authors,
            &self.config,
            self.dictionary.as_deref(),
        )
    }

    /// Run the extraction pipeline on already-extracted text.
    pub fn extract_references_from_text(
        &self,
        text: &str,
    ) -> Result<ExtractionResult, ParsingError> {
        // Expand typographic ligatures (ﬁ → fi, ﬂ → fl, etc.) early in the pipeline
        // so all downstream steps see clean ASCII text.
        let text = text_processing::expand_ligatures(text);
        let ref_section = self
            .find_references_section(&text)
            .ok_or(ParsingError::NoReferencesSection)?;

        let raw_refs = self.segment_references(&ref_section);

        let mut stats = SkipStats {
            total_raw: raw_refs.len(),
            ..Default::default()
        };

        let mut references = Vec::new();
        let mut previous_authors: Vec<String> = Vec::new();

        for (raw_idx, ref_text) in raw_refs.iter().enumerate() {
            let parsed = parse_single_reference(
                ref_text,
                &previous_authors,
                &self.config,
                self.dictionary.as_deref(),
            );
            match parsed {
                ParsedRef::Skip(reason, raw_citation, title) => {
                    match reason {
                        SkipReason::ShortTitle => stats.short_title += 1,
                    }
                    references.push(Reference {
                        raw_citation,
                        title,
                        authors: vec![],
                        doi: None,
                        arxiv_id: None,
                        urls: vec![],
                        original_number: raw_idx + 1,
                        skip_reason: Some(match reason {
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
#[derive(Debug)]
pub enum SkipReason {
    ShortTitle,
}

/// Parse a single reference string, applying config overrides.
fn parse_single_reference(
    ref_text: &str,
    prev_authors: &[String],
    config: &ParsingConfig,
    dictionary: Option<&dyn Dictionary>,
) -> ParsedRef {
    // Extract DOI and arXiv ID BEFORE fixing hyphenation
    let doi = identifiers::extract_doi(ref_text);
    let arxiv_id = identifiers::extract_arxiv_id(ref_text);

    // Extract non-academic URLs BEFORE hyphenation fixing, which can mangle URLs
    // by removing hyphens inside domain names (e.g., "Cisco-Talos" → "CiscoTalos")
    let urls = identifiers::extract_urls(ref_text);

    // Remove standalone page/column numbers on their own lines
    static PAGE_NUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\d{1,4}\n").unwrap());
    let ref_text = PAGE_NUM_RE.replace_all(ref_text, "\n");

    // Fix hyphenation: use dictionary if available, otherwise fall back to heuristics
    let ref_text = if let Some(dict) = dictionary {
        text_processing::fix_hyphenation_with_dict(&ref_text, dict)
    } else {
        text_processing::fix_hyphenation_with_config(&ref_text, config)
    };

    // Extract title
    let (extracted_title, from_quotes) =
        title::extract_title_from_reference_with_config(&ref_text, config);
    let cleaned_title = title::clean_title_with_config(&extracted_title, from_quotes, config);

    if cleaned_title.is_empty() || cleaned_title.split_whitespace().count() < config.min_title_words
    {
        // Short titles can still be real citations if we have strong signals:
        // DOI, arXiv ID, URLs, or venue/year markers in the raw text.
        //
        // A verifiable identifier alone is enough to keep the ref alive
        // even when the title extractor returned nothing usable. Typical
        // shape: "Ze Jiang. trace-ruler. https://github.com/…" — the
        // author-period-title-period prefix confuses the title
        // extractor, cleaned_title comes back empty, but the URL is
        // directly checkable. Previously the ref was skipped and URL
        // Check never ran; now URL Check verifies the link, and the
        // ref is reported as Verified (URL Check) with an empty
        // title rather than as a potential hallucination.
        //
        // `looks_like_citation` alone isn't enough without a title — it
        // only inspects structural hints (venue/year/conf words) that
        // aren't verifiable on their own.
        let has_strong_signal = doi.is_some()
            || arxiv_id.is_some()
            || !urls.is_empty()
            || (!cleaned_title.is_empty() && looks_like_citation(&ref_text));

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

    // Second URL extraction pass: try extracting from the whitespace-normalized
    // raw citation. PDF line breaks often split URLs (e.g., "https://example.org/\npath"
    // becomes "https://example.org/ path" after normalization). The first pass may miss
    // these, but after whitespace normalization the URL fragments are closer together.
    let mut urls = urls;
    if urls.is_empty() {
        let extra_urls = identifiers::extract_urls(&raw_citation);
        if !extra_urls.is_empty() {
            urls = extra_urls;
        }
    }

    // A ref that reached this point via an identifier-only strong signal
    // (URL / DOI / arXiv with an empty title) goes out with `title = None`,
    // not `title = Some("")`. Downstream lookups tolerate a None title
    // (URL Check and Wayback don't use it; DOI / arXiv backends compare
    // against an empty string and return NotFound if no additional
    // identifier logic kicks in), so this keeps the identifier-only
    // code path honest without changing behavior for the normal case
    // where cleaned_title is populated.
    ParsedRef::Ref(Reference {
        raw_citation,
        title: if cleaned_title.is_empty() {
            None
        } else {
            Some(cleaned_title)
        },
        authors: ref_authors,
        doi,
        arxiv_id,
        urls,
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
    use crate::ParsingConfigBuilder;

    // ── ReferenceExtractor with default config ──

    #[test]
    fn test_extractor_default_find_section() {
        let ext = ReferenceExtractor::new();
        let text = "Body text.\n\nReferences\n\n[1] First ref.\n[2] Second ref.\n";
        let section = ext.find_references_section(text).unwrap();
        assert!(section.contains("[1] First ref."));
    }

    #[test]
    fn test_extractor_default_segment() {
        let ext = ReferenceExtractor::new();
        let text = "\n[1] First reference text here.\n[2] Second reference text here.\n[3] Third reference.\n";
        let refs = ext.segment_references(text);
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_extractor_default_parse_reference() {
        let ext = ReferenceExtractor::new();
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
        let ext = ReferenceExtractor::new();

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
    fn test_extractor_extracts_urls_from_refs() {
        let ext = ReferenceExtractor::new();

        // References with non-academic URLs should have URLs extracted
        // They may still be skipped if the title is too short, but URLs should be captured
        let ref_text = r#"J. Smith, "A Great Paper About GitHub Projects," https://github.com/some/repo, 2023."#;
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(!r.urls.is_empty(), "Should extract URL from reference");
                assert!(r.urls[0].contains("github.com"));
            }
            ParsedRef::Skip(..) => panic!("Reference with title and URL should not be skipped"),
        }

        // Academic URLs should NOT be in the urls list (handled by dedicated backends)
        let academic_ref = r#"J. Smith, "A Paper Title About Reference Detection Systems," https://doi.org/10.1234/test, 2023."#;
        let parsed2 = ext.parse_reference(academic_ref, &[]);
        match parsed2 {
            ParsedRef::Ref(r) => {
                assert!(r.urls.is_empty(), "doi.org URLs should be excluded");
            }
            ParsedRef::Skip(..) => panic!("Academic URL ref should not be skipped"),
        }
    }

    #[test]
    fn test_extractor_no_references_section() {
        let ext = ReferenceExtractor::new();
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
        let config = ParsingConfigBuilder::new()
            .section_header_regex(r"(?i)\n\s*Bibliografía\s*\n")
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);

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
        let config = ParsingConfigBuilder::new()
            .section_end_regex(r"(?i)\n\s*Apéndice")
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);

        let text = "Body.\n\nReferences\n\n[1] Ref one.\n\nApéndice A\n\nExtra stuff.";
        let section = ext.find_references_section(text).unwrap();
        assert!(section.contains("[1] Ref one."));
        assert!(!section.contains("Extra stuff"));
    }

    #[test]
    fn test_custom_fallback_fraction() {
        let config = ParsingConfigBuilder::new()
            .fallback_fraction(0.9) // only last 10%
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);

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
        let ext_default = ReferenceExtractor::new();
        let parsed_default = ext_default.parse_reference(ref_text, &[]);
        match parsed_default {
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // expected
            _ => panic!("3-word title should be skipped with default min_title_words=4"),
        }

        // With min_title_words = 3, same reference should PASS
        let config = ParsingConfigBuilder::new()
            .min_title_words(3)
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(r.title.as_ref().unwrap().contains("Three Word Title"));
            }
            ParsedRef::Skip(..) => panic!("3-word title should pass with min_title_words=3"),
        }

        // With min_title_words = 10, a normal title should be skipped
        // (no strong signals to override the threshold)
        let config_strict = ParsingConfigBuilder::new()
            .min_title_words(10)
            .build()
            .unwrap();
        let ext_strict = ReferenceExtractor::with_config(config_strict);
        let long_ref = "Smith, J. A Five Word Paper Title Here";
        let parsed2 = ext_strict.parse_reference(long_ref, &[]);
        match parsed2 {
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // expected
            _ => panic!("5-word title should be skipped with min_title_words=10"),
        }
    }

    #[test]
    fn test_custom_max_authors() {
        let config = ParsingConfigBuilder::new().max_authors(2).build().unwrap();
        let ext = ReferenceExtractor::with_config(config);

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
        let config = ParsingConfigBuilder::new()
            .ieee_segment_regex(r"\n\s*\{(\d+)\}\s*")
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);

        let text = "\n{1} First ref text here.\n{2} Second ref text here.\n{3} Third ref.\n";
        let refs = ext.segment_references(text);
        assert_eq!(refs.len(), 3);
        assert!(refs[0].starts_with("First"));
    }

    #[test]
    fn test_custom_compound_suffix() {
        let config = ParsingConfigBuilder::new()
            .add_compound_suffix("powered".to_string())
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);

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
        let ext = ReferenceExtractor::new();
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
        let ext = ReferenceExtractor::new();
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
        let ext = ReferenceExtractor::new();
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
    fn test_short_title_rescued_by_url() {
        let ext = ReferenceExtractor::new();
        // 3-word title but has URL → should NOT be skipped
        let ref_text = "Smith, J. Word Affect Intensities. https://github.com/user/repo";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(!r.urls.is_empty(), "Should have extracted URL");
            }
            ParsedRef::Skip(..) => panic!("Short title with URL should be rescued"),
        }
    }

    #[test]
    fn test_empty_title_with_url_is_not_skipped() {
        // NDSS 2026 f456 refs 14/29/43/46/47 and many huggingface refs:
        // `Author. short-repo-name. https://github.com/user/repo` — the
        // `author. name. url` prefix confuses the title extractor and
        // cleaned_title comes back empty. Previously the ref was skipped
        // and URL Check never ran. With the strong-signal tweak, any
        // identifier alone keeps the ref alive so URL Check / Wayback
        // can verify it.
        let ext = ReferenceExtractor::new();
        let ref_text = "Ze Jiang. trace-ruler. https://github.com/Ming-bc/trace-ruler";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(!r.urls.is_empty(), "Should surface the GitHub URL");
                assert!(
                    r.urls[0].contains("github.com/Ming-bc/trace-ruler"),
                    "URL should be the cited GitHub repo, got {:?}",
                    r.urls
                );
            }
            ParsedRef::Skip(reason, _, _) => {
                panic!("URL-bearing ref should not be skipped (skipped due to {:?})", reason);
            }
        }
    }

    #[test]
    fn test_identifier_only_ref_keeps_title_as_none() {
        // Companion to test_empty_title_with_url_is_not_skipped: when
        // cleaned_title is empty and the ref survives via the URL
        // strong-signal, the output Reference.title is `None` — not
        // `Some("")` — so downstream display / lookup logic can cleanly
        // distinguish "we have a title" from "we have no title".
        let ext = ReferenceExtractor::new();
        let ref_text = "Author. short. https://github.com/a/b";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                // The extractor may successfully recover a short title
                // like "short" or return None; either is acceptable
                // here. What we're pinning is that if title is present,
                // it's non-empty — never Some("").
                if let Some(t) = &r.title {
                    assert!(!t.is_empty(), "title field must be None, not Some(\"\")");
                }
            }
            ParsedRef::Skip(reason, _, _) => {
                panic!("URL-bearing ref should not be skipped (skipped due to {:?})", reason);
            }
        }
    }

    #[test]
    fn test_short_title_with_no_signal_still_skipped() {
        // Guard: refs with neither a title nor any identifier MUST still
        // get skipped — the Fix 1 change only keeps them alive when
        // there is something verifiable to act on.
        let ext = ReferenceExtractor::new();
        let ref_text = "Foo. bar.";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Skip(..) => { /* expected */ }
            ParsedRef::Ref(r) => {
                panic!(
                    "Ref with no title and no identifier should still be skipped (got title={:?}, urls={:?})",
                    r.title, r.urls
                );
            }
        }
    }

    #[test]
    fn test_edudata_reference() {
        // Real-world case from arxiv paper: one-word title "EduData" with GitHub URL
        let ext = ReferenceExtractor::new();
        let ref_text = "BigData Lab @USTC. 2021. EduData. Online, accessed February 5, 2025. https://github.com/bigdata-ustc/EduData";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert_eq!(
                    r.title.as_deref(),
                    Some("EduData"),
                    "Title should be 'EduData'"
                );
                assert!(!r.urls.is_empty(), "Should have extracted GitHub URL");
                assert!(
                    r.urls[0].contains("github.com"),
                    "URL should be the GitHub URL"
                );
            }
            ParsedRef::Skip(reason, _, _) => {
                panic!(
                    "EduData with GitHub URL should not be skipped (was skipped due to {:?})",
                    reason
                );
            }
        }
    }

    #[test]
    fn test_short_title_rescued_by_venue() {
        let ext = ReferenceExtractor::new();
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
        let ext = ReferenceExtractor::new();
        // 3-word title with no strong signals → should be skipped
        let ref_text = "Smith, J. Three Word Title";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Skip(SkipReason::ShortTitle, _, _) => {} // expected
            _ => panic!("Short title without signals should be skipped"),
        }
    }

    // ── URL extraction from references ──

    #[test]
    fn test_url_reference_extracts_url_and_title() {
        let ext = ReferenceExtractor::new();
        // A reference with a non-academic URL should be parsed (not skipped)
        // with both title and URL extracted
        let ref_text =
            "Smith, J. 2023. Some Interesting Report About Software. https://example.com/report";
        let parsed = ext.parse_reference(ref_text, &[]);
        match parsed {
            ParsedRef::Ref(r) => {
                assert!(r.title.is_some(), "Should extract a title");
                assert!(!r.urls.is_empty(), "Should extract the URL");
                assert!(r.urls[0].contains("example.com"));
            }
            ParsedRef::Skip(..) => {
                panic!("Reference with URL and valid title should not be skipped")
            }
        }
    }

    #[test]
    fn test_two_word_title_rescued_by_venue() {
        // "Translation-based Recommendation" is 2 words — below min_title_words=4.
        // But it has venue ("Proceedings", "ACM", "Conference") + year → should be rescued.
        let ext = ReferenceExtractor::new();
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
        let ext = ReferenceExtractor::new();
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
        let config = ParsingConfigBuilder::new()
            .add_venue_cutoff_pattern(r"(?i)\.\s*My Niche Journal\b.*$".to_string())
            .build()
            .unwrap();
        let ext = ReferenceExtractor::with_config(config);

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
