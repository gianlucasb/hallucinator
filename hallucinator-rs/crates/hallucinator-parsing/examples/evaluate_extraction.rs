//! Evaluate title extraction accuracy
//!
//! For each extracted reference, check if its title matches any entry in bib/bbl.
//! This measures: "When we extract a title, is it correct?"
//!
//! Usage: cargo run --release --example evaluate_extraction -- --corpus /path/to/papers

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use hallucinator_parsing::ParsingConfig;
use hallucinator_parsing::scoring::{ScoringWeights, score_segmentation};
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

    let config = ParsingConfig::default();
    let weights = ScoringWeights::default();

    let mut total_extracted = 0usize;
    let mut total_matched = 0usize;
    let mut papers_evaluated = 0usize;

    // Track near-misses for analysis
    let mut near_misses: Vec<(String, String, f64)> = Vec::new();
    let mut no_match: Vec<String> = Vec::new();

    for entry in fs::read_dir(corpus_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "pdf")
            && let Some(tarball) = find_matching_tarball(&path)
            && let Ok((extracted, matched, paper_near_misses, paper_no_match)) =
                evaluate_paper(&path, &tarball, &config, &weights)
        {
            total_extracted += extracted;
            total_matched += matched;
            papers_evaluated += 1;

            // Collect samples
            if near_misses.len() < 30 {
                near_misses.extend(paper_near_misses.into_iter().take(2));
            }
            if no_match.len() < 30 {
                no_match.extend(paper_no_match.into_iter().take(2));
            }
        }
    }

    let accuracy = total_matched as f64 / total_extracted.max(1) as f64;

    eprintln!("\n=== Title Extraction Accuracy ===");
    eprintln!("Papers evaluated: {}", papers_evaluated);
    eprintln!("Total titles extracted: {}", total_extracted);
    eprintln!("Titles matching bib/bbl: {}", total_matched);
    eprintln!();
    eprintln!("Accuracy: {:.1}%", accuracy * 100.0);
    eprintln!(
        "  (When we extract a title, {:.1}% match ground truth)",
        accuracy * 100.0
    );

    // Show near-misses (0.80 <= score < 0.90)
    if !near_misses.is_empty() {
        eprintln!("\n=== Near Misses (0.80-0.90 similarity) ===");
        for (i, (extracted, gt, score)) in near_misses.iter().take(15).enumerate() {
            eprintln!("\n[{}] Score: {:.3}", i + 1, score);
            eprintln!("  Extracted: {}", truncate(extracted, 70));
            eprintln!("  GT:        {}", truncate(gt, 70));
        }
    }

    // Show complete failures (best match < 0.70)
    if !no_match.is_empty() {
        eprintln!("\n=== No Match Found (<0.70 similarity) ===");
        for (i, title) in no_match.iter().take(10).enumerate() {
            eprintln!("[{}] {}", i + 1, truncate(title, 80));
        }
    }

    Ok(())
}

/// (extracted_count, matched_count, near_misses, no_match_titles)
type EvalResult = (usize, usize, Vec<(String, String, f64)>, Vec<String>);

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max).collect::<String>())
    }
}

fn evaluate_paper(
    pdf_path: &Path,
    tarball_path: &Path,
    config: &ParsingConfig,
    weights: &ScoringWeights,
) -> Result<EvalResult> {
    let ground_truth = extract_bibtex_titles(tarball_path)?;
    if ground_truth.is_empty() {
        anyhow::bail!("No ground truth");
    }

    let text = extract_text_from_pdf(pdf_path)?;
    let ref_section =
        find_references_section(&text).ok_or_else(|| anyhow::anyhow!("No refs section"))?;

    let all_results = segment_references_all_strategies(&ref_section, config);

    // Select best strategy by score
    let best = all_results.into_iter().max_by(|a, b| {
        let sa = score_segmentation(a, &ref_section, config, weights);
        let sb = score_segmentation(b, &ref_section, config, weights);
        sa.partial_cmp(&sb).unwrap()
    });

    let Some(result) = best else {
        return Ok((0, 0, vec![], vec![]));
    };

    // Extract titles from references
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

    let mut matched = 0;
    let mut near_misses = Vec::new();
    let mut no_match = Vec::new();

    for et in &extracted_titles {
        let et_norm = normalize_title(et);

        let best_match = ground_truth
            .iter()
            .map(|gt| {
                let gt_norm = normalize_title(gt);
                (gt, similarity(&et_norm, &gt_norm))
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        if let Some((gt, score)) = best_match {
            if score >= 0.90 {
                matched += 1;
            } else if score >= 0.80 {
                near_misses.push((et.clone(), gt.clone(), score));
            } else if score < 0.70 {
                no_match.push(et.clone());
            }
        } else {
            no_match.push(et.clone());
        }
    }

    Ok((extracted_titles.len(), matched, near_misses, no_match))
}

fn similarity(a: &str, b: &str) -> f64 {
    strsim::jaro_winkler(a, b)
}

fn normalize_title(s: &str) -> String {
    // Remove common artifacts
    let s = s.replace("- ", "").replace("-\n", "");

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

    // Handle multi-line title fields
    let mut in_title = false;
    let mut current_title = String::new();
    let mut brace_depth = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.to_lowercase().starts_with("title")
            && let Some(eq_pos) = trimmed.find('=')
        {
            let value = trimmed[eq_pos + 1..].trim();
            if value.starts_with('{') {
                in_title = true;
                current_title.clear();
                brace_depth = 0;
                for c in value.chars() {
                    match c {
                        '{' => brace_depth += 1,
                        '}' => {
                            brace_depth -= 1;
                            if brace_depth == 0 {
                                in_title = false;
                                if !current_title.is_empty() {
                                    titles.push(current_title.trim().to_string());
                                }
                                break;
                            }
                        }
                        _ => {
                            if brace_depth > 0 {
                                current_title.push(c);
                            }
                        }
                    }
                }
            } else if value.starts_with('"')
                && let Some(title) = extract_quoted(value)
            {
                titles.push(title);
            }
        } else if in_title {
            // Continue multi-line title
            for c in trimmed.chars() {
                match c {
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth -= 1;
                        if brace_depth == 0 {
                            in_title = false;
                            if !current_title.is_empty() {
                                titles.push(current_title.trim().to_string());
                            }
                            break;
                        }
                    }
                    _ => {
                        if brace_depth > 0 {
                            current_title.push(c);
                        }
                    }
                }
            }
            if in_title {
                current_title.push(' ');
            }
        }
    }

    titles
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
