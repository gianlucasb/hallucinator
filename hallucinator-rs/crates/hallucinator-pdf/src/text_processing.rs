use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::config::PdfParsingConfig;

/// Common compound-word suffixes that should keep the hyphen.
pub(crate) static COMPOUND_SUFFIXES: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "centered",
        "based",
        "driven",
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
    fix_hyphenation_with_config(text, &PdfParsingConfig::default())
}

/// Config-aware version of [`fix_hyphenation`].
pub(crate) fn fix_hyphenation_with_config(text: &str, config: &PdfParsingConfig) -> String {
    static RE: Lazy<Regex> = Lazy::new(|| {
        // Match: word-char, hyphen, whitespace (including newlines), then word chars
        Regex::new(r"(\w)-\s+(\w)(\w*)").unwrap()
    });

    // Resolve compound suffixes: convert defaults to owned Strings for uniform handling
    let default_suffixes: Vec<String> = COMPOUND_SUFFIXES.iter().map(|s| s.to_string()).collect();
    let resolved = config.compound_suffixes.resolve(&default_suffixes);
    let suffix_set: HashSet<String> = resolved.into_iter().collect();

    RE.replace_all(text, |caps: &regex::Captures| {
        let before = &caps[1];
        let after_char = &caps[2];
        let after_rest = &caps[3];

        let after_word = format!("{}{}", after_char, after_rest);
        let after_lower = after_word.to_lowercase();

        // If the character before the hyphen is a digit, keep the hyphen
        // (product/model names like "Qwen2-VL", "GPT-4-turbo")
        let before_chars: Vec<char> = before.chars().collect();
        if before_chars.last().is_some_and(|c| c.is_ascii_digit()) {
            return format!("{}-{}", before, after_word);
        }

        // Check if the word after the hyphen is a common compound suffix
        for suffix in suffix_set.iter() {
            if after_lower == *suffix
                || after_lower.starts_with(&format!("{} ", suffix))
                || after_lower.starts_with(&format!("{},", suffix))
            {
                return format!("{}-{}", before, after_word);
            }
        }

        // Check if the full word (stripped of trailing punctuation) matches a suffix
        let stripped = after_lower.trim_end_matches(['.', ',', ';', ':']);
        if suffix_set.contains(stripped) {
            return format!("{}-{}", before, after_word);
        }

        // If the word after the hyphen starts with uppercase, it's likely a compound
        // proper noun (e.g., "Over-The-Air", "Up-To-Date"), not a syllable break.
        // Syllable breaks like "detec-\ntion" have lowercase continuation.
        let after_is_titlecase = after_char
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if after_is_titlecase {
            return format!("{}-{}", before, after_word);
        }

        // Otherwise, it's likely a syllable break — remove hyphen
        format!("{}{}", before, after_word)
    })
    .into_owned()
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
        use crate::PdfParsingConfigBuilder;
        let config = PdfParsingConfigBuilder::new()
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
        use crate::PdfParsingConfigBuilder;
        // Replace ALL suffixes — only "powered" is a compound suffix now
        let config = PdfParsingConfigBuilder::new()
            .set_compound_suffixes(vec!["powered".to_string()])
            .build()
            .unwrap();
        // "AI- powered" keeps hyphen
        assert_eq!(
            fix_hyphenation_with_config("AI- powered", &config),
            "AI-powered"
        );
        // "human- centered" is NO LONGER a compound suffix (defaults replaced)
        assert_eq!(
            fix_hyphenation_with_config("human- centered", &config),
            "humancentered"
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
}
