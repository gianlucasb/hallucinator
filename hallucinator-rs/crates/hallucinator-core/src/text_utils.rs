use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

/// Extract URLs from reference text.
///
/// Handles common PDF extraction artifacts:
/// - Broken URLs with spaces in "http://" (e.g., "http : //")
/// - Spaced punctuation in URLs (e.g., "www . example . org / path")
/// - Line breaks within URLs
/// - Trailing punctuation
///
/// Excludes:
/// - DOI URLs (already handled via extract_doi)
/// - Academic URLs (doi.org, arxiv.org, etc.)
pub fn extract_urls(text: &str) -> Vec<String> {
    // First, fix broken URL prefixes (common PDF extraction issue)
    // "http : //" or "ht tp://" or "https : / /" or "https : // " etc.
    // Note: Some PDFs even have spaces between the slashes: "https : / /"
    // The trailing \s* consumes any space between the slashes and the domain
    static BROKEN_PREFIX: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)ht\s*tp\s*(s?)\s*:\s*/\s*/\s*").unwrap());
    let text_fixed = BROKEN_PREFIX.replace_all(text, "http$1://");

    // Fix spaced punctuation in URL regions: " . " → "." and " / " → "/"
    // This handles PDFs that add spaces around all punctuation.
    // We apply this aggressively after the protocol is fixed.
    let text_fixed = fix_spaced_url_punctuation(&text_fixed);

    // Fix URLs split across lines - multiple patterns:
    //
    // Pattern 1: Protocol split after colon: "https:\n//www.example.com"
    static PROTOCOL_SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(https?):\s*\n\s*(//[^\s\]>]+)").unwrap());
    let text_fixed = PROTOCOL_SPLIT.replace_all(&text_fixed, "$1:$2");

    // Pattern 2: Domain split mid-word: "https://www.pytho\nn.org" or "https://www.julien\nverneaut.com"
    // Match URL followed by newline and continuation that looks like domain/path (starts with lowercase letter)
    static DOMAIN_SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(https?://[^\s\]>\n]+)\s*\n\s*([a-z][^\s\]>]*)").unwrap());
    let text_fixed = DOMAIN_SPLIT.replace_all(&text_fixed, "$1$2");

    // Pattern 3: Path continuation: URL continues with /path after newline
    static PATH_SPLIT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(https?://[^\s\]>]+)\s*\n\s*(/[^\s\]>]*)").unwrap());
    let text_fixed = PATH_SPLIT.replace_all(&text_fixed, "$1$2");

    // URL regex that captures common URL patterns
    static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"https?://[^\s\]>\)\},]+").unwrap());

    // Academic domains to exclude (these are handled by dedicated backends)
    static ACADEMIC_DOMAINS: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(doi\.org|arxiv\.org|acm\.org|ieee\.org|usenix\.org|semanticscholar\.org|dblp\.org|aclanthology\.org|openreview\.net|neurips\.cc|proceedings\.mlr\.press)").unwrap()
    });

    let mut urls = Vec::new();
    let mut seen = HashSet::new();

    for m in URL_RE.find_iter(&text_fixed) {
        let mut url = m.as_str().to_string();

        // Clean trailing punctuation (common in citations)
        url = url
            .trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '"', '\''])
            .to_string();

        // Skip academic URLs (handled by dedicated backends)
        if ACADEMIC_DOMAINS.is_match(&url) {
            continue;
        }

        // Skip DOI URLs (already extracted separately)
        if url.contains("doi.org") {
            continue;
        }

        // Deduplicate
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }

    urls
}

/// Fix spaced punctuation within URL regions.
///
/// PDFs sometimes add spaces around punctuation, producing URLs like:
/// `https://www . example . org / path / to / file`
///
/// This function finds URL regions (starting with `https://`) and removes
/// spaces around `.` and `/` within them.
fn fix_spaced_url_punctuation(text: &str) -> String {
    // Find URL-like regions and fix spacing within them
    static URL_REGION: Lazy<Regex> = Lazy::new(|| {
        // Match https:// followed by characters that could be URL parts (including spaces around punctuation)
        // Exclude () to avoid capturing "(visited on...)" annotations
        Regex::new(r"https?://[\w\s.\-/~:@!$&'+,;=%?#\[\]]+").unwrap()
    });

    let mut result = text.to_string();

    // Process each potential URL region
    for m in URL_REGION.find_iter(text) {
        let region = m.as_str();
        let fixed = fix_url_spacing(region);
        if fixed != region {
            result = result.replace(region, &fixed);
        }
    }

    result
}

/// Fix spacing within a single URL region.
fn fix_url_spacing(url_region: &str) -> String {
    let mut result = url_region.to_string();

    // Remove spaces around dots: " . " or " ." or ". " → "."
    // But be careful: we don't want to join unrelated words
    // Only fix when surrounded by alphanumeric/URL-like chars
    static SPACED_DOT: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s*\.\s*(\w)").unwrap());
    result = SPACED_DOT.replace_all(&result, "$1.$2").to_string();

    // Remove spaces around slashes when between URL parts: " / " → "/"
    // Only fix when the slash is between alphanumeric/URL-like characters
    // This avoids joining "url/ (visited" → "url/(visited"
    static SPACED_SLASH: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s*/\s*(\w)").unwrap());
    result = SPACED_SLASH.replace_all(&result, "$1/$2").to_string();

    // Remove spaces around hyphens in paths: "call- for- papers" → "call-for-papers"
    static SPACED_HYPHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)\s*-\s*(\w)").unwrap());
    result = SPACED_HYPHEN.replace_all(&result, "$1-$2").to_string();

    result
}

/// Strip unbalanced trailing parentheses, brackets, and braces from a DOI.
fn clean_doi(doi: &str) -> String {
    let mut doi = doi.trim_end_matches(['.', ',', ';', ':']);

    // Strip unbalanced trailing )
    loop {
        if doi.ends_with(')') && doi.matches(')').count() > doi.matches('(').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(['.', ',', ';', ':']);
        } else {
            break;
        }
    }

    // Strip unbalanced trailing ]
    loop {
        if doi.ends_with(']') && doi.matches(']').count() > doi.matches('[').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(['.', ',', ';', ':']);
        } else {
            break;
        }
    }

    // Strip unbalanced trailing }
    loop {
        if doi.ends_with('}') && doi.matches('}').count() > doi.matches('{').count() {
            doi = &doi[..doi.len() - 1];
            doi = doi.trim_end_matches(['.', ',', ';', ':']);
        } else {
            break;
        }
    }

    doi.to_string()
}

/// Extract DOI from reference text.
///
/// Handles formats like:
/// - `10.1234/example`
/// - `doi:10.1234/example`
/// - `https://doi.org/10.1234/example`
/// - `http://dx.doi.org/10.1234/example`
///
/// Also handles DOIs split across lines (common in PDFs) and DOIs
/// containing parentheses (e.g., `10.1016/0021-9681(87)90171-8`).
pub fn extract_doi(text: &str) -> Option<String> {
    // Fix DOIs that are split across lines

    // Pattern 1: DOI ending with period + newline + 3+ digits
    static FIX1: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(10\.\d{4,}/[^\s\]>,]+\.)\s*\n\s*(\d{3,})").unwrap());
    let text_fixed = FIX1.replace_all(text, "$1$2");

    // Pattern 1b: DOI ending with digits + newline + DOI continuation
    static FIX1B: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(10\.\d{4,}/[^\s\]>,]+\d)\s*\n\s*(\d+(?:\.\d+)*)").unwrap());
    let text_fixed = FIX1B.replace_all(&text_fixed, "$1$2");

    // Pattern 2: DOI ending with dash + newline + continuation
    static FIX2: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(10\.\d{4,}/[^\s\]>,]+-)\s*\n\s*(\S+)").unwrap());
    let text_fixed = FIX2.replace_all(&text_fixed, "$1$2");

    // Pattern 3: URL split across lines (period variant)
    static FIX3: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(https?://(?:dx\.)?doi\.org/10\.\d{4,}/[^\s\]>,]+\.)\s*\n\s*(\d+)")
            .unwrap()
    });
    let text_fixed = FIX3.replace_all(&text_fixed, "$1$2");

    // Pattern 3b: URL split mid-number
    static FIX3B: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)(https?://(?:dx\.)?doi\.org/10\.\d{4,}/[^\s\]>,]+\d)\s*\n\s*(\d[^\s\]>,]*)",
        )
        .unwrap()
    });
    let text_fixed = FIX3B.replace_all(&text_fixed, "$1$2");

    // Priority 1: Extract from URL format (most reliable)
    static URL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)https?://(?:dx\.)?doi\.org/(10\.\d{4,}/[^\s\]>},]+)").unwrap()
    });
    if let Some(caps) = URL_RE.captures(&text_fixed) {
        let doi = caps.get(1).unwrap().as_str();
        return Some(clean_doi(doi));
    }

    // Priority 2: DOI pattern without URL prefix
    static DOI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"10\.\d{4,}/[^\s\]>},]+").unwrap());
    if let Some(m) = DOI_RE.find(&text_fixed) {
        let doi = m.as_str();
        return Some(clean_doi(doi));
    }

    None
}

/// Extract arXiv ID from reference text.
///
/// Handles formats like:
/// - `arXiv:2301.12345`
/// - `arXiv:2301.12345v1`
/// - `arxiv.org/abs/2301.12345`
/// - `arXiv:hep-th/9901001` (old format)
/// - `10.48550/arXiv.2301.12345` (DOI format)
/// - `CoRR, abs/2301.12345` (CoRR format)
///
/// Also handles IDs split across lines.
pub fn extract_arxiv_id(text: &str) -> Option<String> {
    // Fix IDs split across lines
    static FIX1: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(arXiv:\d{4}\.)\s*\n\s*(\d+)").unwrap());
    let text_fixed = FIX1.replace_all(text, "$1$2");

    static FIX2: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(arxiv\.org/abs/\d{4}\.)\s*\n\s*(\d+)").unwrap());
    let text_fixed = FIX2.replace_all(&text_fixed, "$1$2");

    // arXiv DOI format: 10.48550/arXiv.YYMM.NNNNN (newer DOI format for arXiv papers)
    static ARXIV_DOI_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)10\.48550/arXiv\.(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = ARXIV_DOI_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // New format: YYMM.NNNNN (with optional version)
    static NEW_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arXiv[:\s]+(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = NEW_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // URL format: arxiv.org/abs/YYMM.NNNNN
    static URL_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arxiv\.org/abs/(\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = URL_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // CoRR format: "CoRR, abs/YYMM.NNNNN" or "CoRR abs/YYMM.NNNNN"
    static CORR_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)CoRR[,:\s]+abs[/:](\d{4}\.\d{4,5}(?:v\d+)?)").unwrap());
    if let Some(caps) = CORR_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // Old format: category/YYMMNNN (e.g., hep-th/9901001)
    static OLD_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arXiv[:\s]+([a-z-]+/\d{7}(?:v\d+)?)").unwrap());
    if let Some(caps) = OLD_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // URL old format
    static URL_OLD_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)arxiv\.org/abs/([a-z-]+/\d{7}(?:v\d+)?)").unwrap());
    if let Some(caps) = URL_OLD_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    // CoRR old format: "CoRR, abs/category/YYMMNNN"
    static CORR_OLD_FMT: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)CoRR[,:\s]+abs[/:]([a-z-]+/\d{7}(?:v\d+)?)").unwrap());
    if let Some(caps) = CORR_OLD_FMT.captures(&text_fixed) {
        return Some(caps.get(1).unwrap().as_str().to_string());
    }

    None
}

/// Common words to skip when building search queries.
static STOP_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "a", "an", "the", "of", "and", "or", "for", "to", "in", "on", "with", "by",
    ]
    .into_iter()
    .collect()
});

/// Extract `n` significant words from a title for building search queries.
///
/// Skips stop words and very short words, but keeps short alphanumeric
/// terms like "L2", "3D", "AI", "5G".
pub fn get_query_words(title: &str, n: usize) -> Vec<String> {
    // Strip BibTeX capitalization braces: {BERT} → BERT, {M}ixup → Mixup
    let title = title.replace(['{', '}'], "");

    static WORD_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[a-zA-Z0-9]+(?:['\u{2019}\u{2018}\-][a-zA-Z0-9]+)*[?!]?").unwrap()
    });

    let all_words: Vec<&str> = WORD_RE.find_iter(&title).map(|m| m.as_str()).collect();

    let significant: Vec<&str> = all_words
        .iter()
        .copied()
        .filter(|w| is_significant(w))
        .collect();

    if significant.len() >= 3 {
        significant.into_iter().take(n).map(String::from).collect()
    } else {
        all_words.into_iter().take(n).map(String::from).collect()
    }
}

fn is_significant(w: &str) -> bool {
    // Strip trailing ?! before checking (e.g., "important?" → "important")
    let w = w.trim_end_matches(['?', '!']);
    if STOP_WORDS.contains(w.to_lowercase().as_str()) {
        return false;
    }
    if w.len() >= 3 {
        return true;
    }
    // Keep short words that mix letters and digits (technical terms)
    let has_letter = w.chars().any(|c| c.is_ascii_alphabetic());
    let has_digit = w.chars().any(|c| c.is_ascii_digit());
    has_letter && has_digit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_doi_basic() {
        assert_eq!(
            extract_doi("doi: 10.1145/3442381.3450048"),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_url() {
        assert_eq!(
            extract_doi("https://doi.org/10.1145/3442381.3450048"),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_split_across_lines() {
        assert_eq!(
            extract_doi("10.1145/3442381.\n3450048"),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_trailing_punct() {
        assert_eq!(
            extract_doi("10.1145/3442381.3450048."),
            Some("10.1145/3442381.3450048".into())
        );
    }

    #[test]
    fn test_extract_doi_none() {
        assert_eq!(extract_doi("No DOI here"), None);
    }

    #[test]
    fn test_extract_doi_with_balanced_parentheses() {
        assert_eq!(
            extract_doi("10.1016/0021-9681(87)90171-8"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_with_unbalanced_trailing_paren() {
        assert_eq!(
            extract_doi("(doi: 10.1016/0021-9681(87)90171-8)"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_url_with_parentheses() {
        assert_eq!(
            extract_doi("https://doi.org/10.1016/0021-9681(87)90171-8"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_url_with_unbalanced_paren() {
        assert_eq!(
            extract_doi("(https://doi.org/10.1016/0021-9681(87)90171-8)"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_clean_doi_no_parens() {
        assert_eq!(
            clean_doi("10.1145/3442381.3450048"),
            "10.1145/3442381.3450048"
        );
    }

    #[test]
    fn test_clean_doi_balanced_parens() {
        assert_eq!(
            clean_doi("10.1016/0021-9681(87)90171-8"),
            "10.1016/0021-9681(87)90171-8"
        );
    }

    #[test]
    fn test_clean_doi_unbalanced_trailing_paren() {
        assert_eq!(
            clean_doi("10.1016/0021-9681(87)90171-8)"),
            "10.1016/0021-9681(87)90171-8"
        );
    }

    #[test]
    fn test_clean_doi_unbalanced_trailing_bracket() {
        assert_eq!(clean_doi("10.1234/test[1]extra]"), "10.1234/test[1]extra");
    }

    #[test]
    fn test_clean_doi_trailing_punct_after_paren() {
        assert_eq!(
            clean_doi("10.1016/0021-9681(87)90171-8)."),
            "10.1016/0021-9681(87)90171-8"
        );
    }

    #[test]
    fn test_extract_arxiv_new_format() {
        assert_eq!(
            extract_arxiv_id("arXiv:2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_with_version() {
        assert_eq!(
            extract_arxiv_id("arXiv:2301.12345v2"),
            Some("2301.12345v2".into())
        );
    }

    #[test]
    fn test_extract_arxiv_url() {
        assert_eq!(
            extract_arxiv_id("arxiv.org/abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_old_format() {
        assert_eq!(
            extract_arxiv_id("arXiv:hep-th/9901001"),
            Some("hep-th/9901001".into())
        );
    }

    #[test]
    fn test_extract_arxiv_split() {
        assert_eq!(
            extract_arxiv_id("arXiv:2301.\n12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_none() {
        assert_eq!(extract_arxiv_id("No arXiv here"), None);
    }

    #[test]
    fn test_extract_arxiv_doi_format() {
        // arXiv DOI format: 10.48550/arXiv.YYMM.NNNNN
        assert_eq!(
            extract_arxiv_id("10.48550/arXiv.2510.14861"),
            Some("2510.14861".into())
        );
        assert_eq!(
            extract_arxiv_id("https://doi.org/10.48550/arXiv.2301.12345v2"),
            Some("2301.12345v2".into())
        );
    }

    #[test]
    fn test_extract_arxiv_corr_format() {
        // CoRR format: CoRR, abs/YYMM.NNNNN
        assert_eq!(
            extract_arxiv_id("CoRR, abs/2503.19786"),
            Some("2503.19786".into())
        );
        assert_eq!(
            extract_arxiv_id("CoRR abs/2301.12345v1"),
            Some("2301.12345v1".into())
        );
        // With colon separator
        assert_eq!(
            extract_arxiv_id("CoRR: abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_corr_old_format() {
        // CoRR old format: CoRR, abs/category/YYMMNNN
        assert_eq!(
            extract_arxiv_id("CoRR, abs/cs/0701001"),
            Some("cs/0701001".into())
        );
        assert_eq!(
            extract_arxiv_id("CoRR abs/hep-th/9901001v2"),
            Some("hep-th/9901001v2".into())
        );
    }

    #[test]
    fn test_get_query_words_basic() {
        let words = get_query_words("Detecting Hallucinated References in Academic Papers", 6);
        assert_eq!(words.len(), 5); // "in" is a stop word, so only 5 significant words
        assert!(!words.contains(&"in".to_string()));
    }

    #[test]
    fn test_get_query_words_technical() {
        let words = get_query_words("L2 Regularization for 3D Models", 6);
        assert!(words.contains(&"L2".to_string()));
        assert!(words.contains(&"3D".to_string()));
    }

    #[test]
    fn test_get_query_words_short_title() {
        let words = get_query_words("A B C", 6);
        // Less than 3 significant words, falls back to all_words
        assert_eq!(words, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_get_query_words_bibtex_braces() {
        let words = get_query_words("{BERT}: Pre-training of Deep Bidirectional Transformers", 6);
        assert!(words.contains(&"BERT".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex_partial_braces() {
        let words = get_query_words("{M}ixup Training for Robust Models", 6);
        assert!(words.contains(&"Mixup".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex_hyphenated() {
        let words = get_query_words("{COVID}-19 Detection with Deep Learning", 6);
        assert!(words.contains(&"COVID-19".to_string()));
    }

    // ── extract_urls tests ──

    #[test]
    fn test_extract_urls_basic() {
        let urls = extract_urls("See https://github.com/user/repo for details.");
        assert_eq!(urls, vec!["https://github.com/user/repo"]);
    }

    #[test]
    fn test_extract_urls_multiple() {
        let urls = extract_urls(
            "Code at https://github.com/user/repo and docs at https://example.com/docs",
        );
        assert_eq!(urls.len(), 2);
        assert!(urls.contains(&"https://github.com/user/repo".to_string()));
        assert!(urls.contains(&"https://example.com/docs".to_string()));
    }

    #[test]
    fn test_extract_urls_trailing_punct() {
        let urls = extract_urls("Visit https://github.com/repo.");
        assert_eq!(urls, vec!["https://github.com/repo"]);

        let urls2 = extract_urls("(see https://example.com/page)");
        assert_eq!(urls2, vec!["https://example.com/page"]);
    }

    #[test]
    fn test_extract_urls_broken_prefix() {
        // PDF extraction sometimes adds spaces in "http://"
        let urls = extract_urls("See http : //github.com/repo for code.");
        assert_eq!(urls, vec!["http://github.com/repo"]);

        let urls2 = extract_urls("Visit ht tp://example.com/page.");
        assert_eq!(urls2, vec!["http://example.com/page"]);
    }

    #[test]
    fn test_extract_urls_excludes_academic() {
        // Academic URLs should be excluded (handled by dedicated backends)
        let urls = extract_urls(
            "Paper at https://arxiv.org/abs/2301.12345 and code at https://github.com/user/repo",
        );
        assert_eq!(urls, vec!["https://github.com/user/repo"]);

        // doi.org should be excluded
        let urls2 = extract_urls("https://doi.org/10.1234/test");
        assert!(urls2.is_empty());

        // Other academic domains
        let urls3 = extract_urls("https://semanticscholar.org/paper/123 https://dblp.org/rec/test");
        assert!(urls3.is_empty());
    }

    #[test]
    fn test_extract_urls_deduplicates() {
        let urls = extract_urls("https://github.com/repo and again https://github.com/repo");
        assert_eq!(urls.len(), 1);
    }

    #[test]
    fn test_extract_urls_none() {
        let urls = extract_urls("No URLs in this text.");
        assert!(urls.is_empty());
    }

    #[test]
    fn test_extract_urls_with_path() {
        let urls = extract_urls("https://github.com/user/repo/blob/main/README.md");
        assert_eq!(
            urls,
            vec!["https://github.com/user/repo/blob/main/README.md"]
        );
    }

    #[test]
    fn test_extract_urls_edudata_case() {
        // Real-world case: citation with GitHub URL
        let text = "BigData Lab @USTC. 2021. EduData. Online, accessed February 5, 2025. https://github.com/bigdata-ustc/EduData";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://github.com/bigdata-ustc/EduData"]);
    }

    #[test]
    fn test_extract_urls_pdf_broken_colon_space() {
        // PDF extraction often produces "https: //" with space after colon
        let text = "Online. https: //www.example.org/page";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://www.example.org/page"]);
    }

    #[test]
    fn test_extract_urls_space_in_domain() {
        // PDF extraction can split domain across lines creating spaces
        // Now fixed! Spaces within URL regions are removed
        let text = "See https://www.cs. cmu.edu/paper.pdf for details";
        let urls = extract_urls(text);
        // URL is now properly reconstructed
        assert_eq!(urls, vec!["https://www.cs.cmu.edu/paper.pdf"]);
    }

    #[test]
    fn test_extract_urls_real_pdf_cases() {
        // Real cases from 300_Transparency Practices.pdf
        let text1 = "https: / / cra . org / wp - content / uploads / 2024 / 07 / Report.pdf";
        let urls1 = extract_urls(text1);
        assert_eq!(
            urls1,
            vec!["https://cra.org/wp-content/uploads/2024/07/Report.pdf"]
        );

        // Space in path with hyphens
        let text2 =
            "https : / / www . usenix . org / conference/usenixsecurity25/call- for- papers";
        let urls2 = extract_urls(text2);
        // usenix.org is academic, so should be excluded
        assert!(urls2.is_empty());

        // chi2024 with space after domain
        let text3 = "https://chi2024. acm.org/2024/02/08/artifacts-at-chi-2024/";
        let urls3 = extract_urls(text3);
        // acm.org is academic, so should be excluded
        assert!(urls3.is_empty());

        // go-fair.org - not academic, should be extracted
        // Note: need space before (visited) for proper parsing
        let text4 = "URL: https://www.go-fair.org/fair-principles/ (visited on...)";
        let urls4 = extract_urls(text4);
        assert_eq!(urls4, vec!["https://www.go-fair.org/fair-principles/"]);

        // cos.io - not academic, should be extracted
        let text5 = "URL: https: //www.cos.io/initiatives/top-guidelines";
        let urls5 = extract_urls(text5);
        assert_eq!(urls5, vec!["https://www.cos.io/initiatives/top-guidelines"]);

        // icpsr.umich.edu - not academic (data repository), should be extracted
        let text6 = "URL: https://www.icpsr.umich.edu/files/deposit/ data.pdf";
        let urls6 = extract_urls(text6);
        // Note: space in path gets fixed
        assert_eq!(
            urls6,
            vec!["https://www.icpsr.umich.edu/files/deposit/data.pdf"]
        );
    }

    #[test]
    fn test_extract_urls_extreme_spacing() {
        // Extreme case: spaces between all URL parts (using non-academic domain)
        let text = "URL: https : / / www . github . com / user / repo";
        let urls = extract_urls(text);
        assert_eq!(urls, vec!["https://www.github.com/user/repo"]);

        // Also test academic domain - should be excluded (handled by dedicated backends)
        let text2 = "URL: https : / / www . acm . org / publications / policies/test";
        let urls2 = extract_urls(text2);
        assert!(
            urls2.is_empty(),
            "acm.org should be excluded as academic domain"
        );
    }

    #[test]
    fn test_extract_urls_space_after_domain() {
        // Space after domain, before path - now fixed!
        let text = "URL: https://www.sigsac.org/ ccs/CCS2024/call-for-papers.html";
        let urls = extract_urls(text);
        // Path is now properly joined
        assert_eq!(
            urls,
            vec!["https://www.sigsac.org/ccs/CCS2024/call-for-papers.html"]
        );
    }

    #[test]
    fn test_extract_urls_line_break_patterns() {
        // Pattern 1: Protocol split after colon
        let text1 = "[97] Wappalyzer. n.d.. Find Out. https:\n//www.wappalyzer.com Accessed";
        let urls1 = extract_urls(text1);
        assert_eq!(urls1, vec!["https://www.wappalyzer.com"]);

        // Pattern 2: Domain split mid-word
        let text2 =
            "[63] Python. 2025. Download Python. https://www.python.o\nrg/downloads Accessed";
        let urls2 = extract_urls(text2);
        assert_eq!(urls2, vec!["https://www.python.org/downloads"]);

        // Pattern 3: Domain split mid-word (longer domain)
        let text3 = "[96] Julien. n.d.. Reverse-Engineering. https://www.julien\nverneaut.com/en/experiments Accessed";
        let urls3 = extract_urls(text3);
        assert_eq!(urls3, vec!["https://www.julienverneaut.com/en/experiments"]);
    }
}
