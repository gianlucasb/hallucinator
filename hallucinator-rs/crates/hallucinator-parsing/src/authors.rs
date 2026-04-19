use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::ParsingConfig;

/// Special sentinel value indicating the reference uses em-dashes to
/// indicate "same authors as previous entry."
pub const SAME_AS_PREVIOUS: &str = "__SAME_AS_PREVIOUS__";

/// Extract author names from a reference string.
///
/// Handles multiple formats:
/// - IEEE: `J. Smith, A. Jones, and C. Williams, "Title..."`
/// - ACM: `FirstName LastName, FirstName LastName, and FirstName LastName. Year.`
/// - AAAI: `Surname, I.; Surname, I.; and Surname, I.`
/// - ALL CAPS: `SURNAME, I., SURNAME, I., AND SURNAME, I. Title...`
/// - USENIX: `FirstName LastName and FirstName LastName. Title...`
/// - Springer/Nature: `Surname I, Surname I (Year) Title...`
///
/// Returns a list of author names, or `["__SAME_AS_PREVIOUS__"]` if the
/// reference uses em-dashes.
pub fn extract_authors_from_reference(ref_text: &str) -> Vec<String> {
    extract_authors_from_reference_with_config(ref_text, &ParsingConfig::default())
}

/// Config-aware version of [`extract_authors_from_reference`].
pub(crate) fn extract_authors_from_reference_with_config(
    ref_text: &str,
    config: &ParsingConfig,
) -> Vec<String> {
    // Strip leading reference-number markers before any other processing.
    // Without this, the digits inside `[57]` / `57.` trip the "skip parts
    // containing digits" heuristic below, causing the first author to be
    // dropped on IEEE-numbered references where the number prefix survived
    // segmentation (e.g. `[57] Petar Maymounkov and David Mazieres. …`
    // extracted only "David Mazieres"). The main pipeline also strips this
    // prefix later for display, but that happens *after* author extraction.
    static REF_NUM_PREFIX: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\s*(?:\[\d+\]|\d{1,3}\.)\s*").unwrap());
    let ref_text = REF_NUM_PREFIX.replace(ref_text, "");

    // Fix hyphenation from PDF line breaks in author names.
    // Only fix "word- word" patterns (with space after hyphen) — these are clearly
    // line break artifacts. We do NOT use the no-space heuristic here because it
    // can incorrectly break legitimate hyphenated names (e.g., "Agha-Janyan").
    static HYPHEN_BREAK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\w)-\s+(\w)").unwrap());
    let ref_text = HYPHEN_BREAK_RE.replace_all(&ref_text, "$1$2");

    // Normalize whitespace
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    let ref_text = WS_RE.replace_all(&ref_text, " ");
    let ref_text = ref_text.trim();

    // Fix "word{and}" patterns where a space was lost between a name and "and"
    // e.g., "E. Dasand J. W. Burdick" → "E. Das and J. W. Burdick"
    static MERGED_AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"([a-z])and ([A-Z])").unwrap());
    let ref_text = MERGED_AND_RE.replace_all(ref_text, "$1 and $2");
    let ref_text = ref_text.as_ref();

    // Check for em-dash pattern meaning "same authors as previous"
    static EM_DASH_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[\u{2014}\u{2013}\-]{2,}\s*,").unwrap());
    if EM_DASH_RE.is_match(ref_text) {
        return vec![SAME_AS_PREVIOUS.to_string()];
    }

    // Determine where authors section ends based on format

    // IEEE format: authors end at quoted title
    static QUOTE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"["\u{201c}\u{201d}]"#).unwrap());
    let quote_match = QUOTE_RE.find(ref_text);

    // Springer/Nature format: authors end before "(Year)" pattern
    static SPRINGER_YEAR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\s+\((\d{4}[a-z]?)\)\s+").unwrap());
    let springer_year_match = SPRINGER_YEAR_RE.find(ref_text);

    // ACM format: authors end before ". Year." pattern
    static ACM_YEAR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\.\s*((?:19|20)\d{2})\.\s*").unwrap());
    let acm_year_match = ACM_YEAR_RE.find(ref_text);

    // USENIX/default: find first "real" period (not after initials)
    let first_period = find_first_real_period(ref_text);

    // Determine author section end
    let author_end = if let Some(qm) = quote_match {
        qm.start()
    } else if let Some(sm) = springer_year_match {
        sm.start()
    } else if let Some(am) = acm_year_match {
        am.start() + 1 // Include the period
    } else if let Some(fp) = first_period {
        fp
    } else {
        ref_text.len()
    };

    let author_section = ref_text[..author_end].trim();

    // Remove trailing punctuation
    static TRAIL_PUNCT: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.,;:]+$").unwrap());
    let author_section = TRAIL_PUNCT.replace(author_section, "");
    let author_section = author_section.trim();

    if author_section.is_empty() {
        return vec![];
    }

    // Check for ALL CAPS format: LASTNAME, I., LASTNAME, I., AND LASTNAME, I.
    // Must match pattern like "BACKES, M." (all-caps surname, comma, space, single uppercase initial, period)
    static ALL_CAPS_CHECK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[A-Z]{2,},\s+[A-Z]\.,").unwrap());
    let authors = if ALL_CAPS_CHECK.is_match(author_section) {
        parse_all_caps_authors_with_max(author_section, config.max_authors)
    } else if author_section.contains("; ")
        && Regex::new(r"[A-Z][A-Za-z]+,\s+[A-Z]\.").unwrap().is_match(author_section)
    // AAAI format (semicolon-separated): Surname, I.; Surname, I.
    // Also handles ALL CAPS variant: SURNAME, I.; SURNAME, I.
    {
        parse_aaai_authors_with_max(author_section, config.max_authors)
    } else {
        parse_general_authors_with_max(author_section, config.max_authors)
    };

    // Post-process: normalise each author name.
    authors
        .into_iter()
        .map(|a| repair_line_break_hyphen(&a))
        .collect()
}

/// Repair PDF line-break hyphenation inside an individual author name.
///
/// A hyphen connecting a capitalised prefix to a lowercase suffix
/// (`Hol-lick`, `Guil-laume`, `Man-galvedhe`) is almost always a line-break
/// artefact: legitimate hyphenated surnames capitalise both parts
/// (`Agha-Janyan`, `Jean-Pierre`, `Martin-Löf`), as do Arabic-article
/// compounds (`Al-Fakhri`). We only merge when the continuation starts
/// lowercase, so true hyphenated names are preserved.
///
/// Applied iteratively to catch cascaded breaks such as `Man-galve-dhe`.
pub(crate) fn repair_line_break_hyphen(name: &str) -> String {
    static BROKEN: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"([A-Za-z])-([a-z])").unwrap());

    let mut out = name.to_string();
    loop {
        let next = BROKEN.replace_all(&out, "$1$2").to_string();
        if next == out {
            break;
        }
        out = next;
    }
    out
}

/// Find the first "real" period — one that's not after an author initial like "M." or "J."
/// and not after a name suffix like "Jr." or "Sr."
fn find_first_real_period(text: &str) -> Option<usize> {
    static PERIOD_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s").unwrap());
    static SUFFIX_WORDS: Lazy<std::collections::HashSet<&'static str>> =
        Lazy::new(|| ["Jr", "Sr"].into_iter().collect());

    for m in PERIOD_SPACE.find_iter(text) {
        let pos = m.start();
        if pos == 0 {
            continue;
        }
        let char_before = text.as_bytes()[pos - 1];
        if char_before.is_ascii_uppercase()
            && (pos == 1 || !text.as_bytes()[pos - 2].is_ascii_alphabetic())
        {
            // This is likely an initial — skip
            continue;
        }

        // Check for name suffixes like "Jr." or "Sr."
        let mut word_start = pos;
        while word_start > 0 && text.as_bytes()[word_start - 1].is_ascii_alphabetic() {
            word_start -= 1;
        }
        let word_before = &text[word_start..pos];
        if SUFFIX_WORDS.contains(word_before) {
            continue;
        }

        return Some(pos);
    }
    None
}

fn parse_aaai_authors_with_max(section: &str, max_authors: usize) -> Vec<String> {
    // Replace "; and " with "; "
    static AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i);\s+and\s+").unwrap());
    let section = AND_RE.replace_all(section, "; ");

    let mut authors = Vec::new();
    for part in section.split(';') {
        let part = part.trim();
        if part.len() > 2 && part.chars().any(|c| c.is_uppercase()) {
            authors.push(part.to_string());
        }
    }

    authors.truncate(max_authors);
    authors
}

/// Parse ALL CAPS author format: LASTNAME, I., LASTNAME, I., AND LASTNAME, I.
///
/// This format is common in some European conferences and journals.
/// Example: "BACKES, M., RIECK, K., SKORUPPA, M., STOCK, B., AND YAMAGUCHI, F."
fn parse_all_caps_authors_with_max(section: &str, max_authors: usize) -> Vec<String> {
    // Replace ", AND " with ", " (case insensitive)
    static AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i),\s+AND\s+").unwrap());
    let section = AND_RE.replace_all(section, ", ");

    // Pattern to match "LASTNAME, I." where LASTNAME is all caps and I is a single uppercase letter
    static AUTHOR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"([A-Z][A-Z]+),\s+([A-Z]\.)").unwrap());

    let mut authors = Vec::new();
    for caps in AUTHOR_RE.captures_iter(&section) {
        let surname = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let initial = caps.get(2).map(|m| m.as_str()).unwrap_or("");
        if !surname.is_empty() && !initial.is_empty() {
            // Format as "Surname, I." (title case surname)
            let surname_title = surname
                .chars()
                .enumerate()
                .map(|(i, c)| if i == 0 { c } else { c.to_ascii_lowercase() })
                .collect::<String>();
            authors.push(format!("{}, {}", surname_title, initial));
        }
        if authors.len() >= max_authors {
            break;
        }
    }

    authors
}

fn parse_general_authors_with_max(section: &str, max_authors: usize) -> Vec<String> {
    // Normalize "and" and "&"
    static AND_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i),?\s+and\s+").unwrap());
    let section = AND_RE.replace_all(section, ", ");

    static AMP_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*&\s*").unwrap());
    let section = AMP_RE.replace_all(&section, ", ");

    // Remove "et al." but keep the authors before it
    static ET_AL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i),?\s*et\s+al\.?").unwrap());
    let section = ET_AL_RE.replace_all(&section, "");

    // If after removing et al. the section is empty or just whitespace,
    // the original had only "et al." - nothing to extract
    if section.trim().is_empty() {
        return vec![];
    }

    let mut authors = Vec::new();

    for part in section.split(',') {
        let part = part.trim();
        if part.len() < 2 {
            continue;
        }

        // Skip if contains numbers (probably not an author)
        if part.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }

        // Skip if too many words
        let words: Vec<&str> = part.split_whitespace().collect();
        if words.len() > 5 {
            continue;
        }

        // Skip if it looks like a sentence/title (lowercase words that aren't prepositions)
        static NAME_PREPOSITIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
            ["and", "de", "van", "von", "la", "del", "di", "le", "jr", "sr"]
                .into_iter()
                .collect()
        });

        let lowercase_words: Vec<&&str> = words
            .iter()
            .filter(|w| {
                w.chars().next().is_some_and(|c| c.is_lowercase())
                    && !NAME_PREPOSITIONS.contains(w.to_lowercase().as_str())
            })
            .collect();

        if lowercase_words.len() > 1 {
            continue;
        }

        // Check if it looks like a name:
        // - Has both upper and lower case (normal names), OR
        // - Is all uppercase with 2+ letters (ALL CAPS format like "SMITH")
        let has_upper = part.chars().any(|c| c.is_uppercase());
        let has_lower = part.chars().any(|c| c.is_lowercase());
        let is_all_caps =
            has_upper && !has_lower && part.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2;

        // Skip venue/journal names that got into the author section
        static VENUE_WORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
            [
                "journal",
                "transactions",
                "proceedings",
                "conference",
                "workshop",
                "symposium",
                "review",
                "society",
                "association",
                "networks",
                "computing",
                "intelligence",
                "engineering",
                "software",
                "systems",
                "science",
                "research",
                "letters",
                "advances",
                "foundations",
                "international",
                "quarterly",
                "annual",
                "bulletin",
            ]
            .into_iter()
            .collect()
        });
        let lower_words: Vec<&str> = part
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()))
            .collect();
        if lower_words.len() >= 2
            && lower_words
                .iter()
                .any(|w| VENUE_WORDS.contains(w.to_lowercase().as_str()))
        {
            continue;
        }

        if part.len() >= 2 && (has_upper && has_lower || is_all_caps) {
            authors.push(part.to_string());
        }
    }

    authors.truncate(max_authors);
    authors
}

use std::collections::HashSet;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ieee_format() {
        let ref_text =
            r#"J. Smith, A. Jones, and C. Williams, "Detecting Fake References," in IEEE, 2023."#;
        let authors = extract_authors_from_reference(ref_text);
        assert!(!authors.is_empty());
    }

    #[test]
    fn test_em_dash() {
        let ref_text = "\u{2014}\u{2014}\u{2014}, Another paper title, 2023.";
        let authors = extract_authors_from_reference(ref_text);
        assert_eq!(authors, vec![SAME_AS_PREVIOUS]);
    }

    #[test]
    fn test_aaai_format() {
        let ref_text = "Smith, J.; Jones, A.; and Williams, C. 2023. Title here.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(authors.len() >= 2);
    }

    #[test]
    fn test_springer_format() {
        let ref_text = "Smith J, Jones A (2023) A novel approach to detection.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(!authors.is_empty());
    }

    #[test]
    fn test_empty() {
        assert!(extract_authors_from_reference("").is_empty());
    }

    #[test]
    fn test_acm_format() {
        let ref_text = "John Smith and Alice Jones. 2022. Title of paper. In Proceedings.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(!authors.is_empty());
    }

    #[test]
    fn test_all_caps_format() {
        // Issue #161: ALL CAPS author names were not being parsed
        let ref_text = "BACKES, M., RIECK, K., SKORUPPA, M., STOCK, B., AND YAMAGUCHI, F. Efficient and flexible discovery of php application vulnerabilities. In 2017 IEEE european symposium on security and privacy (EuroS&P) (2017), IEEE, pp. 334–349.";
        let authors = extract_authors_from_reference(ref_text);
        assert_eq!(authors.len(), 5, "Expected 5 authors, got: {:?}", authors);
        assert_eq!(authors[0], "Backes, M.");
        assert_eq!(authors[1], "Rieck, K.");
        assert_eq!(authors[2], "Skoruppa, M.");
        assert_eq!(authors[3], "Stock, B.");
        assert_eq!(authors[4], "Yamaguchi, F.");
    }

    #[test]
    fn test_all_caps_format_short() {
        let ref_text = "SMITH, J., AND JONES, A. Title here.";
        let authors = extract_authors_from_reference(ref_text);
        assert_eq!(authors.len(), 2);
        assert_eq!(authors[0], "Smith, J.");
        assert_eq!(authors[1], "Jones, A.");
    }

    #[test]
    fn test_all_caps_with_semicolons() {
        // ALL CAPS with AAAI-style semicolons
        let ref_text = "SMITH, J.; JONES, A.; AND WILLIAMS, C. 2023. Title.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.len() >= 2,
            "Expected at least 2 authors, got: {:?}",
            authors
        );
    }

    #[test]
    fn test_et_al_preserves_listed_authors() {
        // "et al." should be removed but authors before it should be kept
        let ref_text = r#"Wu, J., et al.: A survey on llm-generated text detection. arXiv preprint arXiv:2310.14724 (2023)"#;
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            !authors.is_empty(),
            "Should extract authors before et al.: {:?}",
            authors
        );
    }

    #[test]
    fn test_venue_not_parsed_as_author() {
        // Venue/journal names should not be parsed as author names
        let ref_text = "Badis, L., Amad, M., A\u{00EF}ssani, D., Abbar, S.: P2PCF. Journal of High Speed Networks, 25(1), 2019.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            !authors
                .iter()
                .any(|a| a.contains("Journal") || a.contains("Networks")),
            "Journal name should not be in authors: {:?}",
            authors
        );
    }

    #[test]
    fn test_merged_and_fixed() {
        // "Dasand" should be split to "Das and" when followed by uppercase
        let ref_text =
            r#"E. Dasand J. W. Burdick, "Robust control barrier functions," in IEEE, 2023."#;
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Das")),
            "Should split 'Dasand' into 'Das' and next author: {:?}",
            authors
        );
        assert!(
            authors.iter().any(|a| a.contains("Burdick")),
            "Should extract Burdick: {:?}",
            authors
        );
    }

    #[test]
    fn test_hyphenated_author_name_fixed() {
        // PDF line break in author name should be fixed
        let ref_text = r#"Hans, A., Schwarzschild, A., Cherepanova, V., Kazemi, H., Saha, A., Goldblum, M., Geiping, J., Goldstein, T.: Spotting LLMs with binoc- ulars: Zero-shot detection of machine-generated text. In: ICML (2024)"#;
        let authors = extract_authors_from_reference(ref_text);
        // The hyphenation in "binoc- ulars" is in the title, not authors
        // but checking that author parsing still works correctly
        assert!(
            authors.iter().any(|a| a.contains("Hans")),
            "Should extract authors: {:?}",
            authors
        );
    }

    #[test]
    fn test_name_with_jr_suffix() {
        // "Jr." should not be treated as a sentence boundary
        let ref_text =
            "Jamar L. Sullivan Jr. and Alice Jones. A Novel Method for Detection. In Proceedings.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Sullivan")),
            "Should extract Sullivan as author: {:?}",
            authors,
        );
    }

    #[test]
    fn test_name_with_sr_suffix() {
        let ref_text = "Robert K. Williams Sr. and Jane Doe. Some Paper Title. In Proceedings.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Williams")),
            "Should extract Williams as author: {:?}",
            authors,
        );
    }

    #[test]
    fn test_name_with_le_particle() {
        // "Le" is a surname particle, not a title word
        let ref_text = "Christopher A. Le Dantec and Alice Jones. Community Informatics Design. In Proceedings.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors
                .iter()
                .any(|a| a.contains("Le Dantec") || a.contains("Dantec")),
            "Should extract Le Dantec as author: {:?}",
            authors,
        );
    }

    // ─── Fix C: IEEE [NN] prefix must not hide the first author ───

    #[test]
    fn test_ieee_num_prefix_preserves_first_author() {
        // Regression test for the USENIX 2025 bug: when the reference text
        // still carries its `[NN]` numeric marker, digits inside `[57]` tripped
        // the "skip parts containing digits" heuristic and the first author
        // silently vanished (e.g. "Petar Maymounkov" lost, leaving only
        // "David Mazieres" for the Kademlia citation).
        let ref_text = "[57] Petar Maymounkov and David Mazieres. Kademlia: A Peer-to-Peer Information System Based on the XOR Metric. In International Workshop on Peer-to-Peer Systems, 2002.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Maymounkov")),
            "first author Petar Maymounkov must be preserved, got {:?}",
            authors
        );
        assert!(authors.iter().any(|a| a.contains("Mazieres")));
    }

    #[test]
    fn test_numeric_dot_prefix_preserves_first_author() {
        // "23. Author A, Author B. Title." — numbered-list marker variant
        let ref_text =
            "23. Yuval Marcus, Ethan Heilman, and Sharon Goldberg. Low-Resource Eclipse Attacks.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Marcus")),
            "first author Yuval Marcus must be preserved, got {:?}",
            authors
        );
    }

    // ─── Fix B: line-break hyphen in surnames ───

    #[test]
    fn test_hyphen_lowercase_surname_is_merged() {
        // "Hol-lick" (PDF line-break inside a surname) must be rejoined.
        let ref_text = "Alexander Heinrich, Leon Würsching, and Matthias Hol-lick. Please Unstalk Me. In PoPETS, 2024.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Hollick")),
            "Hol-lick must be rejoined to Hollick, got {:?}",
            authors
        );
        assert!(
            !authors.iter().any(|a| a.contains("Hol-lick")),
            "Hol-lick must not survive, got {:?}",
            authors
        );
    }

    #[test]
    fn test_legitimate_hyphenated_surname_is_preserved() {
        // Both sides capitalised → real hyphenated surname, do NOT merge.
        let ref_text =
            "Aboozar Agha-Janyan and Jean-Pierre Schmitz. A Study of Names. In Proc., 2021.";
        let authors = extract_authors_from_reference(ref_text);
        assert!(
            authors.iter().any(|a| a.contains("Agha-Janyan")),
            "Agha-Janyan must be preserved, got {:?}",
            authors
        );
        assert!(
            authors.iter().any(|a| a.contains("Jean-Pierre")),
            "Jean-Pierre must be preserved, got {:?}",
            authors
        );
    }

    #[test]
    fn test_repair_line_break_hyphen_unit() {
        assert_eq!(repair_line_break_hyphen("Hol-lick"), "Hollick");
        assert_eq!(repair_line_break_hyphen("Man-galvedhe"), "Mangalvedhe");
        // Cascaded: Man-galve-dhe → Mangalvedhe
        assert_eq!(repair_line_break_hyphen("Man-galve-dhe"), "Mangalvedhe");
        // Both capitalised → preserved
        assert_eq!(repair_line_break_hyphen("Agha-Janyan"), "Agha-Janyan");
        assert_eq!(repair_line_break_hyphen("Jean-Pierre"), "Jean-Pierre");
        assert_eq!(repair_line_break_hyphen("Martin-Löf"), "Martin-Löf");
        // No-op on names without hyphens
        assert_eq!(repair_line_break_hyphen("Goldreich"), "Goldreich");
    }
}
