use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

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
    static WORD_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[a-zA-Z0-9]+(?:['\u{2019}\u{2018}\-][a-zA-Z0-9]+)*[?!]?").unwrap()
    });

    let all_words: Vec<&str> = WORD_RE.find_iter(title).map(|m| m.as_str()).collect();

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
        // DOIs like 10.1016/0021-9681(87)90171-8 contain parentheses
        assert_eq!(
            extract_doi("10.1016/0021-9681(87)90171-8"),
            Some("10.1016/0021-9681(87)90171-8".into())
        );
    }

    #[test]
    fn test_extract_doi_with_unbalanced_trailing_paren() {
        // DOI inside parentheses: "(10.1016/0021-9681(87)90171-8)"
        // The trailing ) is unbalanced relative to the DOI itself
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
        // URL in parenthetical context
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
}
