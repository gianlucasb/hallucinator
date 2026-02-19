//! Ground truth test for the PDF extraction pipeline.
//!
//! For each paper in `test-data/arxiv-problematic-papers/`:
//!   1. Extract ground truth references from `.bbl`/`.bib` files inside the tar.gz
//!   2. Extract references from the PDF via the hallucinator-pdf pipeline
//!   3. Fuzzy-match each PDF-extracted title against the ground truth set
//!   4. Report per-paper and aggregate recall metrics
//!
//! Run with:
//!   cargo test -p hallucinator-pdf --test bbl_ground_truth -- --ignored --nocapture

use std::collections::HashSet;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use hallucinator_bbl::{extract_references_from_bbl_str, extract_references_from_bib_str};
use hallucinator_core::matching::{normalize_title, titles_match};
use hallucinator_pdf::{ExtractionResult, PdfBackend, PdfError, Reference};

/// Local mupdf backend for use in this integration test.
///
/// Lives here (not in hallucinator-pdf-mupdf) to avoid a circular dependency:
/// hallucinator-pdf-mupdf depends on hallucinator-pdf, so the latter cannot
/// depend on the former even in dev-dependencies.
struct LocalMupdfBackend;

impl PdfBackend for LocalMupdfBackend {
    fn extract_text(&self, path: &Path) -> Result<String, PdfError> {
        use mupdf::{Document, TextPageFlags};
        use hallucinator_pdf::text_processing::expand_ligatures;

        let path_str = path
            .to_str()
            .ok_or_else(|| PdfError::OpenError("invalid path encoding".into()))?;
        let document =
            Document::open(path_str).map_err(|e| PdfError::OpenError(e.to_string()))?;
        let mut pages_text = Vec::new();
        for page_result in document
            .pages()
            .map_err(|e| PdfError::ExtractionError(e.to_string()))?
        {
            let page = page_result.map_err(|e| PdfError::ExtractionError(e.to_string()))?;
            let text_page = page
                .to_text_page(TextPageFlags::empty())
                .map_err(|e| PdfError::ExtractionError(e.to_string()))?;
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
        Ok(expand_ligatures(&pages_text.join("\n")))
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

struct PaperPair {
    stem: String,
    pdf_path: PathBuf,
    tar_gz_path: PathBuf,
}

struct GroundTruthFiles {
    bbl_contents: Vec<String>,
    bib_contents: Vec<String>,
}

struct GroundTruth {
    source: &'static str, // "bbl" or "bib"
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
// Step 1: Discover paper pairs
// ---------------------------------------------------------------------------

fn test_data_dir() -> PathBuf {
    // hallucinator-rs/crates/hallucinator-pdf -> ../../.. -> repo root
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-data")
        .join("arxiv-problematic-papers")
}

fn discover_paper_pairs(dir: &Path) -> Vec<PaperPair> {
    let mut pdf_stems: HashSet<String> = HashSet::new();
    let mut tar_stems: HashSet<String> = HashSet::new();

    let entries = std::fs::read_dir(dir).expect("cannot read test-data directory");
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(stem) = name.strip_suffix(".pdf") {
            pdf_stems.insert(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(".tar.gz") {
            tar_stems.insert(stem.to_string());
        }
    }

    let mut pairs: Vec<PaperPair> = pdf_stems
        .intersection(&tar_stems)
        .map(|stem| PaperPair {
            stem: stem.clone(),
            pdf_path: dir.join(format!("{stem}.pdf")),
            tar_gz_path: dir.join(format!("{stem}.tar.gz")),
        })
        .collect();

    pairs.sort_by(|a, b| a.stem.cmp(&b.stem));
    pairs
}

// ---------------------------------------------------------------------------
// Step 2: Extract .bbl/.bib from tar.gz
// ---------------------------------------------------------------------------

/// Filenames to skip — these are template/abbreviation files, not real bibliographies.
const SKIP_PATTERNS: &[&str] = &[
    "ieeeabrv",
    "ieeefull",
    "ieeetran",
    "sample",
    "template",
    "abbrev",
    "abbrv",
    "strings.bib",
    "acronyms.bib",
    "IEEEtranBST",
];

fn should_skip_bib_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    SKIP_PATTERNS
        .iter()
        .any(|pat| lower.contains(&pat.to_lowercase()))
}

fn extract_bbl_bib_from_tar_gz(path: &Path) -> Result<GroundTruthFiles, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open tar.gz: {e}"))?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    let mut bbl_contents = Vec::new();
    let mut bib_contents = Vec::new();

    for entry in archive.entries().map_err(|e| format!("read tar: {e}"))? {
        let mut entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let entry_path = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(_) => continue,
        };

        let filename = entry_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_default();

        if filename.is_empty() {
            continue;
        }

        let is_bbl = filename.ends_with(".bbl");
        let is_bib = filename.ends_with(".bib");

        if !is_bbl && !is_bib {
            continue;
        }

        if should_skip_bib_file(&filename) {
            continue;
        }

        let mut content = String::new();
        if entry.read_to_string(&mut content).is_err() {
            continue;
        }

        if content.trim().is_empty() {
            continue;
        }

        if is_bbl {
            bbl_contents.push(content);
        } else {
            bib_contents.push(content);
        }
    }

    Ok(GroundTruthFiles {
        bbl_contents,
        bib_contents,
    })
}

// ---------------------------------------------------------------------------
// Step 3: Build ground truth from .bbl/.bib content
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

fn build_ground_truth(files: &GroundTruthFiles) -> Option<GroundTruth> {
    // Prefer .bbl — it reflects exactly what's compiled into the PDF.
    if !files.bbl_contents.is_empty() {
        let mut all_titles = Vec::new();
        for content in &files.bbl_contents {
            if let Ok(result) = extract_references_from_bbl_str(content) {
                all_titles.extend(refs_to_titles(&result));
            }
        }
        let titles = deduplicate_titles(all_titles);
        if !titles.is_empty() {
            return Some(GroundTruth {
                source: "bbl",
                titles,
            });
        }
    }

    // Fall back to .bib (superset — may contain uncited entries).
    if !files.bib_contents.is_empty() {
        let mut all_titles = Vec::new();
        for content in &files.bib_contents {
            if let Ok(result) = extract_references_from_bib_str(content) {
                all_titles.extend(refs_to_titles(&result));
            }
        }
        let titles = deduplicate_titles(all_titles);
        if !titles.is_empty() {
            return Some(GroundTruth {
                source: "bib",
                titles,
            });
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Step 4: Match PDF refs against ground truth
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

        // Use titles_match for the match decision (95% threshold with prefix awareness)
        let is_match = gt.titles.iter().any(|gt_t| titles_match(title, gt_t));

        if is_match {
            matched += 1;
        } else {
            // Check for near misses (80-95% raw score)
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

    // Per-paper recall
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
    println!("    PDF Extraction Ground Truth Test — BBL/BIB Baseline");
    println!("==============================================================");
    println!();
    println!(
        "  Dataset: {total_papers} papers ({} PDFs, {} tar.gz)",
        total_papers, total_papers
    );
    println!("  Ground truth: {bbl_count} from BBL, {bib_count} from BIB-only");
    println!();
    println!("-- Extraction Summary --");
    println!(
        "  Both available:       {} / {total_papers}",
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
        let arxiv_id = r.stem.split('_').next().unwrap_or(&r.stem);
        println!(
            "  {arxiv_id}: {recall:.0}% ({}/{} matched, {} no_title, pdf_refs={}, gt={} [{}])",
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
        for nm in all_near_misses.iter().take(10) {
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
            let arxiv_id = f.stem.split('_').next().unwrap_or(&f.stem);
            println!("  {arxiv_id}: {}", f.error.as_deref().unwrap_or("unknown"));
        }
    }

    // Large .bib warnings
    let large_bib: Vec<_> = successes
        .iter()
        .filter(|r| r.gt_source == "bib" && r.gt_count > 500)
        .collect();
    if !large_bib.is_empty() {
        println!();
        println!("-- Large BIB Warnings (>500 entries, no BBL) --");
        for r in &large_bib {
            let arxiv_id = r.stem.split('_').next().unwrap_or(&r.stem);
            println!("  {arxiv_id}: {} entries", r.gt_count);
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
fn bbl_ground_truth() {
    let dir = test_data_dir();
    if !dir.exists() {
        eprintln!(
            "Skipping: test-data directory not found at {}",
            dir.display()
        );
        return;
    }

    let pairs = discover_paper_pairs(&dir);
    if pairs.is_empty() {
        eprintln!("Skipping: no paper pairs found in {}", dir.display());
        return;
    }

    let total_papers = pairs.len();
    println!("Found {total_papers} paper pairs in {}", dir.display());

    let mut results = Vec::with_capacity(total_papers);

    for (i, pair) in pairs.iter().enumerate() {
        let arxiv_id = pair.stem.split('_').next().unwrap_or(&pair.stem);
        eprint!("[{}/{}] {arxiv_id} ... ", i + 1, total_papers);

        // Extract ground truth from tar.gz
        let gt_files = match extract_bbl_bib_from_tar_gz(&pair.tar_gz_path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("tar.gz error: {e}");
                results.push(PaperResult {
                    stem: pair.stem.clone(),
                    gt_source: "none",
                    gt_count: 0,
                    pdf_count: 0,
                    matched: 0,
                    unmatched: 0,
                    no_title: 0,
                    near_misses: vec![],
                    error: Some(format!("tar.gz extraction: {e}")),
                });
                continue;
            }
        };

        let gt = match build_ground_truth(&gt_files) {
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

        // Extract references from PDF
        let pdf_result: ExtractionResult =
            match hallucinator_pdf::extract_references(&pair.pdf_path, &LocalMupdfBackend) {
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
