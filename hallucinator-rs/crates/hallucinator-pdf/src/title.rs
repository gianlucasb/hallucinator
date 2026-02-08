use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::text_processing::fix_hyphenation;

/// Abbreviations that should NEVER be sentence boundaries (mid-title abbreviations).
static MID_SENTENCE_ABBREVIATIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    ["vs", "eg", "ie", "cf", "fig", "figs", "eq", "eqs", "sec", "ch", "pt", "no"]
        .into_iter()
        .collect()
});

/// Extract title from a reference string.
///
/// Handles multiple formats:
/// - IEEE/USENIX: Authors, "Title," in Venue, Year
/// - LNCS/Springer: Authors, I.: Title. In: Venue
/// - ACM: Authors. Year. Title. In Venue
/// - Springer/Nature/Harvard: Authors (Year) Title. Journal
/// - AAAI: Authors. Title. Venue
/// - Journal style: Authors. Title. Journal Name, Vol(Issue)
/// - ALL CAPS authors: SURNAME, F. Title here.
///
/// Returns `(title, from_quotes)` where `from_quotes` indicates if the title was in quotes.
pub fn extract_title_from_reference(ref_text: &str) -> (String, bool) {
    // Normalize whitespace and fix hyphenation
    let ref_text = fix_hyphenation(ref_text);
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    let ref_text = WS_RE.replace_all(&ref_text, " ");
    let ref_text = ref_text.trim();

    // === Format 1: IEEE/USENIX - Quoted titles ===
    if let Some(result) = try_quoted_title(ref_text) {
        return result;
    }

    // === Format 1b: LNCS/Springer - "Authors, I.: Title. In: Venue" ===
    if let Some(result) = try_lncs(ref_text) {
        return result;
    }

    // === Format 1c: Organization/Documentation - "Organization: Title (Year)" ===
    if let Some(result) = try_org_doc(ref_text) {
        return result;
    }

    // === Format 2a: Springer/Nature/Harvard - "Authors (Year) Title" ===
    if let Some(result) = try_springer_year(ref_text) {
        return result;
    }

    // === Format 2b: ACM - "Authors. Year. Title. In Venue" ===
    if let Some(result) = try_acm_year(ref_text) {
        return result;
    }

    // === Format 3: USENIX/ICML/NeurIPS/Elsevier - "Authors. Title. In Venue" ===
    if let Some(result) = try_venue_marker(ref_text) {
        return result;
    }

    // === Format 4: Journal style ===
    if let Some(result) = try_journal(ref_text) {
        return result;
    }

    // === Format 4b: Elsevier journal ===
    if let Some(result) = try_elsevier_journal(ref_text) {
        return result;
    }

    // === Format 5: ALL CAPS authors ===
    if let Some(result) = try_all_caps_authors(ref_text) {
        return result;
    }

    // === Fallback: second sentence ===
    if let Some(result) = try_fallback_sentence(ref_text) {
        return result;
    }

    (String::new(), false)
}

/// Clean extracted title by removing trailing venue/metadata.
pub fn clean_title(title: &str, from_quotes: bool) -> String {
    if title.is_empty() {
        return String::new();
    }

    let mut title = fix_hyphenation(title);

    // For non-quoted titles, truncate at first sentence-ending period
    if !from_quotes {
        title = truncate_at_sentence_end(&title);
    }

    // Handle "? In" and "? In:" patterns
    static IN_VENUE_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\?\s*[Ii]n:?\s+(?:[A-Z]|[12]\d{3}\s)").unwrap());
    if let Some(m) = IN_VENUE_RE.find(&title) {
        // Keep the question mark
        let qmark_pos = title[..m.end()].rfind('?').unwrap();
        title = title[..=qmark_pos].to_string();
    }

    // Apply cutoff patterns to remove trailing venue/metadata
    title = apply_cutoff_patterns(&title);

    title = title.trim().to_string();
    static TRAILING_PUNCT: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.,;:]+$").unwrap());
    title = TRAILING_PUNCT.replace(&title, "").to_string();

    title.trim().to_string()
}

// ───────────────── Format-specific extractors ─────────────────

fn try_quoted_title(ref_text: &str) -> Option<(String, bool)> {
    static QUOTE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            // Smart quotes (any combo of \u201c, \u201d, \u201c)
            Regex::new(r#"[\u{201c}\u{201d}"]([^\u{201c}\u{201d}"]+)[\u{201c}\u{201d}"]"#)
                .unwrap(),
            // Regular quotes
            Regex::new(r#""([^"]+)""#).unwrap(),
        ]
    });

    for re in QUOTE_PATTERNS.iter() {
        if let Some(caps) = re.captures(ref_text) {
            let quoted_part = caps.get(1).unwrap().as_str().trim();
            let after_quote = ref_text[caps.get(0).unwrap().end()..].trim();

            // IEEE: comma inside quotes means title is complete
            if quoted_part.ends_with(',') {
                if quoted_part.split_whitespace().count() >= 3 {
                    return Some((quoted_part.to_string(), true));
                }
                continue;
            }

            // Check for subtitle after the quote
            if !after_quote.is_empty() {
                let subtitle_text = if after_quote.starts_with(':') || after_quote.starts_with('-')
                {
                    Some(after_quote[1..].trim())
                } else if after_quote
                    .chars()
                    .next()
                    .map_or(false, |c| c.is_uppercase())
                {
                    Some(after_quote)
                } else {
                    None
                };

                if let Some(sub) = subtitle_text {
                    let subtitle_end = find_subtitle_end(sub);
                    let subtitle = sub[..subtitle_end].trim();
                    static TRAIL: Lazy<Regex> =
                        Lazy::new(|| Regex::new(r"[.,;:]+$").unwrap());
                    let subtitle = TRAIL.replace(subtitle, "");
                    if subtitle.split_whitespace().count() >= 2 {
                        return Some((format!("{}: {}", quoted_part, subtitle), true));
                    }
                }
            }

            // No subtitle — use quoted part if long enough
            if quoted_part.split_whitespace().count() >= 3 {
                return Some((quoted_part.to_string(), true));
            }
        }
    }
    None
}

fn find_subtitle_end(text: &str) -> usize {
    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s*[Ii]n\s+").unwrap(),
            Regex::new(r"\.\s*(?:Proc|IEEE|ACM|USENIX|NDSS|CCS|AAAI|WWW|CHI|arXiv)").unwrap(),
            Regex::new(r",\s*[Ii]n\s+").unwrap(),
            Regex::new(r"\.\s*\((?:19|20)\d{2}\)").unwrap(),
            Regex::new(r"[,.]\s*(?:19|20)\d{2}").unwrap(),
            Regex::new(r"\s+(?:19|20)\d{2}\.").unwrap(),
            Regex::new(r"[.,]\s+[A-Z][a-z]+\s+\d+[,\s]").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z]+(?:\s+(?:in|of|on|and|for|the|a|an|&|[A-Za-z]+))+,\s*\d+\s*[,:]").unwrap(),
        ]
    });

    let mut end = text.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(text) {
            end = end.min(m.start());
        }
    }
    end
}

fn try_lncs(ref_text: &str) -> Option<(String, bool)> {
    // Pattern: comma/space + Initial(s) + colon, then title
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[,\s][A-Z]\.(?:[-\u{2013}][A-Z]\.)?\s*:\s*(.+)").unwrap()
    });

    let caps = RE.captures(ref_text)?;
    let after_colon = caps.get(1).unwrap().as_str().trim();

    let title_end = find_title_end_lncs(after_colon);
    let title = after_colon[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    if title.split_whitespace().count() >= 3 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn find_title_end_lncs(text: &str) -> usize {
    static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s*[Ii]n:\s+").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|IEEE|ACM|USENIX|NDSS|arXiv)").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s]+(?:Access|Journal|Review|Transactions)").unwrap(),
            Regex::new(r"\.\s*https?://").unwrap(),
            Regex::new(r"\.\s*pp?\.?\s*\d+").unwrap(),
            Regex::new(r"\s+\((?:19|20)\d{2}\)\s*[,.]?\s*(?:https?://|$)").unwrap(),
            Regex::new(r"\s+\((?:19|20)\d{2}\)\s*,").unwrap(),
        ]
    });

    let mut end = text.len();
    for re in PATTERNS.iter() {
        if let Some(m) = re.find(text) {
            end = end.min(m.start());
        }
    }
    end
}

fn try_org_doc(ref_text: &str) -> Option<(String, bool)> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^([A-Z][a-zA-Z\s]+):\s*(.+)").unwrap());

    let caps = RE.captures(ref_text)?;
    let after_colon = caps.get(2).unwrap().as_str().trim();

    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\s+\((?:19|20)\d{2}\)\s*[,.]?\s*(?:https?://|$)").unwrap(),
            Regex::new(r"\s+\((?:19|20)\d{2}\)\s*,").unwrap(),
            Regex::new(r"\.\s*https?://").unwrap(),
        ]
    });

    let mut title_end = after_colon.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(after_colon) {
            title_end = title_end.min(m.start());
        }
    }

    let title = after_colon[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    if title.split_whitespace().count() >= 2 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn try_springer_year(ref_text: &str) -> Option<(String, bool)> {
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\((\d{4}[a-z]?)\)\.?\s+").unwrap());

    let caps = RE.captures(ref_text)?;
    let after_year = &ref_text[caps.get(0).unwrap().end()..];

    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s*[Ii]n:\s+").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s]+\d+\s*\(\d+\)").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s&]+\d+:\d+").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s&-]+,\s*\d+").unwrap(),
            Regex::new(r"\.\s*https?://").unwrap(),
            Regex::new(r"\.\s*URL\s+").unwrap(),
            Regex::new(r"\.\s*Tech\.\s*rep\.").unwrap(),
            Regex::new(r"\.\s*pp?\.?\s*\d+").unwrap(),
        ]
    });

    let mut title_end = after_year.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(after_year) {
            title_end = title_end.min(m.start());
        }
    }

    let title = after_year[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    if title.split_whitespace().count() >= 3 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn try_acm_year(ref_text: &str) -> Option<(String, bool)> {
    // ". YYYY. Title" — require \s+ after year to avoid matching DOIs
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\.\s*((?:19|20)\d{2})\.\s+").unwrap());

    let caps = RE.captures(ref_text)?;
    let after_year = &ref_text[caps.get(0).unwrap().end()..];

    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)").unwrap(),
            Regex::new(r"\s+doi:").unwrap(),
        ]
    });

    let mut title_end = after_year.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(after_year) {
            title_end = title_end.min(m.start());
        }
    }

    let title = after_year[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    if title.split_whitespace().count() >= 3 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn try_venue_marker(ref_text: &str) -> Option<(String, bool)> {
    static VENUE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s*[Ii]n:\s+(?:Proceedings|Workshop|Conference|Symposium|IFIP|IEEE|ACM)").unwrap(),
            Regex::new(r"\.\s*[Ii]n:\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+(?:Proceedings|Workshop|Conference|Symposium|AAAI|IEEE|ACM|USENIX)").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+[A-Z][a-z]+\s+(?:Conference|Workshop|Symposium)").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+(?:The\s+)?(?:\w+\s+)+(?:International\s+)?(?:Conference|Workshop|Symposium)").unwrap(),
            Regex::new(r"\.\s*(?:NeurIPS|ICML|ICLR|CVPR|ICCV|ECCV|AAAI|IJCAI|CoRR|JMLR),").unwrap(),
            Regex::new(r"\.\s*arXiv\s+preprint").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r",\s*(?:19|20)\d{2}\.\s*(?:URL|$)").unwrap(),
            Regex::new(r",\s*(?:19|20)\d{2}\.\s*$").unwrap(),
        ]
    });

    for vp in VENUE_PATTERNS.iter() {
        if let Some(venue_match) = vp.find(ref_text) {
            let before_venue = ref_text[..venue_match.start()].trim();

            // First try: split into sentences
            let parts = split_sentences_skip_initials(before_venue);
            if parts.len() >= 2 {
                let title = parts[1].trim();
                static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
                let title = TRAIL.replace(title, "");
                if title.split_whitespace().count() >= 3 {
                    // Verify it doesn't look like authors
                    static AUTHOR_CHECK: Lazy<Regex> =
                        Lazy::new(|| Regex::new(r"^[A-Z][a-z]+\s+[A-Z][a-z]+,").unwrap());
                    if !AUTHOR_CHECK.is_match(&title) {
                        return Some((title.to_string(), false));
                    }
                }
            }

            // Second try: look for author initial pattern followed by title
            static AUTHOR_END: Lazy<Regex> = Lazy::new(|| {
                Regex::new(r"(?:,\s+[A-Z]\.(?:[-\s]+[A-Z]\.)*|(?:Jr|Sr|III|II|IV)\.)\s+(.)")
                    .unwrap()
            });
            let all_matches: Vec<_> = AUTHOR_END.captures_iter(before_venue).collect();

            for caps in all_matches.iter().rev() {
                let title_start_match = caps.get(1).unwrap();
                let remaining = &before_venue[title_start_match.start()..];

                // Skip if looks like another author
                static AUTHOR_LIKE: Lazy<Regex> =
                    Lazy::new(|| Regex::new(r"^[A-Z]\.,|^[A-Z][a-z]+,").unwrap());
                if AUTHOR_LIKE.is_match(remaining) {
                    continue;
                }

                let title = remaining.trim();
                static TRAIL2: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
                let title = TRAIL2.replace(title, "");
                if title.split_whitespace().count() >= 3 {
                    static AUTHOR_CHECK2: Lazy<Regex> =
                        Lazy::new(|| Regex::new(r"^[A-Z][a-z]+,\s+[A-Z]\.").unwrap());
                    if !AUTHOR_CHECK2.is_match(&title) {
                        return Some((title.to_string(), false));
                    }
                }
                break;
            }

            break; // Only try the first matching venue pattern
        }
    }
    None
}

fn try_journal(ref_text: &str) -> Option<(String, bool)> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\.\s*([A-Z][^.]+(?:Journal|Review|Transactions|Letters|Magazine|Science|Nature|Processing|Advances)[^.]*),\s*(?:vol\.|Volume|\d+\(|\d+,)").unwrap()
    });

    let m = RE.find(ref_text)?;
    let before_journal = ref_text[..m.start()].trim();
    let parts = split_sentences_skip_initials(before_journal);
    if parts.len() >= 2 {
        let title = parts[1].trim();
        if title.split_whitespace().count() >= 3 {
            return Some((title.to_string(), false));
        }
    }
    None
}

fn try_elsevier_journal(ref_text: &str) -> Option<(String, bool)> {
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"\.\s*([A-Z][A-Za-z\s]+)\s+(?:19|20)\d{2};\d+(?:\(\d+\))?").unwrap()
    });

    let m = RE.find(ref_text)?;
    let before_journal = ref_text[..m.start()].trim();
    let parts = split_sentences_skip_initials(before_journal);
    if parts.len() >= 2 {
        let title = parts.last().unwrap().trim();
        static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
        let title = TRAIL.replace(title, "");
        if title.split_whitespace().count() >= 3 {
            return Some((title.to_string(), false));
        }
    }
    None
}

fn try_all_caps_authors(ref_text: &str) -> Option<(String, bool)> {
    static STARTS_CAPS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Z]{2,}").unwrap());
    if !STARTS_CAPS.is_match(ref_text) {
        return None;
    }

    // Find title start: period + space + Capital followed by lowercase
    static TITLE_START: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\.\s+([A-Z][a-z]*\s+[a-z])").unwrap());
    let caps = TITLE_START.captures(ref_text)?;
    let title_text = &ref_text[caps.get(1).unwrap().start()..];

    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|IEEE|ACM|USENIX|NDSS|arXiv|Technical\s+report)")
                .unwrap(),
            Regex::new(r"\.\s*[A-Z][a-z]+\s+\d+,\s*\d+\s*\(").unwrap(),
            Regex::new(r"\.\s*(?:Ph\.?D\.?\s+thesis|Master.s\s+thesis)").unwrap(),
        ]
    });

    let mut title_end = title_text.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(title_text) {
            title_end = title_end.min(m.start());
        }
    }

    if title_end > 0 {
        let title = title_text[..title_end].trim();
        static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
        let title = TRAIL.replace(title, "");
        if title.split_whitespace().count() >= 3 {
            return Some((title.to_string(), false));
        }
    }
    None
}

fn try_fallback_sentence(ref_text: &str) -> Option<(String, bool)> {
    let sentences = split_sentences_skip_initials(ref_text);
    if sentences.len() < 2 {
        return None;
    }

    let mut potential_title = sentences[1].trim().to_string();

    // Check if it looks like authors (high ratio of capitalized words + "and")
    let words: Vec<&str> = potential_title.split_whitespace().collect();
    if !words.is_empty() {
        static CAP_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Z][a-z]+$").unwrap());
        let cap_words = words.iter().filter(|w| CAP_WORD.is_match(w)).count();
        let and_count = words
            .iter()
            .filter(|w| w.to_lowercase() == "and")
            .count();

        if (cap_words as f64 / words.len() as f64) > 0.7 && and_count > 0 {
            // Try third sentence
            if sentences.len() >= 3 {
                potential_title = sentences[2].trim().to_string();
            }
        }
    }

    // Skip if starts with "In " (venue)
    static IN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[Ii]n\s+").unwrap());
    if IN_RE.is_match(&potential_title) {
        return None;
    }

    if potential_title.split_whitespace().count() >= 3 {
        Some((potential_title, false))
    } else {
        None
    }
}

// ───────────────── Sentence splitting ─────────────────

/// Split text into sentences, but skip periods that are author initials
/// (e.g., "M." "J.") or mid-sentence abbreviations (e.g., "vs.").
fn split_sentences_skip_initials(text: &str) -> Vec<String> {
    static PERIOD_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s+").unwrap());

    // Regex for matching characters in surname (letters, accents, apostrophes, hyphens)
    // Note: Rust regex doesn't support Unicode properties the same way, so we use
    // character classes for common diacritics
    static AUTHOR_AFTER: Lazy<Vec<Regex>> = Lazy::new(|| {
        let sc = r"[a-zA-Z\u{00A0}-\u{017F}'\-`\u{00B4}]"; // surname chars
        vec![
            // Surname followed by comma: "Smith,"
            Regex::new(&format!(r"^([A-Z]{}+)\s*,", sc)).unwrap(),
            // Surname + Initial(s) + comma: "Smith JK,"
            Regex::new(&format!(r"^([A-Z]{}+)\s+([A-Z][A-Z]?)\s*,", sc)).unwrap(),
            // Surname + Initial comma: "Smith J,"
            Regex::new(&format!(r"^([A-Z]{}+)\s+[A-Z]{{1,2}},", sc)).unwrap(),
            // "and Surname" pattern
            Regex::new(r"(?i)^and\s+[A-Z]").unwrap(),
            // Another initial: "X."
            Regex::new(r"^[A-Z]\.").unwrap(),
            // Compound initial: "X.-Y."
            Regex::new(r"^[A-Z]\.-[A-Z]\.").unwrap(),
            // Surname + period + Capital: "Smith. X"
            Regex::new(&format!(r"^([A-Z]{}+)\.\s+[A-Z]", sc)).unwrap(),
            // Surname + "and" + Capital
            Regex::new(&format!(r"(?i)^([A-Z]{}+)\s+and\s+[A-Z]", sc)).unwrap(),
            // Multi-part surname: "Van Goethem,"
            Regex::new(&format!(r"^([A-Z]{}+)\s+([A-Z]{}+)\s*,", sc, sc)).unwrap(),
        ]
    });

    let mut sentences = Vec::new();
    let mut current_start = 0;

    for m in PERIOD_SPACE.find_iter(text) {
        let pos = m.start();
        if pos == 0 {
            continue;
        }

        let char_before = text.as_bytes()[pos - 1];

        // Check if period follows a single capital letter (potential initial)
        if char_before.is_ascii_uppercase()
            && (pos == 1 || !text.as_bytes()[pos - 2].is_ascii_alphabetic())
        {
            let after_period = &text[m.end()..];
            let is_author = AUTHOR_AFTER.iter().any(|re| re.is_match(after_period));
            if is_author {
                continue; // Skip — this is an author initial
            }
        }

        // Check for mid-sentence abbreviation
        let mut word_start = pos - 1;
        while word_start > 0 && text.as_bytes()[word_start - 1].is_ascii_alphabetic() {
            word_start -= 1;
        }
        let word_before = &text[word_start..pos];
        if MID_SENTENCE_ABBREVIATIONS.contains(word_before.to_lowercase().as_str()) {
            continue;
        }

        // This is a real sentence boundary
        let sentence = text[current_start..pos].trim();
        if !sentence.is_empty() {
            sentences.push(sentence.to_string());
        }
        current_start = m.end();
    }

    // Add remaining text
    let remaining = text[current_start..].trim();
    if !remaining.is_empty() {
        sentences.push(remaining.to_string());
    }

    sentences
}

// ───────────────── Title cleaning helpers ─────────────────

fn truncate_at_sentence_end(title: &str) -> String {
    for m in title.match_indices('.') {
        let pos = m.0;
        // Find start of segment
        let last_period = title[..pos].rfind('.').map(|p| p + 1).unwrap_or(0);
        let last_space = title[..pos].rfind(' ').map(|p| p + 1).unwrap_or(0);
        let segment_start = last_period.max(last_space);
        let segment = &title[segment_start..pos];

        // If segment > 2 chars, it's a real sentence end
        // OR 2-char ALL-CAPS segment (acronyms like "AI.", "ML.")
        if segment.len() > 2 || (segment.len() == 2 && segment.chars().all(|c| c.is_uppercase()))
        {
            // Skip if period is immediately followed by a letter (product names)
            if pos + 1 < title.len() && title.as_bytes()[pos + 1].is_ascii_alphabetic() {
                continue;
            }
            return title[..pos].to_string();
        }
    }
    title.to_string()
}

fn apply_cutoff_patterns(title: &str) -> String {
    static CUTOFF_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"(?i)\.\s*[Ii]n:\s+[A-Z].*$").unwrap(),
            Regex::new(r"(?i)\.\s*[Ii]n\s+[A-Z].*$").unwrap(),
            Regex::new(r"(?i)[.?!]\s*(?:Proceedings|Conference|Workshop|Symposium|IEEE|ACM|USENIX|AAAI|EMNLP|NAACL|arXiv|Available|CoRR|PACM[- ]\w+).*$").unwrap(),
            Regex::new(r"(?i)[.?!]\s*(?:Advances\s+in|Journal\s+of|Transactions\s+of|Transactions\s+on|Communications\s+of).*$").unwrap(),
            Regex::new(r"(?i)[.?!]\s+International\s+Journal\b.*$").unwrap(),
            Regex::new(r"(?i)\.\s*[A-Z][a-z]+\s+(?:Journal|Review|Transactions|Letters|advances|Processing|medica|Intelligenz)\b.*$").unwrap(),
            Regex::new(r"(?i)\.\s*(?:Patterns|Data\s+&\s+Knowledge).*$").unwrap(),
            Regex::new(r"[.,]\s+[A-Z][a-z]+\s+\d+[,\s].*$").unwrap(),
            Regex::new(r"(?i),\s*volume\s+\d+.*$").unwrap(),
            Regex::new(r",\s*\d+\s*\(\d+\).*$").unwrap(),
            Regex::new(r",\s*\d+\s*$").unwrap(),
            Regex::new(r"\.\s*\d+\s*$").unwrap(),
            Regex::new(r"\.\s*https?://.*$").unwrap(),
            Regex::new(r"\.\s*ht\s*tps?://.*$").unwrap(),
            Regex::new(r"(?i),\s*(?:vol\.|pp\.|pages).*$").unwrap(),
            Regex::new(r"(?i)\.\s*Data\s+in\s+brief.*$").unwrap(),
            Regex::new(r"(?i)\.\s*Biochemia\s+medica.*$").unwrap(),
            Regex::new(r"(?i)\.\s*KI-K\u{00FC}nstliche.*$").unwrap(),
            Regex::new(r"\s+arXiv\s+preprint.*$").unwrap(),
            Regex::new(r"\s+arXiv:\d+.*$").unwrap(),
            Regex::new(r"\s+CoRR\s+abs/.*$").unwrap(),
            Regex::new(r"(?i),?\s*(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+(?:19|20)\d{2}.*$").unwrap(),
            Regex::new(r"(?i)[.,]\s*[Aa]ccessed\s+.*$").unwrap(),
            Regex::new(r"\s*\(\d+[\u{2013}\-]\d*\)\s*$").unwrap(),
            Regex::new(r"\s*\(pp\.?\s*\d+[\u{2013}\-]\d*\)\s*$").unwrap(),
            Regex::new(r",?\s+\d+[\u{2013}\-]\d+\s*$").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z]+(?:\s+(?:in|of|on|and|for|the|a|an|&|[A-Z]?[a-zA-Z]+))+,\s*\d+\s*[,:\s]\s*\d+[\u{2013}\-]?\d*.*$").unwrap(),
        ]
    });

    let mut result = title.to_string();
    for re in CUTOFF_PATTERNS.iter() {
        result = re.replace(&result, "").to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ieee_quoted_title() {
        let ref_text = r#"J. Smith, A. Jones, and C. Williams, "Detecting Fake References in Papers," in Proc. IEEE Conf., 2023."#;
        let (title, from_quotes) = extract_title_from_reference(ref_text);
        assert!(from_quotes);
        assert!(title.contains("Detecting Fake References"));
    }

    #[test]
    fn test_acm_year_title() {
        let ref_text = "Smith, J. and Jones, A. 2022. A Novel Approach to Reference Detection. In Proceedings of ACM SIGIR.";
        let (title, from_quotes) = extract_title_from_reference(ref_text);
        assert!(!from_quotes);
        assert!(title.contains("Novel Approach"));
    }

    #[test]
    fn test_clean_title_trailing_venue() {
        let title = "My Great Paper. In Proceedings of USENIX Security";
        let cleaned = clean_title(title, false);
        assert_eq!(cleaned, "My Great Paper");
    }

    #[test]
    fn test_clean_title_arxiv() {
        let title = "Some Title arXiv preprint arXiv:2301.12345";
        let cleaned = clean_title(title, false);
        assert_eq!(cleaned, "Some Title");
    }

    #[test]
    fn test_clean_title_from_quotes() {
        // From quotes: no sentence truncation
        let title = "A Short Sentence. With More Words Here";
        let cleaned_quotes = clean_title(title, true);
        // Should preserve multi-sentence for quoted titles
        assert!(cleaned_quotes.contains("More Words"));

        let cleaned_no_quotes = clean_title(title, false);
        // Should truncate at first sentence boundary
        assert!(!cleaned_no_quotes.contains("More Words"));
    }

    #[test]
    fn test_split_sentences_skip_initials() {
        let text = "J. Smith and A. Jones. A Novel Detection Method. In Proceedings.";
        let parts = split_sentences_skip_initials(text);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].contains("Smith"));
        assert!(parts[1].contains("Novel Detection"));
    }

    #[test]
    fn test_springer_year_format() {
        let ref_text = "Smith J, Jones A (2023) A novel approach to detection. Nature 500(3):123-456";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(title.contains("novel approach"));
    }

    #[test]
    fn test_empty_ref() {
        let (title, from_quotes) = extract_title_from_reference("");
        assert!(title.is_empty());
        assert!(!from_quotes);
    }
}
