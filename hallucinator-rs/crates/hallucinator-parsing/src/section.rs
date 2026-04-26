use once_cell::sync::Lazy;
use regex::Regex;

use crate::config::ParsingConfig;

/// Segmentation strategy identifier for scoring and debugging
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SegmentationStrategy {
    Ieee,
    Numbered,
    Aaai,
    Neurips,
    MlFullName,
    SpringerNature,
    Fallback,
}

impl SegmentationStrategy {
    /// Specificity score: explicit structural markers score higher
    pub fn specificity_score(&self) -> f64 {
        match self {
            Self::Ieee => 1.0,
            Self::Numbered => 0.95,
            Self::Aaai => 0.8,
            Self::Neurips => 0.8,
            Self::MlFullName => 0.8,
            Self::SpringerNature => 0.75,
            Self::Fallback => 0.3,
        }
    }
}

/// Result of a single segmentation strategy attempt
#[derive(Debug, Clone)]
pub struct SegmentationResult {
    pub strategy: SegmentationStrategy,
    pub references: Vec<String>,
}

/// Locate the references section in the document text.
///
/// Searches for common reference section headers (References, Bibliography, Works Cited)
/// and returns the text between the header and any end markers (Appendix, Acknowledgments, etc.).
/// Falls back to the last 30% of the document if no header is found.
pub fn find_references_section(text: &str) -> Option<String> {
    find_references_section_with_config(text, &ParsingConfig::default())
}

/// Config-aware version of [`find_references_section`].
pub(crate) fn find_references_section_with_config(
    text: &str,
    config: &ParsingConfig,
) -> Option<String> {
    static HEADER_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\n\s*(?:References|Bibliography|Works\s+Cited)\s*\n").unwrap()
    });

    let header_re = config.section_header_re.as_ref().unwrap_or(&HEADER_RE);

    // When multiple header matches exist (e.g. "Bibliography" appears as both
    // a section heading AND as running headers on subsequent pages), pick the
    // match whose subsequent text contains the most bracketed reference markers
    // like [1], [2], etc. This prefers the real section heading (which captures
    // all references) over running headers (which only capture a tail).
    //
    // If no match has bracketed markers, fall back to the last match (handles
    // the case where "References" appears in table headers before the real list).
    static BRACKET_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\s*\[\d+\]").unwrap());

    let matches: Vec<_> = header_re.find_iter(text).collect();
    let best = if matches.len() > 1 {
        matches.iter().max_by_key(|m| {
            let rest = &text[m.end()..];
            BRACKET_RE.find_iter(rest).count()
        })
    } else {
        matches.last()
    };
    if let Some(m) = best {
        let ref_start = m.end();
        let rest = &text[ref_start..];

        static END_RE: Lazy<Regex> = Lazy::new(|| {
            // Match common end-of-references markers:
            // - Explicit section headers: Appendix, Acknowledgments, etc.
            // - Single-letter appendix sections: "A\nAppendix", "A\nTechnical Lemmas" (common in NeurIPS)
            // - Conference checklists: "NeurIPS Paper Checklist", "ICML Checklist", etc.
            // - Mathematical proof sections with equation numbers
            //
            // IMPORTANT: "Appendix" must be followed by whitespace, letter/number, or end-of-line.
            // NOT followed by a colon (e.g., "Artifact Appendix: Title" in a reference).
            Regex::new(concat!(
                r"(?i)\n\s*(?:",
                // Explicit "Appendix" header (not followed by colon to avoid matching reference titles)
                r"Appendix(?:\s+[A-Z0-9]|\s*\n|\s*$)|",
                // Common post-bibliography section headers
                r"Acknowledgments|Acknowledgements|Supplementary|",
                r"Ethics\s+Statement|Ethical\s+Considerations|Broader\s+Impact|",
                // Conference checklists
                r"(?:\w+\s+)?(?:Paper\s+)?Checklist|",
                // Single-letter appendix sections: "A\nTechnical Lemmas" (NeurIPS style)
                r"[A-Z]\n\s*(?:Appendix|Technical|Proofs?|Additional|Extended|Experimental|",
                r"Derivations?|Algorithms?|Detailed?|Implementation|Analysis|Benchmark|",
                r"Datasets?|Ablation|Hyperparameters?|Prompt|Annotation|Evaluation|",
                r"Training|Baseline|Reproducibility|Limitations?|Discussion|",
                r"Examples?|Supplementary|Survey|Questionnaire|Full|Further|",
                r"Post-Processing|Human|Category|Scoring|Results)|",
                // "A.\n\nTitle" pattern (ACM/LREC appendix with period after letter)
                r"[A-Z]\.\s*\n\s*(?:Prompt|Annotation|Evaluation|Training|Baseline|",
                r"Full|Further|Additional|Detailed?|Post-Processing|Human|Category|",
                r"Scoring|Results|Supplementary|Survey|Questionnaire|Examples?|",
                r"Reproducibility|Implementation|Limitations?|Discussion|",
                r"Proof|Derivation|Algorithm|Benchmark|Dataset|Ablation|",
                r"Hyperparameter|Extended|Experimental|Analysis|Error)|",
                // "A.1", "B.2" numbered appendix sub-sections on their own line
                r"[A-Z]\.\d+\s*\n",
                r")",
            ))
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
    static USENIX_ASSOC_ONLY: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)^\s*USENIX\s+Association\s*$").unwrap());

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

    // ACM conference headers: "CONF 'YY, Month DD–DD, YYYY, City, Country"
    // Examples: "ASIA CCS '26, June 01–05, 2026, Bangalore, India"
    //           "CHI '24, May 11-16, 2024, Honolulu, HI, USA"
    static ACM_CONF_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)(?:ASIA\s+CCS|CHI|UIST|CSCW|MobiSys|MobiCom|SenSys|UbiComp|IMC|SIGCOMM|SOSP|OSDI|PLDI|POPL|ICSE|FSE|ASE|ISSTA|WWW|KDD|SIGIR|SIGMOD|VLDB|ICML|NeurIPS|ICLR|CVPR|ICCV|ECCV|ACL|EMNLP|NAACL)\s*['']?\d{2}(?:,\s*(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+\d{1,2}[–-]\d{1,2},\s*\d{4})?,\s*[A-Za-z\s,.]+"
        ).unwrap()
    });

    // ACM author/affiliation header that sometimes appears: "Author Name, Affiliation"
    // Pattern: "O.A Akanji, M. Egele, and G. StringhiniASIA CCS"
    // This handles concatenated author names with conference names
    static ACM_AUTHOR_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)[A-Z]\.\s*[A-Za-z]+(?:,\s*[A-Z]\.\s*[A-Za-z]+)*(?:,\s*and\s+[A-Z]\.\s*[A-Za-z]+)?(?:ASIA\s+CCS|CHI|WWW|CCS)"
        ).unwrap()
    });

    // Thesis/book running headers: "Bibliography" or "References" repeated at the
    // top of each page, adjacent to a page number. Two orderings:
    //   "84\nBIBLIOGRAPHY\n"  (page number before header)
    //   "BIBLIOGRAPHY\n85\n"  (header before page number)
    static THESIS_RUNNING_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?im)(?:\n\s*\d{1,4}\s*\n\s*(?:bibliography|references)\s*\n|\n\s*(?:bibliography|references)\s*\n\s*\d{1,4}\s*\n)"
        ).unwrap()
    });

    // ACM acmart template placeholder running header. When authors don't fill in
    // \acmConference, the default placeholder appears at the top of every page:
    //   "[Paper short title]
    //    Conference'17, July 2017, Washington, DC, USA"
    // Both lines must be stripped (the line above "Conference'YY" is the paper
    // title running header, paired with the conference info in acmart format).
    // Also handles real conference info with day ranges (e.g., "July 14-18, 2017").
    static ACM_PLACEHOLDER_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?m)\n[^\n]+\n\s*Conference\s*['\u{2018}\u{2019}]?\d{2,4},\s*\w+\s+(?:\d{1,2}(?:[-–]\d{1,2})?,\s*)?\d{4},\s*[^\n]+"
        ).unwrap()
    });

    // PoPETs (Proceedings on Privacy Enhancing Technologies) running headers.
    //
    // PoPETs uses two running-header formats, one per page side:
    //
    //   Even page (left):  "Proceedings on Privacy Enhancing Technologies 2026(1)"
    //                      (journal name + year/issue, no title, no page number)
    //
    //   Odd page (right):  "QUICstep: Evaluating … circumvention
    //                       Proceedings on Privacy Enhancing Technologies 2026(1)"
    //                      (paper title preceding the journal name + year/issue)
    //
    // The KEY distinguishing marker is that the year/issue follows the journal
    // name with NO comma — i.e.:
    //   Header:   "Proceedings on Privacy Enhancing Technologies 2026(1)"
    //   Citation: "Proceedings on Privacy Enhancing Technologies, 2017(4)"
    //                                                           ^--- comma
    //
    // Pattern: optionally consume a preceding capital-letter title fragment,
    // then match the journal name followed by year(issue) with whitespace (not comma).
    //
    // `[^\[.]*?` allows the prefix to span newlines (for wrapped titles) while
    // still stopping at:
    //   - `.` (periods)  → prevents crossing sentence boundaries in citation text
    //   - `[` (brackets) → prevents matching across IEEE reference markers like [1]
    //
    // This correctly handles:
    //   • Same-line:   "QUICstep: … circumvention Proceedings on Privacy … 2026(1)"
    //   • Multi-line:  "QUICstep: … circumvention\nProceedings on Privacy … 2026(1)"
    //   • Even-page:   "Proceedings on Privacy Enhancing Technologies 2026(1)"  (no prefix)
    static POPETS_HEADER: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?:[A-Z][^\[.]*?)?Proceedings on Privacy Enhancing Technologies\s+\d{4}\(\d+\)",
        )
        .unwrap()
    });

    let mut result = USENIX_HEADER.replace_all(text, "\n").to_string();
    result = USENIX_ASSOC_ONLY.replace_all(&result, "\n").to_string();
    result = IEEE_HEADER.replace_all(&result, "\n").to_string();
    result = NDSS_HEADER.replace_all(&result, "\n").to_string();
    result = CCS_HEADER.replace_all(&result, "\n").to_string();
    result = ACM_CONF_HEADER.replace_all(&result, "\n").to_string();
    result = ACM_AUTHOR_HEADER.replace_all(&result, "\n").to_string();
    result = THESIS_RUNNING_HEADER.replace_all(&result, "\n").to_string();
    result = ACM_PLACEHOLDER_HEADER
        .replace_all(&result, "\n")
        .to_string();
    result = POPETS_HEADER.replace_all(&result, "\n").to_string();

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
    segment_references_with_config(ref_text, &ParsingConfig::default())
}

/// Run all segmentation strategies and return all valid results.
///
/// This function is used by the scoring system to evaluate all possible
/// segmentation approaches before selecting the best one.
pub fn segment_references_all_strategies(
    ref_text: &str,
    config: &ParsingConfig,
) -> Vec<SegmentationResult> {
    let ref_text = strip_page_headers(ref_text);
    let ref_text = ref_text.as_str();
    let mut results = Vec::new();

    // Strategy 1: IEEE
    if let Some(refs) = try_ieee_with_config(ref_text, config) {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::Ieee,
            references: refs,
        });
    }

    // Strategy 2: Numbered
    if let Some(refs) = try_numbered_with_config(ref_text, config) {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::Numbered,
            references: refs,
        });
    }

    // Strategy 3a: AAAI
    if let Some(refs) = try_aaai(ref_text) {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::Aaai,
            references: refs,
        });
    }

    // Strategy 3b: NeurIPS
    if let Some(refs) = try_neurips(ref_text) {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::Neurips,
            references: refs,
        });
    }

    // Strategy 3c: ML Full Name
    if let Some(refs) = try_ml_full_name(ref_text) {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::MlFullName,
            references: refs,
        });
    }

    // Strategy 4: Springer/Nature
    if let Some(refs) = try_springer_nature(ref_text) {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::SpringerNature,
            references: refs,
        });
    }

    // Strategy 5: Fallback (always succeeds)
    let fallback_refs = fallback_double_newline_with_config(ref_text, config);
    if !fallback_refs.is_empty() {
        results.push(SegmentationResult {
            strategy: SegmentationStrategy::Fallback,
            references: fallback_refs,
        });
    }

    results
}

/// Config-aware version of [`segment_references`].
///
/// Uses a scoring-based approach to select the best segmentation strategy.
/// All strategies are run and scored based on quality metrics (coverage,
/// completeness, consistency, specificity). The highest-scoring result is
/// returned.
pub(crate) fn segment_references_with_config(
    ref_text: &str,
    config: &ParsingConfig,
) -> Vec<String> {
    use crate::scoring::select_best_segmentation;

    let all_results = segment_references_all_strategies(ref_text, config);

    if all_results.is_empty() {
        return Vec::new();
    }

    // Use configured weights or defaults
    let weights = config.scoring_weights();

    // Strip headers for scoring (same preprocessing as in segment_references_all_strategies)
    let preprocessed = strip_page_headers(ref_text);

    let chosen = select_best_segmentation(all_results, &preprocessed, config, &weights)
        .map(|r| r.references)
        .unwrap_or_default();
    chosen
        .into_iter()
        .filter(|s| looks_like_reference(s))
        .collect()
}

/// Decide whether a segment produced by the segmenters is plausibly an
/// academic reference. Drops body text that snuck into the references
/// section because the section-end detector (`END_RE` above) missed an
/// appendix header and the segmenter dutifully chunked the appendix
/// prose into pseudo-references.
///
/// Two cheap signals — both required:
/// 1. **Punctuation prefix**: real refs always start with a letter,
///    digit, bullet, em-dash, or quote — never a comma, semicolon,
///    period, or bang. Segments leading with those are column-break
///    artifacts (e.g., a continuation line that got split off when
///    the segmenter saw a stray period before it).
/// 2. **Academic marker**: any ref ≥100 chars almost always carries a
///    year, DOI, arXiv ID, URL, or recognisable venue keyword. Body
///    paragraphs from appendix sections — theorems, hybrid games,
///    figure captions, experimental setups — rarely do. Short
///    segments are exempt so legitimate URL-only refs ("guac.
///    https://guac.sh/") and short standards ("RFC 5246") still pass.
fn looks_like_reference(seg: &str) -> bool {
    let trimmed = seg.trim();
    let Some(first) = trimmed.chars().next() else {
        return false;
    };

    // Punctuation-prefix guard
    if matches!(first, ',' | ';' | '!' | '.') {
        return false;
    }

    // Length-gated academic-marker check
    if trimmed.chars().count() < 100 {
        return true;
    }

    static ACADEMIC_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(concat!(
            // 4-digit year (1900-2099)
            r"\b(?:19|20)\d{2}\b|",
            // 2-digit year preceded by an apostrophe (EUROCRYPT '24)
            r"['\u{2018}\u{2019}]\d{2}\b|",
            // DOI prefix or doi.org URL
            r"(?i)\b(?:doi\.org/|10\.\d{4,9}/)|",
            // arXiv identifier
            r"(?i)\barxiv[:\.]|",
            // Any URL
            r"https?://|",
            // Common venue cues — case-insensitive
            r"(?i)\b(?:",
            r"in\s+(?:proc|proceedings)|proceedings\s+of|",
            r"trans\.\s|journal\s+of|workshop\s+on|",
            r"symposium\s+on|conference\s+on|",
            r"usenix|ndss|s&p|crypto|eurocrypt|",
            r"acm\s+(?:trans|conference|sigsac)|ieee\s+(?:trans|conference|symposium)|",
            r"rfc\s*\d+|iso[/\s\-]?\d+",
            r")\b",
        ))
        .unwrap()
    });

    ACADEMIC_RE.is_match(trimmed)
}

fn try_ieee_with_config(ref_text: &str, config: &ParsingConfig) -> Option<Vec<String>> {
    // Match [1], [2], etc. at start of string, after newline, period, or closing bracket
    // - Start/newline: standard IEEE format
    // - Period: handles PDFs where text extraction doesn't preserve newlines
    // - Closing bracket `]`: handles ACM format where DOIs end with `]` before next ref `[N]`
    // - Digit: handles refs ending with year/page number directly before `[N]`
    static RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?m)(?:^|\n|[.\]0-9])\s*\[(\d+)\]\s*").unwrap());

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
            // Check if the next match starts with a digit (from prev ref's DOI)
            // If so, include that digit in the current reference
            let next_start = matches[i + 1].start();
            let next_match_str = matches[i + 1].as_str();
            // If match starts with a digit (e.g., "0[3]" from DOI ending in ...0),
            // the digit belongs to the current reference
            if let Some(first_char) = next_match_str.chars().next() {
                if first_char.is_ascii_digit() {
                    next_start + 1 // Include the digit in current ref
                } else {
                    next_start
                }
            } else {
                next_start
            }
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

fn try_numbered_with_config(ref_text: &str, config: &ParsingConfig) -> Option<Vec<String>> {
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

fn fallback_double_newline_with_config(ref_text: &str, config: &ParsingConfig) -> Vec<String> {
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
    fn test_find_references_section_with_analysis_appendix() {
        // Test "A\nANALYSIS" pattern (common in NeurIPS/ICLR papers)
        let text = "Body.\n\nReferences\n\n[1] Ref one.\n[2] Ref two.\n\nA\nANALYSIS OF GRADIENTS\n\nMath stuff.";
        let section = find_references_section(text).unwrap();
        assert!(section.contains("[1] Ref one."));
        assert!(section.contains("[2] Ref two."));
        assert!(
            !section.contains("ANALYSIS"),
            "Should truncate at A\\nANALYSIS boundary"
        );
        assert!(!section.contains("Math stuff"));
    }

    #[test]
    fn test_find_references_section_with_detailed_appendix() {
        // Test "A\nDetailed" pattern
        let text =
            "Body.\n\nReferences\n\n[1] Ref one.\n\nA\nDetailed Benchmark Results\n\nTables here.";
        let section = find_references_section(text).unwrap();
        assert!(section.contains("[1] Ref one."));
        assert!(
            !section.contains("Detailed"),
            "Should truncate at A\\nDetailed boundary"
        );
        assert!(!section.contains("Tables here"));
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

    #[test]
    fn test_find_references_running_header_bibliography() {
        // Simulate a thesis where "Bibliography" appears as the section heading
        // on the first page, then "BIBLIOGRAPHY" as a running header on subsequent pages.
        let text = concat!(
            "Body text.\n\n",
            "Bibliography\n\n",
            "[1] Author A. Title one. In Conf, 2020.\n",
            "[2] Author B. Title two. In Conf, 2021.\n",
            "[3] Author C. Title three. In Conf, 2022.\n\n",
            "BIBLIOGRAPHY\n\n",
            "[4] Author D. Title four. In Conf, 2023.\n",
            "[5] Author E. Title five. In Conf, 2024.\n\n",
            "BIBLIOGRAPHY\n\n",
            "[6] Author F. Title six. In Conf, 2025.\n",
        );
        let section = find_references_section(text).unwrap();
        // Should capture ALL references, not just [6] from the last running header
        assert!(section.contains("[1]"));
        assert!(section.contains("[6]"));
    }

    // ── Config-aware tests ──

    #[test]
    fn test_find_section_custom_header_re() {
        let config = crate::ParsingConfigBuilder::new()
            .section_header_regex(r"(?i)\n\s*Literatur\s*\n")
            .build()
            .unwrap();
        let text = "Body.\n\nLiteratur\n\nRef A.\nRef B.\n";
        let section = find_references_section_with_config(text, &config).unwrap();
        assert!(section.contains("Ref A."));
    }

    #[test]
    fn test_find_section_custom_end_re() {
        let config = crate::ParsingConfigBuilder::new()
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
        let config = crate::ParsingConfigBuilder::new()
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
        let config = crate::ParsingConfigBuilder::new()
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
        let config = crate::ParsingConfigBuilder::new()
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
    fn test_strip_page_headers_thesis_running_header() {
        // "84\nBIBLIOGRAPHY\n" (page number before header)
        let text = "some text before\n84\nBIBLIOGRAPHY\nsome text after\n";
        let stripped = strip_page_headers(text);
        assert!(
            !stripped.contains("BIBLIOGRAPHY"),
            "Should strip running header: {}",
            stripped
        );
        assert!(stripped.contains("some text before"));
        assert!(stripped.contains("some text after"));

        // "BIBLIOGRAPHY\n85\n" (header before page number)
        let text2 = "some text before\nBIBLIOGRAPHY\n85\nsome text after\n";
        let stripped2 = strip_page_headers(text2);
        assert!(
            !stripped2.contains("BIBLIOGRAPHY"),
            "Should strip running header: {}",
            stripped2
        );

        // Case-insensitive "References" variant
        let text3 = "some text before\n42\nReferences\nsome text after\n";
        let stripped3 = strip_page_headers(text3);
        assert!(
            !stripped3.contains("References"),
            "Should strip References running header: {}",
            stripped3
        );
    }

    #[test]
    fn test_strip_page_headers_acm_placeholder() {
        // ACM acmart template default running header (two lines: paper title + conference info)
        // appearing in the middle of a reference at a page break.
        // Real-world variant uses curly apostrophe (U+2019) from PDF text extraction.
        let text = concat!(
            "[23] Martin Kleppmann et al. 2024. Bluesky and the\n",
            "Blocking in the Bluesky with Diamonds\n",
            "Conference\u{2019}17, July 2017, Washington, DC, USA\n",
            "at protocol: Usable decentralized social media. In Workshop.\n",
        );
        let stripped = strip_page_headers(text);
        assert!(
            !stripped.contains("Conference\u{2019}17") && !stripped.contains("Conference'17"),
            "Should strip Conference'17 placeholder: {}",
            stripped
        );
        assert!(
            !stripped.contains("Blocking in the Bluesky with Diamonds"),
            "Should strip paper title running header above conference info: {}",
            stripped
        );
        assert!(stripped.contains("Bluesky and the"));
        assert!(stripped.contains("at protocol"));

        // ASCII apostrophe variant with day range
        let text2 = concat!(
            "before text\n",
            "Some Paper Title\n",
            "Conference '24, October 14-18, 2024, Salt Lake City, UT, USA\n",
            "after text\n",
        );
        let stripped2 = strip_page_headers(text2);
        assert!(!stripped2.contains("Conference '24"));
        assert!(!stripped2.contains("Some Paper Title"));
    }

    #[test]
    fn test_strip_page_headers_popets_odd_page() {
        // Odd page header: title line + journal-name line (two lines, as extracted from the PDF)
        // MuPDF extracts each printed text line separately, so the title and journal
        // are on adjacent lines:
        //   "QUICstep: Evaluating connection migration based QUIC censorship circumvention"
        //   "Proceedings on Privacy Enhancing Technologies 2026(1)"
        let header = "QUICstep: Evaluating connection migration based QUIC censorship circumvention\nProceedings on Privacy Enhancing Technologies 2026(1)";
        let text = format!("some text before\n{header}\nsome text after");
        let stripped = strip_page_headers(&text);
        assert!(
            !stripped.contains("QUICstep: Evaluating"),
            "Should strip PoPETs title fragment from odd-page header: {stripped}"
        );
        assert!(
            !stripped.contains("Proceedings on Privacy Enhancing Technologies 2026"),
            "Should strip PoPETs journal+year from odd-page header: {stripped}"
        );
        assert!(stripped.contains("some text before"));
        assert!(stripped.contains("some text after"));
    }

    #[test]
    fn test_strip_page_headers_popets_odd_page_same_line() {
        // Odd page header: title and journal name on the SAME line
        // (also valid — some PDF layouts produce this)
        let header = "QUICstep: Evaluating connection migration based QUIC censorship circumvention Proceedings on Privacy Enhancing Technologies 2026(1)";
        let text = format!("some text before\n{header}\nsome text after");
        let stripped = strip_page_headers(&text);
        assert!(
            !stripped.contains("QUICstep: Evaluating"),
            "Should strip same-line PoPETs header: {stripped}"
        );
        assert!(
            !stripped.contains("Proceedings on Privacy Enhancing Technologies 2026"),
            "Should strip journal+year from same-line PoPETs header: {stripped}"
        );
        assert!(stripped.contains("some text before"));
        assert!(stripped.contains("some text after"));
    }

    #[test]
    fn test_strip_page_headers_popets_even_page() {
        // Even page header: just "Proceedings on Privacy Enhancing Technologies <year>(<issue>)"
        // (no page number, no title — appears as a standalone fragment between references)
        let text = "...USENIX, 2020. Proceedings on Privacy Enhancing Technologies 2026(1) Next author et al.";
        let stripped = strip_page_headers(text);
        assert!(
            !stripped.contains("Proceedings on Privacy Enhancing Technologies 2026"),
            "Should strip PoPETs journal+year from even-page header: {stripped}"
        );
        assert!(stripped.contains("USENIX, 2020."));
        assert!(stripped.contains("Next author et al."));
    }

    #[test]
    fn test_strip_page_headers_popets_preserves_references() {
        // Citations to PoPETs papers use a comma after the journal name — must NOT be stripped.
        let text = concat!(
            "Barradas et al. DeltaShaper: Enabling unobservable TCP tunneling. ",
            "Proceedings on Privacy Enhancing Technologies, 2017(4):1–20.\n",
            "Fifield et al. Blocking-resistant communication through domain fronting. ",
            "Proceedings on Privacy Enhancing Technologies, 2015(2).\n",
        );
        let stripped = strip_page_headers(text);
        assert!(
            stripped.contains("DeltaShaper"),
            "Should preserve citations to PoPETs papers: {stripped}"
        );
        assert!(
            stripped.contains("Blocking-resistant communication"),
            "Should preserve citations to PoPETs papers: {stripped}"
        );
        assert!(
            stripped.contains("Proceedings on Privacy Enhancing Technologies, 2017"),
            "Journal name in citations must be preserved: {stripped}"
        );
    }

    #[test]
    fn test_segment_ieee_with_popets_page_header() {
        // Simulates a PoPETs paper where the running page header appears between references.
        // Odd-page format: "<paper_title> Proceedings on Privacy Enhancing Technologies <year>(<issue>)"
        // (no leading page number — confirmed by inspecting popets-2026-0014.pdf)
        let text = concat!(
            "[1] First reference with a long enough title here.\n",
            "[2] Second reference also with sufficient content.\n",
            "[3] Third reference ends before the page break.\n",
            "QUICstep: Evaluating connection migration based QUIC censorship circumvention ",
            "Proceedings on Privacy Enhancing Technologies 2026(1)\n",
            "[4] Fourth reference starts new page content here.\n",
            "[5] Fifth reference continues normally with text.\n",
            "[6] Sixth reference completes the section here.\n",
        );
        let refs = segment_references(text);
        assert_eq!(
            refs.len(),
            6,
            "Should find 6 IEEE refs after stripping PoPETs header: {:?}",
            refs
        );
        assert!(refs[0].contains("First reference"));
        assert!(refs[3].contains("Fourth reference"));
        assert!(
            !refs.iter().any(|r| r.contains("QUICstep")),
            "PoPETs page-number+title must not appear in any reference"
        );
        assert!(
            !refs
                .iter()
                .any(|r| r.contains("Proceedings on Privacy Enhancing Technologies 2026")),
            "PoPETs journal+year header must not appear in any reference"
        );
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
        assert!(
            section.contains("[1] First real reference"),
            "Section: {}",
            section
        );
        assert!(
            !section.contains("Classification"),
            "Should not contain table content"
        );
    }

    // ─── looks_like_reference: structural validity ──────────────────

    #[test]
    fn looks_like_reference_drops_punctuation_prefixed_fragment() {
        // baweja ref 58 in the USENIX 2026 corpus: a column-break
        // artifact that opens with a comma. Real refs never do.
        let body = ", and as a result they suffer from a logN overhead on the prover time.";
        assert!(!looks_like_reference(body));
    }

    #[test]
    fn looks_like_reference_drops_period_prefixed_fragment() {
        // gong ref 66: starts with ". Theorem 4. Let p0 = N(0,C2σ2)..."
        let body = ". Theorem 4. Let p0 = N(0,C2σ2) and p1 = N(1,C2σ2) denote the probability density functions.";
        assert!(!looks_like_reference(body));
    }

    #[test]
    fn looks_like_reference_drops_long_body_text() {
        // fan ref 63: "Datasets. The experiments are performed on five
        // commonly-used scalable realistic graph datasets..." Long, no
        // year, no DOI, no URL, no venue cue → body text.
        let body = "Datasets. The experiments are performed on five commonly-used scalable realistic graph datasets: Cora, Cora_ML, Cite-seer, Amazon Photo, and PubMed. Baselines. To evaluate our approach, we employ four types of graph adversarial attacks.";
        assert!(!looks_like_reference(body));
    }

    #[test]
    fn looks_like_reference_drops_algorithm_pseudocode() {
        // chen-liqun ref 110: KP-ABS algorithm pseudocode from an appendix.
        let body = "KP-ABS with Type-III pairings (mpk, msk) ←Setup(1λ). Run GroupGen(1λ) to obtain the group parameters par. Pick α and a hash function H.";
        assert!(!looks_like_reference(body));
    }

    #[test]
    fn looks_like_reference_keeps_ref_with_4digit_year() {
        let r = "Allan Wigfield and Jacquelynne S. Eccles. Expectancy-Value Theory of Achievement Motivation. Contemporary Educational Psychology, 25(1):68-81, 2000.";
        assert!(looks_like_reference(r));
    }

    #[test]
    fn looks_like_reference_keeps_ref_with_apostrophe_year() {
        // baweja's heavy '24-style citations: long but no 4-digit year.
        let r = "A. Arun, S. T. V. Setty, and J. Thaler. \"Jolt: SNARKs for Virtual Machines via Lookups\". In: EUROCRYPT '24. Lecture Notes in Computer Science.";
        assert!(looks_like_reference(r));
    }

    #[test]
    fn looks_like_reference_keeps_ref_with_doi() {
        let r = "S. Smith. A title. Some Journal, 10.1145/1234567.1234568. Pages 1-10. With more text padding to push past one hundred characters total.";
        assert!(looks_like_reference(r));
    }

    #[test]
    fn looks_like_reference_keeps_ref_with_url() {
        let r = "bomctl/bomctl. https://github.com/bomctl/bomctl. original-date: 2024-01-10T18:42:31Z. Lower-case GitHub repo references are legitimate.";
        assert!(looks_like_reference(r));
    }

    #[test]
    fn looks_like_reference_keeps_short_ref_without_markers() {
        // Short refs like "RFC 5246" or "ISO 9001" don't always carry
        // any of the academic-marker patterns, but they're real refs.
        // The 100-char floor preserves them.
        assert!(looks_like_reference("RFC 5246"));
        assert!(looks_like_reference("ISO 9001"));
        assert!(looks_like_reference(
            "Dmitry Kogan and Henry Corrigan-Gibbs. Private Blocklist Lookups with Checklist."
        ));
    }

    #[test]
    fn looks_like_reference_drops_empty() {
        assert!(!looks_like_reference(""));
        assert!(!looks_like_reference("   \n  "));
    }
}
