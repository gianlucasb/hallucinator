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
    #[error("no BibTeX entries found")]
    NoBibEntries,
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

    for (raw_idx, entry) in entries.iter().enumerate() {
        // Build raw citation for display (collapse whitespace) — needed for both skipped and normal refs
        static WS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
        let raw_citation = WS_RE.replace_all(entry.trim(), " ").to_string();

        // Extract title
        let title = extract_title(entry).map(|t| strip_latex(&t));

        // Skip entries without a title or with very short titles
        let title = match title {
            Some(t) if !t.is_empty() && t.split_whitespace().count() >= 4 => t,
            Some(t) if t.is_empty() => {
                stats.no_title += 1;
                references.push(Reference {
                    raw_citation,
                    title: None,
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    original_number: raw_idx + 1,
                    skip_reason: Some("no_title".to_string()),
                });
                continue;
            }
            Some(t) => {
                stats.short_title += 1;
                references.push(Reference {
                    raw_citation,
                    title: Some(t),
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    original_number: raw_idx + 1,
                    skip_reason: Some("short_title".to_string()),
                });
                continue;
            }
            None => {
                stats.no_title += 1;
                references.push(Reference {
                    raw_citation,
                    title: None,
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    original_number: raw_idx + 1,
                    skip_reason: Some("no_title".to_string()),
                });
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
            references.push(Reference {
                raw_citation,
                title: Some(title),
                authors,
                doi: None,
                arxiv_id: None,
                original_number: raw_idx + 1,
                skip_reason: Some("url_only".to_string()),
            });
            continue;
        }

        // Extract identifiers
        let doi = extract_doi_from_bbl(entry);
        let arxiv_id = hallucinator_pdf::identifiers::extract_arxiv_id(entry);

        references.push(Reference {
            raw_citation,
            title: Some(title),
            authors,
            doi,
            arxiv_id,
            original_number: raw_idx + 1,
            skip_reason: None,
        });
    }

    Ok(ExtractionResult {
        references,
        skip_stats: stats,
    })
}

/// Extract references from a .bib file (BibTeX bibliography database).
///
/// Uses the `biblatex` crate for robust parsing with LaTeX accent decoding
/// and structured field extraction.
pub fn extract_references_from_bib(path: &Path) -> Result<ExtractionResult, BblError> {
    let content = std::fs::read_to_string(path)?;
    extract_references_from_bib_str(&content)
}

/// Parse .bib content from a string.
pub fn extract_references_from_bib_str(content: &str) -> Result<ExtractionResult, BblError> {
    // Try parsing the whole file first (fast path)
    match biblatex::Bibliography::parse(content) {
        Ok(bibliography) => {
            let entries: Vec<_> = bibliography.iter().collect();
            if entries.is_empty() {
                return Err(BblError::NoBibEntries);
            }
            Ok(process_bib_entries(&entries))
        }
        Err(_) => {
            // Fallback: split by @ entries and parse each individually.
            // Real .bib files often have minor syntax errors (extra braces,
            // missing @ prefix, non-standard entry types, raw text separators)
            // that cause the whole-file parse to fail. By splitting and parsing
            // each entry independently, we recover whatever we can.
            parse_bib_entries_individually(content)
        }
    }
}

/// Split .bib content into individual entry strings and parse each one.
fn parse_bib_entries_individually(content: &str) -> Result<ExtractionResult, BblError> {
    // Find positions of @ followed by a word character (entry type)
    static ENTRY_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^@[a-zA-Z]").unwrap());

    let positions: Vec<usize> = ENTRY_RE.find_iter(content).map(|m| m.start()).collect();
    if positions.is_empty() {
        return Err(BblError::NoBibEntries);
    }

    let mut all_entries = Vec::new();
    // We need to own the parsed bibliographies so entries live long enough
    let mut parsed_bibs = Vec::new();

    for i in 0..positions.len() {
        let start = positions[i];
        let end = if i + 1 < positions.len() {
            positions[i + 1]
        } else {
            content.len()
        };
        let chunk = &content[start..end];

        if let Ok(bib) = biblatex::Bibliography::parse(chunk) {
            parsed_bibs.push(bib);
        }
    }

    for bib in &parsed_bibs {
        for entry in bib.iter() {
            all_entries.push(entry);
        }
    }

    if all_entries.is_empty() {
        return Err(BblError::NoBibEntries);
    }

    Ok(process_bib_entries(&all_entries))
}

/// Process parsed biblatex entries into References.
fn process_bib_entries(entries: &[&biblatex::Entry]) -> ExtractionResult {
    let mut stats = SkipStats {
        total_raw: entries.len(),
        ..Default::default()
    };
    let mut references = Vec::new();

    for (raw_idx, entry) in entries.iter().enumerate() {
        // Extract title (convert chunks → string, then strip residual LaTeX)
        let title = entry
            .title()
            .ok()
            .map(chunks_to_string)
            .map(|t| strip_latex(&t));

        // Same skip logic as BBL: no title, short title (<4 words)
        let title = match title {
            Some(t) if !t.is_empty() && t.split_whitespace().count() >= 4 => t,
            Some(t) if t.is_empty() => {
                stats.no_title += 1;
                references.push(Reference {
                    raw_citation: String::new(),
                    title: None,
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    original_number: raw_idx + 1,
                    skip_reason: Some("no_title".to_string()),
                });
                continue;
            }
            Some(t) => {
                stats.short_title += 1;
                references.push(Reference {
                    raw_citation: String::new(),
                    title: Some(t),
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    original_number: raw_idx + 1,
                    skip_reason: Some("short_title".to_string()),
                });
                continue;
            }
            None => {
                stats.no_title += 1;
                references.push(Reference {
                    raw_citation: String::new(),
                    title: None,
                    authors: vec![],
                    doi: None,
                    arxiv_id: None,
                    original_number: raw_idx + 1,
                    skip_reason: Some("no_title".to_string()),
                });
                continue;
            }
        };

        // Extract authors via biblatex's Person parser
        let authors: Vec<String> = entry
            .author()
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.name != "others")
            .filter(|p| !p.name.is_empty() || !p.given_name.is_empty())
            .map(|p| format_bib_person(&p))
            .collect();

        if authors.is_empty() {
            stats.no_authors += 1;
            // Still include (tracked only, like BBL)
        }

        // Extract DOI (normalize URL-form DOIs like "https://doi.org/10.xxxx" → "10.xxxx")
        let doi = entry
            .get("doi")
            .map(chunks_to_string)
            .filter(|d| !d.is_empty())
            .and_then(|d| hallucinator_pdf::identifiers::extract_doi(&d));

        // Extract arXiv ID from eprint field or journal field
        let arxiv_id = extract_arxiv_from_bib_entry(entry);

        // Build raw citation for display
        let mut raw_parts = Vec::new();
        if !authors.is_empty() {
            raw_parts.push(authors.join(", "));
        }
        raw_parts.push(title.clone());
        if let Some(journal) = entry.get("journal").map(chunks_to_string) {
            if !journal.is_empty() {
                raw_parts.push(journal);
            }
        }
        if let Some(booktitle) = entry.get("booktitle").map(chunks_to_string) {
            if !booktitle.is_empty() {
                raw_parts.push(booktitle);
            }
        }
        if let Some(year) = entry.get("year").map(chunks_to_string) {
            if !year.is_empty() {
                raw_parts.push(year);
            }
        }
        let raw_citation = raw_parts.join(". ");

        references.push(Reference {
            raw_citation,
            title: Some(title),
            authors,
            doi,
            arxiv_id,
            original_number: raw_idx + 1,
            skip_reason: None,
        });
    }

    ExtractionResult {
        references,
        skip_stats: stats,
    }
}

/// Convert biblatex chunks to a plain string.
fn chunks_to_string(chunks: &[biblatex::Spanned<biblatex::Chunk>]) -> String {
    chunks
        .iter()
        .map(|c| match &c.v {
            biblatex::Chunk::Normal(s) => s.as_str(),
            biblatex::Chunk::Verbatim(s) => s.as_str(),
            biblatex::Chunk::Math(s) => s.as_str(),
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Format a biblatex Person as "Given Family" (western name order).
fn format_bib_person(p: &biblatex::Person) -> String {
    let mut parts = Vec::new();
    if !p.given_name.is_empty() {
        parts.push(p.given_name.as_str());
    }
    if !p.prefix.is_empty() {
        parts.push(p.prefix.as_str());
    }
    if !p.name.is_empty() {
        parts.push(p.name.as_str());
    }
    if !p.suffix.is_empty() {
        parts.push(p.suffix.as_str());
    }
    parts.join(" ")
}

/// Extract arXiv ID from a .bib entry's `eprint` or `journal` field.
fn extract_arxiv_from_bib_entry(entry: &biblatex::Entry) -> Option<String> {
    // Check eprint field
    if let Some(eprint_chunks) = entry.get("eprint") {
        let eprint = chunks_to_string(eprint_chunks);
        // Skip URL-style eprints (some .bib files put publisher PDF URLs here)
        if !eprint.is_empty() && !eprint.starts_with("http") {
            // Verify archiveprefix is arXiv (or absent — many .bib files omit it)
            let prefix = entry
                .get("archiveprefix")
                .map(chunks_to_string)
                .unwrap_or_default();
            if prefix.is_empty() || prefix.eq_ignore_ascii_case("arxiv") {
                if let Some(id) = hallucinator_pdf::identifiers::extract_arxiv_id(&eprint) {
                    return Some(id);
                }
                // Some .bib files have bare IDs like "2403.10573"
                static BARE_ARXIV: Lazy<Regex> =
                    Lazy::new(|| Regex::new(r"^\d{4}\.\d{4,5}(v\d+)?$").unwrap());
                if BARE_ARXIV.is_match(&eprint) {
                    return Some(eprint);
                }
            }
        }
    }

    // Check journal field for "arXiv preprint arXiv:XXXX.XXXXX"
    if let Some(journal_chunks) = entry.get("journal") {
        let journal = chunks_to_string(journal_chunks);
        if let Some(id) = hallucinator_pdf::identifiers::extract_arxiv_id(&journal) {
            return Some(id);
        }
    }

    None
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

    // ── .bib parser tests ──

    #[test]
    fn test_bib_basic_extraction() {
        let bib = r#"
@article{doe2023,
  title={A Very Important Research Paper Title},
  author={Doe, John and Smith, Jane},
  journal={Journal of Testing},
  year={2023},
  doi={10.1234/test.2023}
}
"#;
        let result = extract_references_from_bib_str(bib).unwrap();
        assert_eq!(result.skip_stats.total_raw, 1);
        assert_eq!(result.references.len(), 1);

        let r = &result.references[0];
        assert_eq!(
            r.title.as_deref().unwrap(),
            "A Very Important Research Paper Title"
        );
        assert_eq!(r.authors.len(), 2);
        assert!(r.authors[0].contains("John"));
        assert!(r.authors[0].contains("Doe"));
        assert!(r.authors[1].contains("Jane"));
        assert!(r.authors[1].contains("Smith"));
        assert_eq!(r.doi.as_deref(), Some("10.1234/test.2023"));
    }

    #[test]
    fn test_bib_accent_handling() {
        let bib = r#"
@inproceedings{jegou2020,
  title={Radioactive data: tracing through training is very important},
  author={Sablayrolles, Alexandre and Douze, Matthijs and Schmid, Cordelia and J{\'e}gou, Herv{\'e}},
  booktitle={International Conference on Machine Learning},
  year={2020}
}
"#;
        let result = extract_references_from_bib_str(bib).unwrap();
        assert_eq!(result.references.len(), 1);

        let r = &result.references[0];
        // biblatex should decode LaTeX accents
        let jegou = r.authors.iter().find(|a| a.contains("gou"));
        assert!(jegou.is_some(), "Should find Jégou author: {:?}", r.authors);
        let jegou = jegou.unwrap();
        assert!(
            jegou.contains("é") || jegou.contains("e"),
            "Should decode accent: {}",
            jegou
        );
    }

    #[test]
    fn test_bib_arxiv_from_journal() {
        let bib = r#"
@article{sun2024,
  title={Medical Unlearnable Examples: Securing Medical Data from Unauthorized Training},
  author={Sun, Weixiang and others},
  journal={arXiv preprint arXiv:2403.10573},
  year={2024}
}
"#;
        let result = extract_references_from_bib_str(bib).unwrap();
        assert_eq!(result.references.len(), 1);

        let r = &result.references[0];
        assert_eq!(r.arxiv_id.as_deref(), Some("2403.10573"));
        // "others" should be filtered out
        assert!(
            !r.authors.iter().any(|a| a.contains("others")),
            "Should filter out 'others': {:?}",
            r.authors
        );
    }

    #[test]
    fn test_bib_short_title_skipped() {
        let bib = r#"
@misc{short2023,
  title={Short Title},
  author={Author, Test},
  year={2023}
}

@article{long2023,
  title={This Is a Sufficiently Long Title for Testing},
  author={Author, Test},
  year={2023}
}
"#;
        let result = extract_references_from_bib_str(bib).unwrap();
        assert_eq!(result.skip_stats.total_raw, 2);
        assert_eq!(result.skip_stats.short_title, 1);
        // Both refs are included (skipped ones have skip_reason set)
        assert_eq!(result.references.len(), 2);
        assert!(
            result.references[0].skip_reason.is_some(),
            "Short title should be skipped"
        );
        assert!(
            result.references[1].skip_reason.is_none(),
            "Long title should not be skipped"
        );
    }

    #[test]
    fn test_bib_no_entries() {
        let result = extract_references_from_bib_str("not a bib file");
        assert!(result.is_err());
    }

    #[test]
    fn test_bib_integration_sample_file() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test-data")
            .join("arxiv")
            .join("bbl-bib")
            .join("2602.09284v1.bib");

        if !path.exists() {
            // Skip if test file not available
            return;
        }

        let result = extract_references_from_bib(&path).unwrap();

        // The file has 44 entries
        assert!(
            result.skip_stats.total_raw >= 40,
            "Expected 40+ raw entries, got {}",
            result.skip_stats.total_raw
        );
        assert!(
            !result.references.is_empty(),
            "Should have extracted some references"
        );

        // Spot-check: "Deep learning with differential privacy"
        let dp = result.references.iter().find(|r| {
            r.title
                .as_ref()
                .map(|t| t.contains("Deep learning with differential privacy"))
                .unwrap_or(false)
        });
        assert!(dp.is_some(), "Should find differential privacy entry");
        let dp = dp.unwrap();
        assert!(
            dp.authors.iter().any(|a| a.contains("Abadi")),
            "Should have Abadi as author: {:?}",
            dp.authors
        );

        // Spot-check: entry with arXiv in journal field
        let arxiv_entry = result
            .references
            .iter()
            .find(|r| r.arxiv_id.as_deref() == Some("2403.10573"));
        assert!(
            arxiv_entry.is_some(),
            "Should extract arXiv ID from journal field"
        );

        // Spot-check: accented author (Jégou/Jegou)
        let jegou = result.references.iter().find(|r| {
            r.authors
                .iter()
                .any(|a| a.contains("gou") && (a.contains("é") || a.contains("e")))
        });
        assert!(jegou.is_some(), "Should find Jégou entry with accent");
    }

    #[test]
    fn test_bib_cs_cy_all_files() {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test-data")
            .join("arxiv")
            .join("cs-cy-bbl-bib");

        if !base.exists() {
            return;
        }

        let bib_files: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("bib"))
            })
            .map(|e| e.path())
            .collect();

        assert!(
            !bib_files.is_empty(),
            "Should find .bib files in cs-cy-bbl-bib"
        );

        for path in &bib_files {
            let filename = path.file_name().unwrap().to_string_lossy();
            let result = extract_references_from_bib(path);
            match &result {
                Ok(res) => {
                    eprintln!(
                        "OK  {}: {} total, {} refs, {} skipped (no_title={}, short={}, no_authors={})",
                        filename,
                        res.skip_stats.total_raw,
                        res.references.len(),
                        res.skip_stats.no_title + res.skip_stats.short_title,
                        res.skip_stats.no_title,
                        res.skip_stats.short_title,
                        res.skip_stats.no_authors,
                    );
                }
                Err(e) => {
                    eprintln!("ERR {}: {}", filename, e);
                }
            }
            // All files should parse without error
            assert!(
                result.is_ok(),
                "{} failed to parse: {:?}",
                filename,
                result.err()
            );
        }
    }
}
