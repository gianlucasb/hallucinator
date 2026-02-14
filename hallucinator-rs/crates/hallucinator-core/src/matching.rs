use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

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

    // =========================================================================
    // Basic normalization
    // =========================================================================

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

    // =========================================================================
    // Greek letter transliteration
    // =========================================================================

    #[test]
    fn test_greek_epsilon() {
        assert_eq!(
            normalize_title("εpsolute: Efficiently querying databases"),
            "epsilonpsoluteefficientlyqueryingdatabases"
        );
    }

    #[test]
    fn test_greek_alpha() {
        assert_eq!(
            normalize_title("αdiff: Cross-version binary code similarity"),
            "alphadiffcrossversionbinarycodesimilarity"
        );
    }

    #[test]
    fn test_greek_tau() {
        assert_eq!(
            normalize_title("τCFI: Type-assisted Control Flow Integrity"),
            "taucfitypeassistedcontrolflowintegrity"
        );
    }

    #[test]
    fn test_greek_phi() {
        assert_eq!(
            normalize_title("Prooφ: A zkp market mechanism"),
            "proophiazkpmarketmechanism"
        );
    }

    #[test]
    fn test_greek_mixed() {
        assert_eq!(
            normalize_title("oφoς: Forward secure searchable encryption"),
            "ophiosigmaforwardsecuresearchableencryption"
        );
    }

    #[test]
    fn test_greek_alpha_beta_pair() {
        assert_eq!(
            normalize_title("(α,β)-Core Query over Bipartite Graphs"),
            "alphabetacorequeryoverbipartitegraphs"
        );
    }

    #[test]
    fn test_greek_uppercase() {
        assert_eq!(
            normalize_title("Δ-learning for robotics"),
            "deltalearningforrobotics"
        );
    }

    // =========================================================================
    // Separated diacritics from PDF extraction
    // =========================================================================

    #[test]
    fn test_diacritic_umlaut_space() {
        // "B ¨UNZ" -> "BÜNZ" -> NFKD -> "BUNZ" -> "bunz"
        assert_eq!(normalize_title("B \u{a8}UNZ"), "bunz");
    }

    #[test]
    fn test_diacritic_umlaut_dottling() {
        assert_eq!(normalize_title("D \u{a8}OTTLING"), "dottling");
    }

    #[test]
    fn test_diacritic_acute_renyi() {
        // "R´enyi" -> "Rényi" -> NFKD -> "Renyi" -> "renyi"
        assert_eq!(normalize_title("R\u{b4}enyi"), "renyi");
    }

    #[test]
    fn test_diacritic_mixed_ordonez() {
        // "Ord´o˜nez" -> "Ordóñez" -> NFKD -> "Ordonez" -> "ordonez"
        assert_eq!(normalize_title("Ord\u{b4}o\u{2dc}nez"), "ordonez");
    }

    #[test]
    fn test_diacritic_caron_novacek() {
        // "Nov´aˇcek" -> "Nováček" -> NFKD -> "Novacek" -> "novacek"
        assert_eq!(normalize_title("Nov\u{b4}a\u{2c7}cek"), "novacek");
    }

    #[test]
    fn test_diacritic_leading_umlaut() {
        // "¨Uber" -> "Über" -> NFKD -> "Uber" -> "uber"
        assert_eq!(
            normalize_title("\u{a8}Uber das paulische"),
            "uberdaspaulische"
        );
    }

    #[test]
    fn test_diacritic_habock() {
        assert_eq!(normalize_title("HAB \u{a8}OCK"), "habock");
    }

    #[test]
    fn test_diacritic_krol() {
        assert_eq!(normalize_title("KR \u{b4}OL"), "krol");
    }

    #[test]
    fn test_diacritic_grave_riviere() {
        assert_eq!(normalize_title("RIVI`ERE"), "riviere");
    }

    #[test]
    fn test_diacritic_no_change() {
        assert_eq!(
            normalize_title("Normal text without diacritics"),
            "normaltextwithoutdiacritics"
        );
    }

    // =========================================================================
    // Math symbol replacement
    // =========================================================================

    #[test]
    fn test_normalize_h_infinity() {
        assert_eq!(
            normalize_title("H\u{221E} almost state synchronization"),
            "hinfinityalmoststate synchronization".replace(' ', "")
        );
        assert_eq!(
            normalize_title("Robust H\u{221E} filtering"),
            "robusthinfinityfiltering"
        );
    }

    #[test]
    fn test_h_infinity_fuzzy_match() {
        assert!(titles_match(
            "H\u{221E} almost state synchronization for homogeneous networks",
            "H-infinity almost state synchronization for homogeneous networks"
        ));
    }

    #[test]
    fn test_math_sqrt() {
        assert_eq!(
            normalize_title("Breaking the o(√n)-bit barrier"),
            "breakingtheosqrtnbitbarrier"
        );
    }

    #[test]
    fn test_math_leq_geq() {
        assert_eq!(normalize_title("x ≤ y"), "xleqy");
        assert_eq!(normalize_title("y ≥ z"), "ygeqz");
    }

    #[test]
    fn test_math_set_ops() {
        assert_eq!(normalize_title("A ∪ B ∩ C"), "acupbcapc");
    }

    #[test]
    fn test_math_arrow() {
        assert_eq!(normalize_title("f: A → B"), "fatob");
    }

    #[test]
    fn test_math_implies() {
        assert_eq!(normalize_title("P ⇒ Q"), "pimpliesq");
    }

    #[test]
    fn test_math_pm_times() {
        assert_eq!(normalize_title("a ± b × c"), "apmbtimesc");
    }

    #[test]
    fn test_math_nabla_partial() {
        assert_eq!(normalize_title("∇f and ∂g"), "nablafandpartialg");
    }

    #[test]
    fn test_math_no_change() {
        assert_eq!(
            normalize_title("Normal title without math"),
            "normaltitlewithoutmath"
        );
    }

    // =========================================================================
    // Combined pipeline
    // =========================================================================

    #[test]
    fn test_combined_greek_and_diacritics() {
        // τCFI with accented author context
        assert_eq!(
            normalize_title("τCFI: Type-assisted Control Flow"),
            "taucfitypeassistedcontrolflow"
        );
    }

    #[test]
    fn test_combined_greek_and_math() {
        // Mix of Greek + math symbols
        assert_eq!(normalize_title("α ≤ β → γ"), "alphaleqbetatogamma");
    }

    #[test]
    fn test_combined_diacritic_and_math() {
        // Diacritic + math in same title
        assert_eq!(
            normalize_title("R\u{b4}enyi divergence ≤ KL divergence"),
            "renyidivergenceleqkldivergence"
        );
    }

    #[test]
    fn test_combined_all_three() {
        // Greek + separated diacritic + math
        assert_eq!(
            normalize_title("εpsolute with B \u{a8}UNZ and √n bound"),
            "epsilonpsolutewithbunzandsqrtnbound"
        );
    }

    // =========================================================================
    // Fuzzy matching across normalized forms
    // =========================================================================

    #[test]
    fn test_fuzzy_greek_vs_spelled_out() {
        // PDF has Greek letter, database has spelled-out name
        assert!(titles_match(
            "εpsolute: Efficiently querying databases while providing differential privacy",
            "Epsilonpsolute: Efficiently querying databases while providing differential privacy"
        ));
    }

    #[test]
    fn test_fuzzy_diacritic_vs_clean() {
        // PDF has separated diacritics, database has clean ASCII
        assert!(titles_match(
            "R\u{b4}enyi differential privacy of the sampled Gaussian mechanism",
            "Renyi differential privacy of the sampled Gaussian mechanism"
        ));
    }

    #[test]
    fn test_fuzzy_math_vs_word() {
        // PDF has √ symbol, database uses "sqrt"
        assert!(titles_match(
            "Breaking the o(√n)-bit barrier: Byzantine agreement with polylog bits",
            "Breaking the o(sqrt n)-bit barrier: Byzantine agreement with polylog bits"
        ));
    }

    #[test]
    fn test_fuzzy_accented_vs_ascii() {
        // Standard accented chars (handled by NFKD) still work
        assert!(titles_match(
            "Déjà Vu: Side-Channel Analysis of Randomization",
            "Deja Vu: Side-Channel Analysis of Randomization"
        ));
    }
}
