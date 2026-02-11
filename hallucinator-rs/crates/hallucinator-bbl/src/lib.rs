use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use thiserror::Error;

use hallucinator_pdf::{ExtractionResult, Reference, SkipStats};

#[derive(Error, Debug)]
pub enum BblError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no \\bibitem entries found")]
    NoBibItems,
}

/// Extract references from a .bbl file (BibTeX-generated bibliography).
///
/// Parses `\bibitem` entries and extracts structured fields from
/// `\bibinfo{field}{value}` markup (ACM-Reference-Format style).
pub fn extract_references_from_bbl(path: &Path) -> Result<ExtractionResult, BblError> {
    let content = std::fs::read_to_string(path)?;
    extract_references_from_bbl_str(&content)
}

/// Parse .bbl content from a string (useful for testing).
pub fn extract_references_from_bbl_str(content: &str) -> Result<ExtractionResult, BblError> {
    let entries = segment_bibitem_entries(content);

    if entries.is_empty() {
        return Err(BblError::NoBibItems);
    }

    let mut stats = SkipStats {
        total_raw: entries.len(),
        ..Default::default()
    };

    let mut references = Vec::new();

    for entry in &entries {
        // Extract title
        let title = extract_title(entry).map(|t| strip_latex(&t));

        // Skip entries without a title or with very short titles
        let title = match title {
            Some(t) if !t.is_empty() && t.split_whitespace().count() >= 4 => t,
            Some(t) if t.is_empty() => {
                stats.no_title += 1;
                continue;
            }
            Some(_) => {
                stats.short_title += 1;
                continue;
            }
            None => {
                stats.no_title += 1;
                continue;
            }
        };

        // Extract authors
        let authors: Vec<String> = extract_authors(entry)
            .into_iter()
            .map(|a| strip_latex(&a))
            .collect();

        if authors.is_empty() {
            stats.no_authors += 1;
            // Still include (tracked only)
        }

        // Skip URL-only entries (non-academic URLs without a real title)
        if is_url_only_entry(entry) {
            stats.url_only += 1;
            continue;
        }

        // Extract identifiers
        let doi = extract_doi_from_bbl(entry);
        let arxiv_id = hallucinator_pdf::identifiers::extract_arxiv_id(entry);

        // Build raw citation for display (collapse whitespace)
        static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
        let raw_citation = WS_RE.replace_all(entry.trim(), " ").to_string();

        references.push(Reference {
            raw_citation,
            title: Some(title),
            authors,
            doi,
            arxiv_id,
        });
    }

    Ok(ExtractionResult {
        references,
        skip_stats: stats,
    })
}

/// Segment .bbl content into individual `\bibitem` entries.
fn segment_bibitem_entries(content: &str) -> Vec<String> {
    static BIBITEM_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^\\bibitem").unwrap());

    let matches: Vec<_> = BIBITEM_RE.find_iter(content).collect();

    if matches.is_empty() {
        return vec![];
    }

    let mut entries = Vec::with_capacity(matches.len());
    for i in 0..matches.len() {
        let start = matches[i].start();
        let end = if i + 1 < matches.len() {
            matches[i + 1].start()
        } else {
            // Find \end{thebibliography} or use end of content
            content[start..]
                .find("\\end{thebibliography}")
                .map(|pos| start + pos)
                .unwrap_or(content.len())
        };
        let entry = content[start..end].trim().to_string();
        if !entry.is_empty() {
            entries.push(entry);
        }
    }

    entries
}

/// Extract title from a bibitem entry.
///
/// Tries in order:
/// 1. `\showarticletitle{...}` — article titles
/// 2. `\bibinfo{title}{...}` — misc/informal titles
/// 3. `\bibinfo{booktitle}{...}` — book titles
fn extract_title(entry: &str) -> Option<String> {
    // 1. \showarticletitle{...}
    if let Some(t) = extract_braced_arg(entry, "\\showarticletitle") {
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 2. \bibinfo{title}{...}
    if let Some(t) = extract_bibinfo(entry, "title") {
        if !t.is_empty() {
            return Some(t);
        }
    }

    // 3. \bibinfo{booktitle}{...}
    if let Some(t) = extract_bibinfo(entry, "booktitle") {
        if !t.is_empty() {
            return Some(t);
        }
    }

    None
}

/// Extract authors from `\bibinfo{person}{Name}` patterns.
fn extract_authors(entry: &str) -> Vec<String> {
    static PERSON_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\\bibinfo\s*\{person\}\s*\{").unwrap());

    let mut authors = Vec::new();

    for m in PERSON_RE.find_iter(entry) {
        let after = &entry[m.end()..];
        if let Some(name) = extract_balanced_braces(after) {
            let name = name.trim().to_string();
            if !name.is_empty() {
                authors.push(name);
            }
        }
    }

    authors
}

/// Extract DOI from `\showDOI{...}` or raw DOI patterns.
fn extract_doi_from_bbl(entry: &str) -> Option<String> {
    // Try \showDOI{...} first
    if let Some(doi_text) = extract_braced_arg(entry, "\\showDOI") {
        // The content might be a URL like https://doi.org/10.xxx or just the DOI
        return hallucinator_pdf::identifiers::extract_doi(&doi_text);
    }

    // Fall back to raw DOI pattern in the text
    hallucinator_pdf::identifiers::extract_doi(entry)
}

/// Check if an entry is URL-only (has a URL but the "title" is just a URL or news headline).
fn is_url_only_entry(entry: &str) -> bool {
    static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\url\s*\{").unwrap());
    static HOWPUB_URL_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"\\bibinfo\s*\{howpublished\}\s*\{\\url").unwrap());

    // If the entry's main content is just a howpublished URL with no article title, skip
    let has_article_title = entry.contains("\\showarticletitle");
    let has_bibinfo_title = {
        static TITLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\bibinfo\s*\{title\}").unwrap());
        TITLE_RE.is_match(entry)
    };
    let has_url = URL_RE.is_match(entry);
    let is_howpub_url = HOWPUB_URL_RE.is_match(entry);

    // URL-only: has a URL, no article title, and either the "title" is just in howpublished or absent
    if has_url && !has_article_title && !has_bibinfo_title {
        return true;
    }

    // Entries where the howpublished is just a URL and there's no real title
    if is_howpub_url && !has_article_title && !has_bibinfo_title {
        return true;
    }

    false
}

/// Extract the argument of a LaTeX command like `\command{argument}`,
/// handling nested braces.
fn extract_braced_arg(text: &str, command: &str) -> Option<String> {
    let pos = text.find(command)?;
    let after_cmd = &text[pos + command.len()..];

    // Skip whitespace to find opening brace
    let after_cmd = after_cmd.trim_start();
    if !after_cmd.starts_with('{') {
        return None;
    }

    extract_balanced_braces(&after_cmd[1..])
}

/// Extract text up to the matching closing brace, handling nesting.
/// Input should start AFTER the opening `{`.
fn extract_balanced_braces(text: &str) -> Option<String> {
    let mut depth = 1;
    let mut end = 0;

    for (i, ch) in text.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }

    if depth == 0 {
        Some(text[..end].to_string())
    } else {
        None
    }
}

/// Extract `\bibinfo{field}{value}` where field matches the given name.
fn extract_bibinfo(entry: &str, field: &str) -> Option<String> {
    let pattern = format!("\\bibinfo{{{}}}", field);
    let pos = entry.find(&pattern)?;
    let after = &entry[pos + pattern.len()..];
    let after = after.trim_start();

    if !after.starts_with('{') {
        return None;
    }

    extract_balanced_braces(&after[1..])
}

/// Strip common LaTeX markup from extracted text.
fn strip_latex(text: &str) -> String {
    let mut result = text.to_string();

    // \emph{X} → X
    static EMPH_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\emph\s*\{([^}]*)\}").unwrap());
    result = EMPH_RE.replace_all(&result, "$1").to_string();

    // \textbf{X} → X
    static TEXTBF_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\textbf\s*\{([^}]*)\}").unwrap());
    result = TEXTBF_RE.replace_all(&result, "$1").to_string();

    // \textit{X} → X
    static TEXTIT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\textit\s*\{([^}]*)\}").unwrap());
    result = TEXTIT_RE.replace_all(&result, "$1").to_string();

    // \url{X} → X
    static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\url\s*\{([^}]*)\}").unwrap());
    result = URL_RE.replace_all(&result, "$1").to_string();

    // Common LaTeX accents: {\'e} → é, etc.
    result = expand_latex_accents(&result);

    // \& → &
    result = result.replace("\\&", "&");

    // \_ → _
    result = result.replace("\\_", "_");

    // \# → #
    result = result.replace("\\#", "#");

    // \~ → ~ (non-breaking space, but in text just use space)
    static TILDE_SPACE: Lazy<Regex> = Lazy::new(|| Regex::new(r"~").unwrap());
    result = TILDE_SPACE.replace_all(&result, " ").to_string();

    // Remove remaining stray braces
    result = result.replace(['{', '}'], "");

    // Collapse whitespace
    static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
    result = WS_RE.replace_all(&result, " ").to_string();

    result.trim().to_string()
}

/// Expand common LaTeX accent commands to Unicode characters.
fn expand_latex_accents(text: &str) -> String {
    let mut result = text.to_string();

    // Braced forms: {\'e}, {\`a}, {\"o}, {\^i}, {\~n}, {\c{c}}, etc.
    static ACCENT_BRACED: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"\{\\(['"`^~])\{?([a-zA-Z])\}?\}"#).unwrap());
    result = ACCENT_BRACED
        .replace_all(&result, |caps: &regex::Captures| {
            let accent = &caps[1];
            let letter = &caps[2];
            apply_accent(accent, letter)
        })
        .to_string();

    // Unbraced forms: \'e, \"o, etc.
    static ACCENT_UNBRACED: Lazy<Regex> =
        Lazy::new(|| Regex::new(r#"\\(['"`^~])([a-zA-Z])"#).unwrap());
    result = ACCENT_UNBRACED
        .replace_all(&result, |caps: &regex::Captures| {
            let accent = &caps[1];
            let letter = &caps[2];
            apply_accent(accent, letter)
        })
        .to_string();

    result
}

fn apply_accent(accent: &str, letter: &str) -> String {
    match (accent, letter) {
        ("'", "e") => "é".to_string(),
        ("'", "E") => "É".to_string(),
        ("'", "a") => "á".to_string(),
        ("'", "A") => "Á".to_string(),
        ("'", "i") => "í".to_string(),
        ("'", "I") => "Í".to_string(),
        ("'", "o") => "ó".to_string(),
        ("'", "O") => "Ó".to_string(),
        ("'", "u") => "ú".to_string(),
        ("'", "U") => "Ú".to_string(),
        ("`", "e") => "è".to_string(),
        ("`", "E") => "È".to_string(),
        ("`", "a") => "à".to_string(),
        ("`", "A") => "À".to_string(),
        ("`", "i") => "ì".to_string(),
        ("`", "o") => "ò".to_string(),
        ("`", "u") => "ù".to_string(),
        ("\"", "o") => "ö".to_string(),
        ("\"", "O") => "Ö".to_string(),
        ("\"", "u") => "ü".to_string(),
        ("\"", "U") => "Ü".to_string(),
        ("\"", "a") => "ä".to_string(),
        ("\"", "A") => "Ä".to_string(),
        ("^", "e") => "ê".to_string(),
        ("^", "E") => "Ê".to_string(),
        ("^", "a") => "â".to_string(),
        ("^", "o") => "ô".to_string(),
        ("^", "i") => "î".to_string(),
        ("~", "n") => "ñ".to_string(),
        ("~", "N") => "Ñ".to_string(),
        ("~", "a") => "ã".to_string(),
        ("~", "o") => "õ".to_string(),
        _ => letter.to_string(), // Unknown accent, just return the letter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_segment_bibitem_entries() {
        let content = r#"
\begin{thebibliography}{10}

\bibitem[Author1(2020)]{key1}
First entry content.

\bibitem[Author2(2021)]{key2}
Second entry content.

\end{thebibliography}
"#;
        let entries = segment_bibitem_entries(content);
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("First entry"));
        assert!(entries[1].contains("Second entry"));
    }

    #[test]
    fn test_extract_authors() {
        let entry = r#"\bibfield{author}{\bibinfo{person}{Pantelis Agathangelou},
  \bibinfo{person}{Ioannis Katakis}, {and} \bibinfo{person}{Barry Richards}.}"#;
        let authors = extract_authors(entry);
        assert_eq!(authors.len(), 3);
        assert_eq!(authors[0], "Pantelis Agathangelou");
        assert_eq!(authors[1], "Ioannis Katakis");
        assert_eq!(authors[2], "Barry Richards");
    }

    #[test]
    fn test_extract_title_showarticletitle() {
        let entry = r#"\newblock \showarticletitle{Understanding online political networks: The case
  of the far-right and far-left in Greece}."#;
        let title = extract_title(entry).unwrap();
        assert!(title.contains("Understanding online political networks"));
    }

    #[test]
    fn test_extract_title_bibinfo_title() {
        let entry = r#"\newblock \bibinfo{title}{Spacy}.
\newblock \bibinfo{howpublished}{\url{https://spacy.io/}}."#;
        let title = extract_title(entry).unwrap();
        assert_eq!(title, "Spacy");
    }

    #[test]
    fn test_extract_title_bibinfo_booktitle() {
        let entry = r#"\newblock \bibinfo{booktitle}{\emph{The Khrushchev Era: De-Stalinization and
  the Limits of Reform in the USSR 1953-64}}."#;
        let title = extract_title(entry).unwrap();
        assert!(title.contains("Khrushchev Era"));
    }

    #[test]
    fn test_strip_latex_emph() {
        assert_eq!(strip_latex("\\emph{Journal Name}"), "Journal Name");
    }

    #[test]
    fn test_strip_latex_accents() {
        assert_eq!(strip_latex("Ren{\\'e}e DiResta"), "Renée DiResta");
        assert_eq!(strip_latex("Fiskesj{\\\"o}"), "Fiskesjö");
    }

    #[test]
    fn test_strip_latex_ampersand() {
        assert_eq!(
            strip_latex("IEEE Security \\& Privacy"),
            "IEEE Security & Privacy"
        );
    }

    #[test]
    fn test_strip_latex_braces() {
        assert_eq!(strip_latex("{Perspective API}"), "Perspective API");
    }

    #[test]
    fn test_extract_balanced_braces() {
        assert_eq!(
            extract_balanced_braces("hello {world}}"),
            Some("hello {world}".to_string())
        );
        assert_eq!(
            extract_balanced_braces("simple}"),
            Some("simple".to_string())
        );
    }

    #[test]
    fn test_extract_doi_show_doi() {
        let entry = r#"\showDOI{https://doi.org/10.1145/3442381.3450048}"#;
        let doi = extract_doi_from_bbl(entry);
        assert_eq!(doi, Some("10.1145/3442381.3450048".to_string()));
    }

    #[test]
    fn test_integration_sample_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test-data")
            .join("no_comments.bbl");

        if !path.exists() {
            // Skip if test file not available
            return;
        }

        let result = extract_references_from_bbl(&path).unwrap();

        // The file has 122 bibitem entries; some will be skipped (URL-only, short titles)
        assert!(
            result.skip_stats.total_raw >= 100,
            "Expected 100+ raw entries, got {}",
            result.skip_stats.total_raw
        );
        assert!(
            !result.references.is_empty(),
            "Should have extracted some references"
        );

        // Spot-check: "Beyond Fish and Bicycles" entry
        let fish = result
            .references
            .iter()
            .find(|r| {
                r.title
                    .as_ref()
                    .map(|t| t.contains("Beyond Fish and Bicycles"))
                    .unwrap_or(false)
            })
            .expect("Should find 'Beyond Fish and Bicycles' entry");

        assert!(
            fish.title
                .as_ref()
                .unwrap()
                .contains("Exploring the Varieties of Online Women"),
            "Title should contain full text: {:?}",
            fish.title
        );

        // Check authors
        assert!(
            fish.authors.iter().any(|a| a.contains("Balci")),
            "Should have Balci as author: {:?}",
            fish.authors
        );
        assert!(
            fish.authors.iter().any(|a| a.contains("Blackburn")),
            "Should have Blackburn as author: {:?}",
            fish.authors
        );

        // Spot-check: entry with LaTeX accents
        let mamie = result.references.iter().find(|r| {
            r.authors
                .iter()
                .any(|a| a.contains("Mamié") || a.contains("Mamie"))
        });
        if let Some(mamie_ref) = mamie {
            assert!(
                mamie_ref.title.as_ref().unwrap().contains("Anti-Feminist"),
                "Mamié entry should have correct title"
            );
        }
    }
}
