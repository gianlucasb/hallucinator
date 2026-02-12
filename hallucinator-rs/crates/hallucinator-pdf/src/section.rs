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

    if let Some(m) = header_re.find(text) {
        let ref_start = m.end();
        let rest = &text[ref_start..];

        static END_RE: Lazy<Regex> = Lazy::new(|| {
            Regex::new(r"(?i)\n\s*(?:Appendix|Acknowledgments|Acknowledgements|Supplementary|Ethics\s+Statement|Ethical\s+Considerations|Broader\s+Impact|Paper\s+Checklist|Checklist)")
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
    // Strategy 1: IEEE style [1], [2], ...
    if let Some(refs) = try_ieee_with_config(ref_text, config) {
        return refs;
    }

    // Strategy 2: Numbered list 1., 2., ...
    if let Some(refs) = try_numbered_with_config(ref_text, config) {
        return refs;
    }

    // Strategy 3: AAAI/ACM author-year (period + newline + Surname, I.)
    if let Some(refs) = try_aaai(ref_text) {
        return refs;
    }

    // Strategy 4: Springer/Nature (line starts with capital + has (Year))
    if let Some(refs) = try_springer_nature(ref_text) {
        return refs;
    }

    // Strategy 5: Fallback — split by double newlines
    fallback_double_newline_with_config(ref_text, config)
}

fn try_ieee_with_config(ref_text: &str, config: &PdfParsingConfig) -> Option<Vec<String>> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\s*\[(\d+)\]\s*").unwrap());

    let re = config.ieee_segment_re.as_ref().unwrap_or(&RE);
    let matches: Vec<_> = re.find_iter(ref_text).collect();
    if matches.len() < 3 {
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
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)(?:^|\n)\s*(\d+)\.\s+").unwrap());

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

    // AAAI pattern: end of previous ref (lowercase/digit/paren/CAPS). + newline
    // + optional page number line + Surname, I. (next ref start)
    // Rust regex doesn't support look-ahead, so we match without (?!In\s) and filter in code
    let re_pattern = format!(
        r"([a-z0-9)]|[A-Z]{{2}})\.\n(?:\d{{1,4}}\n)?\s*({}{}+(?:[ \-]{}+)?,\s+[A-Z]\.)",
        r"[A-Z\u{00C0}-\u{024F}]", sc, sc,
    );
    let re = Regex::new(&re_pattern).unwrap();

    // Secondary pattern for organization authors: "OrgName. Year." on its own boundary
    // e.g., "European Union. 2022a." or "World Health Organization. 2021."
    let org_re = Regex::new(
        r"([a-z0-9)]|[A-Z]{2})\.\n(?:\d{1,4}\n)?\s*([A-Z][a-zA-Z]+(?:\s+[A-Z]?[a-zA-Z]+)+\.\s+(?:19|20)\d{2}[a-z]?\.)",
    ).unwrap();

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

    // Organization pattern matches
    for caps in org_re.captures_iter(ref_text) {
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
}
