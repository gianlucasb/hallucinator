//! Check hyphenation improvements with dictionary on USENIX 2025 papers.
//!
//! Run with:
//!   cargo test -p hallucinator-parsing --test usenix_dict_hyphen_check -- --ignored --nocapture

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hallucinator_core::{BackendError, PdfBackend};
use hallucinator_parsing::{ExtractionResult, ReferenceExtractor};
use hallucinator_scowl::ScowlDictionary;
use regex::Regex;

struct LocalMupdfBackend;

impl PdfBackend for LocalMupdfBackend {
    fn extract_text(&self, path: &Path) -> Result<String, BackendError> {
        use mupdf::{Document, TextPageFlags};

        let path_str = path
            .to_str()
            .ok_or_else(|| BackendError::OpenError("invalid path encoding".into()))?;
        let document =
            Document::open(path_str).map_err(|e| BackendError::OpenError(e.to_string()))?;
        let mut pages_text = Vec::new();
        for page_result in document
            .pages()
            .map_err(|e| BackendError::ExtractionError(e.to_string()))?
        {
            let page = page_result.map_err(|e| BackendError::ExtractionError(e.to_string()))?;
            let text_page = page
                .to_text_page(TextPageFlags::empty())
                .map_err(|e| BackendError::ExtractionError(e.to_string()))?;
            let mut page_text = String::new();
            for block in text_page.blocks() {
                for line in block.lines() {
                    let line_text: String = line
                        .chars()
                        .map(|c| c.char().unwrap_or('\u{FFFD}'))
                        .collect();
                    page_text.push_str(&line_text);
                    page_text.push('\n');
                }
            }
            pages_text.push(page_text);
        }
        Ok(pages_text.join("\n"))
    }
}

fn usenix_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot get home dir")
        .join("Data")
        .join("hallucinator-data")
        .join("usenix-2025")
        .join("papers")
}

fn discover_pdfs(dir: &Path) -> Vec<PathBuf> {
    let entries = std::fs::read_dir(dir).expect("cannot read data directory");
    let mut pdfs: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".pdf") {
                Some(e.path())
            } else {
                None
            }
        })
        .collect();
    pdfs.sort();
    pdfs
}

fn refs_to_titles(result: &ExtractionResult) -> Vec<String> {
    result
        .references
        .iter()
        .filter(|r| r.skip_reason.is_none())
        .filter_map(|r| r.title.clone())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Check if a hyphenated word looks like broken PDF hyphenation
fn is_likely_broken_hyphen(word: &str) -> bool {
    let parts: Vec<&str> = word.split('-').collect();
    if parts.len() != 2 {
        return false;
    }

    let before = parts[0].to_lowercase();
    let after = parts[1].to_lowercase();

    // Skip numbers and single letters
    if before.chars().all(|c| c.is_ascii_digit()) || after.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }

    if before.len() <= 1 || after.len() <= 1 {
        return false;
    }

    // Check for suspicious syllable-break patterns
    let syllable_suffixes = [
        "tion", "sion", "ment", "ness", "ance", "ence", "ity", "ory", "ary", "ery",
        "able", "ible", "ical", "ally", "ular", "ology", "ization", "ized", "ised",
        "ing", "ings", "ism", "isms", "ist", "ists", "ure", "ures", "age", "ages",
        "ous", "ive", "eous", "ious", "ant", "ent", "ful", "less", "ward", "wards",
        "wise", "like", "ship", "hood", "dom", "let", "ling", "ette", "ess",
        "er", "or", "ar", "ry", "ly", "ty", "cy", "sy", "ny", "my", "py",
        "ware", "pendent", "metry", "morphism", "pology", "graphy", "scopy",
        "ology", "ometry", "onomy", "opathy", "otherapy", "ectomy", "oscopy",
        "els", // added for "mod-els"
    ];

    for suffix in syllable_suffixes {
        if after == suffix || after.starts_with(suffix) {
            return true;
        }
    }

    // If "after" is short (2-4 chars) and looks like a word ending, flag it
    if after.len() <= 4 && before.len() >= 3 {
        let short_endings = ["er", "or", "ar", "ry", "ly", "ty", "cy", "ny", "my", "py", "gy", "ky"];
        if short_endings.iter().any(|e| after == *e) {
            return true;
        }
    }

    false
}

#[test]
#[ignore]
fn usenix_dict_hyphen_check() {
    let usenix_dir = usenix_dir();

    if !usenix_dir.exists() {
        eprintln!("Skipping: USENIX directory not found at {}", usenix_dir.display());
        return;
    }

    // Load dictionary
    let scowl = ScowlDictionary::embedded();
    println!("Loaded SCOWL dictionary with {} words", scowl.len());
    let dict: Arc<dyn hallucinator_parsing::Dictionary> = Arc::new(scowl);

    let pdfs = discover_pdfs(&usenix_dir);
    println!("Scanning {} USENIX 2025 PDFs for broken hyphenation...\n", pdfs.len());

    let mut broken_without_dict: HashMap<String, Vec<String>> = HashMap::new();
    let mut broken_with_dict: HashMap<String, Vec<String>> = HashMap::new();
    let mut papers_checked = 0;

    let hyphen_word_re = Regex::new(r"\b([a-zA-Z]+-[a-zA-Z]+)\b").unwrap();

    // Create extractors
    let extractor_heuristic = ReferenceExtractor::new();
    let extractor_dict = ReferenceExtractor::new().with_shared_dictionary(Arc::clone(&dict));

    for (i, pdf_path) in pdfs.iter().enumerate() {
        let stem = pdf_path.file_stem().unwrap().to_string_lossy().to_string();

        if i % 50 == 0 {
            eprint!("[{}/{}] Scanning...\r", i + 1, pdfs.len());
        }

        // Extract with heuristics
        let heuristic_result = match extractor_heuristic.extract_references_via_backend(pdf_path, &LocalMupdfBackend) {
            Ok(r) => r,
            Err(_) => continue,
        };

        // Extract with dictionary
        let dict_result = match extractor_dict.extract_references_via_backend(pdf_path, &LocalMupdfBackend) {
            Ok(r) => r,
            Err(_) => continue,
        };

        papers_checked += 1;

        let heuristic_titles = refs_to_titles(&heuristic_result);
        let dict_titles = refs_to_titles(&dict_result);

        // Check for broken patterns in heuristic extraction
        for title in &heuristic_titles {
            for cap in hyphen_word_re.captures_iter(title) {
                let word = &cap[1];
                if is_likely_broken_hyphen(word) {
                    broken_without_dict
                        .entry(word.to_lowercase())
                        .or_default()
                        .push(stem.clone());
                }
            }
        }

        // Check for broken patterns in dictionary extraction
        for title in &dict_titles {
            for cap in hyphen_word_re.captures_iter(title) {
                let word = &cap[1];
                if is_likely_broken_hyphen(word) {
                    broken_with_dict
                        .entry(word.to_lowercase())
                        .or_default()
                        .push(stem.clone());
                }
            }
        }
    }

    println!("\n==============================================================");
    println!("    Dictionary vs Heuristic Hyphenation - USENIX 2025");
    println!("==============================================================\n");
    println!("Papers checked: {}", papers_checked);
    println!("Broken patterns (heuristic): {}", broken_without_dict.len());
    println!("Broken patterns (dictionary): {}", broken_with_dict.len());

    // Find patterns fixed by dictionary
    let fixed_patterns: Vec<_> = broken_without_dict.keys()
        .filter(|k| !broken_with_dict.contains_key(*k))
        .cloned()
        .collect();

    println!("\n-- Patterns FIXED by dictionary ({}) --\n", fixed_patterns.len());
    for pattern in fixed_patterns.iter().take(30) {
        let count = broken_without_dict.get(pattern).map(|v| v.len()).unwrap_or(0);
        let merged: String = pattern.chars().filter(|c| *c != '-').collect();
        println!("  \"{}\" → \"{}\" (x{})", pattern, merged, count);
    }

    // Patterns still broken with dictionary
    if !broken_with_dict.is_empty() {
        println!("\n-- Patterns STILL broken with dictionary ({}) --\n", broken_with_dict.len());
        let mut sorted: Vec<_> = broken_with_dict.iter().collect();
        sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        for (pattern, papers) in sorted.iter().take(20) {
            println!("  \"{}\" (x{})", pattern, papers.len());
        }
    }

    println!("\n==============================================================");
}
