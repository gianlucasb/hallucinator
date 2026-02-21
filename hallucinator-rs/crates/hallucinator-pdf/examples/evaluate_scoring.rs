//! Evaluate segmentation scoring against ground truth BibTeX files.
//!
//! This example evaluates the scoring-based segmentation approach by comparing
//! extracted references against ground truth BibTeX files from arXiv papers.
//!
//! Usage:
//!   cargo run --example evaluate_scoring -- --corpus /path/to/papers
//!
//! The corpus directory should contain:
//! - PDF files (*.pdf)
//! - Matching tar.gz files containing .bib files
//!
//! Output: CSV with per-paper, per-strategy metrics and summary statistics.

use std::collections::HashMap;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use hallucinator_pdf::section::{segment_references_all_strategies, SegmentationStrategy};
use hallucinator_pdf::scoring::{score_segmentation, ScoringWeights};
use hallucinator_pdf::{PdfParsingConfig, section::find_references_section};
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

    eprintln!("Evaluating papers in: {}", corpus_dir);

    let mut results = Vec::new();

    for entry in fs::read_dir(corpus_dir).context("Failed to read corpus directory")? {
        let path = entry?.path();
        if path.extension().map_or(false, |e| e == "pdf") {
            // Find matching tar.gz
            let tarball = find_matching_tarball(&path);
            if let Some(tarball_path) = tarball {
                match evaluate_paper(&path, &tarball_path) {
                    Ok(eval) => results.push(eval),
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to evaluate {}: {}",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    if results.is_empty() {
        eprintln!("No papers evaluated. Check that the corpus directory contains PDF files with matching .tar.gz archives.");
        return Ok(());
    }

    // Output CSV
    println!("pdf,strategy,f1,precision,recall,score,ref_count,gt_count");
    for r in &results {
        for s in &r.strategy_results {
            println!(
                "{},{:?},{:.3},{:.3},{:.3},{:.3},{},{}",
                r.pdf_name,
                s.strategy,
                s.f1,
                s.precision,
                s.recall,
                s.score,
                s.ref_count,
                r.ground_truth_count
            );
        }
    }

    // Summary statistics
    print_summary(&results);

    Ok(())
}

struct PaperEvaluation {
    pdf_name: String,
    ground_truth_count: usize,
    strategy_results: Vec<StrategyEval>,
}

struct StrategyEval {
    strategy: SegmentationStrategy,
    ref_count: usize,
    precision: f64,
    recall: f64,
    f1: f64,
    score: f64,
}

fn evaluate_paper(pdf_path: &Path, tarball_path: &Path) -> Result<PaperEvaluation> {
    let config = PdfParsingConfig::default();
    let weights = ScoringWeights::default();

    // Extract ground truth
    let ground_truth = extract_bibtex_titles(tarball_path)
        .context("Failed to extract BibTeX titles")?;

    if ground_truth.is_empty() {
        anyhow::bail!("No BibTeX entries found");
    }

    // Extract text from PDF
    let text = extract_text_from_pdf(pdf_path)?;

    // Find references section
    let ref_section = find_references_section(&text)
        .ok_or_else(|| anyhow::anyhow!("No references section found"))?;

    // Run all strategies
    let all_results = segment_references_all_strategies(&ref_section, &config);

    let mut strategy_results = Vec::new();
    for result in all_results {
        let score = score_segmentation(&result, &ref_section, &config, &weights);
        let (precision, recall, f1) = compute_f1(&result.references, &ground_truth, &config);

        strategy_results.push(StrategyEval {
            strategy: result.strategy,
            ref_count: result.references.len(),
            precision,
            recall,
            f1,
            score,
        });
    }

    Ok(PaperEvaluation {
        pdf_name: pdf_path.file_name().unwrap().to_string_lossy().to_string(),
        ground_truth_count: ground_truth.len(),
        strategy_results,
    })
}

fn extract_text_from_pdf(path: &Path) -> Result<String> {
    let doc = Document::open(path.to_str().unwrap())
        .context("Failed to open PDF")?;

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

fn compute_f1(
    extracted: &[String],
    ground_truth: &[String],
    config: &PdfParsingConfig,
) -> (f64, f64, f64) {
    // Extract titles from raw references
    let extracted_titles: Vec<String> = extracted
        .iter()
        .filter_map(|r| {
            let (title, _) =
                hallucinator_pdf::title::extract_title_from_reference(r);
            if title.is_empty() || title.split_whitespace().count() < config.min_title_words() {
                None
            } else {
                Some(title)
            }
        })
        .collect();

    if extracted_titles.is_empty() {
        return (0.0, 0.0, 0.0);
    }

    let mut matches = 0;
    for et in &extracted_titles {
        if ground_truth.iter().any(|gt| fuzzy_match(et, gt)) {
            matches += 1;
        }
    }

    let precision = matches as f64 / extracted_titles.len() as f64;
    let recall = if ground_truth.is_empty() {
        0.0
    } else {
        matches as f64 / ground_truth.len() as f64
    };
    let f1 = if precision + recall == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };

    (precision, recall, f1)
}

fn fuzzy_match(a: &str, b: &str) -> bool {
    let a_norm = normalize_title(a);
    let b_norm = normalize_title(b);

    let similarity = strsim::jaro_winkler(&a_norm, &b_norm);
    similarity >= 0.90
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

fn extract_bibtex_titles(tarball_path: &Path) -> Result<Vec<String>> {
    let file = fs::File::open(tarball_path)?;
    let gz = GzDecoder::new(file);
    let mut archive = Archive::new(gz);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path.extension().map_or(false, |e| e == "bib") {
            let mut contents = String::new();
            entry.read_to_string(&mut contents)?;
            return Ok(parse_bibtex_titles(&contents));
        }
    }

    anyhow::bail!("No .bib file found in archive")
}

fn parse_bibtex_titles(bib_content: &str) -> Vec<String> {
    let mut titles = Vec::new();

    // Simple regex-free parsing for title = {Title Here} or title = "Title Here"
    for line in bib_content.lines() {
        let line = line.trim();
        if line.to_lowercase().starts_with("title") {
            // Find the value after the = sign
            if let Some(eq_pos) = line.find('=') {
                let value = line[eq_pos + 1..].trim();
                // Extract content within {} or ""
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
    if let Some(end) = s.find('"') {
        Some(s[..end].to_string())
    } else {
        None
    }
}

fn find_matching_tarball(pdf_path: &Path) -> Option<PathBuf> {
    let stem = pdf_path.file_stem()?.to_str()?;
    let parent = pdf_path.parent()?;

    // Try exact match first
    let tarball = parent.join(format!("{}.tar.gz", stem));
    if tarball.exists() {
        return Some(tarball);
    }

    // Try matching by arxiv ID prefix (before first underscore or dash)
    let arxiv_id: String = stem
        .chars()
        .take_while(|c| *c != '_' && *c != '-')
        .collect();

    for entry in fs::read_dir(parent).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "gz") {
            let name = path.file_name()?.to_str()?;
            if name.starts_with(&arxiv_id) && name.ends_with(".tar.gz") {
                return Some(path);
            }
        }
    }

    None
}

fn print_summary(results: &[PaperEvaluation]) {
    eprintln!("\n=== Summary ===");
    eprintln!("Papers evaluated: {}", results.len());

    // Count how often each strategy wins (best F1)
    let mut wins: HashMap<SegmentationStrategy, usize> = HashMap::new();
    let mut scoring_correct = 0;

    for r in results {
        if let Some(best_f1) = r
            .strategy_results
            .iter()
            .max_by(|a, b| a.f1.partial_cmp(&b.f1).unwrap())
        {
            *wins.entry(best_f1.strategy).or_insert(0) += 1;

            // Check if scoring picked the same strategy
            if let Some(best_score) = r
                .strategy_results
                .iter()
                .max_by(|a, b| a.score.partial_cmp(&b.score).unwrap())
            {
                if best_score.strategy == best_f1.strategy {
                    scoring_correct += 1;
                }
            }
        }
    }

    eprintln!("\nBest strategy by F1:");
    let mut wins_vec: Vec<_> = wins.iter().collect();
    wins_vec.sort_by(|a, b| b.1.cmp(a.1));
    for (strategy, count) in wins_vec {
        eprintln!("  {:?}: {} papers", strategy, count);
    }

    eprintln!(
        "\nScoring accuracy: {}/{} ({:.1}%)",
        scoring_correct,
        results.len(),
        100.0 * scoring_correct as f64 / results.len().max(1) as f64
    );
}
