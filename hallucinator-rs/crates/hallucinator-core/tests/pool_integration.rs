//! Integration tests for the [`ValidationPool`].
//!
//! These tests use a Config with all real DBs disabled so that no HTTP
//! requests are made. References without DOIs go through the empty-DB
//! path and return NotFound immediately.

use std::sync::{Arc, Mutex};

use hallucinator_core::pool::{RefJob, ValidationPool};
use hallucinator_core::{Config, ProgressEvent, RateLimiters, Reference, Status, ValidationResult};
use tokio_util::sync::CancellationToken;

/// Build a Config with every real DB disabled (no HTTP calls).
fn config_no_network() -> Config {
    Config {
        disabled_dbs: vec![
            "CrossRef".into(),
            "arXiv".into(),
            "DBLP".into(),
            "Semantic Scholar".into(),
            "ACL Anthology".into(),
            "Europe PMC".into(),
            "PubMed".into(),
            "OpenAlex".into(),
        ],
        rate_limiters: Arc::new(RateLimiters::default()),
        num_workers: 2,
        ..Config::default()
    }
}

/// Build a dummy reference (no DOI, no arxiv_id → skips DOI validation).
fn dummy_ref(title: &str) -> Reference {
    Reference {
        raw_citation: format!("[1] {title}"),
        title: Some(title.to_string()),
        authors: vec![],
        doi: None,
        arxiv_id: None,
        urls: vec![],
        original_number: 1,
        skip_reason: None,
    }
}

#[tokio::test]
async fn single_job_completes() {
    let config = Arc::new(config_no_network());
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 2);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = RefJob {
        reference: dummy_ref("A Test Paper"),
        result_tx: tx,
        ref_index: 0,
        total: 1,
        progress: Arc::new(|_| {}),
    };

    pool.submit(job).await;
    let result: ValidationResult = rx.await.expect("should receive result");
    assert_eq!(result.status, Status::NotFound);
    assert_eq!(result.title, "A Test Paper");

    pool.shutdown().await;
}

#[tokio::test]
async fn multiple_jobs_all_collected() {
    let config = Arc::new(config_no_network());
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 2);

    let total = 5;
    let mut receivers = Vec::with_capacity(total);

    for i in 0..total {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let job = RefJob {
            reference: dummy_ref(&format!("Paper {i}")),
            result_tx: tx,
            ref_index: i,
            total,
            progress: Arc::new(|_| {}),
        };
        pool.submit(job).await;
        receivers.push(rx);
    }

    let mut results = Vec::with_capacity(total);
    for rx in receivers {
        results.push(rx.await.expect("should receive result"));
    }

    assert_eq!(results.len(), total);
    for (i, r) in results.iter().enumerate() {
        assert_eq!(r.title, format!("Paper {i}"));
    }

    pool.shutdown().await;
}

#[tokio::test]
async fn cancellation_stops_pool() {
    let config = Arc::new(config_no_network());
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel.clone(), 2);

    // Cancel before submitting any jobs
    cancel.cancel();

    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = RefJob {
        reference: dummy_ref("Should Not Process"),
        result_tx: tx,
        ref_index: 0,
        total: 1,
        progress: Arc::new(|_| {}),
    };
    pool.submit(job).await;

    // The receiver should error because workers drop without sending
    // (or the send may succeed if a worker already picked it up before cancel)
    // Either way, shutdown should complete promptly.
    pool.shutdown().await;

    // Result may or may not arrive — the key thing is shutdown doesn't hang.
    drop(rx);
}

#[tokio::test]
async fn shutdown_waits_for_completion() {
    let config = Arc::new(config_no_network());
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 2);

    let total = 3;
    let mut receivers = Vec::with_capacity(total);

    for i in 0..total {
        let (tx, rx) = tokio::sync::oneshot::channel();
        pool.submit(RefJob {
            reference: dummy_ref(&format!("Paper {i}")),
            result_tx: tx,
            ref_index: i,
            total,
            progress: Arc::new(|_| {}),
        })
        .await;
        receivers.push(rx);
    }

    // Shutdown closes the sender, workers drain remaining jobs then exit.
    pool.shutdown().await;

    // All results should be available after shutdown.
    for rx in receivers {
        assert!(
            rx.await.is_ok(),
            "all jobs should complete before shutdown returns"
        );
    }
}

#[tokio::test]
async fn progress_events_emitted() {
    let config = Arc::new(config_no_network());
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 1);

    let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let events_clone = events.clone();

    let progress = Arc::new(move |event: ProgressEvent| {
        let tag = match &event {
            ProgressEvent::Checking { .. } => "checking",
            ProgressEvent::Result { .. } => "result",
            ProgressEvent::Warning { .. } => "warning",
            ProgressEvent::DatabaseQueryComplete { .. } => "db_complete",
            _ => "other",
        };
        events_clone.lock().unwrap().push(tag.to_string());
    });

    let (tx, rx) = tokio::sync::oneshot::channel();
    pool.submit(RefJob {
        reference: dummy_ref("Test"),
        result_tx: tx,
        ref_index: 0,
        total: 1,
        progress,
    })
    .await;

    let _ = rx.await;
    pool.shutdown().await;

    let collected = events.lock().unwrap();
    assert!(
        collected.contains(&"checking".to_string()),
        "should emit Checking event, got: {collected:?}"
    );
    assert!(
        collected.contains(&"result".to_string()),
        "should emit Result event, got: {collected:?}"
    );
}

// ── --url-match gate ─────────────────────────────────────────────────

/// Build a reference that carries a non-academic URL (the shape that
/// triggers the `--url-match` gate when all DBs return NotFound).
fn url_bearing_ref(title: &str, url: &str) -> Reference {
    Reference {
        raw_citation: format!("[1] {title}, {url}"),
        title: Some(title.to_string()),
        authors: vec![],
        doi: None,
        arxiv_id: None,
        urls: vec![url.to_string()],
        original_number: 1,
        skip_reason: None,
    }
}

#[tokio::test]
async fn url_match_off_demotes_notfound_with_url_to_skipped() {
    // Core regression guard for the `--url-match` gate. With every DB
    // disabled (so all queries return NotFound) and `url_match = false`
    // (default), a ref that still carries a non-academic URL must come
    // back with `url_check_skipped = true` — the reporting layer uses
    // this to render the ref as "skipped" instead of bucketing it with
    // potential hallucinations.
    let config = Arc::new(config_no_network());
    assert!(
        !config.url_match,
        "test expects default Config with url_match=false"
    );
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 2);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = RefJob {
        reference: url_bearing_ref("A URL-bearing citation", "https://github.com/owner/repo"),
        result_tx: tx,
        ref_index: 0,
        total: 1,
        progress: Arc::new(|_| {}),
    };

    pool.submit(job).await;
    let result: ValidationResult = rx.await.expect("should receive result");
    assert_eq!(result.status, Status::NotFound);
    assert!(
        result.url_check_skipped,
        "url_match=false + NotFound + non-empty urls must set url_check_skipped"
    );

    pool.shutdown().await;
}

#[tokio::test]
async fn url_match_off_without_urls_stays_not_found() {
    // Companion guard: a NotFound ref with NO URLs must NOT be demoted
    // — this is the fake-arXiv-ID / fake-DOI case where
    // `extract_urls` filtered academic domains and left `urls = []`,
    // and the full hallucination signal should still fire.
    let config = Arc::new(config_no_network());
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 2);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = RefJob {
        reference: dummy_ref("A ref with no URLs"),
        result_tx: tx,
        ref_index: 0,
        total: 1,
        progress: Arc::new(|_| {}),
    };

    pool.submit(job).await;
    let result: ValidationResult = rx.await.expect("should receive result");
    assert_eq!(result.status, Status::NotFound);
    assert!(
        !result.url_check_skipped,
        "NotFound with no URLs must stay not_found regardless of url_match"
    );

    pool.shutdown().await;
}

#[tokio::test]
async fn url_match_on_runs_url_check_without_demoting() {
    // Mirror guard: with `url_match = true`, the gate stays open. The
    // URL lookup will still return no_match for `https://example.invalid`
    // (DNS failure), so the ref stays NotFound — but crucially,
    // `url_check_skipped` must be false because the user opted in to
    // URL matching.
    let mut cfg = config_no_network();
    cfg.url_match = true;
    let config = Arc::new(cfg);
    let cancel = CancellationToken::new();
    let pool = ValidationPool::new(config, cancel, 2);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let job = RefJob {
        reference: url_bearing_ref(
            "A URL-bearing citation",
            "https://this-domain-definitely-does-not-exist-12345.invalid/path",
        ),
        result_tx: tx,
        ref_index: 0,
        total: 1,
        progress: Arc::new(|_| {}),
    };

    pool.submit(job).await;
    let result: ValidationResult = rx.await.expect("should receive result");
    // URL Check runs but fails (DNS miss). With Wayback also returning
    // no snapshot for a fabricated domain, status stays NotFound and
    // the gate is inactive.
    assert_eq!(result.status, Status::NotFound);
    assert!(
        !result.url_check_skipped,
        "url_match=true must NEVER set url_check_skipped"
    );

    pool.shutdown().await;
}
