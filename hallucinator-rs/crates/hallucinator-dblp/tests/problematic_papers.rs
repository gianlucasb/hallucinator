//! Test harness for the arxiv-problematic-papers dataset.
//!
//! Loads `problems_categorized.json`, builds an in-memory DBLP database from
//! the expected DBLP titles, then runs `query_fts` for each bibtex_title and
//! reports match rates per category.
//!
//! Run with:
//!   cargo test -p hallucinator-dblp --test problematic_papers -- --ignored --nocapture

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use hallucinator_dblp::db;
use hallucinator_dblp::query;
use rusqlite::Connection;
use serde::Deserialize;

// ---- JSON schema for problems_categorized.json ----

#[derive(Deserialize)]
struct ProblemsFile {
    summary: Summary,
    issues: HashMap<String, Vec<Issue>>,
}

#[derive(Deserialize)]
struct Summary {
    total: usize,
    latex_escape: usize,
    minor_wording: usize,
    fts_query_issues: usize,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct Issue {
    bibtex_title: String,
    dblp_title: String,
    dblp_score: f64,
    #[serde(default)]
    fts_query: Option<String>,
    paper: String,
    category: String,
    #[serde(default)]
    bibtex_key: Option<String>,
    #[serde(default)]
    bibtex_authors: Vec<String>,
    #[serde(default)]
    r#type: Option<String>,
}

// ---- Helpers ----

fn test_data_path() -> PathBuf {
    // crate dir: hallucinator-rs/crates/hallucinator-dblp
    // repo root: ../../..
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("..")
        .join("..")
        .join("test-data")
        .join("arxiv-problematic-papers")
        .join("problems_categorized.json")
}

fn load_problems() -> ProblemsFile {
    let path = test_data_path();
    let data = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    serde_json::from_str(&data).expect("Failed to parse problems_categorized.json")
}

/// Create an in-memory DBLP database populated with the given titles.
fn build_test_db(titles: &HashSet<String>) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    db::init_database(&conn).unwrap();

    // Set schema version so DblpDatabase::open would accept it (not needed for
    // direct query_fts calls, but good hygiene).
    db::set_metadata(&conn, "schema_version", "3").unwrap();

    for (i, title) in titles.iter().enumerate() {
        let key = format!("test/{}", i);
        db::insert_or_get_publication(&conn, &key, title).unwrap();
    }

    db::rebuild_fts_index(&conn).unwrap();

    let count = titles.len();
    println!("  Built in-memory DB with {count} unique DBLP titles");
    conn
}

// ---- Category results ----

struct CategoryResult {
    name: String,
    total: usize,
    passed: usize,
    failures: Vec<FailureInfo>,
}

struct FailureInfo {
    bibtex_title: String,
    dblp_title: String,
    expected_score: f64,
    query_words: Vec<String>,
    fts_query_joined: String,
    got_match: Option<(String, f64)>, // (matched_title, score) if matched wrong title
}

impl CategoryResult {
    fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.passed as f64 / self.total as f64 * 100.0
    }
}

// ---- The test ----

#[test]
#[ignore] // Run explicitly: cargo test -p hallucinator-dblp --test problematic_papers -- --ignored --nocapture
fn problematic_papers_baseline() {
    let problems = load_problems();

    println!();
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          DBLP Matching Test Harness — Rust Baseline         ║");
    println!("╚══════════════════════════════════════════════════════════════╝");
    println!();
    println!(
        "  Dataset: {} total issues ({} latex_escape, {} minor_wording, {} fts_query_issues)",
        problems.summary.total,
        problems.summary.latex_escape,
        problems.summary.minor_wording,
        problems.summary.fts_query_issues,
    );

    // Collect all unique DBLP titles to build the test DB
    let mut all_dblp_titles: HashSet<String> = HashSet::new();
    for issues in problems.issues.values() {
        for issue in issues {
            all_dblp_titles.insert(issue.dblp_title.clone());
        }
    }
    println!(
        "  Unique DBLP titles across all categories: {}",
        all_dblp_titles.len()
    );

    let conn = build_test_db(&all_dblp_titles);

    // Process each category in a fixed order
    let category_order = ["fts_query_issues", "latex_escape", "minor_wording"];
    let mut results: Vec<CategoryResult> = Vec::new();

    for cat_name in &category_order {
        let issues = match problems.issues.get(*cat_name) {
            Some(v) => v,
            None => continue,
        };

        let mut result = CategoryResult {
            name: cat_name.to_string(),
            total: issues.len(),
            passed: 0,
            failures: Vec::new(),
        };

        for issue in issues {
            let query_words = query::get_query_words(&issue.bibtex_title);
            let fts_query_joined = query_words.join(" ");

            match query::query_fts(&conn, &issue.bibtex_title, query::DEFAULT_THRESHOLD) {
                Ok(Some(matched)) => {
                    // Normalize both for comparison — did we match the RIGHT title?
                    let norm_expected = query::normalize_title(&issue.dblp_title);
                    let norm_got = query::normalize_title(&matched.record.title);

                    if norm_expected == norm_got {
                        result.passed += 1;
                    } else {
                        // Matched, but to a different title
                        result.failures.push(FailureInfo {
                            bibtex_title: issue.bibtex_title.clone(),
                            dblp_title: issue.dblp_title.clone(),
                            expected_score: issue.dblp_score,
                            query_words: query_words.clone(),
                            fts_query_joined: fts_query_joined.clone(),
                            got_match: Some((matched.record.title.clone(), matched.score)),
                        });
                    }
                }
                Ok(None) => {
                    result.failures.push(FailureInfo {
                        bibtex_title: issue.bibtex_title.clone(),
                        dblp_title: issue.dblp_title.clone(),
                        expected_score: issue.dblp_score,
                        query_words,
                        fts_query_joined,
                        got_match: None,
                    });
                }
                Err(e) => {
                    // FTS5 query error (e.g., syntax issue) — count as failure
                    result.failures.push(FailureInfo {
                        bibtex_title: issue.bibtex_title.clone(),
                        dblp_title: issue.dblp_title.clone(),
                        expected_score: issue.dblp_score,
                        query_words,
                        fts_query_joined: format!("ERROR: {e}"),
                        got_match: None,
                    });
                }
            }
        }

        results.push(result);
    }

    // ---- Print report ----
    let mut overall_total = 0;
    let mut overall_passed = 0;

    for result in &results {
        overall_total += result.total;
        overall_passed += result.passed;

        let priority = match result.name.as_str() {
            "fts_query_issues" => "HIGH",
            "latex_escape" => "MEDIUM",
            "minor_wording" => "LOW",
            _ => "?",
        };

        println!();
        println!(
            "── {} ({} total, priority: {}) ──",
            result.name, result.total, priority
        );
        println!(
            "  PASS: {:>4} / {} ({:.1}%)",
            result.passed,
            result.total,
            result.pass_rate()
        );
        println!(
            "  FAIL: {:>4} / {} ({:.1}%)",
            result.total - result.passed,
            result.total,
            100.0 - result.pass_rate()
        );

        // Show up to 5 sample failures
        let show = result.failures.len().min(5);
        if show > 0 {
            println!();
            println!("  Sample failures:");
            for failure in &result.failures[..show] {
                println!("    bibtex: {:>80.80}", failure.bibtex_title);
                println!("    dblp:   {:>80.80}", failure.dblp_title);
                println!(
                    "    words:  [{}]",
                    failure
                        .query_words
                        .iter()
                        .map(|w| format!("\"{}\"", w))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                println!("    fts:    {}", failure.fts_query_joined);
                match &failure.got_match {
                    Some((title, score)) => {
                        println!(
                            "    result: WRONG MATCH (score={:.1}%): {}",
                            score * 100.0,
                            title
                        );
                    }
                    None => {
                        println!(
                            "    result: NO MATCH (expected score: {:.1}%)",
                            failure.expected_score
                        );
                    }
                }
                println!();
            }
            if result.failures.len() > show {
                println!("    ... and {} more failures", result.failures.len() - show);
            }
        }
    }

    println!();
    println!("══════════════════════════════════════════════════════════════");
    println!(
        "  OVERALL: {} / {} ({:.1}%)",
        overall_passed,
        overall_total,
        if overall_total > 0 {
            overall_passed as f64 / overall_total as f64 * 100.0
        } else {
            0.0
        }
    );
    println!("══════════════════════════════════════════════════════════════");
    println!();

    // Diagnostic: break down failures by sub-pattern for fts_query_issues
    if let Some(fts_result) = results.iter().find(|r| r.name == "fts_query_issues") {
        let mut no_words = 0;
        let mut fts_miss = 0; // FTS returned nothing or score below threshold
        let mut wrong_match = 0;
        let mut errors = 0;

        for f in &fts_result.failures {
            if f.fts_query_joined.starts_with("ERROR") {
                errors += 1;
            } else if f.query_words.is_empty() {
                no_words += 1;
            } else if f.got_match.is_some() {
                wrong_match += 1;
            } else {
                // No match — could be FTS miss or below threshold.
                // We can't distinguish without more data, so count as FTS miss.
                fts_miss += 1;
            }
        }

        println!("  fts_query_issues failure breakdown:");
        println!("    No query words:  {no_words}");
        println!("    FTS miss/below:  {fts_miss}");
        println!("    Wrong match:     {wrong_match}");
        println!("    Query errors:    {errors}");
        println!();
    }
}
