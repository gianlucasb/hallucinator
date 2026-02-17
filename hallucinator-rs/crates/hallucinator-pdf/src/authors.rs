use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::PdfParsingConfig;

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
    extract_authors_from_reference_with_config(ref_text, &PdfParsingConfig::default())
}

/// Config-aware version of [`extract_authors_from_reference`].
pub(crate) fn extract_authors_from_reference_with_config(
    ref_text: &str,
    config: &PdfParsingConfig,
) -> Vec<String> {
    // Normalize whitespace
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    let ref_text = WS_RE.replace_all(ref_text, " ");
    let ref_text = ref_text.trim();

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
    if ALL_CAPS_CHECK.is_match(author_section) {
        return parse_all_caps_authors_with_max(author_section, config.max_authors);
    }

    // Check for AAAI format (semicolon-separated): Surname, I.; Surname, I.
    // Also handles ALL CAPS variant: SURNAME, I.; SURNAME, I.
    static AAAI_CHECK: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"[A-Z][A-Za-z]+,\s+[A-Z]\.").unwrap());
    if author_section.contains("; ") && AAAI_CHECK.is_match(author_section) {
        return parse_aaai_authors_with_max(author_section, config.max_authors);
    }

    // General parsing
    parse_general_authors_with_max(author_section, config.max_authors)
}

/// Find the first "real" period — one that's not after an author initial like "M." or "J."
fn find_first_real_period(text: &str) -> Option<usize> {
    static PERIOD_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s").unwrap());

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

    // Remove "et al."
    static ET_AL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i),?\s*et\s+al\.?").unwrap());
    let section = ET_AL_RE.replace_all(&section, "");

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
            ["and", "de", "van", "von", "la", "del", "di"]
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
        let is_all_caps = has_upper
            && !has_lower
            && part.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2;

        if part.len() > 2 && (has_upper && has_lower || is_all_caps) {
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
        assert!(authors.len() >= 2, "Expected at least 2 authors, got: {:?}", authors);
    }
}
