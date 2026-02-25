use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::config::ParsingConfig;

/// Common compound-word suffixes that should keep the hyphen.
pub(crate) static COMPOUND_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "centered",
        "based",
        "driven",
        "directed", // e.g., "coverage-directed"
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
        "throughput", // e.g., "high-throughput"
        "flow",       // e.g., "information-flow", "data-flow"
        // ML/AI compound suffixes (to catch short prefixes like "LLM-", "AI-")
        "assisted",   // e.g., "llm-assisted", "AI-assisted"
        "augmented",  // e.g., "retrieval-augmented"
        "integrated", // e.g., "LLM-integrated"
        "empowered",  // e.g., "LLM-empowered"
        "guided",     // e.g., "goal-guided"
        "supervised", // e.g., "self-supervised", "semi-supervised"
        "training",   // e.g., "pre-training"
        // Common short compound suffixes (< 4 letters but clearly compound parts)
        "key",        // e.g., "Fixed-Key", "Public-Key"
        "day",        // e.g., "zero-day"
        "box",        // e.g., "black-box"
        "end",        // e.g., "low-end", "high-end"
        // Crypto/security compound suffixes
        "party",      // e.g., "Two-Party", "Multi-Party"
        "round",      // e.g., "Reduced-Round"
        "size",       // e.g., "Constant-Size"
        "server",     // e.g., "Two-Server"
        "client",     // e.g., "Two-Client"
        "channel",    // e.g., "Side-channel"
        "optimal",    // e.g., "Round-Optimal"
        "resilient",  // e.g., "Quantum-Resilient"
        "resistant",  // e.g., "Collusion-Resistant"
        "tolerant",   // e.g., "Update-Tolerant"
        "hiding",     // e.g., "Attribute-Hiding"
        "preserving", // e.g., "Privacy-Preserving"
        "knowledge",  // e.g., "zero-knowledge"
        "latency",    // e.g., "Low-Latency"
        "precision",  // e.g., "High-Precision"
        "centric",    // e.g., "Hardware-Centric"
        "aided",      // e.g., "Server-Aided"
        "authenticated", // e.g., "Password-Authenticated"
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

/// Fix hyphenation from PDF line breaks while preserving compound words.
///
/// - `"detec- tion"` or `"detec-\ntion"` → `"detection"` (syllable break)
/// - `"human- centered"` → `"human-centered"` (compound word)
pub fn fix_hyphenation(text: &str) -> String {
    fix_hyphenation_with_config(text, &ParsingConfig::default())
}

/// Common syllable-break suffixes that indicate a word was split mid-syllable.
/// These should trigger merging even when both parts are ≥4 letters.
static SYLLABLE_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // Common word endings that are almost never standalone compound parts
        "tion", "tions", "sion", "sions", "ment", "ments", "ness", "ance", "ence",
        "ency", "ity", "able", "ible", "ous", "ious", "eous", "ive", "ical", "ally",
        "ular", "ology", "ization", "ised", "ized", "ises", "izes", "uous", "ling",
        "ward", "wards", "erly", "ween", "tween", "fore", "hind", "ntic", "mous",
        "uous", "cial", "tial", "cious", "tious", "gion", "ntic", "rupt", "duct",
        "struct", "tract", "gress", "plete", "clude", "sume", "duce", "fect",
        "ject", "rect", "lect", "nect", "tect", "dict", "flict", "strict",
        // Extended syllable patterns (longer suffixes from word breaks)
        "fication", "ification", "ation", "ution", "ction", "ption",
        "ering", "uring", "ating", "iting", "uting", "eting", "ling",
        "ness", "less", "ment", "ence", "ance", "ible", "able",
        "ture", "sure", "ture", "dure", "sure",
        "ical", "ular", "eous", "ious",
        // Additional patterns found in testing
        "mentation", "putation", "mization", "tication", "rization",
        "tation", "cation", "sation", "nation",
    ]
    .into_iter()
    .collect()
});

/// Config-aware version of [`fix_hyphenation`].
pub(crate) fn fix_hyphenation_with_config(text: &str, config: &ParsingConfig) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| {
        // Match: word chars, hyphen, whitespace (including newlines), then word chars
        // Changed to capture FULL word before hyphen for length-based heuristic
        Regex::new(r"(\w+)-\s+(\w+)").unwrap()
    });

    // Second pattern: handle hyphenation without space (PDF extraction artifact)
    // Only for common syllable-break suffixes that are never valid compound suffixes
    static RE_NO_SPACE: Lazy<Regex> = Lazy::new(|| {
        // Match: lowercase letter, hyphen (no space), then common syllable suffixes,
        // followed by punctuation, space, or end of string
        // NOTE: rust regex doesn't support look-ahead, so we capture the trailing char too
        Regex::new(r"(?i)([a-z])-(tion|tions|sion|sions|cient|cients|curity|rity|lity|nity|els|ness|ment|ments|ance|ence|ency|ity|ing|ings|ism|isms|ist|ists|ble|able|ible|ure|ures|age|ages|ous|ive|ical|ally|ular|ology|ization|ised|ized|ises|izes|uous|tifying|fying|lying|rying|nying|tying|ating|eting|iting|oting|uting)([.\s,;:?!]|$)").unwrap()
    });

    // Resolve compound suffixes: convert defaults to owned Strings for uniform handling
    let default_suffixes: Vec<String> = COMPOUND_SUFFIXES.iter().map(|s| s.to_string()).collect();
    let resolved = config.compound_suffixes.resolve(&default_suffixes);
    let suffix_set: HashSet<String> = resolved.into_iter().collect();

    let result = RE
        .replace_all(text, |caps: &regex::Captures| {
            let before_word = &caps[1];
            let after_word = &caps[2];
            let after_lower = after_word.to_lowercase();

            // If the word before ends with a digit, keep the hyphen
            // (product/model names like "Qwen2-VL", "GPT-4-turbo")
            if before_word.chars().last().is_some_and(|c| c.is_ascii_digit()) {
                return format!("{}-{}", before_word, after_word);
            }

            // Check if the word after the hyphen is a common compound suffix
            for suffix in suffix_set.iter() {
                if after_lower == *suffix
                    || after_lower.starts_with(&format!("{} ", suffix))
                    || after_lower.starts_with(&format!("{},", suffix))
                {
                    return format!("{}-{}", before_word, after_word);
                }
            }

            // Check if the full word (stripped of trailing punctuation) matches a suffix
            let stripped = after_lower.trim_end_matches(['.', ',', ';', ':']);
            if suffix_set.contains(stripped) {
                return format!("{}-{}", before_word, after_word);
            }

            // If the word after the hyphen is a small connector word starting with uppercase,
            // it's likely a compound proper noun (e.g., "Over-The-Air", "Up-To-Date").
            static HYPHEN_CONNECTORS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
                [
                    "The", "To", "Of", "In", "On", "Up", "Out", "At", "By", "For", "And", "Or",
                    "A", "An",
                ]
                .into_iter()
                .collect()
            });
            if HYPHEN_CONNECTORS.contains(after_word) {
                return format!("{}-{}", before_word, after_word);
            }

            // HEURISTIC: If both parts are ≥4 letters and the second part is NOT a
            // common syllable suffix, it's likely a compound word — keep the hyphen.
            // This catches academic terms like "retrieval-augmented", "two-party", etc.
            let before_alpha_len = before_word.chars().filter(|c| c.is_alphabetic()).count();
            let after_alpha_len = after_word.chars().filter(|c| c.is_alphabetic()).count();

            if before_alpha_len >= 4 && after_alpha_len >= 4 {
                // Check if after_word looks like a syllable suffix (would indicate merge)
                if !SYLLABLE_SUFFIXES.contains(stripped) {
                    return format!("{}-{}", before_word, after_word);
                }
            }

            // Otherwise, it's likely a syllable break — remove hyphen
            format!("{}{}", before_word, after_word)
        })
        .into_owned();

    // Second pass: fix hyphenation without space (e.g., "Mod-els" -> "Models")
    // This handles PDF extraction artifacts where the newline/space was lost
    RE_NO_SPACE.replace_all(&result, "$1$2$3").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_ligatures() {
        assert_eq!(expand_ligatures("ﬁnding ﬂow"), "finding flow");
        assert_eq!(expand_ligatures("eﬃcient oﬄine"), "efficient offline");
        assert_eq!(expand_ligatures("no ligatures here"), "no ligatures here");
    }

    #[test]
    fn test_fix_hyphenation_syllable_break() {
        assert_eq!(fix_hyphenation("detec- tion"), "detection");
        assert_eq!(fix_hyphenation("detec-\ntion"), "detection");
        assert_eq!(fix_hyphenation("classi- fication"), "classification");
    }

    #[test]
    fn test_fix_hyphenation_compound_word() {
        assert_eq!(fix_hyphenation("human- centered"), "human-centered");
        assert_eq!(fix_hyphenation("data- driven"), "data-driven");
        assert_eq!(fix_hyphenation("task- agnostic"), "task-agnostic");
        assert_eq!(fix_hyphenation("fine- grained"), "fine-grained");
        // New suffixes: directed, throughput, flow
        assert_eq!(fix_hyphenation("coverage- directed"), "coverage-directed");
        assert_eq!(fix_hyphenation("high- throughput"), "high-throughput");
        assert_eq!(fix_hyphenation("information-\nflow"), "information-flow");
        assert_eq!(fix_hyphenation("data- flow"), "data-flow");
    }

    #[test]
    fn test_fix_hyphenation_with_trailing_punct() {
        assert_eq!(fix_hyphenation("context- aware,"), "context-aware,");
        assert_eq!(fix_hyphenation("domain- specific."), "domain-specific.");
    }

    #[test]
    fn test_fix_hyphenation_mixed() {
        let input = "We use a human- centered approach for detec- tion of data- driven models.";
        let expected = "We use a human-centered approach for detection of data-driven models.";
        assert_eq!(fix_hyphenation(input), expected);
    }

    // ── Config-aware tests ──

    #[test]
    fn test_fix_hyphenation_custom_suffix() {
        use crate::ParsingConfigBuilder;
        let config = ParsingConfigBuilder::new()
            .add_compound_suffix("powered".to_string())
            .build()
            .unwrap();
        // "AI- powered" should keep hyphen with custom suffix
        assert_eq!(
            fix_hyphenation_with_config("AI- powered", &config),
            "AI-powered"
        );
        // Default behavior still works
        assert_eq!(
            fix_hyphenation_with_config("human- centered", &config),
            "human-centered"
        );
        // Syllable break still works
        assert_eq!(
            fix_hyphenation_with_config("detec- tion", &config),
            "detection"
        );
    }

    #[test]
    fn test_fix_hyphenation_replace_suffixes() {
        use crate::ParsingConfigBuilder;
        // Replace ALL suffixes — only "powered" is a compound suffix now
        let config = ParsingConfigBuilder::new()
            .set_compound_suffixes(vec!["powered".to_string()])
            .build()
            .unwrap();
        // "AI- powered" keeps hyphen
        assert_eq!(
            fix_hyphenation_with_config("AI- powered", &config),
            "AI-powered"
        );
        // "human- centered" still keeps hyphen due to length heuristic:
        // Both parts ≥4 letters and "centered" is not a syllable suffix.
        // The heuristic acts as a safety net even when custom suffixes are set.
        assert_eq!(
            fix_hyphenation_with_config("human- centered", &config),
            "human-centered"
        );
        // But syllable breaks still merge: "detec- tion" → "detection"
        assert_eq!(
            fix_hyphenation_with_config("detec- tion", &config),
            "detection"
        );
    }

    #[test]
    fn test_fix_hyphenation_titlecase_compound() {
        // Titlecase words after hyphen indicate compound proper nouns, not syllable breaks
        assert_eq!(fix_hyphenation("Over-\nThe-Air"), "Over-The-Air");
        assert_eq!(fix_hyphenation("Up-\nTo-Date"), "Up-To-Date");
        assert_eq!(fix_hyphenation("Out-\nOf-Band"), "Out-Of-Band");
        // But lowercase is still treated as syllable break
        assert_eq!(fix_hyphenation("detec-\ntion"), "detection");
        assert_eq!(fix_hyphenation("classi-\nfication"), "classification");
    }

    #[test]
    fn test_fix_hyphenation_capitalized_compounds() {
        // Capitalized compound words should keep their hyphens
        // (common in academic paper titles)
        assert_eq!(fix_hyphenation("Base- Bridge"), "Base-Bridge");
        assert_eq!(fix_hyphenation("Base-\nBridge"), "Base-Bridge");
        assert_eq!(fix_hyphenation("Smart- Phone"), "Smart-Phone");
        assert_eq!(fix_hyphenation("Fixed- Key"), "Fixed-Key");
        assert_eq!(fix_hyphenation("Two- Party"), "Two-Party");
    }

    #[test]
    fn test_fix_hyphenation_no_space() {
        // PDF extraction artifact: hyphen kept but space/newline lost
        // "Mod-els" should become "Models" (syllable break suffix)
        assert_eq!(fix_hyphenation("Language Mod-els."), "Language Models.");
        assert_eq!(fix_hyphenation("Implementa-tion"), "Implementation");
        assert_eq!(fix_hyphenation("classifica-tion and"), "classification and");
        assert_eq!(fix_hyphenation("cluster-ing."), "clustering.");
        // Additional suffixes: -cient, -curity
        assert_eq!(fix_hyphenation("effi-cient"), "efficient");
        assert_eq!(fix_hyphenation("se-curity"), "security");
        // But keep valid compound words
        assert_eq!(fix_hyphenation("data-driven"), "data-driven");
        assert_eq!(fix_hyphenation("task-agnostic"), "task-agnostic");
    }

    // ══════════════════════════════════════════════════════════════════════════
    // LENGTH HEURISTIC TESTS: compound words caught by the ≥4 letter heuristic
    // These are compound words found in academic papers that don't have explicit
    // suffixes in COMPOUND_SUFFIXES but should still preserve hyphens.
    // ══════════════════════════════════════════════════════════════════════════

    #[test]
    fn test_fix_hyphenation_length_heuristic_crypto() {
        // Cryptography compound words (high frequency in ground truth)
        assert_eq!(fix_hyphenation("Fixed- Key"), "Fixed-Key");
        assert_eq!(fix_hyphenation("Two- Party"), "Two-Party");
        assert_eq!(fix_hyphenation("Reduced- Round"), "Reduced-Round");
        assert_eq!(fix_hyphenation("Round- Optimal"), "Round-Optimal");
        assert_eq!(fix_hyphenation("Quantum- Resilient"), "Quantum-Resilient");
        assert_eq!(fix_hyphenation("Collusion- Resistant"), "Collusion-Resistant");
        assert_eq!(fix_hyphenation("Chosen- Ciphertext"), "Chosen-Ciphertext");
        assert_eq!(fix_hyphenation("Primal- Dual"), "Primal-Dual");
    }

    #[test]
    fn test_fix_hyphenation_length_heuristic_ml() {
        // Machine learning compound words
        assert_eq!(fix_hyphenation("retrieval- augmented"), "retrieval-augmented");
        assert_eq!(fix_hyphenation("llm- assisted"), "llm-assisted");
        assert_eq!(fix_hyphenation("self- supervised"), "self-supervised");
        assert_eq!(fix_hyphenation("semi- supervised"), "semi-supervised");
        assert_eq!(fix_hyphenation("Goal- guided"), "Goal-guided");
        assert_eq!(fix_hyphenation("LLM- integrated"), "LLM-integrated");
        assert_eq!(fix_hyphenation("LLM- empowered"), "LLM-empowered");
    }

    #[test]
    fn test_fix_hyphenation_length_heuristic_security() {
        // Security compound words
        assert_eq!(fix_hyphenation("Update- Tolerant"), "Update-Tolerant");
        assert_eq!(fix_hyphenation("Attribute- Hiding"), "Attribute-Hiding");
        assert_eq!(fix_hyphenation("Privacy- Preserving"), "Privacy-Preserving");
        assert_eq!(fix_hyphenation("Side- channel"), "Side-channel");
        assert_eq!(fix_hyphenation("zero- knowledge"), "zero-knowledge");
        assert_eq!(fix_hyphenation("zero- day"), "zero-day");
        assert_eq!(fix_hyphenation("Control- Flow"), "Control-Flow"); // uppercase
    }

    #[test]
    fn test_fix_hyphenation_length_heuristic_general() {
        // General academic compound words
        assert_eq!(fix_hyphenation("Constant- Size"), "Constant-Size");
        assert_eq!(fix_hyphenation("Dual- Space"), "Dual-Space");
        assert_eq!(fix_hyphenation("Two- Server"), "Two-Server");
        assert_eq!(fix_hyphenation("Low- Latency"), "Low-Latency");
        assert_eq!(fix_hyphenation("Server- Aided"), "Server-Aided");
        assert_eq!(fix_hyphenation("Password- Authenticated"), "Password-Authenticated");
        assert_eq!(fix_hyphenation("Hardware- Centric"), "Hardware-Centric");
        assert_eq!(fix_hyphenation("High- Precision"), "High-Precision");
    }

    #[test]
    fn test_fix_hyphenation_length_heuristic_eponyms() {
        // Eponymous compound words (names)
        assert_eq!(fix_hyphenation("Rivest- Shamir"), "Rivest-Shamir");
        assert_eq!(fix_hyphenation("Even- Mansour"), "Even-Mansour");
        assert_eq!(fix_hyphenation("Reed- Solomon"), "Reed-Solomon");
        assert_eq!(fix_hyphenation("Merkle- Damgaard"), "Merkle-Damgaard");
        assert_eq!(fix_hyphenation("Luby- Rackoff"), "Luby-Rackoff");
    }

    #[test]
    fn test_fix_hyphenation_syllable_breaks_with_long_words() {
        // These SHOULD merge even though both parts are ≥4 letters,
        // because the second part is a known syllable suffix
        assert_eq!(fix_hyphenation("classi- fication"), "classification");
        assert_eq!(fix_hyphenation("imple- mentation"), "implementation");
        assert_eq!(fix_hyphenation("compu- tation"), "computation");
        assert_eq!(fix_hyphenation("opti- mization"), "optimization");
        assert_eq!(fix_hyphenation("authen- tication"), "authentication");
        assert_eq!(fix_hyphenation("veri- fication"), "verification");
    }

    #[test]
    fn test_fix_hyphenation_mixed_real_titles() {
        // Real academic paper titles with mixed hyphenation
        let input = "A Two- Party Protocol for Privacy- Preserving Classi- fication";
        let expected = "A Two-Party Protocol for Privacy-Preserving Classification";
        assert_eq!(fix_hyphenation(input), expected);

        let input2 = "Retrieval- Augmented Generation for Zero- Shot Learning";
        let expected2 = "Retrieval-Augmented Generation for Zero-Shot Learning";
        assert_eq!(fix_hyphenation(input2), expected2);

        let input3 = "LLM- Assisted Self- Supervised Pre- training";
        let expected3 = "LLM-Assisted Self-Supervised Pre-training";
        assert_eq!(fix_hyphenation(input3), expected3);
    }

    #[test]
    fn test_fix_hyphenation_short_words_still_merge() {
        // Short words (< 4 letters) should still merge (not caught by heuristic)
        assert_eq!(fix_hyphenation("pre- fix"), "prefix");
        assert_eq!(fix_hyphenation("sub- set"), "subset");
        assert_eq!(fix_hyphenation("re- set"), "reset");
        // But if explicitly in COMPOUND_SUFFIXES, keep hyphen
        assert_eq!(fix_hyphenation("real- time"), "real-time"); // "time" is in suffixes
        assert_eq!(fix_hyphenation("zero- shot"), "zero-shot"); // "shot" is in suffixes
    }
}
