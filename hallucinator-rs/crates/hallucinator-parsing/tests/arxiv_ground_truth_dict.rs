//! Ground truth test for PDF extraction with dictionary-based hyphenation.
//!
//! This test is identical to `arxiv_ground_truth.rs` but uses the SCOWL dictionary
//! for hyphenation fixing instead of heuristics.
//!
//! Run with:
//!   cargo test -p hallucinator-parsing --test arxiv_ground_truth_dict -- --ignored --nocapture

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hallucinator_bbl::{extract_references_from_bbl_str, extract_references_from_bib_str};
use hallucinator_core::matching::{normalize_title, titles_match};
use hallucinator_core::{BackendError, PdfBackend};
use hallucinator_parsing::{ExtractionResult, Reference, ReferenceExtractor};
use hallucinator_scowl::ScowlDictionary;

/// Local mupdf backend for use in this integration test.
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

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

struct PaperPair {
    stem: String,
    pdf_path: PathBuf,
    bbl_path: Option<PathBuf>,
    bib_path: Option<PathBuf>,
}

struct GroundTruth {
    source: &'static str,
    titles: Vec<String>,
}

struct NearMiss {
    pdf_title: String,
    gt_title: String,
    score: f64,
}

struct PaperResult {
    stem: String,
    gt_source: &'static str,
    gt_count: usize,
    pdf_count: usize,
    matched: usize,
    unmatched: usize,
    no_title: usize,
    near_misses: Vec<NearMiss>,
    error: Option<String>,
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("cannot get home dir")
        .join("Data")
        .join("arxiv-ground-truth")
}

fn discover_paper_pairs(dir: &Path) -> Vec<PaperPair> {
    let mut pdf_stems: HashSet<String> = HashSet::new();
    let mut bbl_stems: HashSet<String> = HashSet::new();
    let mut bib_stems: HashSet<String> = HashSet::new();

    let entries = std::fs::read_dir(dir).expect("cannot read data directory");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(stem) = name.strip_suffix(".pdf") {
            pdf_stems.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".bbl") {
            bbl_stems.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".bib") {
            bib_stems.insert(stem.to_string());
        }
    }

    // Only include PDFs that have at least one ground truth file
    let gt_stems: HashSet<_> = bbl_stems.union(&bib_stems).cloned().collect();
    let mut pairs: Vec<PaperPair> = pdf_stems
        .intersection(&gt_stems)
        .map(|stem| PaperPair {
            stem: stem.clone(),
            pdf_path: dir.join(format!("{stem}.pdf")),
            bbl_path: if bbl_stems.contains(stem) {
                Some(dir.join(format!("{stem}.bbl")))
            } else {
                None
            },
            bib_path: if bib_stems.contains(stem) {
                Some(dir.join(format!("{stem}.bib")))
            } else {
                None
            },
        })
        .collect();

    pairs.sort_by(|a, b| a.stem.cmp(&b.stem));
    pairs
}

// ---------------------------------------------------------------------------
// Ground truth extraction
// ---------------------------------------------------------------------------

fn deduplicate_titles(titles: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for title in titles {
        let normalized = normalize_title(&title);
        if !normalized.is_empty() && seen.insert(normalized) {
            result.push(title);
        }
    }
    result
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

fn build_ground_truth(pair: &PaperPair) -> Option<GroundTruth> {
    // Prefer .bbl — it reflects exactly what's compiled into the PDF.
    if let Some(bbl_path) = &pair.bbl_path {
        if let Ok(content) = std::fs::read_to_string(bbl_path) {
            if let Ok(result) = extract_references_from_bbl_str(&content) {
                let titles = deduplicate_titles(refs_to_titles(&result));
                if !titles.is_empty() {
                    return Some(GroundTruth {
                        source: "bbl",
                        titles,
                    });
                }
            }
        }
    }

    // Fall back to .bib (superset — may contain uncited entries).
    if let Some(bib_path) = &pair.bib_path {
        if let Ok(content) = std::fs::read_to_string(bib_path) {
            if let Ok(result) = extract_references_from_bib_str(&content) {
                let titles = deduplicate_titles(refs_to_titles(&result));
                if !titles.is_empty() {
                    return Some(GroundTruth {
                        source: "bib",
                        titles,
                    });
                }
            }
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

fn best_match_score(pdf_title: &str, gt_titles: &[String]) -> (Option<usize>, f64) {
    let norm_pdf = normalize_title(pdf_title);
    if norm_pdf.is_empty() {
        return (None, 0.0);
    }

    let mut best_idx = None;
    let mut best_score: f64 = 0.0;

    for (i, gt) in gt_titles.iter().enumerate() {
        let norm_gt = normalize_title(gt);
        if norm_gt.is_empty() {
            continue;
        }
        let score = rapidfuzz::fuzz::ratio(norm_pdf.chars(), norm_gt.chars());
        if score > best_score {
            best_score = score;
            best_idx = Some(i);
        }
    }

    (best_idx, best_score)
}

fn evaluate_paper(
    pdf_refs: &[Reference],
    gt: &GroundTruth,
) -> (usize, usize, usize, Vec<NearMiss>) {
    let mut matched = 0usize;
    let mut unmatched = 0usize;
    let mut no_title = 0usize;
    let mut near_misses = Vec::new();

    for pdf_ref in pdf_refs {
        if pdf_ref.skip_reason.is_some() {
            continue;
        }

        let title = match &pdf_ref.title {
            Some(t) if !t.is_empty() => t,
            _ => {
                no_title += 1;
                continue;
            }
        };

        let is_match = gt.titles.iter().any(|gt_t| titles_match(title, gt_t));

        if is_match {
            matched += 1;
        } else {
            let (best_idx, best_score) = best_match_score(title, &gt.titles);
            if (80.0..95.0).contains(&best_score)
                && let Some(idx) = best_idx
            {
                near_misses.push(NearMiss {
                    pdf_title: title.clone(),
                    gt_title: gt.titles[idx].clone(),
                    score: best_score,
                });
            }
            unmatched += 1;
        }
    }

    (matched, unmatched, no_title, near_misses)
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

fn print_report(results: &[PaperResult], total_papers: usize) {
    let successes: Vec<&PaperResult> = results.iter().filter(|r| r.error.is_none()).collect();
    let failures: Vec<&PaperResult> = results.iter().filter(|r| r.error.is_some()).collect();

    let bbl_count = successes.iter().filter(|r| r.gt_source == "bbl").count();
    let bib_count = successes.iter().filter(|r| r.gt_source == "bib").count();

    let total_pdf_refs: usize = successes
        .iter()
        .map(|r| r.matched + r.unmatched + r.no_title)
        .sum();
    let total_matched: usize = successes.iter().map(|r| r.matched).sum();
    let total_unmatched: usize = successes.iter().map(|r| r.unmatched).sum();
    let total_no_title: usize = successes.iter().map(|r| r.no_title).sum();

    let mut recalls: Vec<f64> = successes
        .iter()
        .filter(|r| r.matched + r.unmatched > 0)
        .map(|r| r.matched as f64 / (r.matched + r.unmatched) as f64 * 100.0)
        .collect();
    recalls.sort_by(|a, b| a.partial_cmp(b).unwrap());

    let perfect = recalls.iter().filter(|&&r| r >= 99.99).count();
    let above_90 = recalls
        .iter()
        .filter(|&&r| (90.0..99.99).contains(&r))
        .count();
    let above_80 = recalls
        .iter()
        .filter(|&&r| (80.0..90.0).contains(&r))
        .count();
    let below_80 = recalls.iter().filter(|&&r| r < 80.0).count();

    let median = if recalls.is_empty() {
        0.0
    } else {
        recalls[recalls.len() / 2]
    };

    let mean = if recalls.is_empty() {
        0.0
    } else {
        recalls.iter().sum::<f64>() / recalls.len() as f64
    };

    let overall_recall = if total_matched + total_unmatched > 0 {
        total_matched as f64 / (total_matched + total_unmatched) as f64 * 100.0
    } else {
        0.0
    };

    println!();
    println!("==============================================================");
    println!("    PDF Extraction with SCOWL Dictionary — ~/Data/arxiv-ground-truth");
    println!("==============================================================");
    println!();
    println!("  Dataset: {total_papers} papers with ground truth");
    println!("  Ground truth: {bbl_count} from BBL, {bib_count} from BIB-only");
    println!();
    println!("-- Extraction Summary --");
    println!(
        "  Successful:           {} / {total_papers}",
        successes.len()
    );
    println!(
        "  Extraction failures:  {} / {total_papers}",
        failures.len()
    );
    println!();
    println!("-- Matching Results ({} papers) --", successes.len());
    println!("  Total PDF refs:       {total_pdf_refs}");
    println!("  Matched to GT:        {total_matched} ({overall_recall:.1}%)");
    println!("  Unmatched:            {total_unmatched}");
    println!("  No title extracted:   {total_no_title}");
    println!();
    println!("-- Per-Paper Recall Distribution --");
    println!("  100%:     {perfect} papers");
    println!("  90-99%:   {above_90} papers");
    println!("  80-89%:   {above_80} papers");
    println!("  Below 80: {below_80} papers");
    println!("  Median:   {median:.1}%");
    println!("  Mean:     {mean:.1}%");

    // Worst papers
    let mut by_recall: Vec<(&PaperResult, f64)> = successes
        .iter()
        .filter(|r| r.matched + r.unmatched > 0)
        .map(|r| {
            let recall = r.matched as f64 / (r.matched + r.unmatched) as f64 * 100.0;
            (*r, recall)
        })
        .collect();
    by_recall.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    println!();
    println!("-- Worst Papers by Recall --");
    for (r, recall) in by_recall.iter().take(10) {
        println!(
            "  {}: {recall:.0}% ({}/{} matched, {} no_title, pdf_refs={}, gt={} [{}])",
            r.stem,
            r.matched,
            r.matched + r.unmatched,
            r.no_title,
            r.pdf_count,
            r.gt_count,
            r.gt_source,
        );
    }

    // Near misses
    let mut all_near_misses: Vec<&NearMiss> =
        successes.iter().flat_map(|r| &r.near_misses).collect();
    all_near_misses.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());

    if !all_near_misses.is_empty() {
        println!();
        println!("-- Sample Near Misses (80-95%) --");
        for nm in all_near_misses.iter().take(20) {
            println!(
                "  [{:.1}%] PDF: \"{}\"",
                nm.score,
                truncate(&nm.pdf_title, 70)
            );
            println!("         GT:  \"{}\"", truncate(&nm.gt_title, 70));
        }
    }

    // Extraction failures
    if !failures.is_empty() {
        println!();
        println!("-- Extraction Failures --");
        for f in failures.iter().take(20) {
            println!("  {}: {}", f.stem, f.error.as_deref().unwrap_or("unknown"));
        }
    }

    println!();
    println!("==============================================================");
    println!(
        "  OVERALL RECALL: {total_matched} / {} ({overall_recall:.1}%)",
        total_matched + total_unmatched
    );
    println!("==============================================================");
    println!();
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

// ---------------------------------------------------------------------------
// Test entry point
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn arxiv_ground_truth_dict() {
    let dir = data_dir();
    if !dir.exists() {
        eprintln!(
            "Skipping: data directory not found at {}",
            dir.display()
        );
        return;
    }

    let pairs = discover_paper_pairs(&dir);
    if pairs.is_empty() {
        eprintln!("Skipping: no paper pairs found in {}", dir.display());
        return;
    }

    // Load the SCOWL dictionary once and share it
    let scowl = ScowlDictionary::embedded();
    println!("Loaded SCOWL dictionary with {} words", scowl.len());
    let dict: Arc<dyn hallucinator_parsing::Dictionary> = Arc::new(scowl);

    let total_papers = pairs.len();
    println!("Found {total_papers} papers with ground truth in {}", dir.display());

    let mut results = Vec::with_capacity(total_papers);

    for (i, pair) in pairs.iter().enumerate() {
        eprint!("[{}/{}] {} ... ", i + 1, total_papers, pair.stem);

        let gt = match build_ground_truth(pair) {
            Some(gt) => gt,
            None => {
                eprintln!("no ground truth");
                results.push(PaperResult {
                    stem: pair.stem.clone(),
                    gt_source: "none",
                    gt_count: 0,
                    pdf_count: 0,
                    matched: 0,
                    unmatched: 0,
                    no_title: 0,
                    near_misses: vec![],
                    error: Some("no bbl/bib references extracted".into()),
                });
                continue;
            }
        };

        // Create extractor with dictionary
        let extractor = ReferenceExtractor::new().with_shared_dictionary(Arc::clone(&dict));

        let pdf_result: ExtractionResult =
            match extractor.extract_references_via_backend(&pair.pdf_path, &LocalMupdfBackend) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("PDF error: {e}");
                    results.push(PaperResult {
                        stem: pair.stem.clone(),
                        gt_source: gt.source,
                        gt_count: gt.titles.len(),
                        pdf_count: 0,
                        matched: 0,
                        unmatched: 0,
                        no_title: 0,
                        near_misses: vec![],
                        error: Some(format!("PDF extraction: {e}")),
                    });
                    continue;
                }
            };

        let (matched, unmatched, no_title, near_misses) =
            evaluate_paper(&pdf_result.references, &gt);

        let recall = if matched + unmatched > 0 {
            matched as f64 / (matched + unmatched) as f64 * 100.0
        } else {
            100.0
        };

        eprintln!(
            "{:.0}% ({}/{} matched, gt={} [{}])",
            recall,
            matched,
            matched + unmatched,
            gt.titles.len(),
            gt.source,
        );

        results.push(PaperResult {
            stem: pair.stem.clone(),
            gt_source: gt.source,
            gt_count: gt.titles.len(),
            pdf_count: pdf_result.references.len(),
            matched,
            unmatched,
            no_title,
            near_misses,
            error: None,
        });
    }

    print_report(&results, total_papers);
}
