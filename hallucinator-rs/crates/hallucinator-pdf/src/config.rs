use regex::Regex;

/// Controls how a list of patterns/values is overridden from its defaults.
#[derive(Debug, Clone, Default)]
pub enum ListOverride<T> {
    /// Use the built-in defaults.
    #[default]
    Default,
    /// Completely replace the defaults with these values.
    Replace(Vec<T>),
    /// Append these values to the defaults.
    Extend(Vec<T>),
}

impl<T: Clone> ListOverride<T> {
    /// Resolve this override against the given defaults.
    pub fn resolve(&self, defaults: &[T]) -> Vec<T> {
        match self {
            ListOverride::Default => defaults.to_vec(),
            ListOverride::Replace(v) => v.clone(),
            ListOverride::Extend(v) => {
                let mut result = defaults.to_vec();
                result.extend(v.iter().cloned());
                result
            }
        }
    }
}

/// Configuration for the PDF reference extraction pipeline.
///
/// All regex fields are `Option<Regex>` — `None` means "use the built-in default".
/// Use [`PdfParsingConfigBuilder`] to construct with string patterns.
#[derive(Debug, Clone)]
pub struct PdfParsingConfig {
    // ── section.rs ──
    /// Regex to locate the references section header.
    pub(crate) section_header_re: Option<Regex>,
    /// Regex to find end markers (Appendix, Acknowledgments, etc.).
    pub(crate) section_end_re: Option<Regex>,
    /// Fraction of document to use as fallback when no header is found (0.0–1.0).
    pub(crate) fallback_fraction: f64,
    /// Regex for IEEE-style segmentation: `[1]`, `[2]`, etc.
    pub(crate) ieee_segment_re: Option<Regex>,
    /// Regex for numbered-list segmentation: `1.`, `2.`, etc.
    pub(crate) numbered_segment_re: Option<Regex>,
    /// Regex for fallback double-newline segmentation.
    pub(crate) fallback_segment_re: Option<Regex>,

    // ── title.rs ──
    /// Patterns used to detect venue/metadata suffixes for truncation.
    pub(crate) venue_cutoff_patterns: ListOverride<Regex>,
    /// Patterns used to detect quoted titles.
    pub(crate) quote_patterns: ListOverride<Regex>,

    // ── lib.rs / pipeline ──
    /// Minimum number of words a title must have to be kept (default: 4).
    pub(crate) min_title_words: usize,

    // ── authors.rs ──
    /// Maximum number of authors to retain per reference (default: 15).
    pub(crate) max_authors: usize,

    // ── text_processing.rs ──
    /// Compound-word suffixes that should preserve the hyphen.
    pub(crate) compound_suffixes: ListOverride<String>,
}

impl Default for PdfParsingConfig {
    fn default() -> Self {
        Self {
            section_header_re: None,
            section_end_re: None,
            fallback_fraction: 0.7,
            ieee_segment_re: None,
            numbered_segment_re: None,
            fallback_segment_re: None,
            venue_cutoff_patterns: ListOverride::Default,
            quote_patterns: ListOverride::Default,
            min_title_words: 4,
            max_authors: 15,
            compound_suffixes: ListOverride::Default,
        }
    }
}

/// Builder for [`PdfParsingConfig`].
///
/// Accepts string patterns that are compiled to `Regex` in [`build()`](Self::build).
/// Fails fast with `regex::Error` if any pattern is invalid.
#[derive(Debug, Clone, Default)]
pub struct PdfParsingConfigBuilder {
    section_header_re: Option<String>,
    section_end_re: Option<String>,
    fallback_fraction: Option<f64>,
    ieee_segment_re: Option<String>,
    numbered_segment_re: Option<String>,
    fallback_segment_re: Option<String>,
    venue_cutoff_patterns: ListOverrideBuilder,
    quote_patterns: ListOverrideBuilder,
    min_title_words: Option<usize>,
    max_authors: Option<usize>,
    compound_suffixes: ListOverridePlainBuilder,
}

/// Helper for building `ListOverride<Regex>` from string patterns.
#[derive(Debug, Clone, Default)]
enum ListOverrideBuilder {
    #[default]
    Default,
    Replace(Vec<String>),
    Extend(Vec<String>),
}

/// Helper for building `ListOverride<String>`.
#[derive(Debug, Clone, Default)]
enum ListOverridePlainBuilder {
    #[default]
    Default,
    Replace(Vec<String>),
    Extend(Vec<String>),
}

impl PdfParsingConfigBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Section header / end ──

    pub fn section_header_regex(mut self, pattern: &str) -> Self {
        self.section_header_re = Some(pattern.to_string());
        self
    }

    pub fn section_end_regex(mut self, pattern: &str) -> Self {
        self.section_end_re = Some(pattern.to_string());
        self
    }

    pub fn fallback_fraction(mut self, fraction: f64) -> Self {
        self.fallback_fraction = Some(fraction);
        self
    }

    // ── Segmentation ──

    pub fn ieee_segment_regex(mut self, pattern: &str) -> Self {
        self.ieee_segment_re = Some(pattern.to_string());
        self
    }

    pub fn numbered_segment_regex(mut self, pattern: &str) -> Self {
        self.numbered_segment_re = Some(pattern.to_string());
        self
    }

    pub fn fallback_segment_regex(mut self, pattern: &str) -> Self {
        self.fallback_segment_re = Some(pattern.to_string());
        self
    }

    // ── Venue cutoff patterns ──

    pub fn set_venue_cutoff_patterns(mut self, patterns: Vec<String>) -> Self {
        self.venue_cutoff_patterns = ListOverrideBuilder::Replace(patterns);
        self
    }

    pub fn add_venue_cutoff_pattern(mut self, pattern: String) -> Self {
        match &mut self.venue_cutoff_patterns {
            ListOverrideBuilder::Extend(v) => v.push(pattern),
            _ => self.venue_cutoff_patterns = ListOverrideBuilder::Extend(vec![pattern]),
        }
        self
    }

    // ── Quote patterns ──

    pub fn set_quote_patterns(mut self, patterns: Vec<String>) -> Self {
        self.quote_patterns = ListOverrideBuilder::Replace(patterns);
        self
    }

    pub fn add_quote_pattern(mut self, pattern: String) -> Self {
        match &mut self.quote_patterns {
            ListOverrideBuilder::Extend(v) => v.push(pattern),
            _ => self.quote_patterns = ListOverrideBuilder::Extend(vec![pattern]),
        }
        self
    }

    // ── Scalars ──

    pub fn min_title_words(mut self, n: usize) -> Self {
        self.min_title_words = Some(n);
        self
    }

    pub fn max_authors(mut self, n: usize) -> Self {
        self.max_authors = Some(n);
        self
    }

    // ── Compound suffixes ──

    pub fn set_compound_suffixes(mut self, suffixes: Vec<String>) -> Self {
        self.compound_suffixes = ListOverridePlainBuilder::Replace(suffixes);
        self
    }

    pub fn add_compound_suffix(mut self, suffix: String) -> Self {
        match &mut self.compound_suffixes {
            ListOverridePlainBuilder::Extend(v) => v.push(suffix),
            _ => self.compound_suffixes = ListOverridePlainBuilder::Extend(vec![suffix]),
        }
        self
    }

    /// Compile all string patterns into regexes and produce a [`PdfParsingConfig`].
    pub fn build(self) -> Result<PdfParsingConfig, regex::Error> {
        let compile = |opt: Option<String>| -> Result<Option<Regex>, regex::Error> {
            opt.map(|p| Regex::new(&p)).transpose()
        };

        let compile_list =
            |builder: ListOverrideBuilder| -> Result<ListOverride<Regex>, regex::Error> {
                match builder {
                    ListOverrideBuilder::Default => Ok(ListOverride::Default),
                    ListOverrideBuilder::Replace(patterns) => {
                        let regexes: Result<Vec<_>, _> =
                            patterns.iter().map(|p| Regex::new(p)).collect();
                        Ok(ListOverride::Replace(regexes?))
                    }
                    ListOverrideBuilder::Extend(patterns) => {
                        let regexes: Result<Vec<_>, _> =
                            patterns.iter().map(|p| Regex::new(p)).collect();
                        Ok(ListOverride::Extend(regexes?))
                    }
                }
            };

        let compile_plain = |builder: ListOverridePlainBuilder| -> ListOverride<String> {
            match builder {
                ListOverridePlainBuilder::Default => ListOverride::Default,
                ListOverridePlainBuilder::Replace(v) => ListOverride::Replace(v),
                ListOverridePlainBuilder::Extend(v) => ListOverride::Extend(v),
            }
        };

        Ok(PdfParsingConfig {
            section_header_re: compile(self.section_header_re)?,
            section_end_re: compile(self.section_end_re)?,
            fallback_fraction: self.fallback_fraction.unwrap_or(0.7),
            ieee_segment_re: compile(self.ieee_segment_re)?,
            numbered_segment_re: compile(self.numbered_segment_re)?,
            fallback_segment_re: compile(self.fallback_segment_re)?,
            venue_cutoff_patterns: compile_list(self.venue_cutoff_patterns)?,
            quote_patterns: compile_list(self.quote_patterns)?,
            min_title_words: self.min_title_words.unwrap_or(4),
            max_authors: self.max_authors.unwrap_or(15),
            compound_suffixes: compile_plain(self.compound_suffixes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = PdfParsingConfig::default();
        assert_eq!(config.min_title_words, 4);
        assert_eq!(config.max_authors, 15);
        assert!((config.fallback_fraction - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_builder_basic() {
        let config = PdfParsingConfigBuilder::new()
            .min_title_words(3)
            .max_authors(20)
            .fallback_fraction(0.8)
            .build()
            .unwrap();
        assert_eq!(config.min_title_words, 3);
        assert_eq!(config.max_authors, 20);
        assert!((config.fallback_fraction - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_builder_custom_regex() {
        let config = PdfParsingConfigBuilder::new()
            .section_header_regex(r"(?i)\n\s*Bibliografía\s*\n")
            .build()
            .unwrap();
        assert!(config.section_header_re.is_some());
    }

    #[test]
    fn test_builder_invalid_regex() {
        let result = PdfParsingConfigBuilder::new()
            .section_header_regex(r"[invalid")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn test_list_override_resolve() {
        let defaults = vec!["a".to_string(), "b".to_string()];

        let d: ListOverride<String> = ListOverride::Default;
        assert_eq!(d.resolve(&defaults), defaults);

        let r: ListOverride<String> = ListOverride::Replace(vec!["x".to_string()]);
        assert_eq!(r.resolve(&defaults), vec!["x".to_string()]);

        let e: ListOverride<String> = ListOverride::Extend(vec!["c".to_string()]);
        assert_eq!(
            e.resolve(&defaults),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
}
