use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::PdfParsingConfig;

/// Locate the references section in the document text.
///
/// Searches for common reference section headers (References, Bibliography, Works Cited)
/// and returns the text between the header and any end markers (Appendix, Acknowledgments, etc.).
/// Falls back to the last 30% of the document if no header is found.
pub fn find_references_section(text: &str) -> Option<String> {
    find_references_section_with_config(text, &PdfParsingConfig::default())
}

/// Config-aware version of [`find_references_section`].
pub(crate) fn find_references_section_with_config(
    text: &str,
    config: &PdfParsingConfig,
) -> Option<String> {
    static HEADER_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\n\s*(?:References|Bibliography|Works\s+Cited)\s*\n").unwrap()
    });

    let header_re = config.section_header_re.as_ref().unwrap_or(&HEADER_RE);

    // Use the LAST "References" header, not the first.
    // Some papers have multiple "References" headers (e.g., table headers like
    // "Table 2: References to related work") before the actual reference list.
    let matches: Vec<_> = header_re.find_iter(text).collect();
    if let Some(m) = matches.last() {
        let ref_start = m.end();
        let rest = &text[ref_start..];

        static END_RE: Lazy<Regex> = Lazy::new(|| {
            // Match common end-of-references markers:
            // - Explicit section headers: Appendix, Acknowledgments, etc.
            // - Single-letter appendix sections: "A\nAppendix", "A\nTechnical Lemmas" (common in NeurIPS)
            // - Conference checklists: "NeurIPS Paper Checklist", "ICML Checklist", etc.
            //
            // IMPORTANT: "Appendix" must be followed by whitespace, letter/number, or end-of-line.
            // NOT followed by a colon (e.g., "Artifact Appendix: Title" in a reference).
            Regex::new(r"(?i)\n\s*(?:Appendix(?:\s+[A-Z0-9]|\s*\n|\s*$)|Acknowledgments|Acknowledgements|Supplementary|Ethics\s+Statement|Ethical\s+Considerations|Broader\s+Impact|(?:\w+\s+)?(?:Paper\s+)?Checklist|[A-Z]\n\s*(?:Appendix|Technical|Proofs?|Additional|Extended|Experimental|Derivations?|Algorithms?|Details?|Implementation))")
                .unwrap()
        });

        let end_re = config.section_end_re.as_ref().unwrap_or(&END_RE);

        let ref_end = if let Some(end_m) = end_re.find(rest) {
            end_m.start()
        } else {
            rest.len()
        };

        let section = &rest[..ref_end];
        if !section.trim().is_empty() {
            return Some(section.to_string());
        }
    }

    // Fallback: last N% of document (default 30%, i.e. fraction = 0.7)
    let cutoff = (text.len() as f64 * config.fallback_fraction) as usize;
    // Don't split in the middle of a UTF-8 codepoint
    let cutoff = text
        .char_indices()
        .map(|(i, _)| i)
        .find(|&i| i >= cutoff)
        .unwrap_or(cutoff);
    Some(text[cutoff..].to_string())
}

/// Strip conference page headers/footers that get embedded in PDF text extraction.
///
/// These headers appear when PDF pages are concatenated and break pattern matching.
/// Examples:
/// - "USENIX Association\n34th USENIX Security Symposium    2477" (split across lines)
/// - "USENIX Association 34th USENIX Security Symposium 2477" (single line)
/// - "216 34th USENIX Security Symposium USENIX Association"
fn strip_page_headers(text: &str) -> String {
    // USENIX headers can span multiple lines in PDF extraction:
    // "USENIX Association\n34th USENIX Security Symposium    2477"
    // Match both single-line and multi-line variants
    static USENIX_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)(?:USENIX\s+Association\s*\n?\s*)?(?:\d+\s+)?\d+(?:st|nd|rd|th)\s+USENIX\s+(?:Security\s+Symposium|OSDI|ATC|NSDI|HotCloud|WOOT|FAST|LISA|SREcon)(?:\s+USENIX\s+Association)?(?:\s+\d+)?"
        ).unwrap()
    });

    // "USENIX Association" on its own line (often appears before the symposium line)
    static USENIX_ASSOC_ONLY: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?m)^\s*USENIX\s+Association\s*$").unwrap()
    });

    // IEEE S&P, EuroS&P, etc.
    static IEEE_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)^\s*(?:\d+\s+)?(?:IEEE\s+)?(?:Symposium\s+on\s+Security\s+and\s+Privacy|S&P|EuroS&P)(?:\s+\d{4})?(?:\s+\d+)?\s*$"
        ).unwrap()
    });

    // NDSS
    static NDSS_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)^\s*(?:\d+\s+)?(?:Network\s+and\s+Distributed\s+System\s+Security\s+Symposium|NDSS)(?:\s+\d{4})?(?:\s+\d+)?\s*$"
        ).unwrap()
    });

    // CCS
    static CCS_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)^\s*(?:\d+\s+)?(?:ACM\s+)?(?:Conference\s+on\s+Computer\s+and\s+Communications\s+Security|CCS)(?:\s+['']?\d{2,4})?(?:\s+\d+)?\s*$"
        ).unwrap()
    });

    let mut result = USENIX_HEADER.replace_all(text, "\n").to_string();
    result = USENIX_ASSOC_ONLY.replace_all(&result, "\n").to_string();
    result = IEEE_HEADER.replace_all(&result, "\n").to_string();
    result = NDSS_HEADER.replace_all(&result, "\n").to_string();
    result = CCS_HEADER.replace_all(&result, "\n").to_string();

    result
}

/// Split a references section into individual reference strings.
///
/// Tries multiple segmentation strategies in order:
/// 1. IEEE style: `[1]`, `[2]`, etc.
/// 2. Numbered list: `1.`, `2.`, etc. (sequential starting from 1)
/// 3. AAAI/ACM author-year with semicolons
/// 4. Springer/Nature: lines starting with uppercase + `(YYYY)` pattern
/// 5. Fallback: double-newline splitting
pub fn segment_references(ref_text: &str) -> Vec<String> {
    segment_references_with_config(ref_text, &PdfParsingConfig::default())
}

/// Config-aware version of [`segment_references`].
pub(crate) fn segment_references_with_config(
    ref_text: &str,
    config: &PdfParsingConfig,
) -> Vec<String> {
    // Preprocess: strip conference page headers/footers that get embedded in PDF text
    // These break pattern matching (e.g., IEEE [1], [2], [3] sequentiality)
    let ref_text = strip_page_headers(ref_text);
    let ref_text = ref_text.as_str();

    // Strategy 1: IEEE style [1], [2], ...
    if let Some(refs) = try_ieee_with_config(ref_text, config) {
        return refs;
    }

    // Strategy 2: Numbered list 1., 2., ...
    if let Some(refs) = try_numbered_with_config(ref_text, config) {
        return refs;
    }

    // Strategy 3: Author-based formats (ML full names, AAAI, NeurIPS initials)
    // Try all three and return the one that finds the most references
    {
        let ml_refs = try_ml_full_name(ref_text);
        let aaai_refs = try_aaai(ref_text);
        let neurips_refs = try_neurips(ref_text);

        let mut best: Option<Vec<String>> = None;
        for refs in [ml_refs, aaai_refs, neurips_refs].into_iter().flatten() {
            if best.as_ref().is_none_or(|b| refs.len() > b.len()) {
                best = Some(refs);
            }
        }
        if let Some(refs) = best {
            return refs;
        }
    }

    // Strategy 4: Springer/Nature (line starts with capital + has (Year))
    if let Some(refs) = try_springer_nature(ref_text) {
        return refs;
    }

    // Strategy 5: Fallback — split by double newlines
    fallback_double_newline_with_config(ref_text, config)
}

fn try_ieee_with_config(ref_text: &str, config: &PdfParsingConfig) -> Option<Vec<String>> {
    // Match [1], [2], etc. at start of string or after newline
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)(?:^|\n)\s*\[(\d+)\]\s*").unwrap());

    let re = config.ieee_segment_re.as_ref().unwrap_or(&RE);
    let matches: Vec<_> = re.find_iter(ref_text).collect();
    if matches.len() < 3 {
        return None;
    }

    // Extract the captured numbers to check sequentiality
    // This prevents matching years like [2017], [2020] in author-year citations
    let caps: Vec<_> = re.captures_iter(ref_text).collect();
    let first_nums: Vec<i64> = caps
        .iter()
        .take(5)
        .filter_map(|c| c.get(1)?.as_str().parse().ok())
        .collect();

    // First IEEE reference should be [1]
    if first_nums.is_empty() || first_nums[0] != 1 {
        return None;
    }

    // Numbers should be sequential: [1], [2], [3], ...
    let is_sequential = first_nums.windows(2).all(|w| w[1] == w[0] + 1);
    if !is_sequential {
        return None;
    }

    let mut refs = Vec::new();
    for i in 0..matches.len() {
        let start = matches[i].end();
        let end = if i + 1 < matches.len() {
            matches[i + 1].start()
        } else {
            ref_text.len()
        };
        let content = ref_text[start..end].trim();
        if !content.is_empty() {
            refs.push(content.to_string());
        }
    }
    Some(refs)
}

fn try_numbered_with_config(ref_text: &str, config: &PdfParsingConfig) -> Option<Vec<String>> {
    // Match 1-3 digit numbers only (not 4-digit years like 2018, 2024)
    // Papers rarely have 1000+ references, so this is a safe constraint
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)(?:^|\n)\s*(\d{1,3})\.\s+").unwrap());

    let re = config.numbered_segment_re.as_ref().unwrap_or(&RE);
    let matches: Vec<_> = re.find_iter(ref_text).collect();
    if matches.len() < 3 {
        return None;
    }

    // Extract the captured numbers to check sequentiality
    // When using a custom regex, we still need capture group 1 for numbers
    let caps: Vec<_> = re.captures_iter(ref_text).collect();
    let first_nums: Vec<i64> = caps
        .iter()
        .take(5)
        .filter_map(|c| c.get(1)?.as_str().parse().ok())
        .collect();

    if first_nums.is_empty() || first_nums[0] != 1 {
        return None;
    }

    let is_sequential = first_nums.windows(2).all(|w| w[1] == w[0] + 1);

    if !is_sequential {
        return None;
    }

    let mut refs = Vec::new();
    for i in 0..matches.len() {
        let start = matches[i].end();
        let end = if i + 1 < matches.len() {
            matches[i + 1].start()
        } else {
            ref_text.len()
        };
        let content = ref_text[start..end].trim();
        if !content.is_empty() {
            refs.push(content.to_string());
        }
    }
    Some(refs)
}

fn try_aaai(ref_text: &str) -> Option<Vec<String>> {
    // Surname chars: ASCII letters + common diacritics (Latin Extended)
    let sc = r"[a-zA-Z\u{00C0}-\u{024F}\u{00E4}\u{00F6}\u{00FC}\u{00DF}\u{00E8}\u{00E9}]";

    // AAAI pattern: end of previous ref (lowercase/digit/paren/CAPS/slash). + newline
    // + optional page number line + Surname, I. (next ref start)
    // Rust regex doesn't support look-ahead, so we match without (?!In\s) and filter in code
    let re_pattern = format!(
        r"([a-z0-9)/]|[A-Z]{{2}})\.\n(?:\d{{1,4}}\n)?\s*({}{}+(?:[ \-]{}+)?,\s+[A-Z]\.)",
        r"[A-Z\u{00C0}-\u{024F}]", sc, sc,
    );
    let re = Regex::new(&re_pattern).unwrap();

    // Secondary pattern for organization/non-standard authors: any text followed by ". Year."
    // Uses lazy matching to find the shortest author block before a year.
    // Handles: lowercase orgs (noyb), orgs with digits (FORCE11), dashes, etc.
    let org_re = Regex::new(
        r"([a-z0-9)/]|[A-Z]{2})\.\n(?:\d{1,4}\n)?\s*(.{2,200}?\.\s+(?:19|20)\d{2}[a-z]?\.)",
    )
    .unwrap();

    // Collect boundary matches from both patterns
    struct Boundary {
        prefix_end: usize,
        ref_start: usize,
    }

    let mut boundaries: Vec<Boundary> = Vec::new();

    // Primary pattern matches (Surname, I.)
    for caps in re.captures_iter(ref_text) {
        let surname = caps.get(2).unwrap().as_str();
        if surname.starts_with("In ") {
            continue;
        }
        boundaries.push(Boundary {
            prefix_end: caps.get(1).unwrap().end(),
            ref_start: caps.get(2).unwrap().start(),
        });
    }

    // Organization / general year-based boundary matches
    for caps in org_re.captures_iter(ref_text) {
        let author_block = caps.get(2).unwrap().as_str();
        // Skip venue-like patterns (not author names)
        if author_block.starts_with("In ") || author_block.starts_with("in ") {
            continue;
        }
        boundaries.push(Boundary {
            prefix_end: caps.get(1).unwrap().end(),
            ref_start: caps.get(2).unwrap().start(),
        });
    }

    // Sort by position and deduplicate overlapping boundaries
    boundaries.sort_by_key(|b| b.ref_start);
    boundaries.dedup_by(|a, b| {
        // If two boundaries overlap (ref_start within 10 chars), keep the earlier one
        (a.ref_start as isize - b.ref_start as isize).unsigned_abs() < 10
    });

    if boundaries.len() < 3 {
        return None;
    }

    let mut refs = Vec::new();

    // First reference: everything before the first boundary
    let first_ref = ref_text[..boundaries[0].prefix_end].trim();
    if !first_ref.is_empty() && first_ref.len() > 20 {
        refs.push(first_ref.to_string());
    }

    // Remaining references
    for i in 0..boundaries.len() {
        let start = boundaries[i].ref_start;
        let end = if i + 1 < boundaries.len() {
            boundaries[i + 1].prefix_end
        } else {
            ref_text.len()
        };
        let content = ref_text[start..end].trim();
        if !content.is_empty() {
            refs.push(content.to_string());
        }
    }
    Some(refs)
}

fn try_neurips(ref_text: &str) -> Option<Vec<String>> {
    // NeurIPS/ML format: "I. Surname and I. Surname. Title. Venue, Year."
    // Split at ". \n I. Surname" boundaries
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(\.\s*)\n+([A-Z]\.(?:\s*[A-Z]\.)?\s+[A-Z][a-zA-Z\u{00C0}-\u{024F}\-]+(?:\s+and\s+[A-Z]\.|,\s+[A-Z]\.))",
        )
        .unwrap()
    });

    let matches: Vec<_> = RE.find_iter(ref_text).collect();
    if matches.len() < 5 {
        return None;
    }

    let mut refs = Vec::new();

    // First reference: everything before the first boundary
    let first_end = matches[0].start()
        + RE.captures(&ref_text[matches[0].start()..])
            .and_then(|c| c.get(1))
            .map(|m| m.end())
            .unwrap_or(0);
    let first_ref = ref_text[..first_end].trim();
    if !first_ref.is_empty() && first_ref.len() > 20 {
        refs.push(first_ref.to_string());
    }

    // Each subsequent reference starts at the second capture group
    for i in 0..matches.len() {
        let caps = RE.captures(&ref_text[matches[i].start()..]).unwrap();
        let ref_start = matches[i].start() + caps.get(2).unwrap().start();
        let ref_end = if i + 1 < matches.len() {
            let next_caps = RE.captures(&ref_text[matches[i + 1].start()..]).unwrap();
            matches[i + 1].start() + next_caps.get(1).unwrap().end()
        } else {
            ref_text.len()
        };
        let content = ref_text[ref_start..ref_end].trim();
        if !content.is_empty() {
            refs.push(content.to_string());
        }
    }

    Some(refs)
}

fn try_ml_full_name(ref_text: &str) -> Option<Vec<String>> {
    // ML papers with full author names or initials
    // Boundaries: year/URL end + period + newline + author name pattern
    static RE: Lazy<Regex> = Lazy::new(|| {
        // Pattern handles four author formats:
        // 1. Full names: "Eva E Stüeken," / "William E Schiesser." / "Randall J. LeVeque."
        // 2. Initials with periods: "E. Pardoux," / "E. Pardoux and A." / "V. V. Jikov,"
        // 3. Abbreviated initials: "MM Locarnini," / "HE Garcia," (2-3 caps without periods)
        // 4. ALL CAPS first name: "PHILIPPE Courtier," (4+ caps followed by mixed-case surname)
        //
        // Terminators: comma (multi-author), period (single-author/end), " and" (co-author)
        Regex::new(
            r"((?:(?:19|20)\d{2}[a-z]?|html|pdf)\.\n+)((?:[A-Z][a-z]+(?:\s+[A-Z](?:\.|[a-z]+)?)?\s+[A-Z][a-zA-Z\u{00C0}-\u{024F}\-]+|[A-Z]\.(?:\s*[A-Z]\.)?\s+[A-Z][a-zA-Z\u{00C0}-\u{024F}\-]+|[A-Z]{2,3}\s+[A-Z][a-zA-Z\u{00C0}-\u{024F}\-]+|[A-Z]{4,}\s+[A-Z][a-z][a-zA-Z\u{00C0}-\u{024F}\-]*)(?:[,.]| and ))",
        )
        .unwrap()
    });

    let matches: Vec<_> = RE.find_iter(ref_text).collect();
    if matches.len() < 5 {
        return None;
    }

    let mut refs = Vec::new();

    // First reference: everything before the first boundary
    let first_end = matches[0].start()
        + RE.captures(&ref_text[matches[0].start()..])
            .and_then(|c| c.get(1))
            .map(|m| m.end())
            .unwrap_or(0);
    let first_ref = ref_text[..first_end].trim();
    if !first_ref.is_empty() && first_ref.len() > 20 {
        refs.push(first_ref.to_string());
    }

    // Each subsequent reference starts at the second capture group (author name)
    for i in 0..matches.len() {
        let caps = RE.captures(&ref_text[matches[i].start()..]).unwrap();
        let ref_start = matches[i].start() + caps.get(2).unwrap().start();
        let ref_end = if i + 1 < matches.len() {
            let next_caps = RE.captures(&ref_text[matches[i + 1].start()..]).unwrap();
            matches[i + 1].start() + next_caps.get(1).unwrap().end()
        } else {
            ref_text.len()
        };
        let content = ref_text[ref_start..ref_end].trim();
        if !content.is_empty() {
            refs.push(content.to_string());
        }
    }

    Some(refs)
}

fn try_springer_nature(ref_text: &str) -> Option<Vec<String>> {
    static LINE_START_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Z]").unwrap());
    static PURE_NUMBER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+$").unwrap());
    static YEAR_PAREN_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(\d{4}[a-z]?\)").unwrap());
    static TRAILING_PAGENUM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n+\d+\s*$").unwrap());

    let lines: Vec<&str> = ref_text.split('\n').collect();
    let mut ref_starts = Vec::new();
    let mut current_pos: usize = 0;

    for line in &lines {
        let trimmed = line.trim();
        if !line.is_empty()
            && LINE_START_RE.is_match(line)
            && !PURE_NUMBER_RE.is_match(trimmed)
            && YEAR_PAREN_RE.is_match(line)
        {
            ref_starts.push(current_pos);
        }
        current_pos += line.len() + 1; // +1 for newline
    }

    if ref_starts.len() < 5 {
        return None;
    }

    let mut refs = Vec::new();
    for i in 0..ref_starts.len() {
        let start = ref_starts[i];
        let end = if i + 1 < ref_starts.len() {
            ref_starts[i + 1]
        } else {
            ref_text.len()
        };
        let content = &ref_text[start..end];
        // Remove trailing page number
        let content = TRAILING_PAGENUM_RE.replace(content, "");
        let content = content.trim();
        if !content.is_empty() && content.len() > 20 {
            refs.push(content.to_string());
        }
    }
    Some(refs)
}

fn fallback_double_newline_with_config(ref_text: &str, config: &PdfParsingConfig) -> Vec<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\s*\n").unwrap());

    let re = config.fallback_segment_re.as_ref().unwrap_or(&RE);
    re.split(ref_text)
        .map(|p| p.trim())
        .filter(|p| !p.is_empty() && p.len() > 20)
        .map(|p| p.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_references_section_basic() {
        let text = "Some content here.\n\nReferences\n\n[1] First ref.\n[2] Second ref.\n";
        let section = find_references_section(text).unwrap();
        assert!(section.contains("[1] First ref."));
        assert!(section.contains("[2] Second ref."));
    }

    #[test]
    fn test_find_references_section_with_appendix() {
        let text = "Body.\n\nReferences\n\n[1] Ref one.\n\nAppendix A\n\nExtra stuff.";
        let section = find_references_section(text).unwrap();
        assert!(section.contains("[1] Ref one."));
        assert!(!section.contains("Extra stuff"));
    }

    #[test]
    fn test_segment_ieee() {
        let text = "\n[1] First reference text here.\n[2] Second reference text here.\n[3] Third reference.\n";
        let refs = segment_references(text);
        assert_eq!(refs.len(), 3);
        assert!(refs[0].starts_with("First"));
        assert!(refs[1].starts_with("Second"));
    }

    #[test]
    fn test_segment_numbered() {
        let text = "1. First ref content here that is long enough.\n2. Second ref content here that is long enough.\n3. Third ref content.\n4. Fourth ref.\n5. Fifth ref.\n";
        let refs = segment_references(text);
        assert!(refs.len() >= 3);
        assert!(refs[0].starts_with("First"));
    }

    #[test]
    fn test_segment_fallback() {
        let text = "This is a long enough reference paragraph one.\n\nThis is a long enough reference paragraph two.\n\nShort.\n\nThis is a long enough reference paragraph three.";
        let refs = segment_references(text);
        assert_eq!(refs.len(), 3); // "Short." is filtered out (len <= 20)
    }

    #[test]
    fn test_find_references_bibliography() {
        let text = "Body.\n\nBibliography\n\nSome refs here.\n";
        let section = find_references_section(text).unwrap();
        assert!(section.contains("Some refs here."));
    }

    // ── Config-aware tests ──

    #[test]
    fn test_find_section_custom_header_re() {
        let config = crate::PdfParsingConfigBuilder::new()
            .section_header_regex(r"(?i)\n\s*Literatur\s*\n")
            .build()
            .unwrap();
        let text = "Body.\n\nLiteratur\n\nRef A.\nRef B.\n";
        let section = find_references_section_with_config(text, &config).unwrap();
        assert!(section.contains("Ref A."));
    }

    #[test]
    fn test_find_section_custom_end_re() {
        let config = crate::PdfParsingConfigBuilder::new()
            .section_end_regex(r"(?i)\n\s*Anhang")
            .build()
            .unwrap();
        let text = "Body.\n\nReferences\n\nRef one.\n\nAnhang\n\nExtra.";
        let section = find_references_section_with_config(text, &config).unwrap();
        assert!(section.contains("Ref one."));
        assert!(!section.contains("Extra"));
    }

    #[test]
    fn test_find_section_custom_fallback_fraction() {
        let config = crate::PdfParsingConfigBuilder::new()
            .fallback_fraction(0.5)
            .build()
            .unwrap();
        // No header → fallback returns last 50%
        let text = "AAAA BBBB CCCC DDDD";
        let section = find_references_section_with_config(text, &config).unwrap();
        // Should get roughly the last half
        assert!(section.len() <= text.len() / 2 + 2);
    }

    #[test]
    fn test_segment_custom_ieee_regex() {
        let config = crate::PdfParsingConfigBuilder::new()
            .ieee_segment_regex(r"\n\s*<<(\d+)>>\s*")
            .build()
            .unwrap();
        let text = "\n<<1>> First ref text.\n<<2>> Second ref text.\n<<3>> Third ref.\n";
        let refs = segment_references_with_config(text, &config);
        assert_eq!(refs.len(), 3);
        assert!(refs[0].starts_with("First"));
    }

    #[test]
    fn test_segment_aaai_basic() {
        // Standard AAAI format: "Surname, I. Year. Title. Venue."
        let text = concat!(
            "Adams, B.; and Clark, D. 2019. First Paper With a Long Title.\n",
            "In Proceedings of CHI. Glasgow, UK.\n",
            "Baker, E. 2020. Second Paper With Another Long Title.\n",
            "In Proceedings of CSCW. Virtual.\n",
            "Carter, F.; and Davis, G. 2021. Third Paper About Something.\n",
            "In Proceedings of USENIX. Boston.\n",
            "Evans, H. 2022. Fourth Paper On Some Topic Here.\n",
            "In Proceedings of NeurIPS. New Orleans.\n",
        );
        let refs = segment_references(text);
        assert!(
            refs.len() >= 4,
            "Expected >= 4 refs, got {}: {:?}",
            refs.len(),
            refs
        );
        assert!(refs[0].contains("Adams"));
        assert!(refs[1].contains("Baker"));
        assert!(refs[2].contains("Carter"));
        assert!(refs[3].contains("Evans"));
    }

    #[test]
    fn test_segment_aaai_in_venue_not_boundary() {
        // "In Proceedings..." after a period-newline should NOT be treated as a boundary.
        // This tests that the "In " exclusion filter works.
        let text = concat!(
            "Adams, B. 2019. First Paper With a Long Title.\n",
            "In Proceedings of CHI. Glasgow.\n",
            "Baker, E. 2020. Second Paper With a Long Title.\n",
            "In Proceedings of CSCW. Virtual.\n",
            "Carter, F. 2021. Third Paper About Something.\n",
            "In Proceedings of USENIX. Boston.\n",
            "Davis, G. 2022. Fourth Paper On Some Topic.\n",
            "In Proceedings of NeurIPS. New Orleans.\n",
        );
        let refs = segment_references(text);
        // Should be 4 refs, not 8 (venues should not split)
        assert_eq!(
            refs.len(),
            4,
            "Venues should not create false boundaries: {:?}",
            refs
        );
    }

    #[test]
    fn test_segment_aaai_org_with_digits() {
        // FORCE11 has digits — should be detected as a boundary
        let text = concat!(
            "Smith, J.; and Jones, K. 2020. Some Long Title About Neural Networks.\n",
            "In Proceedings of ICML. Montreal, Canada.\n",
            "Taylor, R. 2019. Another Paper Title That Is Long Enough.\n",
            "In Conference on AI. New York.\n",
            "FORCE11. 2020. The FAIR Data Principles and Guidelines.\n",
            "https://force11.org/info/the-fair-data-principles/.\n",
            "Wilson, M.; and Brown, A. 2021. Yet Another Paper With a Title.\n",
            "In NeurIPS. Virtual.\n",
        );
        let refs = segment_references(text);
        // Should have 4 refs, not 3 (FORCE11 should be separate)
        assert!(
            refs.len() >= 4,
            "Expected >= 4 refs, got {}: {:?}",
            refs.len(),
            refs
        );
        assert!(refs.iter().any(|r| r.contains("FORCE11")));
    }

    #[test]
    fn test_segment_aaai_lowercase_org() {
        // noyb starts with lowercase — should be detected as a boundary
        let text = concat!(
            "Adams, B.; and Clark, D. 2019. First Paper Long Enough Title.\n",
            "In Proceedings of CHI. Glasgow.\n",
            "Baker, E. 2020. Second Paper With A Long Enough Title.\n",
            "In Proceedings of CSCW. Virtual.\n",
            "noyb \u{2013} European Center for Digital Rights. 2024. Consent Banner Report.\n",
            "https://noyb.eu/.\n",
            "Davis, F. 2021. Third Paper That Has A Long Title.\n",
            "In Proceedings of USENIX. Boston.\n",
        );
        let refs = segment_references(text);
        assert!(
            refs.len() >= 4,
            "Expected >= 4 refs, got {}: {:?}",
            refs.len(),
            refs
        );
        assert!(refs.iter().any(|r| r.contains("noyb")));
    }

    #[test]
    fn test_segment_aaai_url_slash_boundary() {
        // URL ending with / before next author — slash should be valid boundary char
        let text = concat!(
            "Adams, B. 2018. First Paper About Something Important.\n",
            "In Proceedings of AAAI. New Orleans.\n",
            "Baker, E. 2019. Second Paper With Details and More.\n",
            "In Conference on NLP. Florence.\n",
            "Clark, D. 2020. Third Paper With URL at End.\n",
            "https://example.org/paper/.\n",
            "Davis, F. 2021. Fourth Paper After URL Reference.\n",
            "In Proceedings of ACL. Dublin.\n",
        );
        let refs = segment_references(text);
        assert!(
            refs.len() >= 4,
            "Expected >= 4 refs, got {}: {:?}",
            refs.len(),
            refs
        );
    }

    #[test]
    fn test_segment_custom_fallback_regex() {
        let config = crate::PdfParsingConfigBuilder::new()
            .fallback_segment_regex(r"---+")
            .build()
            .unwrap();
        let text = "First long enough reference text here.---Second long enough reference text here.---Third long enough reference text.";
        // None of the numbered strategies will match, so fallback fires
        let refs = segment_references_with_config(text, &config);
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn test_segment_ieee_with_usenix_page_header() {
        // Simulates USENIX paper where page header appears between references
        // The header spans two lines: "USENIX Association" and "34th USENIX Security Symposium 2477"
        let text = concat!(
            "[1] First reference with a long enough title here.\n",
            "[2] Second reference also with sufficient content.\n",
            "[3] Third reference ends before page break.\n",
            "USENIX Association\n",
            "34th USENIX Security Symposium    2477\n",
            "[4] Fourth reference starts new page content.\n",
            "[5] Fifth reference continues normally with text.\n",
            "[6] Sixth reference completes the section here.\n",
        );
        let refs = segment_references(text);
        assert_eq!(
            refs.len(),
            6,
            "Should find 6 IEEE refs after stripping header: {:?}",
            refs
        );
        assert!(refs[0].contains("First reference"));
        assert!(refs[3].contains("Fourth reference"));
        assert!(!refs.iter().any(|r| r.contains("USENIX Association")));
    }

    #[test]
    fn test_strip_page_headers_usenix() {
        let text = "some text before\nUSENIX Association\n34th USENIX Security Symposium    2477\nsome text after";
        let stripped = strip_page_headers(text);
        assert!(
            !stripped.contains("USENIX Association"),
            "Should strip USENIX Association: {}",
            stripped
        );
        assert!(
            !stripped.contains("Security Symposium"),
            "Should strip USENIX Security Symposium: {}",
            stripped
        );
        assert!(stripped.contains("some text before"));
        assert!(stripped.contains("some text after"));
    }

    #[test]
    fn test_find_references_uses_last_header() {
        // Some papers have multiple "References" headers (e.g., table headers like
        // "Table 2: References to related work") before the actual reference list.
        // We should use the LAST occurrence.
        let text = concat!(
            "Table 2: Classification\n\nReferences\n\n",
            "Type Variants Post Quantum...\n\n",
            "5 Conclusion\n\nReferences\n\n",
            "[1] First real reference here.\n",
            "[2] Second real reference here.\n",
        );
        let section = find_references_section(text).unwrap();
        // Should contain the actual references, not the table content
        assert!(section.contains("[1] First real reference"), "Section: {}", section);
        assert!(!section.contains("Classification"), "Should not contain table content");
    }
}
