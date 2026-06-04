//! Debug F1=0 cases to see if it's a matching or extraction problem
//!
//! Usage: cargo run --release --example debug_f1_zero -- --corpus /path/to/papers

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use hallucinator_parsing::ParsingConfig;
use hallucinator_parsing::section::{find_references_section, segment_references_all_strategies};
use mupdf::Document;
use tar::Archive;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let corpus_dir = args
        .iter()
        .position(|a| a == "--corpus")
        .and_then(|i| args.get(i + 1))
        .map(|s| s.as_str())
        .unwrap_or("./papers");

    let limit = args
        .iter()
        .position(|a| a == "--limit")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let config = ParsingConfig::default();
    let mut checked = 0;

    for entry in fs::read_dir(corpus_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "pdf")
            && let Some(tarball) = find_matching_tarball(&path)
            && let Ok(()) = check_paper(&path, &tarball, &config)
        {
            checked += 1;
            if checked >= limit {
                break;
            }
        }
    }

    Ok(())
}

fn check_paper(pdf_path: &Path, tarball_path: &Path, config: &ParsingConfig) -> Result<()> {
    let ground_truth = extract_bibtex_titles(tarball_path)?;
    if ground_truth.is_empty() {
        return Ok(());
    }

    let text = extract_text_from_pdf(pdf_path)?;
    let ref_section =
        find_references_section(&text).ok_or_else(|| anyhow::anyhow!("No refs section"))?;

    let all_results = segment_references_all_strategies(&ref_section, config);

    // Find the strategy with most references
    let best = all_results.iter().max_by_key(|r| r.references.len());

    if let Some(result) = best {
        // Extract titles
        let extracted_titles: Vec<String> = result
            .references
            .iter()
            .filter_map(|r| {
                let r = &r.text;
                let (title, _) = hallucinator_parsing::title::extract_title_from_reference(r);
                if title.is_empty() || title.split_whitespace().count() < config.min_title_words() {
                    None
                } else {
                    Some(title)
                }
            })
            .collect();

        // Compute matches
        let mut matches = 0;
        for et in &extracted_titles {
            if ground_truth.iter().any(|gt| fuzzy_match(et, gt)) {
                matches += 1;
            }
        }

        let f1 = if extracted_titles.is_empty() || ground_truth.is_empty() {
            0.0
        } else {
            let precision = matches as f64 / extracted_titles.len() as f64;
            let recall = matches as f64 / ground_truth.len() as f64;
            if precision + recall == 0.0 {
                0.0
            } else {
                2.0 * precision * recall / (precision + recall)
            }
        };

        // Only show F1=0 cases where we extracted some titles
        if f1 < 0.1 && !extracted_titles.is_empty() {
            println!("\n{}", "=".repeat(80));
            println!("PDF: {}", pdf_path.file_name().unwrap().to_string_lossy());
            println!(
                "Strategy: {:?}, {} refs extracted, {} titles extracted",
                result.strategy,
                result.references.len(),
                extracted_titles.len()
            );
            println!("Ground truth: {} titles", ground_truth.len());
            println!("F1: {:.3}", f1);

            println!("\n--- EXTRACTED TITLES (first 5) ---");
            for (i, t) in extracted_titles.iter().take(5).enumerate() {
                println!("  [{}] {}", i + 1, t);
            }

            println!("\n--- GROUND TRUTH (first 5) ---");
            for (i, t) in ground_truth.iter().take(5).enumerate() {
                println!("  [{}] {}", i + 1, t);
            }

            // Show close matches
            println!("\n--- CLOSE MATCHES ---");
            for et in extracted_titles.iter().take(5) {
                let best_match = ground_truth
                    .iter()
                    .map(|gt| {
                        (
                            gt,
                            strsim::jaro_winkler(&normalize_title(et), &normalize_title(gt)),
                        )
                    })
                    .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                if let Some((gt, score)) = best_match
                    && score > 0.7
                {
                    println!("  Extracted: {}", truncate(et, 60));
                    println!("  GT:        {}", truncate(gt, 60));
                    println!("  Score:     {:.3}", score);
                    println!();
                }
            }
        }
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

fn fuzzy_match(a: &str, b: &str) -> bool {
    let a_norm = normalize_title(a);
    let b_norm = normalize_title(b);
    strsim::jaro_winkler(&a_norm, &b_norm) >= 0.90
}

fn normalize_title(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn extract_text_from_pdf(path: &Path) -> Result<String> {
    let doc = Document::open(path.to_str().unwrap()).context("Failed to open PDF")?;
    let mut text = String::new();
    for page_num in 0..doc.page_count()? {
        if let Ok(page) = doc.load_page(page_num)
            && let Ok(text_page) = page.to_text_page(mupdf::TextPageFlags::empty())
        {
            for block in text_page.blocks() {
                for line in block.lines() {
                    for ch in line.chars() {
                        if let Some(c) = ch.char() {
                            text.push(c);
                        }
                    }
                }
                text.push('\n');
            }
        }
    }
    Ok(text)
}

fn extract_bibtex_titles(tarball_path: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(tarball_path)?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path.extension().is_some_and(|e| e == "bib" || e == "bbl") {
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            let titles = parse_bibtex_titles(&contents);
            if !titles.is_empty() {
                return Ok(titles);
            }
        }
    }
    anyhow::bail!("No bib/bbl file found")
}

fn parse_bibtex_titles(content: &str) -> Vec<String> {
    let mut titles = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.to_lowercase().starts_with("title")
            && let Some(eq_pos) = line.find('=')
        {
            let value = line[eq_pos + 1..].trim();
            let title = if value.starts_with('{') {
                extract_braced(value)
            } else if value.starts_with('"') {
                extract_quoted(value)
            } else {
                None
            };
            if let Some(t) = title
                && !t.is_empty()
            {
                titles.push(t);
            }
        }
    }
    titles
}

fn extract_braced(s: &str) -> Option<String> {
    let s = s.trim_start_matches('{');
    let mut depth = 1;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        match c {
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
    if end > 0 {
        Some(s[..end].to_string())
    } else {
        None
    }
}

fn extract_quoted(s: &str) -> Option<String> {
    let s = s.trim_start_matches('"');
    s.find('"').map(|end| s[..end].to_string())
}

fn find_matching_tarball(pdf_path: &Path) -> Option<PathBuf> {
    let stem = pdf_path.file_stem()?.to_str()?;
    let parent = pdf_path.parent()?;

    let tarball = parent.join(format!("{}.tar.gz", stem));
    if tarball.exists() {
        return Some(tarball);
    }

    let arxiv_id: String = stem
        .chars()
        .take_while(|c| *c != '_' && *c != '-')
        .collect();
    for entry in fs::read_dir(parent).ok()? {
        let path = entry.ok()?.path();
        if path.extension().is_some_and(|e| e == "gz") {
            let name = path.file_name()?.to_str()?;
            if name.starts_with(&arxiv_id) && name.ends_with(".tar.gz") {
                return Some(path);
            }
        }
    }
    None
}
