//! Per-DB drainer pool for reference validation.
//!
//! Architecture: one dedicated drainer task per enabled remote DB (including DOI),
//! plus coordinator tasks that handle local DBs inline before fanning out
//! to per-DB drainer queues. Each drainer is the sole consumer of its DB's
//! rate limiter, eliminating governor contention.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::authors::validate_authors;
use crate::db::DatabaseBackend;
use crate::db::searxng::Searxng;
use crate::orchestrator::{build_database_list, query_local_databases};
use crate::rate_limit::{self, DoiContext};
use crate::{
    ArxivInfo, Config, DbResult, DbStatus, DoiInfo, ProgressEvent, Reference, Status,
    ValidationResult,
};

// ── Public API (unchanged) ──────────────────────────────────────────────

/// A reference validation job submitted to the pool.
pub struct RefJob {
    pub reference: Reference,
    pub result_tx: oneshot::Sender<ValidationResult>,
    pub ref_index: usize,
    pub total: usize,
    /// Progress callback for this job (emits Checking, Result, Warning, etc.).
    pub progress: Arc<dyn Fn(ProgressEvent) + Send + Sync>,
}

/// A pool of coordinator + drainer tasks that process reference validation jobs.
///
/// Submit jobs via [`submit()`](ValidationPool::submit), receive results via
/// the oneshot receiver returned with each job.
pub struct ValidationPool {
    job_tx: async_channel::Sender<RefJob>,
    pool_handle: JoinHandle<()>,
}

impl ValidationPool {
    /// Create a new pool with `num_workers` coordinator tasks.
    ///
    /// One drainer task is spawned per enabled remote DB. Coordinators handle
    /// local DBs inline, then fan out to per-DB drainer queues (including DOI).
    pub fn new(config: Arc<Config>, cancel: CancellationToken, num_workers: usize) -> Self {
        let (job_tx, job_rx) = async_channel::unbounded::<RefJob>();
        let client = reqwest::Client::builder()
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        // Build database list and partition into local/remote
        let all_dbs: Vec<Arc<dyn DatabaseBackend>> = build_database_list(&config, None)
            .into_iter()
            .map(Arc::from)
            .collect();
        let (local_dbs, remote_dbs): (Vec<_>, Vec<_>) =
            all_dbs.into_iter().partition(|db| db.is_local());

        // Spawn one drainer per remote DB.
        let mut drainer_txs: Vec<(String, bool, async_channel::Sender<DrainerJob>)> = Vec::new();
        let mut drainer_handles: Vec<JoinHandle<()>> = Vec::new();

        for db in remote_dbs {
            let (tx, rx) = async_channel::unbounded::<DrainerJob>();
            drainer_txs.push((db.name().to_string(), db.requires_doi(), tx));
            drainer_handles.push(tokio::spawn(drainer_loop(
                rx,
                Arc::clone(&db),
                config.clone(),
                client.clone(),
                cancel.clone(),
            )));
        }

        let drainer_txs = Arc::new(drainer_txs);

        // Spawn coordinator tasks
        let pool_handle = tokio::spawn(async move {
            let mut coord_handles = Vec::with_capacity(num_workers.max(1));

            for _ in 0..num_workers.max(1) {
                coord_handles.push(tokio::spawn(coordinator_loop(
                    job_rx.clone(),
                    config.clone(),
                    client.clone(),
                    cancel.clone(),
                    local_dbs.clone(),
                    drainer_txs.clone(),
                )));
            }

            // Drop our clone so coordinators are the last holders
            drop(job_rx);

            // Wait for coordinators to finish (they exit when job_tx closes)
            for h in coord_handles {
                let _ = h.await;
            }

            // All coordinator Arc<drainer_txs> clones are dropped.
            // Drop the last reference -> senders close -> drainers drain and exit.
            drop(drainer_txs);

            for h in drainer_handles {
                let _ = h.await;
            }
        });

        Self {
            job_tx,
            pool_handle,
        }
    }

    /// Get a cloneable sender for submitting jobs from multiple tasks.
    pub fn sender(&self) -> async_channel::Sender<RefJob> {
        self.job_tx.clone()
    }

    /// Submit a job to the pool.
    pub async fn submit(&self, job: RefJob) {
        let _ = self.job_tx.send(job).await;
    }

    /// Close the pool and wait for all coordinators and drainers to finish.
    pub async fn shutdown(self) {
        self.job_tx.close();
        let _ = self.pool_handle.await;
    }
}

// ── Internal types ──────────────────────────────────────────────────────

/// Per-ref aggregation hub. Created by a coordinator, shared by all drainers
/// working on that ref. The last drainer to decrement `remaining` calls
/// [`finalize_collector`].
struct RefCollector {
    reference: Reference,
    ref_index: usize,
    total: usize,
    title: String,
    progress: Arc<dyn Fn(ProgressEvent) + Send + Sync>,
    config: Arc<Config>,
    client: reqwest::Client,

    /// Number of drainers still to report. Each drainer decrements once.
    remaining: AtomicUsize,
    /// Set to true when any drainer verifies. Other drainers check this to skip work.
    verified: AtomicBool,

    /// Aggregation state (single Mutex, held briefly).
    state: Mutex<AggState>,

    /// Oneshot sender, taken exactly once by [`finalize_collector`].
    result_tx: Mutex<Option<oneshot::Sender<ValidationResult>>>,

    /// DB results from the local phase (carried forward for merging).
    local_result: crate::orchestrator::DbSearchResult,
}

/// Mutable aggregation state protected by a Mutex.
struct AggState {
    verified_info: Option<VerifiedInfo>,
    first_mismatch: Option<MismatchInfo>,
    failed_dbs: Vec<String>,
    db_results: Vec<DbResult>,
    /// Retraction info extracted inline from CrossRef response (if any).
    retraction: Option<crate::retraction::RetractionResult>,
}

struct VerifiedInfo {
    source: String,
    found_authors: Vec<String>,
    paper_url: Option<String>,
}

struct MismatchInfo {
    source: String,
    found_authors: Vec<String>,
    paper_url: Option<String>,
}

/// A job submitted to a drainer's queue.
struct DrainerJob {
    collector: Arc<RefCollector>,
}

// ── Drainer ─────────────────────────────────────────────────────────────

/// Drainer task for a remote DB. Processes refs sequentially at the DB's natural
/// rate. Multiple drainers may share a channel for the same DB to pipeline
/// requests when response time exceeds the governor interval.
async fn drainer_loop(
    rx: async_channel::Receiver<DrainerJob>,
    db: Arc<dyn DatabaseBackend>,
    config: Arc<Config>,
    client: reqwest::Client,
    cancel: CancellationToken,
) {
    let timeout = Duration::from_secs(config.db_timeout_secs);
    let rate_limiters = config.rate_limiters.clone();
    let cache = config.query_cache.clone();
    let requires_doi = db.requires_doi();

    while let Ok(job) = rx.recv().await {
        let collector = &job.collector;

        // Skip remaining jobs after cancellation
        if cancel.is_cancelled() {
            tracing::debug!(db = db.name(), title = %collector.title, "skipping: cancelled");
            skip_and_decrement(collector, db.name()).await;
            continue;
        }

        // Skip if already verified by another drainer
        if collector.verified.load(Ordering::Acquire) {
            tracing::debug!(db = db.name(), title = %collector.title, "skipping: already verified");
            skip_and_decrement(collector, db.name()).await;
            continue;
        }

        // DOI-requiring backends skip refs without a DOI
        if requires_doi && collector.reference.doi.is_none() {
            tracing::debug!(db = db.name(), title = %collector.title, "skipping: no DOI");
            skip_and_decrement(collector, db.name()).await;
            continue;
        }

        // Build DOI context if this ref has a DOI (used by DOI backend)
        let doi_ctx = collector.reference.doi.as_deref().map(|doi| DoiContext {
            doi,
            authors: &collector.reference.authors,
        });

        // Query (includes cache check + governor acquire + HTTP call)
        let rl_result = rate_limit::query_with_rate_limit(
            db.as_ref(),
            &collector.title,
            &client,
            timeout,
            &rate_limiters,
            cache.as_deref(),
            doi_ctx.as_ref(),
        )
        .await;

        // Process result and decrement remaining
        report_result(collector, db.name(), rl_result).await;
    }
}

/// Emit a Skipped event and decrement the collector's remaining counter.
async fn skip_and_decrement(collector: &RefCollector, db_name: &str) {
    (collector.progress)(ProgressEvent::DatabaseQueryComplete {
        paper_index: 0,
        ref_index: collector.ref_index,
        db_name: db_name.to_string(),
        status: DbStatus::Skipped,
        elapsed: Duration::ZERO,
    });

    {
        let mut state = collector.state.lock().unwrap_or_else(|e| e.into_inner());
        state.db_results.push(DbResult {
            db_name: db_name.to_string(),
            status: DbStatus::Skipped,
            elapsed: None,
            found_authors: vec![],
            paper_url: None,
            error_message: None,
        });
    }

    if collector.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
        finalize_collector(collector).await;
    }
}

/// Process a DB query result, update the collector's aggregation state,
/// and decrement the remaining counter (finalizing if last).
async fn report_result(
    collector: &RefCollector,
    db_name: &str,
    rl_result: rate_limit::RateLimitedResult,
) {
    let elapsed = rl_result.elapsed;
    let check_openalex_authors = collector.config.check_openalex_authors;

    match rl_result.result {
        Ok(ref qr) if qr.is_found() => {
            let found_authors = &qr.authors;
            let paper_url = &qr.paper_url;
            let ref_authors = &collector.reference.authors;
            if ref_authors.is_empty() || validate_authors(ref_authors, found_authors) {
                // Verified — set flag so other drainers can skip
                collector.verified.store(true, Ordering::Release);

                (collector.progress)(ProgressEvent::DatabaseQueryComplete {
                    paper_index: 0,
                    ref_index: collector.ref_index,
                    db_name: db_name.to_string(),
                    status: DbStatus::Match,
                    elapsed,
                });

                let mut state = collector.state.lock().unwrap_or_else(|e| e.into_inner());
                state.db_results.push(DbResult {
                    db_name: db_name.to_string(),
                    status: DbStatus::Match,
                    elapsed: Some(elapsed),
                    found_authors: found_authors.clone(),
                    paper_url: paper_url.clone(),
                    error_message: None,
                });
                if state.verified_info.is_none() {
                    state.verified_info = Some(VerifiedInfo {
                        source: db_name.to_string(),
                        found_authors: found_authors.clone(),
                        paper_url: paper_url.clone(),
                    });
                }
                // Capture inline retraction info (populated by CrossRef)
                if let Some(ref retraction) = qr.retraction
                    && retraction.retracted
                    && state.retraction.is_none()
                {
                    state.retraction = Some(retraction.clone());
                }
            } else {
                // Author mismatch
                (collector.progress)(ProgressEvent::DatabaseQueryComplete {
                    paper_index: 0,
                    ref_index: collector.ref_index,
                    db_name: db_name.to_string(),
                    status: DbStatus::AuthorMismatch,
                    elapsed,
                });

                let mut state = collector.state.lock().unwrap_or_else(|e| e.into_inner());
                state.db_results.push(DbResult {
                    db_name: db_name.to_string(),
                    status: DbStatus::AuthorMismatch,
                    elapsed: Some(elapsed),
                    found_authors: found_authors.clone(),
                    paper_url: paper_url.clone(),
                    error_message: None,
                });
                if state.first_mismatch.is_none()
                    && (db_name != "OpenAlex" || check_openalex_authors)
                {
                    state.first_mismatch = Some(MismatchInfo {
                        source: db_name.to_string(),
                        found_authors: found_authors.clone(),
                        paper_url: paper_url.clone(),
                    });
                }
            }
        }
        Ok(_) => {
            (collector.progress)(ProgressEvent::DatabaseQueryComplete {
                paper_index: 0,
                ref_index: collector.ref_index,
                db_name: db_name.to_string(),
                status: DbStatus::NoMatch,
                elapsed,
            });

            let mut state = collector.state.lock().unwrap_or_else(|e| e.into_inner());
            state.db_results.push(DbResult {
                db_name: db_name.to_string(),
                status: DbStatus::NoMatch,
                elapsed: Some(elapsed),
                found_authors: vec![],
                paper_url: None,
                error_message: None,
            });
        }
        Err(err) => {
            (collector.progress)(ProgressEvent::DatabaseQueryComplete {
                paper_index: 0,
                ref_index: collector.ref_index,
                db_name: db_name.to_string(),
                status: DbStatus::Error,
                elapsed,
            });

            let mut state = collector.state.lock().unwrap_or_else(|e| e.into_inner());
            state.db_results.push(DbResult {
                db_name: db_name.to_string(),
                status: DbStatus::Error,
                elapsed: Some(elapsed),
                found_authors: vec![],
                paper_url: None,
                error_message: Some(err.to_string()),
            });
            tracing::debug!(db = db_name, error = %err, "query error");
            state.failed_dbs.push(db_name.to_string());
        }
    }

    if collector.remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
        finalize_collector(collector).await;
    }
}

/// Build the final result and send it on the oneshot channel.
///
/// Called exactly once, by whichever drainer decrements `remaining` to 0.
async fn finalize_collector(collector: &RefCollector) {
    let (
        status,
        source,
        found_authors,
        paper_url,
        remote_failed_dbs,
        remote_db_results,
        inline_retraction,
    ) = {
        let state = collector.state.lock().unwrap_or_else(|e| e.into_inner());

        if let Some(ref v) = state.verified_info {
            (
                Status::Verified,
                Some(v.source.clone()),
                v.found_authors.clone(),
                v.paper_url.clone(),
                state.failed_dbs.clone(),
                state.db_results.clone(),
                state.retraction.clone(),
            )
        } else if let Some(ref m) = state.first_mismatch {
            (
                Status::AuthorMismatch,
                Some(m.source.clone()),
                m.found_authors.clone(),
                m.paper_url.clone(),
                state.failed_dbs.clone(),
                state.db_results.clone(),
                None,
            )
        } else {
            (
                Status::NotFound,
                None,
                vec![],
                None,
                state.failed_dbs.clone(),
                state.db_results.clone(),
                None,
            )
        }
    };

    // SearxNG fallback for NotFound references
    let (status, source, found_authors, paper_url, remote_db_results) =
        if status == Status::NotFound {
            if let Some(ref searxng_url) = collector.config.searxng_url {
                let searxng = Searxng::new(searxng_url.clone());
                let timeout = Duration::from_secs(collector.config.db_timeout_secs);

                let start = std::time::Instant::now();
                let searxng_result = searxng
                    .query(&collector.title, &collector.client, timeout)
                    .await;
                let elapsed = start.elapsed();

                if let Ok(ref qr) = searxng_result
                    && qr.is_found()
                {
                    let url = qr.paper_url.clone();
                    (collector.progress)(ProgressEvent::DatabaseQueryComplete {
                        paper_index: 0,
                        ref_index: collector.ref_index,
                        db_name: "Web Search".to_string(),
                        status: DbStatus::Match,
                        elapsed,
                    });
                    // Web search found the paper
                    let mut db_results = remote_db_results;
                    db_results.push(DbResult {
                        db_name: "Web Search".into(),
                        status: DbStatus::Match,
                        elapsed: Some(elapsed),
                        found_authors: vec![],
                        paper_url: url.clone(),
                        error_message: None,
                    });
                    (
                        Status::Verified,
                        Some("Web Search".into()),
                        vec![],
                        url,
                        db_results,
                    )
                } else {
                    (collector.progress)(ProgressEvent::DatabaseQueryComplete {
                        paper_index: 0,
                        ref_index: collector.ref_index,
                        db_name: "Web Search".to_string(),
                        status: DbStatus::NoMatch,
                        elapsed,
                    });
                    (status, source, found_authors, paper_url, remote_db_results)
                }
            } else {
                (status, source, found_authors, paper_url, remote_db_results)
            }
        } else {
            (status, source, found_authors, paper_url, remote_db_results)
        };

    // Merge local + remote results
    let mut all_db_results = collector.local_result.db_results.clone();
    all_db_results.extend(remote_db_results);

    let mut all_failed_dbs = collector.local_result.failed_dbs.clone();
    all_failed_dbs.extend(remote_failed_dbs);

    // Build doi_info from reference DOI + DOI drainer result
    let doi_info = collector.reference.doi.as_ref().map(|doi| {
        let valid = all_db_results.iter().any(|r| {
            r.db_name == "DOI" && matches!(r.status, DbStatus::Match | DbStatus::AuthorMismatch)
        });
        DoiInfo {
            doi: doi.clone(),
            valid,
            title: None,
        }
    });

    // Retraction info: use inline data from CrossRef response (no extra API call)
    let retraction_info = if status == Status::Verified {
        inline_retraction.and_then(|r| {
            if r.retracted {
                Some(crate::RetractionInfo {
                    is_retracted: true,
                    retraction_doi: r.retraction_doi,
                    retraction_source: r.retraction_type,
                })
            } else {
                None
            }
        })
    } else {
        None
    };

    let result = ValidationResult {
        title: collector.title.clone(),
        raw_citation: collector.reference.raw_citation.clone(),
        ref_authors: collector.reference.authors.clone(),
        status,
        source,
        found_authors,
        paper_url,
        failed_dbs: all_failed_dbs,
        db_results: all_db_results,
        doi_info,
        arxiv_info: collector.reference.arxiv_id.as_ref().map(|id| ArxivInfo {
            arxiv_id: id.clone(),
            valid: false,
            title: None,
        }),
        retraction_info,
    };

    emit_final_events(
        collector.progress.as_ref(),
        &result,
        collector.ref_index,
        collector.total,
        &collector.title,
    );

    let tx = collector
        .result_tx
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if let Some(tx) = tx {
        let _ = tx.send(result);
    }
}

// ── Cache pre-check ─────────────────────────────────────────────────────

/// Pre-check result from scanning the cache for all remote DBs.
struct CachePreCheck {
    /// DB results from cache hits.
    db_results: Vec<DbResult>,
    /// Verified match info, if any cache hit resolved to Verified.
    verified_info: Option<VerifiedInfo>,
    /// First author mismatch from cache, if any.
    first_mismatch: Option<MismatchInfo>,
    /// Indices into drainer_txs for DBs that had cache misses.
    miss_indices: Vec<usize>,
    /// Retraction info from cached CrossRef response (if any).
    retraction: Option<crate::retraction::RetractionResult>,
}

/// Check cache for all remote DBs before dispatching to drainers.
///
/// This eliminates a race condition where a fast cache hit in one drainer
/// sets `verified`, causing other drainers to skip before checking their
/// own cache entries — preventing those entries from ever being populated.
///
/// Does NOT emit progress events — the caller is responsible for emitting
/// Skipped events for cache-hit DBs to decrement in-flight counters.
fn pre_check_remote_cache(
    cache: Option<&crate::cache::QueryCache>,
    title: &str,
    ref_authors: &[String],
    drainer_txs: &[(String, bool, async_channel::Sender<DrainerJob>)],
    check_openalex_authors: bool,
    has_doi: bool,
) -> CachePreCheck {
    let cache = match cache {
        Some(c) => c,
        None => {
            return CachePreCheck {
                db_results: vec![],
                verified_info: None,
                first_mismatch: None,
                miss_indices: (0..drainer_txs.len())
                    .filter(|&i| has_doi || !drainer_txs[i].1)
                    .collect(),
                retraction: None,
            };
        }
    };

    let mut db_results = Vec::new();
    let mut verified_info: Option<VerifiedInfo> = None;
    let mut first_mismatch: Option<MismatchInfo> = None;
    let mut miss_indices = Vec::new();
    let mut retraction: Option<crate::retraction::RetractionResult> = None;

    for (i, (db_name, requires_doi, _)) in drainer_txs.iter().enumerate() {
        // Skip DOI-requiring backends for refs without a DOI
        if *requires_doi && !has_doi {
            continue;
        }
        match cache.get(title, db_name) {
            Some(qr) if qr.is_found() => {
                // Capture retraction info from cached CrossRef result
                if let Some(ref r) = qr.retraction
                    && r.retracted
                    && retraction.is_none()
                {
                    retraction = Some(r.clone());
                }

                if ref_authors.is_empty() || validate_authors(ref_authors, &qr.authors) {
                    db_results.push(DbResult {
                        db_name: db_name.clone(),
                        status: DbStatus::Match,
                        elapsed: Some(Duration::ZERO),
                        found_authors: qr.authors.clone(),
                        paper_url: qr.paper_url.clone(),
                        error_message: None,
                    });
                    if verified_info.is_none() {
                        verified_info = Some(VerifiedInfo {
                            source: db_name.clone(),
                            found_authors: qr.authors,
                            paper_url: qr.paper_url,
                        });
                    }
                } else {
                    db_results.push(DbResult {
                        db_name: db_name.clone(),
                        status: DbStatus::AuthorMismatch,
                        elapsed: Some(Duration::ZERO),
                        found_authors: qr.authors.clone(),
                        paper_url: qr.paper_url.clone(),
                        error_message: None,
                    });
                    if first_mismatch.is_none() && (db_name != "OpenAlex" || check_openalex_authors)
                    {
                        first_mismatch = Some(MismatchInfo {
                            source: db_name.clone(),
                            found_authors: qr.authors,
                            paper_url: qr.paper_url,
                        });
                    }
                }
            }
            Some(_) => {
                db_results.push(DbResult {
                    db_name: db_name.clone(),
                    status: DbStatus::NoMatch,
                    elapsed: Some(Duration::ZERO),
                    found_authors: vec![],
                    paper_url: None,
                    error_message: None,
                });
            }
            None => {
                miss_indices.push(i);
            }
        }
    }

    let hits = db_results.len();
    let misses = miss_indices.len();
    let verified = verified_info.is_some();
    tracing::debug!(title, hits, misses, verified, "cache pre-check complete");

    CachePreCheck {
        db_results,
        verified_info,
        first_mismatch,
        miss_indices,
        retraction,
    }
}

// ── Coordinator ─────────────────────────────────────────────────────────

/// Coordinator loop: pick a ref, run local DBs inline, fan out to drainers.
async fn coordinator_loop(
    job_rx: async_channel::Receiver<RefJob>,
    config: Arc<Config>,
    client: reqwest::Client,
    cancel: CancellationToken,
    _local_dbs: Vec<Arc<dyn DatabaseBackend>>,
    drainer_txs: Arc<Vec<(String, bool, async_channel::Sender<DrainerJob>)>>,
) {
    while let Ok(job) = job_rx.recv().await {
        if cancel.is_cancelled() {
            break;
        }

        let RefJob {
            reference,
            result_tx,
            ref_index,
            total,
            progress,
        } = job;

        let title = reference.title.clone().unwrap_or_default();

        // Emit Checking event
        progress(ProgressEvent::Checking {
            index: ref_index,
            total,
            title: title.clone(),
        });

        // --- Local DB phase (inline, <1ms) ---
        let db_complete_cb = make_db_callback(progress.clone(), ref_index);
        let local_result = query_local_databases(
            &title,
            &reference.authors,
            &config,
            &client,
            false,
            None,
            Some(&db_complete_cb),
        )
        .await;

        if local_result.status == Status::Verified {
            // query_local_databases already emitted Skipped for remaining DBs
            // (including remote) via the on_db_complete callback
            let result = build_validation_result(&reference, &title, local_result, None);
            emit_final_events(progress.as_ref(), &result, ref_index, total, &title);
            let _ = result_tx.send(result);
            continue;
        }

        // --- Fan out to drainer queues ---
        if drainer_txs.is_empty() {
            // No remote DBs enabled — try SearxNG fallback if configured
            let result = if local_result.status == Status::NotFound {
                if let Some(ref searxng_url) = config.searxng_url {
                    let searxng = Searxng::new(searxng_url.clone());
                    let timeout = Duration::from_secs(config.db_timeout_secs);

                    let start = std::time::Instant::now();
                    let searxng_result = searxng.query(&title, &client, timeout).await;
                    let elapsed = start.elapsed();

                    if let Ok(ref qr) = searxng_result
                        && qr.is_found()
                    {
                        let url = qr.paper_url.clone();
                        progress(ProgressEvent::DatabaseQueryComplete {
                            paper_index: 0,
                            ref_index,
                            db_name: "Web Search".to_string(),
                            status: DbStatus::Match,
                            elapsed,
                        });
                        // Web search found the paper
                        let mut db_results = local_result.db_results.clone();
                        db_results.push(DbResult {
                            db_name: "Web Search".into(),
                            status: DbStatus::Match,
                            elapsed: Some(elapsed),
                            found_authors: vec![],
                            paper_url: url.clone(),
                            error_message: None,
                        });
                        ValidationResult {
                            title: title.clone(),
                            raw_citation: reference.raw_citation.clone(),
                            ref_authors: reference.authors.clone(),
                            status: Status::Verified,
                            source: Some("Web Search".into()),
                            found_authors: vec![],
                            paper_url: url,
                            failed_dbs: local_result.failed_dbs.clone(),
                            db_results,
                            doi_info: None,
                            arxiv_info: reference.arxiv_id.as_ref().map(|id| ArxivInfo {
                                arxiv_id: id.clone(),
                                valid: false,
                                title: None,
                            }),
                            retraction_info: None,
                        }
                    } else {
                        progress(ProgressEvent::DatabaseQueryComplete {
                            paper_index: 0,
                            ref_index,
                            db_name: "Web Search".to_string(),
                            status: DbStatus::NoMatch,
                            elapsed,
                        });
                        build_validation_result(&reference, &title, local_result, None)
                    }
                } else {
                    build_validation_result(&reference, &title, local_result, None)
                }
            } else {
                build_validation_result(&reference, &title, local_result, None)
            };
            emit_final_events(progress.as_ref(), &result, ref_index, total, &title);
            let _ = result_tx.send(result);
            continue;
        }

        // --- Cache pre-check for all remote DBs ---
        // Check cache for ALL remote DBs synchronously before dispatching
        // to drainers. This prevents the race where a fast drainer sets
        // `verified`, causing other drainers to skip without ever caching
        // their results.
        let pre = pre_check_remote_cache(
            config.query_cache.as_deref(),
            &title,
            &reference.authors,
            &drainer_txs,
            config.check_openalex_authors,
            reference.doi.is_some(),
        );

        // Emit Skipped for cache-hit DBs to decrement in-flight counters
        // without inflating per-DB query stats.
        for (i, (db_name, _, _)) in drainer_txs.iter().enumerate() {
            if !pre.miss_indices.contains(&i) {
                db_complete_cb(DbResult {
                    db_name: db_name.clone(),
                    status: DbStatus::Skipped,
                    elapsed: None,
                    found_authors: vec![],
                    paper_url: None,
                    error_message: None,
                });
            }
        }

        // If verified from cache, skip all drainers
        if let Some(verified) = pre.verified_info {
            // Emit Skipped for cache-miss DBs (they won't be queried either)
            for &i in &pre.miss_indices {
                db_complete_cb(DbResult {
                    db_name: drainer_txs[i].0.clone(),
                    status: DbStatus::Skipped,
                    elapsed: None,
                    found_authors: vec![],
                    paper_url: None,
                    error_message: None,
                });
            }

            let mut all_db_results = local_result.db_results;
            all_db_results.extend(pre.db_results);
            for &i in &pre.miss_indices {
                all_db_results.push(DbResult {
                    db_name: drainer_txs[i].0.clone(),
                    status: DbStatus::Skipped,
                    elapsed: None,
                    found_authors: vec![],
                    paper_url: None,
                    error_message: None,
                });
            }

            // Use inline retraction from cached CrossRef response (no extra API call)
            let retraction_info = pre.retraction.and_then(|r| {
                if r.retracted {
                    Some(crate::RetractionInfo {
                        is_retracted: true,
                        retraction_doi: r.retraction_doi,
                        retraction_source: r.retraction_type,
                    })
                } else {
                    None
                }
            });

            let doi_info = reference.doi.as_ref().map(|doi| {
                let valid = all_db_results.iter().any(|r| {
                    r.db_name == "DOI"
                        && matches!(r.status, DbStatus::Match | DbStatus::AuthorMismatch)
                });
                DoiInfo {
                    doi: doi.clone(),
                    valid,
                    title: None,
                }
            });

            let result = ValidationResult {
                title: title.clone(),
                raw_citation: reference.raw_citation.clone(),
                ref_authors: reference.authors.clone(),
                status: Status::Verified,
                source: Some(verified.source),
                found_authors: verified.found_authors,
                paper_url: verified.paper_url,
                failed_dbs: local_result.failed_dbs,
                db_results: all_db_results,
                doi_info,
                arxiv_info: reference.arxiv_id.as_ref().map(|id| ArxivInfo {
                    arxiv_id: id.clone(),
                    valid: false,
                    title: None,
                }),
                retraction_info,
            };

            emit_final_events(progress.as_ref(), &result, ref_index, total, &title);
            let _ = result_tx.send(result);
            continue;
        }

        // If all remote DBs were in cache (no misses) but none verified
        if pre.miss_indices.is_empty() {
            let mut all_db_results = local_result.db_results;
            all_db_results.extend(pre.db_results);

            let first_mismatch = pre.first_mismatch.or_else(|| {
                if local_result.status == Status::AuthorMismatch {
                    Some(MismatchInfo {
                        source: local_result.source.clone().unwrap_or_default(),
                        found_authors: local_result.found_authors.clone(),
                        paper_url: local_result.paper_url.clone(),
                    })
                } else {
                    None
                }
            });

            let (status, source, found_authors, paper_url) = if let Some(m) = first_mismatch {
                (
                    Status::AuthorMismatch,
                    Some(m.source),
                    m.found_authors,
                    m.paper_url,
                )
            } else {
                (Status::NotFound, None, vec![], None)
            };

            let result = ValidationResult {
                title: title.clone(),
                raw_citation: reference.raw_citation.clone(),
                ref_authors: reference.authors.clone(),
                status,
                source,
                found_authors,
                paper_url,
                failed_dbs: local_result.failed_dbs,
                db_results: all_db_results,
                doi_info: reference.doi.as_ref().map(|doi| DoiInfo {
                    doi: doi.clone(),
                    valid: false,
                    title: None,
                }),
                arxiv_info: reference.arxiv_id.as_ref().map(|id| ArxivInfo {
                    arxiv_id: id.clone(),
                    valid: false,
                    title: None,
                }),
                retraction_info: None,
            };

            emit_final_events(progress.as_ref(), &result, ref_index, total, &title);
            let _ = result_tx.send(result);
            continue;
        }

        // --- Fan out only cache-miss DBs to drainers ---
        let first_mismatch = pre.first_mismatch.or_else(|| {
            if local_result.status == Status::AuthorMismatch {
                Some(MismatchInfo {
                    source: local_result.source.clone().unwrap_or_default(),
                    found_authors: local_result.found_authors.clone(),
                    paper_url: local_result.paper_url.clone(),
                })
            } else {
                None
            }
        });

        let collector = Arc::new(RefCollector {
            reference,
            ref_index,
            total,
            title,
            progress,
            config: config.clone(),
            client: client.clone(),
            remaining: AtomicUsize::new(pre.miss_indices.len()),
            verified: AtomicBool::new(false),
            state: Mutex::new(AggState {
                verified_info: None,
                first_mismatch,
                failed_dbs: vec![],
                db_results: pre.db_results,
                retraction: pre.retraction,
            }),
            result_tx: Mutex::new(Some(result_tx)),
            local_result,
        });

        for &i in &pre.miss_indices {
            let _ = drainer_txs[i].2.try_send(DrainerJob {
                collector: collector.clone(),
            });
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Build per-ref DB completion callback.
fn make_db_callback(
    progress: Arc<dyn Fn(ProgressEvent) + Send + Sync>,
    ref_index: usize,
) -> impl Fn(DbResult) + Send + Sync {
    move |db_result: DbResult| {
        progress(ProgressEvent::DatabaseQueryComplete {
            paper_index: 0,
            ref_index,
            db_name: db_result.db_name.clone(),
            status: db_result.status.clone(),
            elapsed: db_result.elapsed.unwrap_or_default(),
        });
    }
}

/// Emit Warning + Result progress events and log the final outcome.
fn emit_final_events(
    progress: &(dyn Fn(ProgressEvent) + Send + Sync),
    result: &ValidationResult,
    ref_index: usize,
    total: usize,
    title: &str,
) {
    let status_str = match result.status {
        Status::Verified => "Verified",
        Status::NotFound => "NotFound",
        Status::AuthorMismatch => "AuthorMismatch",
    };
    tracing::info!(
        ref_index,
        title,
        status = status_str,
        source = result.source.as_deref().unwrap_or("-"),
        "reference result"
    );

    if !result.failed_dbs.is_empty() {
        let context = match result.status {
            Status::NotFound => "not found in other DBs".to_string(),
            Status::Verified => format!(
                "verified via {}",
                result.source.as_deref().unwrap_or("unknown")
            ),
            Status::AuthorMismatch => format!(
                "author mismatch via {}",
                result.source.as_deref().unwrap_or("unknown")
            ),
        };
        progress(ProgressEvent::Warning {
            index: ref_index,
            total,
            title: title.to_string(),
            failed_dbs: result.failed_dbs.clone(),
            message: format!("{} timed out; {}", result.failed_dbs.join(", "), context),
        });
    }

    progress(ProgressEvent::Result {
        index: ref_index,
        total,
        result: Box::new(result.clone()),
    });
}

/// Build ValidationResult from a DbSearchResult.
fn build_validation_result(
    reference: &Reference,
    title: &str,
    db_result: crate::orchestrator::DbSearchResult,
    retraction_info: Option<crate::RetractionInfo>,
) -> ValidationResult {
    ValidationResult {
        title: title.to_string(),
        raw_citation: reference.raw_citation.clone(),
        ref_authors: reference.authors.clone(),
        status: db_result.status,
        source: db_result.source,
        found_authors: db_result.found_authors,
        paper_url: db_result.paper_url,
        failed_dbs: db_result.failed_dbs,
        db_results: db_result.db_results,
        doi_info: None,
        arxiv_info: reference.arxiv_id.as_ref().map(|id| ArxivInfo {
            arxiv_id: id.clone(),
            valid: false,
            title: None,
        }),
        retraction_info,
    }
}
