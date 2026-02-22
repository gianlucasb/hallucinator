//! Evaluate extraction on a per-reference basis
//!
//! Usage: cargo run --release --example evaluate_per_ref -- --corpus /path/to/papers

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use hallucinator_pdf::section::{find_references_section, segment_references_all_strategies};
use hallucinator_pdf::scoring::{score_segmentation, ScoringWeights};
use hallucinator_pdf::PdfParsingConfig;
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

    let config = PdfParsingConfig::default();
    let weights = ScoringWeights::default();

    let mut total_gt_refs = 0usize;
    let mut total_extracted_refs = 0usize;
    let mut total_matched_refs = 0usize;
    let mut papers_evaluated = 0usize;
    let mut papers_with_zero_matches = 0usize;

    // Track unmatched for analysis
    let mut sample_unmatched: Vec<(String, String, f64)> = Vec::new(); // (extracted, best_gt, score)

    for entry in fs::read_dir(corpus_dir)? {
        let path = entry?.path();
        if path.extension().map_or(false, |e| e == "pdf") {
            if let Some(tarball) = find_matching_gt(&path) {
                match evaluate_paper(&path, &tarball, &config, &weights) {
                    Ok((gt_count, extracted_count, matched_count, unmatched)) => {
                        total_gt_refs += gt_count;
                        total_extracted_refs += extracted_count;
                        total_matched_refs += matched_count;
                        papers_evaluated += 1;

                        if matched_count == 0 && extracted_count > 0 {
                            papers_with_zero_matches += 1;
                        }

                        // Collect sample unmatched
                        if sample_unmatched.len() < 50 {
                            sample_unmatched.extend(unmatched.into_iter().take(2));
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    eprintln!("\n=== Per-Reference Evaluation ===");
    eprintln!("Papers evaluated: {}", papers_evaluated);
    eprintln!("Papers with zero matches: {}", papers_with_zero_matches);
    eprintln!();
    eprintln!("Ground truth references: {}", total_gt_refs);
    eprintln!("Extracted references:    {}", total_extracted_refs);
    eprintln!("Matched references:      {}", total_matched_refs);
    eprintln!();

    let precision = total_matched_refs as f64 / total_extracted_refs.max(1) as f64;
    let recall = total_matched_refs as f64 / total_gt_refs.max(1) as f64;
    let f1 = if precision + recall > 0.0 {
        2.0 * precision * recall / (precision + recall)
    } else {
        0.0
    };

    eprintln!("Precision: {:.1}% ({}/{} extracted refs matched GT)",
        precision * 100.0, total_matched_refs, total_extracted_refs);
    eprintln!("Recall:    {:.1}% ({}/{} GT refs were extracted)",
        recall * 100.0, total_matched_refs, total_gt_refs);
    eprintln!("F1:        {:.3}", f1);

    // Show sample unmatched
    if !sample_unmatched.is_empty() {
        eprintln!("\n=== Sample Unmatched (extracted vs closest GT) ===");
        for (i, (extracted, gt, score)) in sample_unmatched.iter().take(20).enumerate() {
            eprintln!("\n[{}] Score: {:.3}", i + 1, score);
            eprintln!("  Extracted: {}", truncate(extracted, 70));
            eprintln!("  GT:        {}", truncate(gt, 70));
        }
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{}...", truncated)
    }
}

fn evaluate_paper(
    pdf_path: &Path,
    gt_path: &Path,
    config: &PdfParsingConfig,
    weights: &ScoringWeights,
) -> Result<(usize, usize, usize, Vec<(String, String, f64)>)> {
    let ground_truth = extract_bibtex_titles(gt_path)?;
    if ground_truth.is_empty() {
        anyhow::bail!("No ground truth");
    }

    let text = extract_text_from_pdf(pdf_path)?;
    let ref_section = find_references_section(&text)
        .ok_or_else(|| anyhow::anyhow!("No refs section"))?;

    let all_results = segment_references_all_strategies(&ref_section, config);

    // Select best strategy by score
    let best = all_results
        .into_iter()
        .max_by(|a, b| {
            let sa = score_segmentation(a, &ref_section, config, weights);
            let sb = score_segmentation(b, &ref_section, config, weights);
            sa.partial_cmp(&sb).unwrap()
        });

    let Some(result) = best else {
        return Ok((ground_truth.len(), 0, 0, vec![]));
    };

    // Extract titles from references and apply clean_title
    let extracted_titles: Vec<String> = result.references
        .iter()
        .filter_map(|r| {
            let (title, from_quotes) = hallucinator_pdf::title::extract_title_from_reference(r);
            if title.is_empty() {
                return None;
            }
            // Apply clean_title to remove trailing venue/metadata
            let cleaned = hallucinator_pdf::title::clean_title(&title, from_quotes);
            if cleaned.is_empty() || cleaned.split_whitespace().count() < config.min_title_words() {
                None
            } else {
                Some(cleaned)
            }
        })
        .collect();

    // Count matches
    let mut matched = 0;
    let mut unmatched = Vec::new();

    for et in &extracted_titles {
        let best_match = ground_truth.iter()
            .map(|gt| (gt, similarity(&normalize_title(et), &normalize_title(gt))))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        if let Some((gt, score)) = best_match {
            if score >= 0.90 {
                matched += 1;
            } else if score >= 0.70 {
                // Near miss - collect for analysis
                unmatched.push((et.clone(), gt.clone(), score));
            }
        }
    }

    Ok((ground_truth.len(), extracted_titles.len(), matched, unmatched))
}

fn similarity(a: &str, b: &str) -> f64 {
    strsim::jaro_winkler(a, b)
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
        if let Ok(page) = doc.load_page(page_num) {
            if let Ok(text_page) = page.to_text_page(mupdf::TextPageFlags::empty()) {
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
    }
    Ok(text)
}

fn extract_bibtex_titles(gt_path: &Path) -> Result<Vec<String>> {
    // Check if it's a direct bib/bbl file or a tarball
    let ext = gt_path.extension().and_then(|e| e.to_str()).unwrap_or("");

    if ext == "bib" || ext == "bbl" {
        // Direct bib/bbl file
        let contents = fs::read_to_string(gt_path)?;
        let titles = parse_bibtex_titles(&contents);
        if !titles.is_empty() {
            return Ok(titles);
        }
        anyhow::bail!("No titles in bib/bbl file")
    } else {
        // Tarball format
        let file = fs::File::open(gt_path)?;
        let gz = GzDecoder::new(file);
        let mut archive = Archive::new(gz);

        for entry in archive.entries()? {
            let mut entry = entry?;
            let path = entry.path()?.to_path_buf();
            if path.extension().map_or(false, |e| e == "bib" || e == "bbl") {
                let mut contents = String::new();
                entry.read_to_string(&mut contents)?;
                let titles = parse_bibtex_titles(&contents);
                if !titles.is_empty() {
                    return Ok(titles);
                }
            }
        }
        anyhow::bail!("No bib/bbl file found in tarball")
    }
}

fn parse_bibtex_titles(content: &str) -> Vec<String> {
    let mut titles = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.to_lowercase().starts_with("title") {
            if let Some(eq_pos) = line.find('=') {
                let value = line[eq_pos + 1..].trim();
                let title = if value.starts_with('{') {
                    extract_braced(value)
                } else if value.starts_with('"') {
                    extract_quoted(value)
                } else {
                    None
                };
                if let Some(t) = title {
                    if !t.is_empty() {
                        titles.push(t);
                    }
                }
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
            '}' => { depth -= 1; if depth == 0 { end = i; break; } }
            _ => {}
        }
    }
    if end > 0 { Some(s[..end].to_string()) } else { None }
}

fn extract_quoted(s: &str) -> Option<String> {
    let s = s.trim_start_matches('"');
    s.find('"').map(|end| s[..end].to_string())
}

fn find_matching_gt(pdf_path: &Path) -> Option<PathBuf> {
    let stem = pdf_path.file_stem()?.to_str()?;
    let parent = pdf_path.parent()?;

    // First, try direct bib/bbl files (new format)
    let bib = parent.join(format!("{}.bib", stem));
    if bib.exists() { return Some(bib); }
    let bbl = parent.join(format!("{}.bbl", stem));
    if bbl.exists() { return Some(bbl); }

    // Fall back to tarball (old format)
    let tarball = parent.join(format!("{}.tar.gz", stem));
    if tarball.exists() { return Some(tarball); }

    let arxiv_id: String = stem.chars().take_while(|c| *c != '_' && *c != '-').collect();
    for entry in fs::read_dir(parent).ok()? {
        let path = entry.ok()?.path();
        if path.extension().map_or(false, |e| e == "gz") {
            let name = path.file_name()?.to_str()?;
            if name.starts_with(&arxiv_id) && name.ends_with(".tar.gz") {
                return Some(path);
            }
        }
    }
    None
}
