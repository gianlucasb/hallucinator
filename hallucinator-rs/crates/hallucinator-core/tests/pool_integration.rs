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
