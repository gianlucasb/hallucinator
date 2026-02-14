use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use crate::config::PdfParsingConfig;
use crate::text_processing::fix_hyphenation;

/// Abbreviations that should NEVER be sentence boundaries (mid-title abbreviations).
static MID_SENTENCE_ABBREVIATIONS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        "vs", "eg", "ie", "cf", "fig", "figs", "eq", "eqs", "sec", "ch", "pt", "no",
    ]
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
    extract_title_from_reference_with_config(ref_text, &PdfParsingConfig::default())
}

/// Config-aware version of [`extract_title_from_reference`].
pub(crate) fn extract_title_from_reference_with_config(
    ref_text: &str,
    config: &PdfParsingConfig,
) -> (String, bool) {
    // Normalize whitespace and fix hyphenation
    let ref_text = fix_hyphenation(ref_text);
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    let ref_text = WS_RE.replace_all(&ref_text, " ");
    let ref_text = ref_text.trim();

    // Strip reference number prefixes: [N] or N.
    static REF_NUM_BRACKET: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[\d+\]\s*").unwrap());
    static REF_NUM_DOT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+\.\s*").unwrap());
    let ref_text = REF_NUM_BRACKET.replace(ref_text, "");
    let ref_text = REF_NUM_DOT.replace(&ref_text, "");
    let ref_text = ref_text.trim_start_matches(['.', ' ']);

    // === Format 1: IEEE/USENIX - Quoted titles ===
    if let Some(result) = try_quoted_title_with_config(ref_text, config) {
        return result;
    }

    // === Format 1a: Bracket citation - "[ACGH20] Authors. Title. In Venue" ===
    if let Some(result) = try_bracket_code(ref_text) {
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

    // === Format 2: Author names with particles (von, van der, etc.) ===
    // Must run before venue marker and year-based formats, since those
    // mis-split at "P. von Styp-Rekowsky" treating "P." as sentence end
    if let Some(result) = try_author_particles(ref_text) {
        return result;
    }

    // === Format 3a: Springer/Nature/Harvard - "Authors (Year) Title" ===
    if let Some(result) = try_springer_year(ref_text) {
        return result;
    }

    // === Format 3b: ACM - "Authors. Year. Title. In Venue" ===
    if let Some(result) = try_acm_year(ref_text) {
        return result;
    }

    // === Format 4: USENIX/ICML/NeurIPS/Elsevier - "Authors. Title. In Venue" ===
    if let Some(result) = try_venue_marker(ref_text) {
        return result;
    }

    // === Format 5: Journal style ===
    if let Some(result) = try_journal(ref_text) {
        return result;
    }

    // === Format 5b: Elsevier journal ===
    if let Some(result) = try_elsevier_journal(ref_text) {
        return result;
    }

    // === Format 6a: Chinese ALL CAPS authors (SURNAME I, SURNAME I, et al. Title) ===
    if let Some(result) = try_chinese_allcaps(ref_text) {
        return result;
    }

    // === Format 6b: Western ALL CAPS authors (SURNAME, F. Title) ===
    if let Some(result) = try_all_caps_authors(ref_text) {
        return result;
    }

    // === Format 7: Direct "Title. In Venue" fallback ===
    if let Some(result) = try_direct_in_venue(ref_text) {
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
    clean_title_with_config(title, from_quotes, &PdfParsingConfig::default())
}

/// Config-aware version of [`clean_title`].
pub(crate) fn clean_title_with_config(
    title: &str,
    from_quotes: bool,
    config: &PdfParsingConfig,
) -> String {
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

    // Handle "? JournalName, vol(issue)" — journal name bleeding after question mark
    static QMARK_JOURNAL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[?!]\s+[A-Z][a-zA-Z\s&+\u{00AE}\u{2013}\u{2014}\-]+,\s*(?:vol\.?\s*)?\d+")
            .unwrap()
    });
    if let Some(m) = QMARK_JOURNAL_RE.find(&title) {
        let punct_pos = title[..m.end()].rfind(['?', '!']).unwrap();
        title = title[..=punct_pos].to_string();
    }

    // Handle "? Automatica 34(" or "? IEEE Trans... 53(" — journal + volume with parens
    static QMARK_JOURNAL_VOL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"[?!]\s+(?:IEEE\s+Trans[a-z.]*|ACM\s+Trans[a-z.]*|Automatica|J\.\s*[A-Z][a-z]+|[A-Z][a-z]+\.?\s+[A-Z][a-z]+\.?)\s+\d+\s*[(\[]",
        )
        .unwrap()
    });
    if let Some(m) = QMARK_JOURNAL_VOL_RE.find(&title) {
        let punct_pos = title[..m.end()].rfind(['?', '!']).unwrap();
        title = title[..=punct_pos].to_string();
    }

    // Handle "? IEEE Trans. Aut. Contr. 53" — abbreviated journal + volume, no parens
    static QMARK_ABBREV_JOURNAL_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"[?!]\s+(?:IEEE|ACM|SIAM)\s+Trans[a-z.]*(?:\s+[A-Z][a-z]+\.?)+\s+\d+").unwrap()
    });
    if let Some(m) = QMARK_ABBREV_JOURNAL_RE.find(&title) {
        let punct_pos = title[..m.end()].rfind(['?', '!']).unwrap();
        title = title[..=punct_pos].to_string();
    }

    // Remove editor lists: ". In Name, Name, and Name, editors, Venue"
    static EDITOR_LIST_RE: Lazy<Regex> = Lazy::new(|| {
        let name = r"[A-Za-z\u{00C0}-\u{024F}]+(?:\s+[A-Z]\.)*(?:\s+[A-Za-z\u{00C0}-\u{024F}]+)?";
        Regex::new(&format!(
            r"\.\s*In\s+{n}(?:,\s*{n})*(?:,?\s*and\s+{n})?,\s*editors?,",
            n = name
        ))
        .unwrap()
    });
    if let Some(m) = EDITOR_LIST_RE.find(&title) {
        title = title[..m.start()].to_string();
    }

    // Apply cutoff patterns to remove trailing venue/metadata
    title = apply_cutoff_patterns_with_config(&title, config);

    title = title.trim().to_string();
    static TRAILING_PUNCT: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.,;:]+$").unwrap());
    title = TRAILING_PUNCT.replace(&title, "").to_string();

    title.trim().to_string()
}

// ───────────────── Format-specific extractors ─────────────────

fn try_quoted_title_with_config(
    ref_text: &str,
    config: &PdfParsingConfig,
) -> Option<(String, bool)> {
    static DEFAULT_QUOTE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            // Smart quotes (any combo of \u201c, \u201d, \u201c)
            Regex::new(r#"[\u{201c}\u{201d}"]([^\u{201c}\u{201d}"]+)[\u{201c}\u{201d}"]"#).unwrap(),
            // Regular quotes
            Regex::new(r#""([^"]+)""#).unwrap(),
            // Smart single quotes (Harvard/APA style): \u2018...\u2019
            Regex::new(r"[\u{2018}]([^\u{2018}\u{2019}]{10,})[\u{2019}]").unwrap(),
            // Plain single quotes (Harvard/APA style): require ') ' or similar delimiter
            // to avoid matching possessive apostrophes
            Regex::new(r"(?:^|[\s(])'([^']{10,})'(?:\s*[,.]|\s*$)").unwrap(),
        ]
    });

    let resolved = config.quote_patterns.resolve(&DEFAULT_QUOTE_PATTERNS);

    for re in resolved.iter() {
        if let Some(caps) = re.captures(ref_text) {
            let quoted_part = caps.get(1).unwrap().as_str().trim();
            let after_quote = ref_text[caps.get(0).unwrap().end()..].trim();

            // IEEE: comma inside quotes means title is complete
            // Accept 2+ words for quoted titles (quotes are a strong indicator)
            if quoted_part.ends_with(',') {
                if quoted_part.split_whitespace().count() >= 2 {
                    return Some((quoted_part.to_string(), true));
                }
                continue;
            }

            // Check for subtitle after the quote
            if !after_quote.is_empty() {
                let subtitle_text = if after_quote.starts_with(':') || after_quote.starts_with('-')
                {
                    Some(after_quote[1..].trim())
                } else if after_quote.chars().next().is_some_and(|c| c.is_uppercase()) {
                    Some(after_quote)
                } else {
                    None
                };

                if let Some(sub) = subtitle_text {
                    let subtitle_end = find_subtitle_end(sub);
                    let subtitle = sub[..subtitle_end].trim();
                    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.,;:]+$").unwrap());
                    let subtitle = TRAIL.replace(subtitle, "");
                    if subtitle.split_whitespace().count() >= 2 {
                        return Some((format!("{}: {}", quoted_part, subtitle), true));
                    }
                }
            }

            // No subtitle — use quoted part if long enough
            // Accept 2+ words for quoted titles (quotes are a strong indicator)
            if quoted_part.split_whitespace().count() >= 2 {
                return Some((quoted_part.to_string(), true));
            }
        }
    }
    None
}

fn find_subtitle_end(text: &str) -> usize {
    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        let j = r"[a-zA-Z&+\u{00AE}\u{2013}\u{2014}\-]"; // journal name chars (no \s)
        vec![
            Regex::new(r"\.\s*[Ii]n\s+").unwrap(),
            Regex::new(r"\.\s*(?:Proc|IEEE|ACM|USENIX|NDSS|CCS|AAAI|WWW|CHI|arXiv)").unwrap(),
            Regex::new(r",\s*[Ii]n\s+").unwrap(),
            Regex::new(r"\.\s*\((?:19|20)\d{2}\)").unwrap(),
            Regex::new(r"[,.]\s*(?:19|20)\d{2}").unwrap(),
            Regex::new(r"\s+(?:19|20)\d{2}\.").unwrap(),
            Regex::new(r"[.,]\s+[A-Z][a-z]+\s+\d+[,\s]").unwrap(),
            Regex::new(&format!(r"\.\s*[A-Z](?:{}|\s)+,\s*\d+\s*[,(:]", j)).unwrap(),
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
    // Enhanced Springer/LNCS format: "Author, I., Author, I.: Title. In: Venue"
    // Also handles multi-initial patterns like "B.S.:", "C.P.:", "L.:"
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?:,\s*)?[A-Z](?:\.[A-Z])*\.\s*:\s*(.+)").unwrap());

    let caps = RE.captures(ref_text)?;
    let after_colon = caps.get(1).unwrap().as_str().trim();

    let title_end = find_title_end_lncs(after_colon);
    let title = after_colon[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"[.,;:]+\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    // Reject if what we extracted looks like journal metadata, not a title
    if is_journal_metadata(&title) {
        return None;
    }

    if title.split_whitespace().count() >= 2 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn find_title_end_lncs(text: &str) -> usize {
    static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            // Journal + volume(issue): ". Journal 13(6),"
            Regex::new(r"\.\s+[A-Z][A-Za-z\s&\-]+\s+\d+\s*\(\d+\)\s*[,:]").unwrap(),
            // Journal + volume, pages: ". Journal 13, 456" or ": 456"
            Regex::new(r"\.\s+[A-Z][A-Za-z\s&\-]+\s+\d+\s*[,:]\s*\d+").unwrap(),
            // In/In: venue markers
            Regex::new(r"\.\s*[Ii]n:\s+").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|Proc\.)\s+of").unwrap(),
            // Common venues/publishers
            Regex::new(r"\.\s*(?:IEEE|ACM|USENIX|NDSS|arXiv|Nature|Science)").unwrap(),
            Regex::new(r"\.\s*(?:Journal|Trans\.|Transactions|Review)\s+(?:of|on)").unwrap(),
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s]+(?:Access|Journal|Review|Transactions)").unwrap(),
            Regex::new(r"\.\s*(?:Springer|Elsevier|Wiley|Cambridge|Oxford)\b").unwrap(),
            // Volume without journal name
            Regex::new(r"\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[\u{2013}\-]").unwrap(),
            // DOI/URL
            Regex::new(r",?\s+doi:\s*10\.").unwrap(),
            Regex::new(r"\.\s*https?://").unwrap(),
            // Year patterns
            Regex::new(r"\s+\((?:19|20)\d{2}\)\s*[,.]?\s*(?:https?://|$)").unwrap(),
            Regex::new(r"\s+\((?:19|20)\d{2}\)\s*,").unwrap(),
            Regex::new(r"\.\s*pp?\.?\s*\d+").unwrap(),
            // Venue name with year: ". Venue Name (2020)"
            Regex::new(r"\.\s+[A-Z][A-Za-z\s]+\s+\(\d{4}\)").unwrap(),
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

/// Check if extracted text looks like journal metadata rather than a title.
fn is_journal_metadata(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }

    static PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            // "In:" or "In " venue markers
            Regex::new(r"(?i)^In[:\s]").unwrap(),
            // "In: Proceedings..." or "In: 2019 IEEE..."
            Regex::new(r"(?i)^In[:\s]+(?:Proceedings|Proc\.)").unwrap(),
            Regex::new(r"(?i)^In[:\s]+\d{4}\s+(?:IEEE|ACM|USENIX)").unwrap(),
            // Journal Vol(Issue), Pages (Year): "Educational Researcher 13(6), 4–16 (1984)"
            Regex::new(
                r"^[A-Z][A-Za-z\s&\-]+\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[\u{2013}\-]\d+\s*\(\d{4}\)",
            )
            .unwrap(),
            // Journal Vol:Pages (Year): "Nature 123:456-789 (2020)"
            Regex::new(r"^[A-Z][A-Za-z\s&\-]+\s+\d+\s*:\s*\d+[\u{2013}\-]\d+\s*\(\d{4}\)").unwrap(),
            // Journal with acronym in parens: "Journal of the ACM (JACM) 32(2)"
            Regex::new(r"^[A-Z][A-Za-z\s]+\([A-Z]+\)\s+\d+\s*\(\d+\)").unwrap(),
            // Short journal: "Nature 299(5886)"
            Regex::new(r"^(?:Nature|Science|Cell|PNAS|PLoS)\s+\d+\s*\(\d+\)").unwrap(),
            // Just volume/page: "13(6), 4–16 (1984)"
            Regex::new(r"^\d+\s*\(\d+\)\s*[,:]\s*\d+[\u{2013}\-]\d+").unwrap(),
            // Journal name;year format: "IEEE Trans... 2018;17(3)"
            Regex::new(r"^[A-Z][A-Za-z\s]+\d{4};\d+").unwrap(),
        ]
    });

    PATTERNS.iter().any(|re| re.is_match(text))
}

fn try_org_doc(ref_text: &str) -> Option<(String, bool)> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Z][a-zA-Z\s]+):\s*(.+)").unwrap());

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
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\((\d{4}[a-z]?)\)\.?\s+").unwrap());

    let caps = RE.captures(ref_text)?;
    let after_year = &ref_text[caps.get(0).unwrap().end()..];

    // Journal name character class: letters, spaces, &, +, ®, en-dash, em-dash, hyphen
    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        let j = r"[a-zA-Z\s&+\u{00AE}\u{2013}\u{2014}\-]"; // journal name chars
        vec![
            Regex::new(r"\.\s*[Ii]n:\s+").unwrap(),
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)").unwrap(),
            Regex::new(&format!(r"\.\s*[A-Z]{}+\d+\s*\(\d+\)", j)).unwrap(),
            Regex::new(&format!(r"\.\s*[A-Z]{}+\d+:\d+", j)).unwrap(),
            Regex::new(&format!(r"\.\s*[A-Z]{}+,\s*\d+", j)).unwrap(),
            Regex::new(r"\.\s*https?://").unwrap(),
            Regex::new(r"\.\s*URL\s+").unwrap(),
            Regex::new(r"\.\s*Tech\.\s*rep\.").unwrap(),
            Regex::new(r"\.\s*pp?\.?\s*\d+").unwrap(),
            // Journal name after sentence-ending punctuation: "? JournalName, vol(issue)"
            Regex::new(&format!(r"[?!]\s+[A-Z]{}+,\s*\d+\s*\(", j)).unwrap(),
            // Journal after ? with volume:pages: "? JournalName, vol: pages"
            Regex::new(&format!(r"[?!]\s+[A-Z]{}+,\s*\d+\s*:", j)).unwrap(),
        ]
    });

    let mut title_end = after_year.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(after_year) {
            let candidate = if after_year
                .as_bytes()
                .get(m.start())
                .is_some_and(|&b| b == b'?' || b == b'!')
            {
                m.start() + 1
            } else {
                m.start()
            };
            title_end = title_end.min(candidate);
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
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*((?:19|20)\d{2})\.\s+").unwrap());

    let caps = RE.captures(ref_text)?;
    let after_year = &ref_text[caps.get(0).unwrap().end()..];

    // Journal name character class: letters, spaces, &, +, ®, en-dash, em-dash, hyphen
    static END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        let j = r"[a-zA-Z\s&+\u{00AE}\u{2013}\u{2014}\-]"; // journal name chars
        vec![
            Regex::new(r"\.\s*[Ii]n\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)").unwrap(),
            Regex::new(r"\s+doi:").unwrap(),
            // Journal name after sentence-ending punctuation: "? JournalName, vol(issue)"
            Regex::new(&format!(r"[?!]\s+[A-Z]{}+,\s*\d+\s*\(", j)).unwrap(),
            // Journal after ? with volume:issue pattern: "? JournalName, vol: pages"
            Regex::new(&format!(r"[?!]\s+[A-Z]{}+,\s*\d+\s*:", j)).unwrap(),
            // Period then journal + volume/issue: ". JournalName, vol(issue)"
            Regex::new(&format!(r"\.\s*[A-Z]{}+,\s*\d+\s*\(", j)).unwrap(),
            // Period then journal + volume:pages: ". JournalName, vol: pages"
            Regex::new(&format!(r"\.\s*[A-Z]{}+,\s*\d+\s*:", j)).unwrap(),
            // Period then journal name + comma + volume (no parens/colon): ". JournalName, vol"
            // Catches "Foundations and Trends® in Human–Computer Interaction, 14(4–5)"
            Regex::new(&format!(r"\.\s*[A-Z]{}{{10,}},\s*\d+", j)).unwrap(),
        ]
    });

    let mut title_end = after_year.len();
    for re in END_PATTERNS.iter() {
        if let Some(m) = re.find(after_year) {
            // For patterns anchored on ? or !, keep the punctuation mark
            let candidate = if after_year
                .as_bytes()
                .get(m.start())
                .is_some_and(|&b| b == b'?' || b == b'!')
            {
                m.start() + 1
            } else {
                m.start()
            };
            title_end = title_end.min(candidate);
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

fn try_chinese_allcaps(ref_text: &str) -> Option<(String, bool)> {
    // Chinese ALL CAPS: "SURNAME I, SURNAME I, et al. Title[J]. Venue"
    // Key difference from Western: single-letter initial without period after surname
    static CHINESE_CAPS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[A-Z]{2,}\s+[A-Z](?:,|\s|$)").unwrap());
    if !CHINESE_CAPS.is_match(ref_text) {
        return None;
    }

    // Find end of author list at "et al." or transition to non-author text
    static ET_AL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i),?\s+et\s+al\.?\s*[,.]?\s*").unwrap());

    let after_authors_str: String = if let Some(m) = ET_AL.find(ref_text) {
        ref_text[m.end()..].trim().to_string()
    } else {
        // Find where ALL CAPS author pattern ends by scanning comma-separated parts
        static AUTHOR_PART: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"^[A-Z]{2,}(?:\s+[A-Z](?:\s+[A-Z])?)?$").unwrap());
        let parts: Vec<&str> = ref_text.split(", ").collect();
        let mut title_start_idx = None;
        for (i, part) in parts.iter().enumerate() {
            let trimmed = part.trim();
            if AUTHOR_PART.is_match(trimmed) {
                continue;
            }
            title_start_idx = Some(i);
            break;
        }
        match title_start_idx {
            Some(idx) => parts[idx..].join(", ").trim().to_string(),
            None => return None,
        }
    };

    if after_authors_str.is_empty() {
        return None;
    }

    // Find where title ends — at Chinese citation markers or venue patterns
    static TITLE_END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\[J\]").unwrap(), // Chinese: journal
            Regex::new(r"\[C\]").unwrap(), // Chinese: conference
            Regex::new(r"\[M\]").unwrap(), // Chinese: book/monograph
            Regex::new(r"\[D\]").unwrap(), // Chinese: dissertation
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s]+\d+\s*\(\d+\)").unwrap(), // ". Journal 34(5)"
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s&+]+\d+:\d+").unwrap(), // ". Journal 34:123"
            Regex::new(r"\.\s*[A-Z][a-zA-Z\s&+]+,\s*\d+").unwrap(), // ". Journal, vol"
            Regex::new(r"\.\s*(?:19|20)\d{2}").unwrap(), // ". 2024"
            Regex::new(r"\.\s*https?://").unwrap(),
            Regex::new(r"\.\s*doi:").unwrap(),
        ]
    });

    let mut title_end = after_authors_str.len();
    for re in TITLE_END_PATTERNS.iter() {
        if let Some(m) = re.find(&after_authors_str) {
            title_end = title_end.min(m.start());
        }
    }

    let title = after_authors_str[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    if title.split_whitespace().count() >= 3 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn try_all_caps_authors(ref_text: &str) -> Option<(String, bool)> {
    static STARTS_CAPS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Z]{2,}").unwrap());
    if !STARTS_CAPS.is_match(ref_text) {
        return None;
    }

    // Skip Chinese ALL CAPS pattern (handled by try_chinese_allcaps)
    static CHINESE_CAPS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^[A-Z]{2,}\s+[A-Z](?:,|\s)").unwrap());
    if CHINESE_CAPS.is_match(ref_text) {
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

fn try_bracket_code(ref_text: &str) -> Option<(String, bool)> {
    // Bracket citation format: "[ACGH20] Authors. Title. In Venue"
    static BRACKET_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\[([A-Z]+\d+[a-z]?)\]\s*").unwrap());

    let caps = BRACKET_RE.captures(ref_text)?;
    let after_bracket = &ref_text[caps.get(0).unwrap().end()..];

    // Split into sentences and find author-title boundary
    let sentences = split_sentences_skip_initials(after_bracket);
    if sentences.len() < 2 {
        return None;
    }

    // First sentence is authors, look for title in subsequent sentences
    for i in 0..sentences.len().saturating_sub(1) {
        let sent = &sentences[i];
        // Check if this sentence ends with what looks like an author name
        // and next doesn't start with "In" (venue marker)
        static AUTHOR_END_RE: Lazy<Regex> =
            Lazy::new(|| Regex::new(r"(?:and\s+)?[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*$").unwrap());
        static IN_START_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^In\s+").unwrap());

        if AUTHOR_END_RE.is_match(sent) {
            let next = &sentences[i + 1];
            if !IN_START_RE.is_match(next) && next.starts_with(|c: char| c.is_uppercase()) {
                // Found the title. Reconstruct it and find where it ends (at "In Venue")
                let remaining: String = sentences[i + 1..].join(". ");
                let title_end = remaining.find(". In ").unwrap_or(remaining.len());
                let title = remaining[..title_end].trim();
                if title.split_whitespace().count() >= 3 {
                    return Some((title.to_string(), false));
                }
            }
        }
    }
    None
}

fn try_author_particles(ref_text: &str) -> Option<(String, bool)> {
    // Handles author names with particles: von, van der, Le, etc.
    // Pattern: "I. Name, I. Name, and I. Name. Title"
    // The key is finding ", and Initial. Surname. TitleStart"
    static AND_AUTHOR_TITLE_RE: Lazy<Regex> = Lazy::new(|| {
        let initial = r"[\x41-\x5A\u{00C0}-\u{00D6}\u{00D8}-\u{00DE}\u{0027}\u{0060}\u{00B4}]\.(?:[\s\-]*[A-Z]\.)*";
        let particle =
            r"(?:(?:von|van|de|del|della|di|da|dos|das|du|le|la|les|den|der|ten|ter|op|het)\s+)?";
        let surname_chars = r"[A-Za-z\u{00C0}-\u{024F}\u{0027}\u{0060}\u{00B4}\u{2019}\-]";
        let surname = format!(r"{}{}+(?:\s+{}+)*", particle, surname_chars, surname_chars);
        let pattern = format!(
            r#",?\s+and\s+{}\s*{}\.\s+([A-Z\u{{00C0}}-\u{{00D6}}][a-z]|[A-Z]\s+[a-z]|[0-9]|["\u{{201c}}\u{{201d}}])"#,
            initial, surname,
        );
        Regex::new(&pattern).unwrap()
    });

    let caps = AND_AUTHOR_TITLE_RE.captures(ref_text)?;
    let title_start = caps.get(1).unwrap().start();
    let title_text = &ref_text[title_start..];

    // Find where title ends (venue/year markers)
    static TITLE_END_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
        vec![
            Regex::new(r"\.\s+In\s+").unwrap(),
            Regex::new(r"\s+In\s+Proceedings").unwrap(),
            Regex::new(r"\.\s+(?:Proc\.|Proceedings\s+of)").unwrap(),
            Regex::new(r"\.\s+(?:IEEE|ACM|USENIX|NDSS|CCS|AAAI|ICML|NeurIPS|EuroS&P)\b").unwrap(),
            Regex::new(r"\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\s+\d{4}").unwrap(),
            Regex::new(r"\.\s+[A-Z][a-z]+(?:\s*&\s*[A-Z][a-z]+)+").unwrap(),
            Regex::new(r"\.\s+arXiv\s+preprint").unwrap(),
            Regex::new(r",\s+(?:vol\.|pp\.|pages)\s").unwrap(),
            Regex::new(r",\s+\d{4}\.\s*$").unwrap(),
            Regex::new(r",\s+\d+\(\d+\)").unwrap(),
            Regex::new(r"\.\s+(?:Springer|Elsevier|Wiley|Nature|Science|PLOS|Oxford|Cambridge)\b")
                .unwrap(),
            Regex::new(r"\.\s+(?:The\s+)?(?:Annals|Journal|Proceedings)\s+of\b").unwrap(),
            Regex::new(r"\.\s+Journal\s+of\s+[A-Z]").unwrap(),
            Regex::new(r"\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+,\s*\d").unwrap(),
            Regex::new(r"\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+\s+\d+[:(]").unwrap(),
        ]
    });

    let mut title_end = title_text.len();
    for re in TITLE_END_PATTERNS.iter() {
        if let Some(m) = re.find(title_text) {
            title_end = title_end.min(m.start());
        }
    }

    let title = title_text[..title_end].trim();
    static TRAIL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\.\s*$").unwrap());
    let title = TRAIL.replace(title, "");

    if title.split_whitespace().count() >= 3 {
        Some((title.to_string(), false))
    } else {
        None
    }
}

fn try_direct_in_venue(ref_text: &str) -> Option<(String, bool)> {
    // Fallback: "Title. In Something" where title starts with capital and has 15+ chars
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"^([A-Z][^.]{15,}?)\.\s+In\s+(?:[A-Z]|Proceedings|Proc\.)").unwrap()
    });

    let caps = RE.captures(ref_text)?;
    let title = caps.get(1).unwrap().as_str().trim();

    if title.split_whitespace().count() >= 4 {
        Some((title.to_string(), false))
    } else {
        None
    }
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
        let and_count = words.iter().filter(|w| w.to_lowercase() == "and").count();

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
        // Ensure word_start is on a char boundary (backward byte walk may land inside multi-byte UTF-8)
        while word_start > 0 && !text.is_char_boundary(word_start) {
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
        if segment.len() > 2 || (segment.len() == 2 && segment.chars().all(|c| c.is_uppercase())) {
            // Skip if period is immediately followed by a letter (product names)
            if pos + 1 < title.len() && title.as_bytes()[pos + 1].is_ascii_alphabetic() {
                continue;
            }
            return title[..pos].to_string();
        }
    }
    title.to_string()
}

/// The built-in default cutoff patterns. Exposed as `pub(crate)` so config resolution can use them.
pub(crate) static DEFAULT_CUTOFF_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    let j = r"[a-zA-Z&+\u{00AE}\u{2013}\u{2014}\-]"; // journal name chars (no \s)
    vec![
        // Chinese citation markers: [J]=journal, [C]=conference, [M]=book, [D]=dissertation
        Regex::new(r"\[J\].*$").unwrap(),
        Regex::new(r"\[C\].*$").unwrap(),
        Regex::new(r"\[M\].*$").unwrap(),
        Regex::new(r"\[D\].*$").unwrap(),
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
        // Journal + volume/pages with expanded char class for &, +, ®, dashes
        Regex::new(&format!(r"\.\s*[A-Z](?:{}|\s)+,\s*\d+\s*[,:\s]\s*\d+[\u{{2013}}\-]?\d*.*$", j)).unwrap(),
        // Journal name after period with no volume (just ends): ". Big Data & Society, 1(1)"
        // These are caught by the volume/issue patterns above; this handles standalone names
        Regex::new(&format!(r"\.\s+(?:[A-Z](?:{}|\s)+[&+](?:{}|\s)+)\s*$", j, j)).unwrap(),
    ]
});

fn apply_cutoff_patterns_with_config(title: &str, config: &PdfParsingConfig) -> String {
    let patterns = config
        .venue_cutoff_patterns
        .resolve(&DEFAULT_CUTOFF_PATTERNS);

    let mut result = title.to_string();
    for re in patterns.iter() {
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
        let ref_text =
            "Smith J, Jones A (2023) A novel approach to detection. Nature 500(3):123-456";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(title.contains("novel approach"));
    }

    #[test]
    fn test_empty_ref() {
        let (title, from_quotes) = extract_title_from_reference("");
        assert!(title.is_empty());
        assert!(!from_quotes);
    }

    #[test]
    fn test_journal_name_after_question_mark() {
        // Journal name "New media & society" should NOT be part of the title
        let ref_text = "Baek, Y. M.; Wojcieszak, M.; and Delli Carpini, M. X. 2012. Online versus face-to-face deliberation: Who? Why? What? With what effects? New media & society, 14(3): 363\u{2013}383";
        let (title, _) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, false);
        assert!(
            !cleaned.contains("New media"),
            "Journal name leaked into title: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("With what effects?"),
            "Title should end with question mark: {}",
            cleaned,
        );
    }

    #[test]
    fn test_harvard_single_quoted_title() {
        // Harvard/APA style uses single quotes around the title
        let ref_text = "Biswas, A., Saha, K. and De Choudhury, M. (2025) \u{2018}Political Elites in the Attention Economy: Visibility Over Civility and Credibility?\u{2019}, Proceedings of the International AAAI Conference on Web and Social Media (ICWSM).";
        let (title, from_quotes) = extract_title_from_reference(ref_text);
        assert!(from_quotes, "Should detect single-quoted title");
        assert!(
            title.contains("Political Elites"),
            "Title should contain 'Political Elites': {}",
            title,
        );
    }

    #[test]
    fn test_utf8_smart_quote_no_crash() {
        // Regression: smart quote before period should not cause UTF-8 panic
        let text = "Smith J. \u{201c}Some title\u{201d}. In Proceedings.";
        let parts = split_sentences_skip_initials(text);
        assert!(!parts.is_empty());
    }

    #[test]
    fn test_big_data_society_journal_leak() {
        // "Big Data & Society" should NOT be in the title (& in journal name)
        let ref_text = "Burrell, J. 2016. How the Machine \u{201c}Thinks\u{201d}: Understanding Opacity in Machine Learning Algorithms. Big Data & Society, 3(1)";
        let (title, _) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, false);
        assert!(
            !cleaned.contains("Big Data"),
            "Journal 'Big Data & Society' leaked into title: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("Understanding Opacity"),
            "Title should contain 'Understanding Opacity': {}",
            cleaned,
        );
    }

    #[test]
    fn test_foundations_trends_journal_leak() {
        // "Foundations and Trends® in Human–Computer Interaction" should NOT be in the title
        let ref_text = "Metaxa, D.; Park, J. S.; Robertson, R. E.; Karahalios, K.; Wilson, C.; and Hancock, J. 2021. Auditing Algorithms: Understanding Algorithmic Systems from the Outside In. Foundations and Trends\u{00AE} in Human\u{2013}Computer Interaction, 14(4\u{2013}5)";
        let (title, _) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, false);
        assert!(
            !cleaned.contains("Foundations"),
            "Journal 'Foundations and Trends' leaked into title: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("Outside In"),
            "Title should contain 'Outside In': {}",
            cleaned,
        );
    }

    #[test]
    fn test_communication_research_journal_leak() {
        // "Communication Research" should NOT be in the title — straight quotes
        let ref_text = "Marchal, N. 2021. \"Be nice or leave me alone\": An intergroup perspective on affective polarization in online political discussions. Communication Research, 49(3): 376\u{2013}398";
        let (title, from_quotes) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, from_quotes);
        assert!(
            !cleaned.contains("Communication Research"),
            "Journal 'Communication Research' leaked into title (straight quotes): {}",
            cleaned,
        );

        // Also test with smart quotes (as commonly found in PDFs)
        let ref_text2 = "Marchal, N. 2021. \u{201c}Be nice or leave me alone\u{201d}: An intergroup perspective on affective polarization in online political discussions. Communication Research, 49(3): 376\u{2013}398";
        let (title2, from_quotes2) = extract_title_from_reference(ref_text2);
        let cleaned2 = clean_title(&title2, from_quotes2);
        assert!(
            !cleaned2.contains("Communication Research"),
            "Journal 'Communication Research' leaked into title (smart quotes): {}",
            cleaned2,
        );
    }

    #[test]
    fn test_social_media_plus_society_journal_leak() {
        // "Social Media + Society" should NOT be in the title (+ in journal name)
        let ref_text = "Zhao, H.; Wang, J.; and Hu, X. 2025. \u{201c}A wandering existence\u{201d}: Social media practices of Chinese youth in the context of platform-swinging. Social Media + Society, 11(1)";
        let (title, _) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, false);
        assert!(
            !cleaned.contains("Social Media + Society"),
            "Journal 'Social Media + Society' leaked into title: {}",
            cleaned,
        );
    }

    #[test]
    fn test_journal_after_period() {
        // "American Political Science Review" should not be in the title
        let ref_text = "Fishkin, J.; Siu, A.; Diamond, L.; and Bradburn, N. 2021. Is deliberation an antidote to extreme partisan polarization? Reflections on \u{201c}America in one room\u{201d}. American Political Science Review, 115(4): 1464\u{2013}1481";
        let (title, _) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, false);
        assert!(
            !cleaned.contains("American Political"),
            "Journal name leaked into title: {}",
            cleaned,
        );
    }

    // ───────────────── Tests for regexp_improvements.py ports ─────────────────

    #[test]
    fn test_chinese_allcaps_with_et_al() {
        let ref_text = "CAO X, YANG B, WANG K, et al. AI-empowered multiple access for 6G: A survey of spectrum sensing, protocol designs, and optimizations[J]. Proceedings of the IEEE, 2024, 112(9): 1264-1302.";
        let (title, _) = extract_title_from_reference(ref_text);
        let cleaned = clean_title(&title, false);
        assert!(
            cleaned.contains("AI-empowered multiple access for 6G"),
            "Should extract Chinese ALL CAPS title: {}",
            cleaned,
        );
        assert!(
            !cleaned.contains("[J]"),
            "Should not include [J] marker: {}",
            cleaned,
        );
    }

    #[test]
    fn test_chinese_allcaps_h_infinity() {
        let ref_text = "LIU Z, SABERI A, et al. H\u{221E} almost state synchronization for homogeneous networks[J]. IEEE Trans. Aut. Contr. 53 (2008), no. 4.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("almost state synchronization"),
            "Should extract title from Chinese ALL CAPS with H-infinity: {}",
            title,
        );
    }

    #[test]
    fn test_chinese_citation_marker_cutoff() {
        // [J] marker should terminate the title
        let title = "Some great research title[J]. Journal Name, 2024";
        let cleaned = clean_title(title, false);
        assert!(
            !cleaned.contains("[J]"),
            "Chinese citation marker [J] should be removed: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("great research title"),
            "Title content should be preserved: {}",
            cleaned,
        );
    }

    #[test]
    fn test_venue_leak_abbreviated_journal_after_question() {
        // "? IEEE Trans. Aut. Contr. 53" should not leak into title
        let title = "Can machines think? IEEE Trans. Aut. Contr. 53 (2008), no. 4";
        let cleaned = clean_title(title, false);
        assert!(
            !cleaned.contains("IEEE"),
            "IEEE journal should not leak into title: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("Can machines think"),
            "Title content should be preserved: {}",
            cleaned,
        );
    }

    #[test]
    fn test_venue_leak_journal_vol_after_question() {
        // "? Automatica 34(5)" should be truncated at ?
        let title = "What is consciousness? Automatica 34(5): 123-456";
        let cleaned = clean_title(title, false);
        assert!(
            !cleaned.contains("Automatica"),
            "Should not contain journal name after ?: {}",
            cleaned,
        );
    }

    #[test]
    fn test_two_word_quoted_title() {
        let ref_text = r#"A. van der Schaft, "Cyclo-dissipativity revisited," IEEE Transactions on Automatic Control, vol. 66, no. 6, pp. 2925-2931, 2021."#;
        let (title, from_quotes) = extract_title_from_reference(ref_text);
        assert!(from_quotes, "Should detect quoted title");
        assert!(
            title.contains("Cyclo-dissipativity"),
            "Should extract 2-word quoted title: {}",
            title,
        );
    }

    #[test]
    fn test_reference_prefix_stripping() {
        // [N] prefix should be stripped
        let ref_text =
            r#"[42] Jones, A. "A comprehensive survey on neural networks," Proc. AAAI, 2023."#;
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("comprehensive survey"),
            "Should extract title after stripping [N] prefix: {}",
            title,
        );

        // N. prefix should be stripped
        let ref_text2 = r#"1. Smith, J. "Deep learning for NLP applications," Nature, 2024."#;
        let (title2, _) = extract_title_from_reference(ref_text2);
        assert!(
            title2.contains("Deep learning"),
            "Should extract title after stripping N. prefix: {}",
            title2,
        );
    }

    #[test]
    fn test_format5_skip_chinese_allcaps() {
        // Western ALL CAPS should NOT match Chinese pattern
        let ref_text_western =
            "SMITH, J., AND JONES, A. A novel approach to detection. In Proceedings of AAAI.";
        let (title, _) = extract_title_from_reference(ref_text_western);
        assert!(
            !title.is_empty(),
            "Western ALL CAPS should still extract a title: {}",
            title,
        );
    }

    // ───────────────── Tests for new format extractors ─────────────────

    #[test]
    fn test_bracket_code_format() {
        let ref_text = "[ACGH20] Gorjan Alagic, Andrew M. Childs, Alex B. Grilo, and Shih-Han Hung. Noninteractive classical verification of quantum computation. In CRYPTO 2020.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("Noninteractive classical verification"),
            "Should extract title from bracket code format: {}",
            title,
        );
    }

    #[test]
    fn test_bracket_code_format_ccy20() {
        let ref_text = "[CCY20] Nai-Hui Chia, Kai-Min Chung, and Takashi Yamakawa. Classical verification of quantum computations with efficient verifier. In Theory of Cryptography Conference 2020.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("Classical verification of quantum computations"),
            "Should extract title from bracket code format: {}",
            title,
        );
    }

    #[test]
    fn test_author_particles_von() {
        let ref_text = "M. Backes, S. Bugiel, O. Schranz, P. von Styp-Rekowsky, and S. Weisgerber. Artist: The android runtime instrumentation and security toolkit. In EuroS&P, 2017.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("Artist") || title.contains("android runtime"),
            "Should extract title with von particle in author: {}",
            title,
        );
    }

    #[test]
    fn test_author_particles_van_der() {
        let ref_text = "C. J. Hoofnagle, B. van der Sloot, and F. Z. Borgesius. The european union general data protection regulation: what it is and what it means. Information & Communications Technology Law, 28(1), 2019.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("european union general data protection"),
            "Should extract title with van der particle in author: {}",
            title,
        );
    }

    #[test]
    fn test_author_particles_le() {
        let ref_text = "K. Allix, T. F. Bissyand\u{00B4}e, J. Klein, and Y. Le Traon. Androzoo: Collecting millions of android apps for the research community. In MSR, 2016.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("Androzoo") || title.contains("Collecting millions"),
            "Should extract title with Le particle in author: {}",
            title,
        );
    }

    #[test]
    fn test_author_particles_fifty_ways() {
        let ref_text = "J. Reardon, \u{00B4}A. Feal, P. Wijesekera, A. E. B. On, N. Vallina-Rodriguez, and S. Egelman. 50 ways to leak your data: An exploration of apps' circumvention of the android permissions system. In USENIX Security, 2019.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("50 ways to leak"),
            "Should extract title starting with number: {}",
            title,
        );
    }

    #[test]
    fn test_direct_in_venue() {
        let ref_text = "Beating the random assignment on constraint satisfaction problems of bounded degree. In Naveen Garg, Klaus Jansen, Anup Rao, editors, Approximation.";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("Beating the random assignment"),
            "Should extract title from direct 'Title. In Venue' format: {}",
            title,
        );
    }

    #[test]
    fn test_editor_list_cleaning() {
        let title = "Beating the random assignment on constraint satisfaction problems. In Naveen Garg, Klaus Jansen, Anup Rao, and Jos\u{00E9} Rolim, editors, Approximation";
        let cleaned = clean_title(title, false);
        assert!(
            !cleaned.contains("editors"),
            "Editor list should be removed from title: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("Beating the random assignment"),
            "Title content should be preserved: {}",
            cleaned,
        );
    }

    #[test]
    fn test_editor_list_simple() {
        let title =
            "A great paper title. In John Smith and Jane Doe, editors, Proceedings of Something";
        let cleaned = clean_title(title, false);
        assert!(
            !cleaned.contains("editors"),
            "Editor list should be removed: {}",
            cleaned,
        );
        assert!(
            cleaned.contains("great paper title"),
            "Title content should be preserved: {}",
            cleaned,
        );
    }

    #[test]
    fn test_springer_lncs_bloom() {
        // Enhanced Springer/LNCS with journal metadata detection
        let ref_text = "Bloom, B.S.: The 2 sigma problem: The search for methods of group instruction as effective as one-to-one tutoring. Educational Researcher 13(6), 4\u{2013}16 (1984)";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("2 sigma problem"),
            "Should extract title from Springer format: {}",
            title,
        );
        assert!(
            !title.contains("Educational Researcher"),
            "Should not include journal name: {}",
            title,
        );
    }

    #[test]
    fn test_springer_lncs_multi_initial() {
        // Multi-initial author like "C.P.:" or "B.S.:"
        let ref_text = "Schnorr, C.P.: Efficient signature generation by smart cards. Journal of cryptology 4(3), 161\u{2013}174 (1991)";
        let (title, _) = extract_title_from_reference(ref_text);
        assert!(
            title.contains("Efficient signature generation"),
            "Should extract title with multi-initial author: {}",
            title,
        );
    }

    #[test]
    fn test_is_journal_metadata_detection() {
        // These should be detected as journal metadata
        assert!(is_journal_metadata(
            "Educational Researcher 13(6), 4\u{2013}16 (1984)"
        ));
        assert!(is_journal_metadata(
            "Nature 299(5886), 802\u{2013}803 (1982)"
        ));
        assert!(is_journal_metadata(
            "In: Proceedings of the 8th ACM Conference"
        ));
        assert!(is_journal_metadata("13(6), 4\u{2013}16 (1984)"));

        // These should NOT be detected as metadata
        assert!(!is_journal_metadata(
            "The 2 sigma problem: The search for methods"
        ));
        assert!(!is_journal_metadata(
            "Knowledge tracing: Modeling the acquisition"
        ));
        assert!(!is_journal_metadata(
            "A survey on deep learning for cybersecurity"
        ));
    }
}
