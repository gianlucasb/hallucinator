//! Regression tests for [`ValidationResult::is_inconclusive`].
//!
//! Background: a user behind a slow/filtered network reported references as
//! hallucinations that verified cleanly when the same PDF was re-run
//! elsewhere. The cause was not the PDF and not the machine's locale — it
//! was that every database query timed out, and a `NotFound` produced with
//! zero successful lookups was rendered identically to a `NotFound` where
//! every database answered "no such paper".
//!
//! `is_inconclusive` separates the two. These tests pin the exact boundary,
//! because widening it silently re-introduces the false accusation and
//! narrowing it silently starts suppressing real findings.

use hallucinator_core::{DbResult, DbStatus, MismatchKind, Status, ValidationResult};

fn result(status: Status, failed_dbs: &[&str]) -> ValidationResult {
    ValidationResult {
        title: "Attention Is All You Need".to_string(),
        raw_citation: String::new(),
        ref_authors: vec![],
        status,
        source: None,
        found_authors: vec![],
        paper_url: None,
        failed_dbs: failed_dbs.iter().map(|s| s.to_string()).collect(),
        db_results: vec![],
        doi_info: None,
        arxiv_info: None,
        retraction_info: None,
        url_check_skipped: false,
    }
}

#[test]
fn not_found_with_a_failed_db_is_inconclusive() {
    // The reported bug, reduced: everything timed out, nothing was proven.
    let r = result(
        Status::NotFound,
        &["CrossRef", "Semantic Scholar", "arXiv", "OpenAlex"],
    );
    assert!(
        r.is_inconclusive(),
        "NotFound with failed lookups must not be reported as a hallucination"
    );
}

#[test]
fn a_single_failed_db_is_enough() {
    // Deliberately strict. `failed_dbs` is rebuilt by the retry pass, so a
    // name still present here failed twice. One database that never
    // answered is one database that might have held the paper.
    let r = result(Status::NotFound, &["CrossRef"]);
    assert!(r.is_inconclusive());
}

#[test]
fn clean_not_found_stays_not_found() {
    // Every database answered, all said no. This is the real signal and it
    // must survive untouched — the whole tool is worthless if this case
    // gets suppressed.
    let r = result(Status::NotFound, &[]);
    assert!(
        !r.is_inconclusive(),
        "a NotFound where every database answered is a genuine finding"
    );
}

#[test]
fn verified_is_never_inconclusive() {
    // A paper found in one database is found, no matter how many others
    // fell over on the way.
    let r = result(Status::Verified, &["CrossRef", "PubMed"]);
    assert!(
        !r.is_inconclusive(),
        "a positive match is authoritative regardless of other failures"
    );
}

#[test]
fn mismatch_is_never_inconclusive() {
    // An author mismatch means a database *did* return the paper, so the
    // finding stands on its own evidence.
    let r = result(Status::Mismatch(MismatchKind::AUTHOR), &["arXiv"]);
    assert!(!r.is_inconclusive());
}

#[test]
fn db_results_do_not_influence_the_verdict() {
    // Guards the documented reason `failed_dbs` is the input rather than
    // `db_results`: on the retry path `db_results` holds only the retried
    // subset, so deriving from it would misreport whole-run state. A ref
    // carrying error rows but an empty `failed_dbs` (i.e. the retry
    // succeeded) is a clean NotFound.
    let mut r = result(Status::NotFound, &[]);
    r.db_results = vec![DbResult {
        db_name: "CrossRef".to_string(),
        status: DbStatus::Error,
        elapsed: None,
        found_authors: vec![],
        paper_url: None,
        error_message: Some("timed out".to_string()),
    }];
    assert!(
        !r.is_inconclusive(),
        "failed_dbs is the source of truth; a cleared retry must clear the flag"
    );
}

#[test]
fn retry_clearing_failed_dbs_flips_the_verdict_back() {
    // End-to-end shape of the retry pass: first attempt fails, retry
    // succeeds and rebuilds `failed_dbs` empty, ref becomes a real finding.
    let mut r = result(Status::NotFound, &["CrossRef"]);
    assert!(r.is_inconclusive());
    r.failed_dbs.clear();
    assert!(!r.is_inconclusive());
}
