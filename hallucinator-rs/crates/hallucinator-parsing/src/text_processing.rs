use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::config::ParsingConfig;
use crate::dictionary::Dictionary;

/// Common compound-word suffixes that should keep the hyphen.
/// Used only when no dictionary is available.
pub(crate) static COMPOUND_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "centered", "based", "driven", "directed", "aware", "oriented", "specific",
        "related", "dependent", "independent", "like", "free", "friendly", "rich",
        "poor", "scale", "level", "order", "class", "type", "style", "wise", "fold",
        "shot", "step", "time", "world", "source", "domain", "task", "modal",
        "intensive", "efficient", "agnostic", "invariant", "sensitive", "grained",
        "agent", "site", "throughput", "flow", "assisted", "augmented", "integrated",
        "empowered", "guided", "supervised", "training", "key", "day", "box", "end",
        "party", "round", "size", "server", "client", "channel", "optimal", "resilient",
        "resistant", "tolerant", "hiding", "preserving", "knowledge", "latency",
        "precision", "centric", "aided", "authenticated",
    ]
    .into_iter()
    .collect()
});

/// Expand common typographic ligatures found in PDFs.
pub fn expand_ligatures(text: &str) -> String {
    text.replace('\u{FB00}', "ff")
        .replace('\u{FB01}', "fi")
        .replace('\u{FB02}', "fl")
        .replace('\u{FB03}', "ffi")
        .replace('\u{FB04}', "ffl")
        .replace(['\u{FB05}', '\u{FB06}'], "st")
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
    // Only fix "word- word" patterns (hyphen followed by whitespace).
    // These are clearly PDF line break artifacts.
    //
    // We do NOT fix "word-word" patterns (no space) because these are likely
    // intentional hyphenated compounds (e.g., "co-located", "self-attention").
    // Both "colocated" and "co-located" are valid spellings, and we should
    // preserve the author's choice.
    static RE_WITH_SPACE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(\w+)-\s+(\w+)").unwrap()
    });

    RE_WITH_SPACE
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
        "tion", "tions", "tional", "sion", "sions", "sional", "ment", "ments",
        "ness", "ance", "ence", "ency", "ity", "able", "ible", "ous", "ious",
        "eous", "ive", "ical", "ally", "ular", "ology", "ization", "ised", "ized",
        "ing", "ings", "ism", "isms", "ist", "ists", "ure", "ures", "age", "ages",
        "fication", "ation", "ution", "ction", "ption", "ering", "uring", "ating",
        "mentation", "putation", "mization", "tication", "rization", "tation",
        "bilities", "ilities", "ming", "ning", "ring", "ping", "ting", "king",
        "alist", "ral", "lar", "nar", "ural", "eral", "ber", "der", "ter", "ger",
        "ver", "ner", "per", "fer", "ser", "cer", "ker", "mer", "tor", "sor",
        "mated", "nated", "rated", "lated", "cated", "gated", "tine", "dine",
        "rine", "line", "fier", "fiers", "ship", "ships", "hood", "hoods",
        "archy", "ences", "ances", "morphism", "antees", "tionships", "erware",
    ]
    .into_iter()
    .collect()
});

/// Config-aware hyphenation fixer (no dictionary, uses heuristics).
pub(crate) fn fix_hyphenation_with_config(text: &str, config: &ParsingConfig) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(\w+)-\s+(\w+)").unwrap()
    });

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
            if before_word.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                return format!("{}-{}", before_word, after_word);
            }

            // Check compound suffixes
            let stripped = after_lower.trim_end_matches(['.', ',', ';', ':']);
            if suffix_set.contains(stripped) {
                return format!("{}-{}", before_word, after_word);
            }

            // Check connector words (Over-The-Air, etc.)
            static CONNECTORS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
                ["The", "To", "Of", "In", "On", "Up", "Out", "At", "By", "For", "And", "Or", "A", "An"]
                    .into_iter().collect()
            });
            if CONNECTORS.contains(after_word) {
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
            "bidirectional", "membership", "convolutional", "relationships",
            "observational", "conversational", "computational", "hierarchy",
            "neighbourhood", "international", "differences", "preferences",
            "functional", "considered", "stalkerware", "anthropomorphism",
            "censorship", "multidimensional", "guarantees", "byzantine",
            "identifier", "transformer", "automated", "detection",
        ]);

        // These should merge (words exist in dictionary)
        assert_eq!(fix_hyphenation_with_dict("bidirec- tional", &dict), "bidirectional");
        assert_eq!(fix_hyphenation_with_dict("member- ship", &dict), "membership");
        assert_eq!(fix_hyphenation_with_dict("convolu- tional", &dict), "convolutional");
        assert_eq!(fix_hyphenation_with_dict("hier- archy", &dict), "hierarchy");
        assert_eq!(fix_hyphenation_with_dict("Byzan- tine", &dict), "Byzantine");
        assert_eq!(fix_hyphenation_with_dict("identi- fier", &dict), "identifier");
        assert_eq!(fix_hyphenation_with_dict("trans- former", &dict), "transformer");
        assert_eq!(fix_hyphenation_with_dict("auto- mated", &dict), "automated");
        assert_eq!(fix_hyphenation_with_dict("detec- tion", &dict), "detection");

        // No-space variants are NOT fixed (could be intentional hyphenation)
        // "bidirec-tional" without space is preserved as-is
        assert_eq!(fix_hyphenation_with_dict("bidirec-tional", &dict), "bidirec-tional");
        assert_eq!(fix_hyphenation_with_dict("member-ship", &dict), "member-ship");
    }

    #[test]
    fn test_dict_preserves_compounds() {
        let dict = MockDict::new(&["human", "centered", "data", "driven", "self", "supervised"]);

        // These should keep hyphen (merged word not in dictionary)
        assert_eq!(fix_hyphenation_with_dict("human- centered", &dict), "human-centered");
        assert_eq!(fix_hyphenation_with_dict("data- driven", &dict), "data-driven");
        assert_eq!(fix_hyphenation_with_dict("self- supervised", &dict), "self-supervised");
    }

    #[test]
    fn test_dict_preserves_unknown() {
        let dict = MockDict::new(&["hello", "world"]);

        // Unknown words should keep hyphen (safe default)
        assert_eq!(fix_hyphenation_with_dict("foo- bar", &dict), "foo-bar");
        assert_eq!(fix_hyphenation_with_dict("xyzzy- plugh", &dict), "xyzzy-plugh");
    }

    #[test]
    fn test_dict_preserves_digit_suffix() {
        let dict = MockDict::new(&["gpt4turbo"]);

        // Digit before hyphen should always keep hyphen
        assert_eq!(fix_hyphenation_with_dict("GPT4- turbo", &dict), "GPT4-turbo");
        assert_eq!(fix_hyphenation_with_dict("Qwen2- VL", &dict), "Qwen2-VL");
    }

    #[test]
    fn test_dict_real_titles() {
        let dict = MockDict::new(&[
            "byzantine", "fault", "tolerance", "practical", "identifier",
            "network", "access", "automated", "vulnerability", "localization",
            "bidirectional", "transformers", "membership", "inference",
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
    }

    #[test]
    fn test_heuristic_no_space() {
        // Note: heuristic-based approach only handles known suffixes.
        // For comprehensive coverage, use dictionary-based fix_hyphenation_with_dict.
        assert_eq!(fix_hyphenation("Implementa-tion"), "Implementation");
        assert_eq!(fix_hyphenation("bidirec-tional"), "bidirectional");
        assert_eq!(fix_hyphenation("member-ship"), "membership");
        // "els" is not a known suffix, so heuristics keep the hyphen
        // (dictionary-based approach would handle this correctly)
        assert_eq!(fix_hyphenation("Language Mod-els."), "Language Mod-els.");
    }

    #[test]
    fn test_heuristic_custom_suffix() {
        use crate::ParsingConfigBuilder;
        let config = ParsingConfigBuilder::new()
            .add_compound_suffix("powered".to_string())
            .build()
            .unwrap();

        assert_eq!(fix_hyphenation_with_config("AI- powered", &config), "AI-powered");
        assert_eq!(fix_hyphenation_with_config("detec- tion", &config), "detection");
    }

    #[test]
    fn test_dict_handles_edge_cases_heuristics_miss() {
        // Cases that heuristics miss but dictionary handles correctly
        // NOTE: Dictionary-based approach only fixes "word- word" patterns (with space)
        // to preserve intentional hyphenation like "co-located"
        let dict = MockDict::new(&["models", "language", "neural", "networks"]);

        // With space after hyphen: these ARE fixed
        assert_eq!(fix_hyphenation_with_dict("Language Mod- els.", &dict), "Language Models.");
        assert_eq!(fix_hyphenation_with_dict("Neu- ral net- works", &dict), "Neural networks");

        // Without space: these are preserved (could be intentional hyphenation)
        assert_eq!(fix_hyphenation_with_dict("Mod-els", &dict), "Mod-els");
        assert_eq!(fix_hyphenation_with_dict("Neu-ral", &dict), "Neu-ral");
    }
}
