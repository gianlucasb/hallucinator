//! Grid search for optimal scoring weights.
//!
//! Usage:
//!   cargo run --release --example optimize_weights -- --corpus /path/to/papers

use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use hallucinator_parsing::ParsingConfig;
use hallucinator_parsing::scoring::ScoringWeights;
use hallucinator_parsing::section::{
    SegmentationStrategy, find_references_section, segment_references_all_strategies,
};
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

    eprintln!("Loading papers from: {}", corpus_dir);

    // Load all paper data once
    let papers = load_all_papers(corpus_dir)?;
    eprintln!("Loaded {} papers", papers.len());

    if papers.is_empty() {
        eprintln!("No papers found!");
        return Ok(());
    }

    // Split 80/20 for train/test
    let split = (papers.len() * 8) / 10;
    let (train, test) = papers.split_at(split);
    eprintln!("Train: {}, Test: {}", train.len(), test.len());

    // Grid search
    let mut best_weights = ScoringWeights::default();
    let mut best_accuracy = 0.0;
    let mut best_avg_f1 = 0.0;

    let coverage_vals = [0.05, 0.10, 0.15, 0.20, 0.25];
    let completeness_vals = [0.20, 0.25, 0.30, 0.35, 0.40];
    let consistency_vals = [0.05, 0.10, 0.15, 0.20];
    let specificity_vals = [0.15, 0.20, 0.25, 0.30, 0.35];

    let total_combinations = coverage_vals.len()
        * completeness_vals.len()
        * consistency_vals.len()
        * specificity_vals.len();
    let mut checked = 0;

    for &w_cov in &coverage_vals {
        for &w_comp in &completeness_vals {
            for &w_cons in &consistency_vals {
                for &w_spec in &specificity_vals {
                    let w_count = 1.0 - w_cov - w_comp - w_cons - w_spec;

                    // Skip invalid combinations
                    if !(0.0..=0.35).contains(&w_count) {
                        continue;
                    }

                    checked += 1;

                    let weights = ScoringWeights {
                        coverage: w_cov,
                        completeness: w_comp,
                        consistency: w_cons,
                        specificity: w_spec,
                        count: w_count,
                    };

                    let (accuracy, avg_f1) = evaluate_weights(train, &weights);

                    // Prefer accuracy, then average F1
                    if accuracy > best_accuracy
                        || (accuracy == best_accuracy && avg_f1 > best_avg_f1)
                    {
                        best_accuracy = accuracy;
                        best_avg_f1 = avg_f1;
                        best_weights = weights.clone();
                        eprintln!(
                            "[{}/{}] New best: acc={:.1}%, avgF1={:.3} | cov={:.2}, comp={:.2}, cons={:.2}, spec={:.2}, cnt={:.2}",
                            checked,
                            total_combinations,
                            accuracy * 100.0,
                            avg_f1,
                            w_cov,
                            w_comp,
                            w_cons,
                            w_spec,
                            w_count
                        );
                    }
                }
            }
        }
    }

    // Evaluate on test set
    let (test_accuracy, test_avg_f1) = evaluate_weights(test, &best_weights);

    eprintln!("\n=== Final Results ===");
    eprintln!("Best weights:");
    eprintln!("  coverage:     {:.2}", best_weights.coverage);
    eprintln!("  completeness: {:.2}", best_weights.completeness);
    eprintln!("  consistency:  {:.2}", best_weights.consistency);
    eprintln!("  specificity:  {:.2}", best_weights.specificity);
    eprintln!("  count:        {:.2}", best_weights.count);
    eprintln!();
    eprintln!(
        "Train accuracy: {:.1}% (avg F1: {:.3})",
        best_accuracy * 100.0,
        best_avg_f1
    );
    eprintln!(
        "Test accuracy:  {:.1}% (avg F1: {:.3})",
        test_accuracy * 100.0,
        test_avg_f1
    );

    // Print Rust code
    println!("\n// Optimized weights for ScoringWeights::default()");
    println!("Self {{");
    println!("    coverage: {:.2},", best_weights.coverage);
    println!("    completeness: {:.2},", best_weights.completeness);
    println!("    consistency: {:.2},", best_weights.consistency);
    println!("    specificity: {:.2},", best_weights.specificity);
    println!("    count: {:.2},", best_weights.count);
    println!("}}");

    Ok(())
}

struct PaperData {
    #[allow(dead_code)]
    name: String,
    strategy_metrics: Vec<StrategyMetrics>,
}

struct StrategyMetrics {
    strategy: SegmentationStrategy,
    f1: f64,
    // Raw metrics for scoring
    coverage: f64,
    completeness: f64,
    consistency: f64,
    count: usize,
}

fn load_all_papers(corpus_dir: &str) -> Result<Vec<PaperData>> {
    let mut papers = Vec::new();
    let config = ParsingConfig::default();

    for entry in fs::read_dir(corpus_dir)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "pdf")
            && let Some(tarball) = find_matching_tarball(&path)
        {
            match load_paper_data(&path, &tarball, &config) {
                Ok(data) => papers.push(data),
                Err(e) => {
                    // Silently skip errors during loading
                    let _ = e;
                }
            }
        }
    }

    Ok(papers)
}

fn load_paper_data(
    pdf_path: &Path,
    tarball_path: &Path,
    config: &ParsingConfig,
) -> Result<PaperData> {
    let ground_truth = extract_bibtex_titles(tarball_path)?;
    if ground_truth.is_empty() {
        anyhow::bail!("No BibTeX entries");
    }

    let text = extract_text_from_pdf(pdf_path)?;
    let ref_section =
        find_references_section(&text).ok_or_else(|| anyhow::anyhow!("No refs section"))?;

    let all_results = segment_references_all_strategies(&ref_section, config);

    let mut strategy_metrics = Vec::new();
    for result in all_results {
        let ref_texts: Vec<String> = result.references.iter().map(|r| r.text.clone()).collect();
        let (_, _, f1) = compute_f1(&ref_texts, &ground_truth, config);

        // Compute raw metrics
        let total_ref_len: usize = result.references.iter().map(|r| r.text.len()).sum();
        let coverage = (total_ref_len as f64 / ref_section.len().max(1) as f64).min(1.0);

        let complete_count = result
            .references
            .iter()
            .filter(|r| has_extractable_content(&r.text, config))
            .count();
        let completeness = if result.references.is_empty() {
            0.0
        } else {
            complete_count as f64 / result.references.len() as f64
        };

        let consistency =
            1.0 - coefficient_of_variation(result.references.iter().map(|r| r.text.len()));

        strategy_metrics.push(StrategyMetrics {
            strategy: result.strategy,
            f1,
            coverage,
            completeness,
            consistency,
            count: result.references.len(),
        });
    }

    Ok(PaperData {
        name: pdf_path.file_name().unwrap().to_string_lossy().to_string(),
        strategy_metrics,
    })
}

fn evaluate_weights(papers: &[PaperData], weights: &ScoringWeights) -> (f64, f64) {
    let mut correct = 0;
    let mut total_best_f1 = 0.0;

    for paper in papers {
        if paper.strategy_metrics.is_empty() {
            continue;
        }

        // Find best by F1 (oracle)
        let best_f1_idx = paper
            .strategy_metrics
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.f1.partial_cmp(&b.1.f1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        // Find best by score
        let best_score_idx = paper
            .strategy_metrics
            .iter()
            .enumerate()
            .max_by(|a, b| {
                let score_a = compute_score(a.1, weights);
                let score_b = compute_score(b.1, weights);
                score_a.partial_cmp(&score_b).unwrap()
            })
            .map(|(i, _)| i)
            .unwrap();

        if best_f1_idx == best_score_idx {
            correct += 1;
        }

        total_best_f1 += paper.strategy_metrics[best_f1_idx].f1;
    }

    let accuracy = correct as f64 / papers.len().max(1) as f64;
    let avg_f1 = total_best_f1 / papers.len().max(1) as f64;
    (accuracy, avg_f1)
}

fn compute_score(metrics: &StrategyMetrics, weights: &ScoringWeights) -> f64 {
    let specificity = metrics.strategy.specificity_score();
    let count_score = (metrics.count as f64 / 50.0).min(1.0);

    // Plausibility penalty (simplified - assume 300 chars per ref)
    let expected_min = 1; // Can't compute without text length
    let plausibility = if metrics.count < expected_min {
        metrics.count as f64 / expected_min as f64
    } else {
        1.0
    };

    let base = weights.coverage * metrics.coverage
        + weights.completeness * metrics.completeness
        + weights.consistency * metrics.consistency
        + weights.specificity * specificity
        + weights.count * count_score;

    base * plausibility
}

fn coefficient_of_variation(lengths: impl Iterator<Item = usize>) -> f64 {
    let lengths: Vec<f64> = lengths.map(|l| l as f64).collect();
    if lengths.len() < 2 {
        return 0.0;
    }
    let mean = lengths.iter().sum::<f64>() / lengths.len() as f64;
    if mean == 0.0 {
        return 1.0;
    }
    let variance = lengths.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / lengths.len() as f64;
    (variance.sqrt() / mean).min(1.0)
}

fn has_extractable_content(raw_ref: &str, config: &ParsingConfig) -> bool {
    let (title, _) = hallucinator_parsing::title::extract_title_from_reference(raw_ref);
    let has_title =
        !title.is_empty() && title.split_whitespace().count() >= config.min_title_words();

    let authors = hallucinator_parsing::authors::extract_authors_from_reference(raw_ref);
    let has_authors = !authors.is_empty() && !authors.iter().all(|a| a == "__SAME_AS_PREVIOUS__");

    has_title && has_authors
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

fn compute_f1(
    extracted: &[String],
    ground_truth: &[String],
    config: &ParsingConfig,
) -> (f64, f64, f64) {
    let extracted_titles: Vec<String> = extracted
        .iter()
        .filter_map(|r| {
            let (title, _) = hallucinator_parsing::title::extract_title_from_reference(r);
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
