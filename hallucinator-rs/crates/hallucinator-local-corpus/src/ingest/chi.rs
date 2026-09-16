//! Parse CHI's ACM DL proceedings front-matter PDF (its table of
//! contents) into corpus records.
//!
//! CHI's own program site is a JavaScript app with no static content
//! (`<app-root></app-root>`, confirmed by direct fetch), so unlike every
//! other venue in this module there's no HTML page to scrape. ACM
//! publishes a "front matter" PDF per proceedings volume that's freely
//! downloadable (no paywall) and contains a full table of contents —
//! title, full author list, and DOI for every paper. This module doesn't
//! read the PDF itself (that stays isolated to the `hallucinator-pdf-
//! mupdf` crate, the project's one deliberate AGPL dependency boundary —
//! see that crate's doc comment); the CLI layer extracts the PDF's text
//! (it already depends on that crate for the main check pipeline) and
//! hands this module the plain text, same "pure function over a string"
//! shape every other importer here has.
//!
//! Structure (via `pdftotext -layout`, one paper per block):
//!
//! ```text
//! PAPER001 Title text that may wrap onto one or more indented
//!          continuation lines before the first author starts
//!          Author One, Affiliation, Affiliation Continuation
//!          if the affiliation itself wraps onto its own indented
//!          line, with no name-shaped prefix
//!          Author Two, Affiliation
//!          DOI: 10.1145/xxxxxxx.xxxxxxx
//! ```
//!
//! Title-wrap lines and affiliation-wrap lines are visually
//! indistinguishable (same indentation, no blank-line separator) — the
//! only usable signal is content shape. This walks each entry forward as
//! a small state machine: every line up to the *first* line that looks
//! like `"Name Name, ..."` (title case, one or more words, a comma) is
//! title text; from there on, a line matching that shape starts a new
//! author (we keep just the name, before the first comma), and any line
//! that *doesn't* match is a continuation of the current author's
//! affiliation and is discarded (we only need names).

use once_cell::sync::Lazy;
use regex::Regex;

use crate::db::NewPublication;

/// `PAPER123 ` at the start of a line — the only reliable per-entry
/// boundary marker.
static PAPER_MARKER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^PAPER\d+\s").unwrap());

/// A line that looks like `"Name Name, rest..."` — 1 to 6 space-
/// separated tokens, each starting with an uppercase (Unicode-aware)
/// letter or an opening paren (for parenthetical nicknames like
/// `"Rie Helene (Lindy) Hernandez,"`), followed by a comma. Title-wrap
/// lines almost never take this shape (they resume mid-title, often
/// lowercase, and rarely place a bare comma right after 1-6 capitalized
/// words); affiliation-wrap continuation lines never have a leading
/// comma at all.
static AUTHOR_START: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(?:\p{Lu}[\p{L}'.-]*|\([\p{L}\s]+\))(?:\s+(?:\p{Lu}[\p{L}'.-]*|\([\p{L}\s]+\))){0,5},",
    )
    .unwrap()
});

/// Join a wrapped title's lines back into one string. A line ending in
/// `-` immediately followed by a lowercase-starting continuation is
/// treated as a PDF line-break mid-word and de-hyphenated (`"On-"` +
/// `"boarding"` -> `"Onboarding"`); anything else just gets a space.
/// Imperfect (a genuine compound like `"well-"` / `"known"` loses its
/// hyphen too), but titles only need to clear a fuzzy-match threshold
/// downstream, not render typeset-perfect.
fn join_title_lines(lines: &[&str]) -> String {
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

/// Extract just the name portion (before the first comma) of an author
/// line already confirmed to match [`AUTHOR_START`].
fn extract_author_name(line: &str) -> String {
    line.split_once(',')
        .map(|(name, _)| name)
        .unwrap_or(line)
        .trim()
        .to_string()
}

/// Parse extracted PDF text from a CHI (or any ACM-DL-front-matter-
/// formatted) proceedings front matter into publication records.
/// `source_tag` is the provenance string to store on each record, e.g.
/// `"chi2026"`.
pub fn parse_frontmatter(text: &str, source_tag: &str) -> Vec<NewPublication> {
    let marker_starts: Vec<usize> = PAPER_MARKER.find_iter(text).map(|m| m.start()).collect();

    let mut out = Vec::new();
    for (i, &start) in marker_starts.iter().enumerate() {
        let end = marker_starts.get(i + 1).copied().unwrap_or(text.len());
        let entry = &text[start..end];

        // Drop the "PAPER123 " marker itself, then the "DOI: ..." line
        // and everything after it (organizer bios, session headers
        // between entries, etc. all live past that point).
        let after_marker = match entry.find(char::is_whitespace) {
            Some(idx) => &entry[idx..],
            None => continue,
        };
        let body = match after_marker.find("DOI:") {
            Some(idx) => &after_marker[..idx],
            None => continue, // no DOI line -> not a real paper entry
        };

        let mut title_lines: Vec<&str> = Vec::new();
        let mut authors: Vec<String> = Vec::new();
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if AUTHOR_START.is_match(trimmed) {
                authors.push(extract_author_name(trimmed));
            } else if authors.is_empty() {
                title_lines.push(trimmed);
            }
            // else: affiliation-wrap continuation of the current author — discard.
        }

        let title = join_title_lines(&title_lines);
        if title.is_empty() || authors.is_empty() {
            continue;
        }

        out.push(NewPublication {
            title,
            authors,
            url: None,
            source: source_tag.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
  Paper

  AI & Data Visualization

PAPER001 \u{201c}Hey Dashboard!\u{201d}: Supporting Voice, Text, and Pointing Modalities in Dashboard On-
         boarding using Large Language Models
         Vaishali Dhanoa, Aarhus University, TU Wien
         Gabriela Molina Le\u{f3}n, Aarhus University
         Eve Hoggan, Aarhus University
         DOI: 10.1145/3772318.3791766


PAPER002 Contrastive Learning for Large-scale Color-Name Dataset
         Kecheng Lu, Renmin University of China
         DOI: 10.1145/3772318.3791278


PAPER1702 Player Safety by Design: Co-Designing Child-Centered Safety Mechanisms with Children
          Zinan Zhang, College of Information Sciences and Technology, The Pennsylvania State University
          Rie Helene (Lindy) Hernandez, College of Information Sciences and Technology, The Pennsylvania
          State University
          Yubo Kou, College of Information Sciences and Technology, The Pennsylvania State University
          DOI: 10.1145/3772318.3791090
                            Organization Committee


 General Chairs
";

    #[test]
    fn test_hyphenated_title_wrap_dehyphenated() {
        let out = parse_frontmatter(SAMPLE, "chi2026");
        assert_eq!(
            out[0].title,
            "\u{201c}Hey Dashboard!\u{201d}: Supporting Voice, Text, and Pointing Modalities in Dashboard Onboarding using Large Language Models"
        );
        assert_eq!(out[0].source, "chi2026");
    }

    #[test]
    fn test_authors_extracted_names_only() {
        let out = parse_frontmatter(SAMPLE, "chi2026");
        assert_eq!(
            out[0].authors,
            vec!["Vaishali Dhanoa", "Gabriela Molina Le\u{f3}n", "Eve Hoggan"]
        );
    }

    #[test]
    fn test_single_line_title_second_entry() {
        let out = parse_frontmatter(SAMPLE, "chi2026");
        assert_eq!(
            out[1].title,
            "Contrastive Learning for Large-scale Color-Name Dataset"
        );
        assert_eq!(out[1].authors, vec!["Kecheng Lu"]);
    }

    #[test]
    fn test_wrapped_affiliation_continuation_does_not_become_a_bogus_author_or_leak_into_title() {
        // "State University" is a continuation of Rie Helene (Lindy)
        // Hernandez's affiliation, not a 4th author and not part of the
        // title — it must be silently discarded.
        let out = parse_frontmatter(SAMPLE, "chi2026");
        assert_eq!(
            out[2].title,
            "Player Safety by Design: Co-Designing Child-Centered Safety Mechanisms with Children"
        );
        assert_eq!(
            out[2].authors,
            vec!["Zinan Zhang", "Rie Helene (Lindy) Hernandez", "Yubo Kou"]
        );
    }

    #[test]
    fn test_trailing_non_paper_content_after_last_entry_ignored() {
        let out = parse_frontmatter(SAMPLE, "chi2026");
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn test_parse_empty_text() {
        assert!(parse_frontmatter("", "chi2026").is_empty());
    }
}
