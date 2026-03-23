//! Shared fuzzy matching utilities for title comparison and query word extraction.
//!
//! Consolidates logic previously duplicated across hallucinator-core, hallucinator-dblp,
//! hallucinator-acl, and hallucinator-openalex.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use unicode_normalization::UnicodeNormalization;

// =============================================================================
// Title normalization
// =============================================================================

/// Mapping of (diacritic, letter) pairs to precomposed characters.
/// Used to fix separated diacritics from PDF extraction.
static DIACRITIC_COMPOSITIONS: Lazy<HashMap<(&str, &str), &str>> = Lazy::new(|| {
    let mut m = HashMap::new();
    // Umlaut/diaeresis (¨)
    m.insert(("\u{a8}", "A"), "Ä");
    m.insert(("\u{a8}", "a"), "ä");
    m.insert(("\u{a8}", "E"), "Ë");
    m.insert(("\u{a8}", "e"), "ë");
    m.insert(("\u{a8}", "I"), "Ï");
    m.insert(("\u{a8}", "i"), "ï");
    m.insert(("\u{a8}", "O"), "Ö");
    m.insert(("\u{a8}", "o"), "ö");
    m.insert(("\u{a8}", "U"), "Ü");
    m.insert(("\u{a8}", "u"), "ü");
    m.insert(("\u{a8}", "Y"), "Ÿ");
    m.insert(("\u{a8}", "y"), "ÿ");
    // Acute accent (´)
    m.insert(("\u{b4}", "A"), "Á");
    m.insert(("\u{b4}", "a"), "á");
    m.insert(("\u{b4}", "E"), "É");
    m.insert(("\u{b4}", "e"), "é");
    m.insert(("\u{b4}", "I"), "Í");
    m.insert(("\u{b4}", "i"), "í");
    m.insert(("\u{b4}", "O"), "Ó");
    m.insert(("\u{b4}", "o"), "ó");
    m.insert(("\u{b4}", "U"), "Ú");
    m.insert(("\u{b4}", "u"), "ú");
    m.insert(("\u{b4}", "N"), "Ń");
    m.insert(("\u{b4}", "n"), "ń");
    m.insert(("\u{b4}", "C"), "Ć");
    m.insert(("\u{b4}", "c"), "ć");
    m.insert(("\u{b4}", "S"), "Ś");
    m.insert(("\u{b4}", "s"), "ś");
    m.insert(("\u{b4}", "Z"), "Ź");
    m.insert(("\u{b4}", "z"), "ź");
    m.insert(("\u{b4}", "Y"), "Ý");
    m.insert(("\u{b4}", "y"), "ý");
    // Grave accent (`)
    m.insert(("`", "A"), "À");
    m.insert(("`", "a"), "à");
    m.insert(("`", "E"), "È");
    m.insert(("`", "e"), "è");
    m.insert(("`", "I"), "Ì");
    m.insert(("`", "i"), "ì");
    m.insert(("`", "O"), "Ò");
    m.insert(("`", "o"), "ò");
    m.insert(("`", "U"), "Ù");
    m.insert(("`", "u"), "ù");
    // Tilde (~ and ˜)
    m.insert(("~", "A"), "Ã");
    m.insert(("~", "a"), "ã");
    m.insert(("\u{2dc}", "A"), "Ã");
    m.insert(("\u{2dc}", "a"), "ã");
    m.insert(("~", "N"), "Ñ");
    m.insert(("~", "n"), "ñ");
    m.insert(("\u{2dc}", "N"), "Ñ");
    m.insert(("\u{2dc}", "n"), "ñ");
    m.insert(("~", "O"), "Õ");
    m.insert(("~", "o"), "õ");
    m.insert(("\u{2dc}", "O"), "Õ");
    m.insert(("\u{2dc}", "o"), "õ");
    // Caron/háček (ˇ)
    m.insert(("\u{2c7}", "C"), "Č");
    m.insert(("\u{2c7}", "c"), "č");
    m.insert(("\u{2c7}", "S"), "Š");
    m.insert(("\u{2c7}", "s"), "š");
    m.insert(("\u{2c7}", "Z"), "Ž");
    m.insert(("\u{2c7}", "z"), "ž");
    m.insert(("\u{2c7}", "E"), "Ě");
    m.insert(("\u{2c7}", "e"), "ě");
    m.insert(("\u{2c7}", "R"), "Ř");
    m.insert(("\u{2c7}", "r"), "ř");
    m.insert(("\u{2c7}", "N"), "Ň");
    m.insert(("\u{2c7}", "n"), "ň");
    // Circumflex (^)
    m.insert(("^", "A"), "Â");
    m.insert(("^", "a"), "â");
    m.insert(("^", "E"), "Ê");
    m.insert(("^", "e"), "ê");
    m.insert(("^", "I"), "Î");
    m.insert(("^", "i"), "î");
    m.insert(("^", "O"), "Ô");
    m.insert(("^", "o"), "ô");
    m.insert(("^", "U"), "Û");
    m.insert(("^", "u"), "û");
    m
});

/// Regex: letter followed by space(s) then a diacritic mark (e.g., "B ¨")
static SPACE_BEFORE_DIACRITIC_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([A-Za-z])\s+([\u{a8}\u{b4}`~\u{2dc}\u{2c7}\^])").unwrap());

/// Regex: diacritic mark followed by optional space then a letter (e.g., "¨U")
static SEPARATED_DIACRITIC_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([\u{a8}\u{b4}`~\u{2dc}\u{2c7}\^])\s*([A-Za-z])").unwrap());

/// Fix separated diacritics from PDF extraction.
///
/// Converts patterns like `"B ¨UNZ"` → `"BÜNZ"` and `"R´enyi"` → `"Rényi"`.
fn fix_separated_diacritics(title: &str) -> String {
    // Step 1: Remove space between letter and diacritic (e.g., "B ¨" -> "B¨")
    let title = SPACE_BEFORE_DIACRITIC_RE.replace_all(title, "$1$2");

    // Step 2: Compose diacritic + letter into precomposed character
    SEPARATED_DIACRITIC_RE
        .replace_all(&title, |caps: &regex::Captures| {
            let diacritic = caps.get(1).unwrap().as_str();
            let letter = caps.get(2).unwrap().as_str();
            DIACRITIC_COMPOSITIONS
                .get(&(diacritic, letter))
                .map(|s| s.to_string())
                .unwrap_or_else(|| letter.to_string())
        })
        .to_string()
}

/// Normalize title for comparison — strips to lowercase alphanumeric only.
///
/// Steps (order matters):
/// 1. Unescape HTML entities
/// 2. Fix separated diacritics from PDF extraction (e.g., "B ¨UNZ" → "BÜNZ")
/// 3. Transliterate Greek letters (e.g., "αdiff" → "alphadiff")
/// 4. Replace math symbols (e.g., "√n" → "sqrtn", "∞" → "infinity")
/// 5. Unicode NFKD normalization (decomposes accents)
/// 6. Strip to ASCII
/// 7. Keep only `[a-zA-Z0-9]`
/// 8. Lowercase
pub fn normalize_title(title: &str) -> String {
    // 1. Simple HTML entity unescaping for common cases
    let title = title
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");

    // 2. Fix separated diacritics from PDF extraction (before NFKD)
    let title = fix_separated_diacritics(&title);

    // 3. Transliterate Greek letters (NFKD doesn't convert these to ASCII)
    let title = title
        .replace(['α', 'Α'], "alpha")
        .replace(['β', 'Β'], "beta")
        .replace(['γ', 'Γ'], "gamma")
        .replace(['δ', 'Δ'], "delta")
        .replace(['ε', 'Ε'], "epsilon")
        .replace(['ζ', 'Ζ'], "zeta")
        .replace(['η', 'Η'], "eta")
        .replace(['θ', 'Θ'], "theta")
        .replace(['ι', 'Ι'], "iota")
        .replace(['κ', 'Κ'], "kappa")
        .replace(['λ', 'Λ'], "lambda")
        .replace(['μ', 'Μ'], "mu")
        .replace(['ν', 'Ν'], "nu")
        .replace(['ξ', 'Ξ'], "xi")
        .replace(['ο', 'Ο'], "o")
        .replace(['π', 'Π'], "pi")
        .replace(['ρ', 'Ρ'], "rho")
        .replace(['σ', 'ς', 'Σ'], "sigma")
        .replace(['τ', 'Τ'], "tau")
        .replace(['υ', 'Υ'], "upsilon")
        .replace(['φ', 'Φ'], "phi")
        .replace(['χ', 'Χ'], "chi")
        .replace(['ψ', 'Ψ'], "psi")
        .replace(['ω', 'Ω'], "omega");

    // 4. Replace mathematical symbols before NFKD strips them
    let title = title
        .replace('∞', "infinity")
        .replace('√', "sqrt")
        .replace('≤', "leq")
        .replace('≥', "geq")
        .replace('≠', "neq")
        .replace('±', "pm")
        .replace('×', "times")
        .replace('÷', "div")
        .replace('∑', "sum")
        .replace('∏', "prod")
        .replace('∫', "int")
        .replace('∂', "partial")
        .replace('∇', "nabla")
        .replace('∈', "in")
        .replace('∉', "notin")
        .replace('⊂', "subset")
        .replace('⊃', "supset")
        .replace('∪', "cup")
        .replace('∩', "cap")
        .replace('∧', "and")
        .replace('∨', "or")
        .replace('¬', "not")
        .replace('→', "to")
        .replace('←', "from")
        .replace('↔', "iff")
        .replace('⇒', "implies")
        .replace('⇐', "impliedby")
        .replace('⇔', "iff");

    // 5-6. NFKD normalization and strip to ASCII
    let normalized: String = title.nfkd().filter(|c| c.is_ascii()).collect();

    // 7-8. Keep only alphanumeric, lowercase
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    NON_ALNUM.replace_all(&normalized, "").to_lowercase()
}

/// Simplified title normalization: lowercase alphanumeric only, no diacritic/Greek handling.
///
/// Use this when the input is already clean (e.g., database records that don't come from PDFs).
pub fn normalize_title_simple(title: &str) -> String {
    static NON_ALNUM: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^a-zA-Z0-9]").unwrap());
    let lowered = title.to_lowercase();
    NON_ALNUM.replace_all(&lowered, "").to_string()
}

/// Check if two titles match using fuzzy comparison (95% threshold).
///
/// Includes conservative prefix matching: if a shorter title is a prefix of a
/// longer one but they differ on subtitle presence (text after `?` or `!`),
/// the match is rejected unless there is ≥70% length coverage.
pub fn titles_match(title_a: &str, title_b: &str) -> bool {
    let norm_a = normalize_title(title_a);
    let norm_b = normalize_title(title_b);

    if norm_a.is_empty() || norm_b.is_empty() {
        return false;
    }

    let score = rapidfuzz::fuzz::ratio(norm_a.chars(), norm_b.chars());
    if score >= 0.95 {
        return true;
    }

    // Conservative prefix matching with subtitle awareness
    let (shorter, longer) = if norm_a.len() <= norm_b.len() {
        (&norm_a, &norm_b)
    } else {
        (&norm_b, &norm_a)
    };

    if shorter.len() < 30 {
        return false;
    }

    if !longer.starts_with(shorter.as_str()) {
        return false;
    }

    let has_subtitle = |t: &str| {
        let lower = t.to_lowercase();
        if let Some(pos) = lower.rfind(['?', '!']) {
            lower[pos + 1..].chars().any(|c| c.is_alphanumeric())
        } else {
            false
        }
    };

    let a_has_subtitle = has_subtitle(title_a);
    let b_has_subtitle = has_subtitle(title_b);

    if a_has_subtitle != b_has_subtitle {
        let coverage = shorter.len() as f64 / longer.len() as f64;
        return coverage >= 0.70;
    }

    true
}

// =============================================================================
// Query word extraction
// =============================================================================

/// Strip LaTeX markup from a title string for FTS query extraction.
///
/// Handles common LaTeX commands found in BibTeX title fields:
/// `\textquoteright` → `'`, `\textendash` → `-`, `$...$` → stripped,
/// `\mathbb{X}` → `X`, `\text{X}` → `X`, `\command` → stripped.
fn strip_latex_for_query(title: &str) -> String {
    static MATH_MODE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\$[^$]*\$").unwrap());
    static CMD_WITH_ARG: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\\(?:mathbb|mathcal|mathrm|mathit|mathbf|text|textbf|textit|textsc|textrm|emph)\s*\{([^}]*)\}").unwrap()
    });
    static BARE_CMD: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\[a-zA-Z]+").unwrap());

    let mut s = title.to_string();

    // Named character commands → replacement
    s = s.replace("\\textquoteright", "'");
    s = s.replace("\\textquoteleft", "'");
    s = s.replace("\\textendash", "-");
    s = s.replace("\\textemdash", "--");

    // Remove math mode entirely
    s = MATH_MODE.replace_all(&s, "").to_string();

    // \mathbb{X}, \text{X}, \emph{X}, etc. → X
    s = CMD_WITH_ARG.replace_all(&s, "$1").to_string();

    // Strip remaining bare \commands
    s = BARE_CMD.replace_all(&s, "").to_string();

    s
}

/// Common stop words excluded from query word extraction.
static STOP_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "the", "and", "for", "with", "from", "that", "this", "have", "are", "was", "were",
        "been", "being", "has", "had", "does", "did", "will", "would", "could", "should",
        "may", "might", "must", "shall", "can", "not", "but", "its", "our", "their", "your",
        "into", "over", "under", "about", "between", "through", "during", "before", "after",
        "above", "below", "each", "every", "both", "few", "more", "most", "other", "some",
        "such", "only", "than", "too", "very",
    ]
    .into_iter()
    .collect()
});

/// Extract meaningful query words for full-text search (4+ chars, no stop words).
///
/// Handles digits (`L2`, `3D`), hyphens (`Machine-Learning`), apostrophes (`What's`),
/// BibTeX braces (`{BERT}` → `BERT`), and LaTeX markup.
///
/// Words are scored by distinctiveness (proper nouns, acronyms, length) and the
/// top `max_words` are returned in their original order.
pub fn get_query_words(title: &str, max_words: usize) -> Vec<String> {
    // Strip LaTeX markup and BibTeX capitalization braces
    let title = strip_latex_for_query(title);
    let title = title.replace(['{', '}'], "");

    static WORD_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"[a-zA-Z0-9]+(?:['\u{2019}\u{2018}\-][a-zA-Z0-9]+)*").unwrap());

    // Collect (original_case, lowercased) pairs with position
    let words_with_info: Vec<(String, String, usize)> = WORD_RE
        .find_iter(&title)
        .flat_map(|m| {
            // Split hyphenated words into parts since FTS tokenizers typically split on hyphens.
            m.as_str()
                .split('-')
                .map(|s| (s.to_string(), s.to_lowercase()))
                .collect::<Vec<_>>()
        })
        .enumerate()
        .map(|(i, (orig, lower))| (orig, lower, i))
        .filter(|(_, lower, _)| lower.len() >= 4 && !STOP_WORDS.contains(lower.as_str()))
        .collect();

    if words_with_info.len() <= max_words {
        return words_with_info
            .into_iter()
            .map(|(_, lower, _)| lower)
            .collect();
    }

    // Score words by distinctiveness and take top N
    let mut scored: Vec<(f64, usize, String)> = words_with_info
        .iter()
        .map(|(orig, lower, pos)| {
            let mut score = lower.len() as f64;
            // Proper nouns / distinctive words
            if orig.starts_with(|c: char| c.is_ascii_uppercase()) {
                score += 10.0;
            }
            // Acronyms (e.g., BERT, NLP)
            if orig.len() >= 3
                && orig
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            {
                score += 5.0;
            }
            // Prefer earlier words
            score -= *pos as f64 * 0.5;
            (score, *pos, lower.clone())
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    scored.truncate(max_words);
    // Restore original order for natural query phrasing
    scored.sort_by_key(|&(_, pos, _)| pos);
    scored.into_iter().map(|(_, _, lower)| lower).collect()
}

/// Given a list of (title, metadata) candidates, find the best fuzzy match above the threshold.
///
/// `candidates` is an iterator of `(candidate_title, T)` pairs.
/// Returns `(score, T)` for the best match, or `None` if none exceed the threshold.
///
/// Uses the full `normalize_title()` for the query and `normalize_title_simple()` for
/// candidates (which are assumed to already be clean database records, not PDF text).
pub fn fuzzy_best_match<T>(
    query_title: &str,
    candidates: impl IntoIterator<Item = (String, T)>,
    threshold: f64,
) -> Option<(f64, T)> {
    let norm_query = normalize_title(query_title);
    if norm_query.is_empty() {
        return None;
    }

    let mut best: Option<(f64, T)> = None;

    for (candidate_title, data) in candidates {
        let norm_candidate = normalize_title_simple(&candidate_title);
        if norm_candidate.is_empty() {
            continue;
        }

        let score = rapidfuzz::fuzz::ratio(norm_query.chars(), norm_candidate.chars());

        if score >= threshold && best.as_ref().is_none_or(|(best_score, _)| score > *best_score) {
            best = Some((score, data));
        }
    }

    best
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- normalize_title ---

    #[test]
    fn test_normalize_basic() {
        assert_eq!(normalize_title("Hello, World! 123"), "helloworld123");
    }

    #[test]
    fn test_normalize_html_entities() {
        assert_eq!(normalize_title("Foo &amp; Bar"), "foobar");
    }

    #[test]
    fn test_normalize_unicode() {
        assert_eq!(normalize_title("résumé"), "resume");
    }

    #[test]
    fn test_normalize_greek() {
        assert_eq!(
            normalize_title("αdiff: Cross-version binary code"),
            "alphadiffcrossversionbinarycode"
        );
    }

    #[test]
    fn test_normalize_diacritics() {
        assert_eq!(normalize_title("B \u{a8}UNZ"), "bunz");
        assert_eq!(normalize_title("R\u{b4}enyi"), "renyi");
    }

    #[test]
    fn test_normalize_math() {
        assert_eq!(
            normalize_title("Breaking the o(√n)-bit barrier"),
            "breakingtheosqrtnbitbarrier"
        );
    }

    // --- titles_match ---

    #[test]
    fn test_titles_match_exact() {
        assert!(titles_match(
            "Detecting Hallucinated References",
            "Detecting Hallucinated References"
        ));
    }

    #[test]
    fn test_titles_no_match() {
        assert!(!titles_match(
            "Detecting Hallucinated References",
            "Completely Different Title About Cats"
        ));
    }

    // --- get_query_words ---

    #[test]
    fn test_get_query_words_basic() {
        let words = get_query_words("Attention is All you Need", 6);
        assert!(words.contains(&"attention".to_string()));
        assert!(words.contains(&"need".to_string()));
    }

    #[test]
    fn test_get_query_words_bibtex() {
        let words = get_query_words("{BERT}: Pre-training of Deep Bidirectional Transformers", 6);
        assert!(words.contains(&"bert".to_string()));
        assert!(words.contains(&"training".to_string()));
    }

    // --- fuzzy_best_match ---

    #[test]
    fn test_fuzzy_best_match_found() {
        let candidates = vec![
            ("Attention is All you Need".to_string(), 1),
            ("Completely Different Paper".to_string(), 2),
        ];
        let result = fuzzy_best_match("Attention is All you Need", candidates, 0.90);
        assert!(result.is_some());
        let (score, id) = result.unwrap();
        assert!(score >= 0.90);
        assert_eq!(id, 1);
    }

    #[test]
    fn test_fuzzy_best_match_none() {
        let candidates = vec![("Completely Different Paper".to_string(), 1)];
        let result = fuzzy_best_match("Attention is All you Need", candidates, 0.90);
        assert!(result.is_none());
    }
}
