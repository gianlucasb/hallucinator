use once_cell::sync::Lazy;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Normalize title for comparison — strips to lowercase alphanumeric only.
///
/// Steps:
/// 1. Unescape HTML entities (via a simple replacement approach)
/// 2. Unicode NFKD normalization
/// 3. Strip to ASCII
/// 4. Keep only `[a-zA-Z0-9]`
/// 5. Lowercase
pub fn normalize_title(title: &str) -> String {
    // Simple HTML entity unescaping for common cases
    let title = title
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");

    // NFKD normalization and strip to ASCII
    let normalized: String = title.nfkd().filter(|c| c.is_ascii()).collect();

    // Keep only alphanumeric
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    NON_ALNUM.replace_all(&normalized, "").to_lowercase()
}

/// Check if two titles match using fuzzy comparison (95% threshold).
pub fn titles_match(title_a: &str, title_b: &str) -> bool {
    let norm_a = normalize_title(title_a);
    let norm_b = normalize_title(title_b);

    if norm_a.is_empty() || norm_b.is_empty() {
        return false;
    }

    let score = rapidfuzz::fuzz::ratio(norm_a.chars(), norm_b.chars());
    score >= 0.95
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_title_basic() {
        assert_eq!(normalize_title("Hello, World! 123"), "helloworld123");
    }

    #[test]
    fn test_normalize_title_html_entities() {
        assert_eq!(normalize_title("Foo &amp; Bar"), "foobar");
    }

    #[test]
    fn test_normalize_title_unicode() {
        // é decomposes to e + combining accent, accent gets stripped as non-alnum
        assert_eq!(normalize_title("résumé"), "resume");
    }

    #[test]
    fn test_titles_match_exact() {
        assert!(titles_match(
            "Detecting Hallucinated References",
            "Detecting Hallucinated References"
        ));
    }

    #[test]
    fn test_titles_match_minor_difference() {
        assert!(titles_match(
            "Detecting Hallucinated References in Academic Papers",
            "Detecting Hallucinated References in Academic Paper" // minor typo
        ));
    }

    #[test]
    fn test_titles_no_match() {
        assert!(!titles_match(
            "Detecting Hallucinated References",
            "Completely Different Title About Cats"
        ));
    }

    #[test]
    fn test_titles_match_empty() {
        assert!(!titles_match("", "Something"));
        assert!(!titles_match("Something", ""));
    }
}
