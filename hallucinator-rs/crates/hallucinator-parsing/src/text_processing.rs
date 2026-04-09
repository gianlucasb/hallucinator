use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::config::ParsingConfig;
use crate::dictionary::Dictionary;

/// Common compound-word suffixes that should keep the hyphen.
/// Used only when no dictionary is available.
pub(crate) static COMPOUND_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "centered",
        "based",
        "driven",
        "directed",
        "aware",
        "oriented",
        "specific",
        "related",
        "dependent",
        "independent",
        "like",
        "free",
        "friendly",
        "rich",
        "poor",
        "scale",
        "level",
        "order",
        "class",
        "type",
        "style",
        "wise",
        "fold",
        "shot",
        "step",
        "time",
        "world",
        "source",
        "domain",
        "task",
        "modal",
        "intensive",
        "efficient",
        "agnostic",
        "invariant",
        "sensitive",
        "grained",
        "agent",
        "site",
        "throughput",
        "flow",
        "assisted",
        "augmented",
        "integrated",
        "empowered",
        "guided",
        "supervised",
        "training",
        "key",
        "day",
        "box",
        "end",
        "party",
        "round",
        "size",
        "server",
        "client",
        "channel",
        "optimal",
        "resilient",
        "resistant",
        "tolerant",
        "hiding",
        "preserving",
        "knowledge",
        "latency",
        "precision",
        "centric",
        "aided",
        "authenticated",
    ]
    .into_iter()
    .collect()
});

/// Expand common typographic ligatures found in PDFs and fix separated diacritics.
pub fn expand_ligatures(text: &str) -> String {
    let text = text
        .replace('\u{FB00}', "ff")
        .replace('\u{FB01}', "fi")
        .replace('\u{FB02}', "fl")
        .replace('\u{FB03}', "ffi")
        .replace('\u{FB04}', "ffl")
        .replace(['\u{FB05}', '\u{FB06}'], "st");

    // Fix separated diacritics from PDF extraction (e.g., "´e" → "é", "¨o" → "ö")
    // These appear when PDFs encode accented characters as separate diacritic + letter.
    fix_separated_diacritics_pdf(&text)
}

/// Compose separated diacritics from PDF extraction into proper Unicode characters.
///
/// Handles patterns like:
/// - `"´e"` → `"é"` (acute accent)
/// - `"¨o"` → `"ö"` (umlaut/diaeresis)
/// - `"`a"` → `"à"` (grave accent)
/// - `"ˇc"` → `"č"` (caron)
fn fix_separated_diacritics_pdf(text: &str) -> String {
    use std::collections::HashMap;

    static COMPOSITIONS: Lazy<HashMap<(char, char), char>> = Lazy::new(|| {
        let mut m = HashMap::new();
        // Umlaut/diaeresis (¨ U+00A8)
        for (letter, composed) in [
            ('A', 'Ä'),
            ('a', 'ä'),
            ('E', 'Ë'),
            ('e', 'ë'),
            ('I', 'Ï'),
            ('i', 'ï'),
            ('O', 'Ö'),
            ('o', 'ö'),
            ('U', 'Ü'),
            ('u', 'ü'),
            ('Y', 'Ÿ'),
            ('y', 'ÿ'),
        ] {
            m.insert(('\u{a8}', letter), composed);
        }
        // Acute accent (´ U+00B4)
        for (letter, composed) in [
            ('A', 'Á'),
            ('a', 'á'),
            ('E', 'É'),
            ('e', 'é'),
            ('I', 'Í'),
            ('i', 'í'),
            ('O', 'Ó'),
            ('o', 'ó'),
            ('U', 'Ú'),
            ('u', 'ú'),
            ('N', 'Ń'),
            ('n', 'ń'),
            ('C', 'Ć'),
            ('c', 'ć'),
            ('S', 'Ś'),
            ('s', 'ś'),
            ('Z', 'Ź'),
            ('z', 'ź'),
            ('Y', 'Ý'),
            ('y', 'ý'),
        ] {
            m.insert(('\u{b4}', letter), composed);
        }
        // Grave accent (` U+0060)
        for (letter, composed) in [
            ('A', 'À'),
            ('a', 'à'),
            ('E', 'È'),
            ('e', 'è'),
            ('I', 'Ì'),
            ('i', 'ì'),
            ('O', 'Ò'),
            ('o', 'ò'),
            ('U', 'Ù'),
            ('u', 'ù'),
        ] {
            m.insert(('`', letter), composed);
        }
        // Tilde (˜ U+02DC)
        for (letter, composed) in [
            ('A', 'Ã'),
            ('a', 'ã'),
            ('N', 'Ñ'),
            ('n', 'ñ'),
            ('O', 'Õ'),
            ('o', 'õ'),
        ] {
            m.insert(('\u{2dc}', letter), composed);
        }
        // Caron/háček (ˇ U+02C7)
        for (letter, composed) in [
            ('C', 'Č'),
            ('c', 'č'),
            ('S', 'Š'),
            ('s', 'š'),
            ('Z', 'Ž'),
            ('z', 'ž'),
            ('E', 'Ě'),
            ('e', 'ě'),
            ('R', 'Ř'),
            ('r', 'ř'),
            ('N', 'Ň'),
            ('n', 'ň'),
        ] {
            m.insert(('\u{2c7}', letter), composed);
        }
        // Cedilla (¸ U+00B8)
        for (letter, composed) in [('C', 'Ç'), ('c', 'ç')] {
            m.insert(('\u{b8}', letter), composed);
        }
        m
    });

    static DIACRITICS: &[char] = &['\u{a8}', '\u{b4}', '`', '\u{2dc}', '\u{2c7}', '\u{b8}'];

    // Quick check: if no diacritics present, return as-is
    if !text.chars().any(|c| DIACRITICS.contains(&c)) {
        return text.to_string();
    }

    // Compose diacritic + optional space + letter
    static DIACRITIC_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"([\u{a8}\u{b4}`\u{2dc}\u{2c7}\u{b8}])\s*([A-Za-z])").unwrap());

    // Also handle letter + space + diacritic (e.g., "B ¨UNZ")
    static SPACE_BEFORE_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"([A-Za-z])\s+([\u{a8}\u{b4}`\u{2dc}\u{2c7}\u{b8}])").unwrap());

    // Step 1: Remove space between letter and following diacritic
    let text = SPACE_BEFORE_RE.replace_all(text, "$1$2");

    // Step 2: Compose diacritic + letter
    DIACRITIC_RE
        .replace_all(&text, |caps: &regex::Captures| {
            let diacritic = caps[1].chars().next().unwrap();
            let letter = caps[2].chars().next().unwrap();
            COMPOSITIONS
                .get(&(diacritic, letter))
                .map(|c| c.to_string())
                .unwrap_or_else(|| letter.to_string())
        })
        .to_string()
}

/// Fix hyphenation from PDF line breaks using dictionary lookup.
///
/// Simple algorithm:
/// 1. Find hyphenation patterns (e.g., "word- word" or "word-word")
/// 2. Check if merged word exists in dictionary
/// 3. If yes → merge (it's a broken word like "bidirec-tional" → "bidirectional")
/// 4. If no → keep hyphen (it's a compound like "human-centered")
///
/// # Example
///
/// ```
/// use hallucinator_parsing::text_processing::fix_hyphenation_with_dict;
/// use hallucinator_parsing::Dictionary;
///
/// struct MockDict;
/// impl Dictionary for MockDict {
///     fn contains(&self, word: &str) -> bool {
///         ["bidirectional", "membership", "byzantine"].contains(&word)
///     }
/// }
///
/// let dict = MockDict;
/// assert_eq!(fix_hyphenation_with_dict("bidirec- tional", &dict), "bidirectional");
/// assert_eq!(fix_hyphenation_with_dict("human- centered", &dict), "human-centered");
/// ```
pub fn fix_hyphenation_with_dict<D: Dictionary + ?Sized>(text: &str, dict: &D) -> String {
    // Pass 1: Fix "word- word" patterns (hyphen followed by whitespace).
    // These are clearly PDF line break artifacts.
    static RE_WITH_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w+)-\s+(\w+)").unwrap());

    let result = RE_WITH_SPACE
        .replace_all(text, |caps: &regex::Captures| {
            let before = &caps[1];
            let after = &caps[2];

            // If before ends with a digit, keep hyphen (e.g., "GPT-4-turbo")
            if before.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                return format!("{}-{}", before, after);
            }

            // Check if merged word is in dictionary
            let merged = format!("{}{}", before, after);
            if dict.contains(&merged.to_lowercase()) {
                merged
            } else {
                format!("{}-{}", before, after)
            }
        })
        .into_owned();

    // Pass 2: Fix "word-word" patterns (no space) using dictionary lookup.
    // This handles cases where the PDF removed the space (e.g., "Chal-lenges").
    // If the merged word is in the dictionary, it's a real word split by a
    // line break. If not, it's likely an intentional compound (e.g., "co-located").
    static RE_NO_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([a-zA-Z]+)-([a-zA-Z]+)").unwrap());

    RE_NO_SPACE
        .replace_all(&result, |caps: &regex::Captures| {
            let before = &caps[1];
            let after = &caps[2];

            // If before ends with a digit, keep hyphen (e.g., "GPT4-turbo")
            if before.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                return format!("{}-{}", before, after);
            }

            // Check if merged word is in dictionary
            let merged = format!("{}{}", before, after);
            if dict.contains(&merged.to_lowercase()) {
                merged
            } else {
                format!("{}-{}", before, after)
            }
        })
        .into_owned()
}

/// Fix hyphenation without a dictionary (uses heuristics).
///
/// This is the fallback when no dictionary is available. Uses suffix-based
/// heuristics to guess whether a hyphen is a PDF line break or a compound word.
pub fn fix_hyphenation(text: &str) -> String {
    fix_hyphenation_with_config(text, &ParsingConfig::default())
}

/// Syllable suffixes used by the heuristic-based hyphenation fixer.
/// Only used when no dictionary is available.
static SYLLABLE_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "tion",
        "tions",
        "tional",
        "sion",
        "sions",
        "sional",
        "ment",
        "ments",
        "ness",
        "ance",
        "ence",
        "ency",
        "ity",
        "able",
        "ible",
        "ous",
        "ious",
        "eous",
        "ive",
        "ical",
        "ally",
        "ular",
        "ology",
        "ization",
        "ised",
        "ized",
        "ing",
        "ings",
        "ism",
        "isms",
        "ist",
        "ists",
        "ure",
        "ures",
        "age",
        "ages",
        "fication",
        "ation",
        "ution",
        "ction",
        "ption",
        "ering",
        "uring",
        "ating",
        "mentation",
        "putation",
        "mization",
        "tication",
        "rization",
        "tation",
        "bilities",
        "ilities",
        "ming",
        "ning",
        "ring",
        "ping",
        "ting",
        "king",
        "alist",
        "ral",
        "lar",
        "nar",
        "ural",
        "eral",
        "ber",
        "der",
        "ter",
        "ger",
        "ver",
        "ner",
        "per",
        "fer",
        "ser",
        "cer",
        "ker",
        "mer",
        "tor",
        "sor",
        "mated",
        "nated",
        "rated",
        "lated",
        "cated",
        "gated",
        "tine",
        "dine",
        "rine",
        "line",
        "fier",
        "fiers",
        "ship",
        "ships",
        "hood",
        "hoods",
        "archy",
        "ences",
        "ances",
        "morphism",
        "antees",
        "tionships",
        "erware",
    ]
    .into_iter()
    .collect()
});

/// Config-aware hyphenation fixer (no dictionary, uses heuristics).
pub(crate) fn fix_hyphenation_with_config(text: &str, config: &ParsingConfig) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w+)-\s+(\w+)").unwrap());

    static RE_NO_SPACE: Lazy<Regex> = Lazy::new(|| {
        // Match common syllable suffixes without space
        Regex::new(r"(?i)([a-z])-(tion|tions|tional|sion|sions|sional|ment|ments|ness|ance|ence|ency|ity|ing|ings|ism|isms|ist|ists|able|ible|ure|ures|age|ages|ous|ive|ical|ally|ular|ology|ization|ised|ized|ation|ering|uring|ating|bilities|ilities|ral|lar|nar|ural|eral|ber|der|ter|ger|ver|ner|per|fer|ser|cer|ker|mer|tor|sor|mated|nated|rated|lated|cated|gated|tine|dine|rine|line|fier|fiers|ship|ships|hood|hoods|archy|ences|ances|morphism|antees|tionships|erware)([.\s,;:?!]|$)").unwrap()
    });

    let default_suffixes: Vec<String> = COMPOUND_SUFFIXES.iter().map(|s| s.to_string()).collect();
    let resolved = config.compound_suffixes.resolve(&default_suffixes);
    let suffix_set: HashSet<String> = resolved.into_iter().collect();

    let result = RE
        .replace_all(text, |caps: &regex::Captures| {
            let before_word = &caps[1];
            let after_word = &caps[2];
            let after_lower = after_word.to_lowercase();

            // If before ends with digit, keep hyphen
            if before_word
                .chars()
                .last()
                .is_some_and(|c| c.is_ascii_digit())
            {
                return format!("{}-{}", before_word, after_word);
            }

            // Check compound suffixes
            let stripped = after_lower.trim_end_matches(['.', ',', ';', ':']);
            if suffix_set.contains(stripped) {
                return format!("{}-{}", before_word, after_word);
            }

            // Check connector words (Over-The-Air, Plug-and-Play, etc.)
            static CONNECTORS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
                [
                    "the", "to", "of", "in", "on", "up", "out", "at", "by", "for", "and", "or",
                    "a", "an",
                ]
                .into_iter()
                .collect()
            });
            if CONNECTORS.contains(after_word.to_lowercase().as_str()) {
                return format!("{}-{}", before_word, after_word);
            }

            // Length heuristic: if both parts ≥4 letters and not a syllable suffix, keep hyphen
            let before_len = before_word.chars().filter(|c| c.is_alphabetic()).count();
            let after_len = after_word.chars().filter(|c| c.is_alphabetic()).count();

            if before_len >= 4 && after_len >= 4 {
                let is_syllable = SYLLABLE_SUFFIXES.contains(stripped)
                    || SYLLABLE_SUFFIXES.iter().any(|s| stripped.ends_with(s));
                if !is_syllable {
                    return format!("{}-{}", before_word, after_word);
                }
            }

            // Default: merge (syllable break)
            format!("{}{}", before_word, after_word)
        })
        .into_owned();

    // Second pass: no-space patterns
    RE_NO_SPACE.replace_all(&result, "$1$2$3").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_ligatures() {
        assert_eq!(expand_ligatures("ﬁnding ﬂow"), "finding flow");
        assert_eq!(expand_ligatures("eﬃcient oﬄine"), "efficient offline");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // DICTIONARY-BASED TESTS (primary approach)
    // ══════════════════════════════════════════════════════════════════════════

    struct MockDict {
        words: HashSet<String>,
    }

    impl MockDict {
        fn new(words: &[&str]) -> Self {
            Self {
                words: words.iter().map(|w| w.to_lowercase()).collect(),
            }
        }
    }

    impl Dictionary for MockDict {
        fn contains(&self, word: &str) -> bool {
            self.words.contains(&word.to_lowercase())
        }
    }

    #[test]
    fn test_dict_merges_valid_words() {
        let dict = MockDict::new(&[
            "bidirectional",
            "membership",
            "convolutional",
            "relationships",
            "observational",
            "conversational",
            "computational",
            "hierarchy",
            "neighbourhood",
            "international",
            "differences",
            "preferences",
            "functional",
            "considered",
            "stalkerware",
            "anthropomorphism",
            "censorship",
            "multidimensional",
            "guarantees",
            "byzantine",
            "identifier",
            "transformer",
            "automated",
            "detection",
        ]);

        // These should merge (words exist in dictionary)
        assert_eq!(
            fix_hyphenation_with_dict("bidirec- tional", &dict),
            "bidirectional"
        );
        assert_eq!(
            fix_hyphenation_with_dict("member- ship", &dict),
            "membership"
        );
        assert_eq!(
            fix_hyphenation_with_dict("convolu- tional", &dict),
            "convolutional"
        );
        assert_eq!(fix_hyphenation_with_dict("hier- archy", &dict), "hierarchy");
        assert_eq!(fix_hyphenation_with_dict("Byzan- tine", &dict), "Byzantine");
        assert_eq!(
            fix_hyphenation_with_dict("identi- fier", &dict),
            "identifier"
        );
        assert_eq!(
            fix_hyphenation_with_dict("trans- former", &dict),
            "transformer"
        );
        assert_eq!(fix_hyphenation_with_dict("auto- mated", &dict), "automated");
        assert_eq!(fix_hyphenation_with_dict("detec- tion", &dict), "detection");

        // No-space variants ARE NOW ALSO FIXED when merged word is in dictionary
        // This catches PDF artifacts where the space after hyphen was lost
        assert_eq!(
            fix_hyphenation_with_dict("bidirec-tional", &dict),
            "bidirectional"
        );
        assert_eq!(
            fix_hyphenation_with_dict("member-ship", &dict),
            "membership"
        );
    }

    #[test]
    fn test_dict_preserves_compounds() {
        let dict = MockDict::new(&["human", "centered", "data", "driven", "self", "supervised"]);

        // These should keep hyphen (merged word not in dictionary)
        assert_eq!(
            fix_hyphenation_with_dict("human- centered", &dict),
            "human-centered"
        );
        assert_eq!(
            fix_hyphenation_with_dict("data- driven", &dict),
            "data-driven"
        );
        assert_eq!(
            fix_hyphenation_with_dict("self- supervised", &dict),
            "self-supervised"
        );
    }

    #[test]
    fn test_dict_preserves_unknown() {
        let dict = MockDict::new(&["hello", "world"]);

        // Unknown words should keep hyphen (safe default)
        assert_eq!(fix_hyphenation_with_dict("foo- bar", &dict), "foo-bar");
        assert_eq!(
            fix_hyphenation_with_dict("xyzzy- plugh", &dict),
            "xyzzy-plugh"
        );
    }

    #[test]
    fn test_dict_preserves_digit_suffix() {
        let dict = MockDict::new(&["gpt4turbo"]);

        // Digit before hyphen should always keep hyphen
        assert_eq!(
            fix_hyphenation_with_dict("GPT4- turbo", &dict),
            "GPT4-turbo"
        );
        assert_eq!(fix_hyphenation_with_dict("Qwen2- VL", &dict), "Qwen2-VL");
    }

    #[test]
    fn test_dict_real_titles() {
        let dict = MockDict::new(&[
            "byzantine",
            "fault",
            "tolerance",
            "practical",
            "identifier",
            "network",
            "access",
            "automated",
            "vulnerability",
            "localization",
            "bidirectional",
            "transformers",
            "membership",
            "inference",
        ]);

        assert_eq!(
            fix_hyphenation_with_dict("Practical Byzan- tine fault tolerance", &dict),
            "Practical Byzantine fault tolerance"
        );

        assert_eq!(
            fix_hyphenation_with_dict("Member- ship inference attacks", &dict),
            "Membership inference attacks"
        );

        assert_eq!(
            fix_hyphenation_with_dict("Deep bidirec- tional transformers", &dict),
            "Deep bidirectional transformers"
        );
    }

    // ══════════════════════════════════════════════════════════════════════════
    // HEURISTIC-BASED TESTS (fallback when no dictionary)
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_heuristic_syllable_breaks() {
        assert_eq!(fix_hyphenation("detec- tion"), "detection");
        assert_eq!(fix_hyphenation("classi- fication"), "classification");
        assert_eq!(fix_hyphenation("detec-tion"), "detection");
    }

    #[test]
    fn test_heuristic_compound_words() {
        assert_eq!(fix_hyphenation("human- centered"), "human-centered");
        assert_eq!(fix_hyphenation("data- driven"), "data-driven");
        assert_eq!(fix_hyphenation("task- agnostic"), "task-agnostic");
    }

    #[test]
    fn test_heuristic_connectors() {
        assert_eq!(fix_hyphenation("Over-\nThe-Air"), "Over-The-Air");
        assert_eq!(fix_hyphenation("Up-\nTo-Date"), "Up-To-Date");
        // Lowercase connectors should also be preserved
        assert_eq!(fix_hyphenation("Plug-\nand-Play"), "Plug-and-Play");
    }

    #[test]
    fn test_heuristic_no_space() {
        // Note: heuristic-based approach only handles known suffixes.
        // For comprehensive coverage, use dictionary-based fix_hyphenation_with_dict.
        assert_eq!(fix_hyphenation("Implementa-tion"), "Implementation");
        assert_eq!(fix_hyphenation("bidirec-tional"), "bidirectional");
        assert_eq!(fix_hyphenation("member-ship"), "membership");
        // "lenges", "els", "tography" are not known suffixes - use dictionary for these
        assert_eq!(fix_hyphenation("Chal-lenges"), "Chal-lenges");
        assert_eq!(fix_hyphenation("cryp-tography"), "cryp-tography");
        assert_eq!(fix_hyphenation("Language Mod-els."), "Language Mod-els.");
    }

    #[test]
    fn test_heuristic_custom_suffix() {
        use crate::ParsingConfigBuilder;
        let config = ParsingConfigBuilder::new()
            .add_compound_suffix("powered".to_string())
            .build()
            .unwrap();

        assert_eq!(
            fix_hyphenation_with_config("AI- powered", &config),
            "AI-powered"
        );
        assert_eq!(
            fix_hyphenation_with_config("detec- tion", &config),
            "detection"
        );
    }

    #[test]
    fn test_dict_handles_edge_cases_heuristics_miss() {
        // Cases that heuristics miss but dictionary handles correctly
        let dict = MockDict::new(&["models", "language", "neural", "networks"]);

        // With space after hyphen: these ARE fixed
        assert_eq!(
            fix_hyphenation_with_dict("Language Mod- els.", &dict),
            "Language Models."
        );
        assert_eq!(
            fix_hyphenation_with_dict("Neu- ral net- works", &dict),
            "Neural networks"
        );

        // Without space: NOW ALSO FIXED when merged word is in dictionary
        assert_eq!(fix_hyphenation_with_dict("Mod-els", &dict), "Models");
        assert_eq!(fix_hyphenation_with_dict("Neu-ral", &dict), "Neural");
    }

    #[test]
    fn test_dict_no_space_preserves_compounds() {
        // Compound words should be preserved (merged form not in dictionary)
        let dict = MockDict::new(&["human", "centered", "data", "driven", "self", "attention"]);

        // These should keep hyphen because merged form is NOT in dictionary
        assert_eq!(
            fix_hyphenation_with_dict("human-centered", &dict),
            "human-centered"
        );
        assert_eq!(
            fix_hyphenation_with_dict("data-driven", &dict),
            "data-driven"
        );
        assert_eq!(
            fix_hyphenation_with_dict("self-attention", &dict),
            "self-attention"
        );
    }

    #[test]
    fn test_dict_no_space_merges_real_words() {
        // Real words split by PDF should be merged (merged form IS in dictionary)
        let dict = MockDict::new(&[
            "challenges",
            "cryptography",
            "photography",
            "methodology",
            "protocols",
        ]);

        assert_eq!(
            fix_hyphenation_with_dict("Chal-lenges", &dict),
            "Challenges"
        );
        assert_eq!(
            fix_hyphenation_with_dict("cryp-tography", &dict),
            "cryptography"
        );
        assert_eq!(
            fix_hyphenation_with_dict("pho-tography", &dict),
            "photography"
        );
        assert_eq!(
            fix_hyphenation_with_dict("method-ology", &dict),
            "methodology"
        );
        assert_eq!(fix_hyphenation_with_dict("proto-cols", &dict), "protocols");
        assert_eq!(
            fix_hyphenation_with_dict("Modern adventures with legacy proto-cols", &dict),
            "Modern adventures with legacy protocols"
        );
    }
}
