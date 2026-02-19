use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use hallucinator_core::pool::{RefJob, ValidationPool};
use hallucinator_core::{Config, ExtractionResult, ProgressEvent};

use crate::tui_event::BackendEvent;

/// Remap progress event indices from the filtered (checkable-only) vec back to
/// the full all-refs vec, so the TUI's ref_states (which includes skipped refs)
/// receives updates at the correct positions.
fn remap_progress_index(event: ProgressEvent, index_map: &[usize]) -> ProgressEvent {
    match event {
        ProgressEvent::Checking {
            index,
            total,
            title,
        } => ProgressEvent::Checking {
            index: index_map.get(index).copied().unwrap_or(index),
            total,
            title,
        },
        ProgressEvent::Result {
            index,
            total,
            result,
        } => ProgressEvent::Result {
            index: index_map.get(index).copied().unwrap_or(index),
            total,
            result,
        },
        ProgressEvent::Warning {
            index,
            total,
            title,
            failed_dbs,
            message,
        } => ProgressEvent::Warning {
            index: index_map.get(index).copied().unwrap_or(index),
            total,
            title,
            failed_dbs,
            message,
        },
        ProgressEvent::DatabaseQueryComplete {
            paper_index,
            ref_index,
            db_name,
            status,
            elapsed,
        } => ProgressEvent::DatabaseQueryComplete {
            paper_index,
            ref_index: index_map.get(ref_index).copied().unwrap_or(ref_index),
            db_name,
            status,
            elapsed,
        },
        other => other,
    }
}

/// Run batch validation with paper indices starting at `offset`.
///
/// Creates a single global `ValidationPool` shared by all papers.
/// Each paper gets its own task for extraction + job submission, so all
/// papers can feed refs into the pool concurrently. The `num_workers`
/// setting controls the total number of concurrent reference validations.
pub async fn run_batch_with_offset(
    pdfs: Vec<PathBuf>,
    config: Config,
    tx: mpsc::UnboundedSender<BackendEvent>,
    cancel: CancellationToken,
    offset: usize,
) {
    let num_workers = config.num_workers.max(1);
    let config = Arc::new(config);

    // Create ONE global validation pool for all papers
    let pool = ValidationPool::new(config.clone(), cancel.clone(), num_workers);
    let pool_tx = pool.sender();

    // Spawn one task per paper. Extraction is fast (CPU-bound via spawn_blocking),
    // and each task then submits refs to the shared pool and awaits results.
    // The ValidationPool is the sole concurrency bottleneck.
    let mut handles = Vec::new();
    for (i, pdf_path) in pdfs.into_iter().enumerate() {
        let paper_index = offset + i;
        let pool_tx = pool_tx.clone();
        let tx = tx.clone();
        let cancel = cancel.clone();

        handles.push(tokio::spawn(async move {
            if cancel.is_cancelled() {
                return;
            }
            process_single_paper(paper_index, &pdf_path, &pool_tx, &tx, &cancel).await;
        }));
    }

    // Drop our clone of the pool sender so shutdown can complete
    // once all paper tasks finish submitting
    drop(pool_tx);

    for handle in handles {
        let _ = handle.await;
    }

    // All papers have been extracted and all jobs submitted.
    // Shut down the pool and wait for remaining validations to finish.
    pool.shutdown().await;

    let _ = tx.send(BackendEvent::BatchComplete);
}

/// Process a single paper: extract references, submit to shared pool, collect results.
async fn process_single_paper(
    paper_index: usize,
    pdf_path: &std::path::Path,
    pool_tx: &async_channel::Sender<RefJob>,
    tx: &mpsc::UnboundedSender<BackendEvent>,
    cancel: &CancellationToken,
) {
    // Signal extraction start
    let _ = tx.send(BackendEvent::ExtractionStarted { paper_index });

    // Extract references (blocking call)
    let path = pdf_path.to_path_buf();
    let extraction: Result<ExtractionResult, String> = tokio::task::spawn_blocking(move || {
        hallucinator_ingest::extract_references(&path)
            .map_err(|e| format!("Extraction failed: {}", e))
    })
    .await
    .unwrap_or_else(|e| Err(format!("Task join error: {}", e)));

    let extraction = match extraction {
        Ok(ext) => ext,
        Err(error) => {
            let _ = tx.send(BackendEvent::ExtractionFailed { paper_index, error });
            return;
        }
    };

    let skip_stats = extraction.skip_stats.clone();
    let all_refs = extraction.references;

    // Count only non-skipped refs for the ref_count (used for stats/progress)
    let checkable_count = all_refs.iter().filter(|r| r.skip_reason.is_none()).count();

    let _ = tx.send(BackendEvent::ExtractionComplete {
        paper_index,
        ref_count: checkable_count,
        references: all_refs.clone(),
        skip_stats,
    });

    // Build a mapping from filtered (checkable) index → original all_refs index,
    // so that progress events use indices into the full ref_states array.
    let index_map: Arc<Vec<usize>> = Arc::new(
        all_refs
            .iter()
            .enumerate()
            .filter(|(_, r)| r.skip_reason.is_none())
            .map(|(i, _)| i)
            .collect(),
    );

    // Filter to only checkable refs for validation
    let refs: Vec<_> = all_refs
        .into_iter()
        .filter(|r| r.skip_reason.is_none())
        .collect();

    if refs.is_empty() {
        let _ = tx.send(BackendEvent::PaperComplete { paper_index });
        return;
    }

    let total = refs.len();

    // Submit all refs to the shared pool and collect oneshot receivers
    let mut receivers = Vec::with_capacity(total);
    for (i, reference) in refs.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();

        // Build per-ref progress callback that remaps indices and tags with paper_index
        let tx_progress = tx.clone();
        let index_map = Arc::clone(&index_map);
        let progress_cb = move |event: ProgressEvent| {
            let event = remap_progress_index(event, &index_map);
            let _ = tx_progress.send(BackendEvent::Progress {
                paper_index,
                event: Box::new(event),
            });
        };

        let job = RefJob {
            reference: reference.clone(),
            result_tx,
            ref_index: i,
            total,
            progress: Arc::new(progress_cb),
        };

        let _ = pool_tx.send(job).await;
        receivers.push((i, result_rx));
    }

    // Await all receivers (results are already sent via Progress events)
    for (_i, rx) in receivers {
        let _ = rx.await;
    }

    let _ = tx.send(BackendEvent::PaperComplete { paper_index });
}

/// Retry specific references for a paper, re-checking against failed (or all) databases.
pub async fn retry_references(
    paper_index: usize,
    refs_to_retry: Vec<(usize, hallucinator_core::Reference, Vec<String>)>,
    config: Config,
    tx: mpsc::UnboundedSender<BackendEvent>,
) {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(2)
        .pool_idle_timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let config = Arc::new(config);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.num_workers.max(1)));
    let total = refs_to_retry.len();

    let mut handles = Vec::new();

    for (ref_index, reference, failed_dbs) in refs_to_retry {
        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let client = client.clone();
        let config = Arc::clone(&config);
        let tx = tx.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit;

            let title = reference.title.as_deref().unwrap_or("").to_string();
            let _ = tx.send(BackendEvent::Progress {
                paper_index,
                event: Box::new(hallucinator_core::ProgressEvent::Checking {
                    index: ref_index,
                    total,
                    title,
                }),
            });

            let result = if failed_dbs.is_empty() {
                // Full re-check against all databases
                hallucinator_core::checker::check_single_reference(
                    &reference, &config, &client, true, // longer timeout
                    None,
                )
                .await
            } else {
                // Retry only against previously failed databases
                hallucinator_core::checker::check_single_reference_retry(
                    &reference,
                    &config,
                    &client,
                    &failed_dbs,
                    None,
                )
                .await
            };

            let _ = tx.send(BackendEvent::Progress {
                paper_index,
                event: Box::new(hallucinator_core::ProgressEvent::Result {
                    index: ref_index,
                    total,
                    result: Box::new(result),
                }),
            });
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }
}

/// Open offline DBLP database if a path is configured, returning the Arc<Mutex<..>> handle.
pub fn open_dblp_db(
    path: &std::path::Path,
) -> anyhow::Result<Arc<Mutex<hallucinator_dblp::DblpDatabase>>> {
    if !path.exists() {
        anyhow::bail!(
            "Offline DBLP database not found at {}. Build from Config > Databases (b) or run 'hallucinator-tui update-dblp'.",
            path.display()
        );
    }
    let db = hallucinator_dblp::DblpDatabase::open(path)?;
    Ok(Arc::new(Mutex::new(db)))
}

/// Open offline ACL Anthology database if a path is configured, returning the Arc<Mutex<..>> handle.
pub fn open_acl_db(
    path: &std::path::Path,
) -> anyhow::Result<Arc<Mutex<hallucinator_acl::AclDatabase>>> {
    if !path.exists() {
        anyhow::bail!(
            "Offline ACL database not found at {}. Build from Config > Databases (b) or run 'hallucinator-tui update-acl'.",
            path.display(),
        );
    }
    let db = hallucinator_acl::AclDatabase::open(path)?;
    Ok(Arc::new(Mutex::new(db)))
}

/// Open offline OpenAlex Tantivy index if a path is configured, returning the Arc<Mutex<..>> handle.
pub fn open_openalex_db(
    path: &std::path::Path,
) -> anyhow::Result<Arc<Mutex<hallucinator_openalex::OpenAlexDatabase>>> {
    if !path.exists() {
        anyhow::bail!(
            "Offline OpenAlex index not found at {}. Build from Config > Databases (b) or run 'hallucinator-tui update-openalex'.",
            path.display(),
        );
    }
    let db = hallucinator_openalex::OpenAlexDatabase::open(path)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    Ok(Arc::new(Mutex::new(db)))
}
