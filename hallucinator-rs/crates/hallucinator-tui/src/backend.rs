use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use hallucinator_core::{Config, ProgressEvent};
use hallucinator_pdf::ExtractionResult;

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
/// Spawns `max_concurrent_papers` worker tasks that pull from a shared work
/// queue. Each worker processes one paper at a time, then grabs the next.
pub async fn run_batch_with_offset(
    pdfs: Vec<PathBuf>,
    config: Config,
    tx: mpsc::UnboundedSender<BackendEvent>,
    cancel: CancellationToken,
    offset: usize,
    max_concurrent_papers: usize,
) {
    let config = Arc::new(config);
    let num_workers = max_concurrent_papers.max(1);

    // Work queue: each item is (paper_index, path)
    let (work_tx, work_rx) = mpsc::channel::<(usize, PathBuf)>(pdfs.len().max(1));

    // Feed the queue upfront
    for (i, pdf_path) in pdfs.into_iter().enumerate() {
        let _ = work_tx.send((offset + i, pdf_path)).await;
    }
    drop(work_tx); // close sender so workers exit when queue is drained

    // Wrap receiver in Arc<Mutex> so workers can share it
    let work_rx = Arc::new(tokio::sync::Mutex::new(work_rx));

    let mut handles = Vec::new();
    for _ in 0..num_workers {
        let work_rx = work_rx.clone();
        let config = config.clone();
        let tx = tx.clone();
        let cancel = cancel.clone();

        handles.push(tokio::spawn(async move {
            loop {
                let item = {
                    let mut rx = work_rx.lock().await;
                    rx.recv().await
                };
                match item {
                    Some((paper_index, pdf_path)) => {
                        if cancel.is_cancelled() {
                            break;
                        }
                        process_single_paper(paper_index, &pdf_path, &config, &tx, &cancel).await;
                    }
                    None => break, // queue drained
                }
            }
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    let _ = tx.send(BackendEvent::BatchComplete);
}

/// Process a single paper: extract references, validate, send events.
async fn process_single_paper(
    paper_index: usize,
    pdf_path: &std::path::Path,
    config: &Config,
    tx: &mpsc::UnboundedSender<BackendEvent>,
    cancel: &CancellationToken,
) {
    // Signal extraction start
    let _ = tx.send(BackendEvent::ExtractionStarted { paper_index });

    // Extract references (blocking call)
    let path = pdf_path.to_path_buf();
    let is_bbl = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bbl"));
    let is_bib = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("bib"));

    let extraction: Result<ExtractionResult, String> = tokio::task::spawn_blocking(move || {
        if is_bbl {
            hallucinator_bbl::extract_references_from_bbl(&path)
                .map_err(|e| format!("BBL extraction failed: {}", e))
        } else if is_bib {
            hallucinator_bbl::extract_references_from_bib(&path)
                .map_err(|e| format!("BIB extraction failed: {}", e))
        } else {
            hallucinator_pdf::extract_references(&path)
                .map_err(|e| format!("PDF extraction failed: {}", e))
        }
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
    let ref_titles: Vec<String> = all_refs
        .iter()
        .map(|r| r.title.clone().unwrap_or_default())
        .collect();

    // Count only non-skipped refs for the ref_count (used for stats/progress)
    let checkable_count = all_refs.iter().filter(|r| r.skip_reason.is_none()).count();

    let _ = tx.send(BackendEvent::ExtractionComplete {
        paper_index,
        ref_count: checkable_count,
        ref_titles,
        references: all_refs.clone(),
        skip_stats,
    });

    // Build a mapping from filtered (checkable) index → original all_refs index,
    // so that progress events use indices into the full ref_states array.
    let index_map: Vec<usize> = all_refs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.skip_reason.is_none())
        .map(|(i, _)| i)
        .collect();

    // Filter to only checkable refs for validation
    let refs: Vec<_> = all_refs
        .into_iter()
        .filter(|r| r.skip_reason.is_none())
        .collect();

    if refs.is_empty() {
        let _ = tx.send(BackendEvent::PaperComplete {
            paper_index,
            results: vec![],
        });
        return;
    }

    // Build per-paper config
    let paper_config = (*config).clone();

    // Bridge sync progress callback → async channel via unbounded send.
    // Remap the checker's filtered indices back to the full ref_states indices.
    let tx_progress = tx.clone();
    let progress_cb = move |event: ProgressEvent| {
        let event = remap_progress_index(event, &index_map);
        let _ = tx_progress.send(BackendEvent::Progress {
            paper_index,
            event: Box::new(event),
        });
    };

    let paper_cancel = cancel.clone();
    let results =
        hallucinator_core::check_references(refs, paper_config, progress_cb, paper_cancel).await;

    let _ = tx.send(BackendEvent::PaperComplete {
        paper_index,
        results,
    });
}

/// Retry specific references for a paper, re-checking against failed (or all) databases.
pub async fn retry_references(
    paper_index: usize,
    refs_to_retry: Vec<(usize, hallucinator_core::Reference, Vec<String>)>,
    config: Config,
    tx: mpsc::UnboundedSender<BackendEvent>,
) {
    let client = reqwest::Client::new();
    let config = Arc::new(config);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_refs));
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
            "Offline DBLP database not found at {}. Run 'hallucinator-tui update-dblp' to build it.",
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
            "Offline ACL database not found at {}. Use 'hallucinator-tui update-acl' to build it.",
            path.display(),
        );
    }
    let db = hallucinator_acl::AclDatabase::open(path)?;
    Ok(Arc::new(Mutex::new(db)))
}
