//! Parse a PVLDB (Proceedings of the VLDB Endowment) issue's front-matter
//! PDF table of contents into corpus records.
//!
//! PVLDB publishes monthly, and VLDB's conference program is drawn from
//! whichever issues land before each year's submission-rollover cutoff —
//! there's no single consolidated "VLDB <year> accepted papers" page,
//! only these per-issue front-matter PDFs (freely downloadable from
//! `dl.acm.org/journal/pvldb` or `pvldb.org`, no paywall). Call this once
//! per issue you want indexed.
//!
//! Unlike CHI's front matter (see the `chi` module), this is a genuine
//! table of contents, not a full per-paper block — and it carries no
//! author affiliations at all, just plain names. Structure (via
//! `pdftotext -layout`):
//!
//! ```text
//! Some Paper Title That May Wrap Onto A Second
//! Line Before The Dot Leader ......................... 1867
//! Author One, Author Two, Author Three
//!
//! Next Title .......................................... 1880
//! Solo Author
//! ```
//!
//! The dot-leader-then-page-number is the one unambiguous signal
//! (real titles don't contain long runs of periods): whatever text
//! precedes it, across as many wrapped lines as needed, is the title;
//! the first non-blank, non-page-footer line *after* it is the
//! (single-line, comma-separated, no affiliations) author list. A
//! "PVLDB Vol. <N>, No. <M> ... <page>" header/footer line can land
//! *between* a title's dot-leader line and its author line when a page
//! break falls right there — skipped like blank lines rather than
//! treated as content.

use once_cell::sync::Lazy;
use regex::Regex;

use crate::db::NewPublication;

/// A line ending in a run of dots (a "dot leader") followed by a page
/// number — group 1 is whatever title text precedes it on that same
/// line (possibly empty, if the whole line is just the leader).
static DOT_LEADER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(.*?)\.{3,}\s*\d+\s*$").unwrap());

/// A running header/footer line stamped on every page — not content.
static PAGE_STAMP: Lazy<Regex> = Lazy::new(|| Regex::new(r"^PVLDB Vol\.").unwrap());

/// The bare page number that goes with a `PAGE_STAMP` line — usually
/// part of the same physical line (`"PVLDB Vol. 19, No. 9 ... iii"`),
/// but PDF text extractors don't agree on that: MuPDF (this project's
/// own backend, confirmed empirically — poppler's `pdftotext -layout`
/// keeps them joined) sometimes splits the trailing page number onto
/// its own line, a roman numeral (front matter) or plain integer (body)
/// with nothing else on it. Left unrecognized, a stray line like this
/// gets mistaken for that entry's author line, and its *real* author
/// line then gets mistaken for the start of the next title — corrupting
/// two consecutive entries from one missed line. No legitimate title or
/// author line is ever *just* a bare number/numeral, so skipping this
/// unconditionally is safe.
static STANDALONE_PAGE_NUM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^[ivxlcdm]+$|^\d+$").unwrap());

/// Section-header labels PVLDB issues use to group entries (this list
/// isn't necessarily exhaustive across every issue — new track names
/// appear occasionally). Only skipped when they'd otherwise become the
/// *start* of a title (i.e. no title text has been collected yet for
/// the entry in progress); a real title that happened to be exactly one
/// of these strings would be astronomically unlikely.
const SECTION_HEADERS: &[&str] = &[
    "Research Papers",
    "Industry and Applications Papers",
    "Industrial Track",
    "Experiment, Analysis & Benchmark Papers",
    "Experiment, Analysis and Benchmark Papers",
    "Demonstrations",
    "Tutorials",
    "Tutorial",
    "Vision Papers",
    "Vision Track",
    "Systems and Applications Papers",
    "Scalable Data Science Papers",
    "PhD Workshop",
    "Reproducibility",
];

/// Join wrapped title lines, de-hyphenating a trailing `-` when the next
/// line resumes lowercase (same heuristic as [`super::chi`] uses, for
/// the same reason: good enough for a fuzzy-match threshold, not meant
/// to be typeset-perfect).
fn join_title_lines(lines: &[String]) -> String {
    let mut out = String::new();
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(stripped) = out.strip_suffix('-') {
            let starts_lower = line.chars().next().is_some_and(|c| c.is_lowercase());
            if starts_lower {
                out = format!("{stripped}{line}");
                continue;
            }
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line);
    }
    out
}

enum State {
    CollectingTitle,
    ExpectingAuthors,
}

/// Parse extracted PDF text from a PVLDB issue's front matter into
/// publication records. `source_tag` is the provenance string to store
/// on each record, e.g. `"vldb2026-v19n9"`.
pub fn parse_table_of_contents(text: &str, source_tag: &str) -> Vec<NewPublication> {
    let mut out = Vec::new();
    let mut state = State::CollectingTitle;
    let mut title_lines: Vec<String> = Vec::new();
    let mut pending_title: Option<String> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || PAGE_STAMP.is_match(line) || STANDALONE_PAGE_NUM.is_match(line) {
            continue;
        }

        match state {
            State::CollectingTitle => {
                if title_lines.is_empty() && SECTION_HEADERS.contains(&line) {
                    continue;
                }
                if let Some(caps) = DOT_LEADER.captures(line) {
                    title_lines.push(caps[1].to_string());
                    let title = join_title_lines(&title_lines);
                    title_lines.clear();
                    if !title.is_empty() {
                        pending_title = Some(title);
                        state = State::ExpectingAuthors;
                    }
                    // else: a stray dot-leader-only line with no title
                    // text at all (shouldn't happen in practice) — stay
                    // in CollectingTitle and keep accumulating.
                } else {
                    title_lines.push(line.to_string());
                }
            }
            State::ExpectingAuthors => {
                let authors: Vec<String> = line
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if let (Some(title), false) = (pending_title.take(), authors.is_empty()) {
                    out.push(NewPublication {
                        title,
                        authors,
                        url: None,
                        source: source_tag.to_string(),
                    });
                }
                state = State::CollectingTitle;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Research Papers

  Secure Join Operations in Multi-Identifier Databases: Performance and Practicality ....................................... 1854
  Wen-Jie Lu, Yongchuan Niu, Yongjun Zhao, Wei Dai, Donghang Lu, Li Wang, Qiang Yan

  A Resource-centric Analysis and Optimization of NoSQL Workloads using Distressed Resource Volume
  Metric ......................................................................................................................................................................................................... 1867
  Gunika Verma, Aashutosh A V, Pooja Srinivas, Yogesh Simmhan

  A Comparative Evaluation of Schema Subsetting for LLM-based NL-to-SQL over Large-Schema Databases
  ....................................................................................................................................................................................................................... 2019

 PVLDB Vol. 19, No. 9                                                                                      ii
 Kyle Luoma, Arun Kumar
";

    #[test]
    fn test_single_line_title_and_authors() {
        let out = parse_table_of_contents(SAMPLE, "vldb2026-v19n9");
        assert_eq!(
            out[0].title,
            "Secure Join Operations in Multi-Identifier Databases: Performance and Practicality"
        );
        assert_eq!(
            out[0].authors,
            vec![
                "Wen-Jie Lu",
                "Yongchuan Niu",
                "Yongjun Zhao",
                "Wei Dai",
                "Donghang Lu",
                "Li Wang",
                "Qiang Yan"
            ]
        );
        assert_eq!(out[0].source, "vldb2026-v19n9");
    }

    #[test]
    fn test_title_wraps_before_dot_leader() {
        let out = parse_table_of_contents(SAMPLE, "vldb2026-v19n9");
        assert_eq!(
            out[1].title,
            "A Resource-centric Analysis and Optimization of NoSQL Workloads using Distressed Resource Volume Metric"
        );
        assert_eq!(out[1].authors.len(), 4);
    }

    #[test]
    fn test_page_stamp_between_dot_leader_and_authors_skipped() {
        // The dot-leader line for entry 3 has NO title text on it at all
        // (title fully consumed the previous line) — and a page-footer
        // stamp lands between it and the author line. Neither should
        // break the title/author association.
        let out = parse_table_of_contents(SAMPLE, "vldb2026-v19n9");
        assert_eq!(out.len(), 3);
        assert_eq!(
            out[2].title,
            "A Comparative Evaluation of Schema Subsetting for LLM-based NL-to-SQL over Large-Schema Databases"
        );
        assert_eq!(out[2].authors, vec!["Kyle Luoma", "Arun Kumar"]);
    }

    // Regression: real hallucinator-pdf-mupdf output (not poppler's
    // `pdftotext -layout`, which is what the SAMPLE above mirrors) splits
    // "PVLDB Vol. 19, No. 9" and its trailing roman-numeral page number
    // onto *separate* lines, e.g. from the actual VLDB v19n9 front matter:
    //
    //   PVLDB Vol. 19, No. 9
    //    iii
    //
    // A bare "iii" line isn't matched by PAGE_STAMP (`^PVLDB Vol\.`), so
    // without STANDALONE_PAGE_NUM it gets mistaken for the entry's author
    // line — and that entry's *real* author line then gets mistaken for
    // the start of the next title, corrupting two consecutive entries
    // from one missed line.
    const MUPDF_SPLIT_PAGE_STAMP_SAMPLE: &str = "\
A Comparative Evaluation of Schema Subsetting for LLM-based NL-to-SQL over Large-Schema Databases
 ....................................................................................................................................................................................................................... 2019



PVLDB Vol. 19, No. 9

 iii



Kyle Luoma, Arun Kumar

IncreQueryFusion: On-demand Data Fusion Framework in Dynamic Data Lakes .............................................. 2032
Wenhao Liu, Sai Wu, Xiu Tang, Yitong Zhang, Dong Peng, Guolong Huang, Gang Chen
";

    #[test]
    fn test_mupdf_split_page_stamp_does_not_corrupt_two_entries() {
        let out = parse_table_of_contents(MUPDF_SPLIT_PAGE_STAMP_SAMPLE, "vldb2026-v19n9");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].title,
            "A Comparative Evaluation of Schema Subsetting for LLM-based NL-to-SQL over Large-Schema Databases"
        );
        assert_eq!(out[0].authors, vec!["Kyle Luoma", "Arun Kumar"]);
        assert_eq!(
            out[1].title,
            "IncreQueryFusion: On-demand Data Fusion Framework in Dynamic Data Lakes"
        );
        assert_eq!(out[1].authors.len(), 7);
    }

    #[test]
    fn test_parse_empty_text() {
        assert!(parse_table_of_contents("", "vldb2026-v19n9").is_empty());
    }
}
