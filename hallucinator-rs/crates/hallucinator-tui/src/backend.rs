use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use hallucinator_core::{Config, ProgressEvent};
use hallucinator_pdf::ExtractionResult;

use crate::tui_event::BackendEvent;

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

    let extraction: Result<ExtractionResult, String> = tokio::task::spawn_blocking(move || {
        if is_bbl {
            hallucinator_bbl::extract_references_from_bbl(&path)
                .map_err(|e| format!("BBL extraction failed: {}", e))
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
    let refs = extraction.references;
    let ref_titles: Vec<String> = refs
        .iter()
        .map(|r| r.title.clone().unwrap_or_default())
        .collect();

    let _ = tx.send(BackendEvent::ExtractionComplete {
        paper_index,
        ref_count: refs.len(),
        ref_titles,
        references: refs.clone(),
        skip_stats,
    });

    if refs.is_empty() {
        let _ = tx.send(BackendEvent::PaperComplete {
            paper_index,
            results: vec![],
        });
        return;
    }

    // Build per-paper config
    let paper_config = (*config).clone();

    // Bridge sync progress callback → async channel via unbounded send
    let tx_progress = tx.clone();
    let progress_cb = move |event: ProgressEvent| {
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

/// Open offline DBLP database if a path is configured, returning the Arc<Mutex<..>> handle.
pub fn open_dblp_db(
    path: &std::path::Path,
) -> anyhow::Result<Arc<Mutex<hallucinator_dblp::DblpDatabase>>> {
    if !path.exists() {
        anyhow::bail!(
            "Offline DBLP database not found at {}. Use hallucinator-cli --update-dblp={} to build it.",
            path.display(),
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
