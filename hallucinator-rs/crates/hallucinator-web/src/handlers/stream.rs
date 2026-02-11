use axum::extract::{Multipart, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::IntoResponse;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use hallucinator_core::{Config, ProgressEvent, SkipStats, ValidationResult};
use hallucinator_pdf::ExtractionResult;

use crate::models::*;
use crate::state::AppState;
use crate::upload::{self, FileType, FormFields};
use hallucinator_pdf::archive::{self, ExtractedPdf};

pub async fn stream(State(state): State<Arc<AppState>>, multipart: Multipart) -> impl IntoResponse {
    let (tx, rx) = mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        if let Err(e) = handle_stream(state, multipart, tx.clone()).await {
            let _ = tx
                .send(Ok(sse_event("error", &ErrorEvent { message: e })))
                .await;
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn handle_stream(
    state: Arc<AppState>,
    multipart: Multipart,
    tx: mpsc::Sender<Result<Event, Infallible>>,
) -> Result<(), String> {
    // Parse the multipart form
    let fields = upload::parse_multipart(multipart).await?;

    // Create a temp directory (auto-cleaned on drop)
    let temp_dir =
        tempfile::tempdir().map_err(|e| format!("Failed to create temp directory: {}", e))?;

    match fields.file.file_type {
        FileType::Pdf => handle_single_pdf(state, fields, tx, temp_dir).await,
        FileType::Zip => {
            let result = archive::extract_from_zip(&fields.file.data, temp_dir.path(), 0)?;
            handle_archive(state, fields, result.pdfs, tx, temp_dir).await
        }
        FileType::TarGz => {
            let result = archive::extract_from_tar_gz(&fields.file.data, temp_dir.path(), 0)?;
            handle_archive(state, fields, result.pdfs, tx, temp_dir).await
        }
    }
}

async fn handle_single_pdf(
    state: Arc<AppState>,
    fields: FormFields,
    tx: mpsc::Sender<Result<Event, Infallible>>,
    temp_dir: tempfile::TempDir,
) -> Result<(), String> {
    // Write PDF to temp file
    let filename = &fields.file.filename;
    let pdf_path = temp_dir.path().join("upload.pdf");
    std::fs::write(&pdf_path, &fields.file.data)
        .map_err(|e| format!("Failed to write temp file: {}", e))?;

    // Extract references (blocking I/O via MuPDF)
    let extraction = extract_pdf_blocking(&pdf_path)
        .await
        .map_err(|e| format!("{}: {}", filename, e))?;

    // Temp dir no longer needed after extraction
    drop(temp_dir);

    let skip_stats = extraction.skip_stats.clone();
    let refs = extraction.references;

    // Send extraction_complete event
    send(
        &tx,
        "extraction_complete",
        &ExtractionCompleteEvent {
            total_refs: refs.len(),
            skip_stats: SkipStatsJson {
                total_raw: skip_stats.total_raw,
                skipped_url: skip_stats.url_only,
                skipped_short_title: skip_stats.short_title,
                skipped_no_authors: skip_stats.no_authors,
            },
        },
    )
    .await?;

    // Run validation in a separate task so we can detect client disconnect
    let config = build_config(&state, &fields);
    let cancel = CancellationToken::new();
    let cancel_for_disconnect = cancel.clone();
    let tx_progress = tx.clone();

    let validation_handle = tokio::spawn(async move {
        hallucinator_core::check_references(
            refs,
            config,
            move |event| {
                send_progress_event(&tx_progress, &event, None);
            },
            cancel,
        )
        .await
    });

    // Race between validation completing and client disconnecting
    let results = tokio::select! {
        result = validation_handle => {
            result.map_err(|e| format!("Validation task error: {}", e))?
        }
        _ = tx.closed() => {
            cancel_for_disconnect.cancel();
            return Err("Client disconnected".to_string());
        }
    };

    // Send complete event
    let summary = SummaryJson::from_results(&results, &skip_stats);
    let result_jsons: Vec<ResultJson> = results.iter().map(ResultJson::from).collect();

    send(
        &tx,
        "complete",
        &CompleteEvent {
            summary,
            results: result_jsons,
            file_count: None,
            files: None,
        },
    )
    .await?;

    Ok(())
}

async fn handle_archive(
    state: Arc<AppState>,
    fields: FormFields,
    pdfs: Vec<ExtractedPdf>,
    tx: mpsc::Sender<Result<Event, Infallible>>,
    temp_dir: tempfile::TempDir,
) -> Result<(), String> {
    let file_count = pdfs.len();

    send(&tx, "archive_start", &ArchiveStartEvent { file_count }).await?;

    let mut all_results: Vec<ResultJson> = Vec::new();
    let mut file_results: Vec<FileResultJson> = Vec::new();
    let mut aggregate_skip_stats = SkipStats::default();

    let cancel = CancellationToken::new();
    let cancel_for_disconnect = cancel.clone();

    for (file_index, pdf) in pdfs.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }

        // Check for client disconnect before each file
        if tx.is_closed() {
            cancel.cancel();
            break;
        }

        send(
            &tx,
            "file_start",
            &FileStartEvent {
                file_index,
                file_count,
                filename: pdf.filename.clone(),
            },
        )
        .await?;

        match process_archive_file(&state, &fields, pdf, &tx, &cancel, &cancel_for_disconnect).await
        {
            Ok((results, skip_stats)) => {
                // Accumulate skip stats
                aggregate_skip_stats.total_raw += skip_stats.total_raw;
                aggregate_skip_stats.url_only += skip_stats.url_only;
                aggregate_skip_stats.short_title += skip_stats.short_title;
                aggregate_skip_stats.no_authors += skip_stats.no_authors;

                let summary = SummaryJson::from_results(&results, &skip_stats);
                let result_jsons: Vec<ResultJson> = results.iter().map(ResultJson::from).collect();

                send(
                    &tx,
                    "file_complete",
                    &FileCompleteEvent {
                        filename: pdf.filename.clone(),
                        success: true,
                        error: None,
                        summary: Some(summary.clone()),
                        results: result_jsons.clone(),
                    },
                )
                .await?;

                all_results.extend(result_jsons.clone());
                file_results.push(FileResultJson {
                    filename: pdf.filename.clone(),
                    success: true,
                    summary: Some(summary),
                    results: result_jsons,
                    error: None,
                });
            }
            Err(e) => {
                // Client disconnect propagates as an error — stop processing
                if tx.is_closed() {
                    cancel.cancel();
                    break;
                }

                send(
                    &tx,
                    "file_complete",
                    &FileCompleteEvent {
                        filename: pdf.filename.clone(),
                        success: false,
                        error: Some(e.clone()),
                        summary: None,
                        results: vec![],
                    },
                )
                .await?;

                file_results.push(FileResultJson {
                    filename: pdf.filename.clone(),
                    success: false,
                    summary: None,
                    results: vec![],
                    error: Some(e),
                });
            }
        }
    }

    // Compute aggregate summary from all_results
    let aggregate_summary = compute_aggregate_summary(&all_results, &aggregate_skip_stats);

    send(
        &tx,
        "complete",
        &CompleteEvent {
            summary: aggregate_summary,
            results: all_results,
            file_count: Some(file_count),
            files: Some(file_results),
        },
    )
    .await?;

    drop(temp_dir);
    Ok(())
}

async fn process_archive_file(
    state: &Arc<AppState>,
    fields: &FormFields,
    pdf: &ExtractedPdf,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    cancel: &CancellationToken,
    cancel_for_disconnect: &CancellationToken,
) -> Result<(Vec<ValidationResult>, SkipStats), String> {
    let extraction = extract_pdf_blocking(&pdf.path)
        .await
        .map_err(|e| format!("{}: {}", pdf.filename, e))?;

    let skip_stats = extraction.skip_stats.clone();
    let refs = extraction.references;

    send(
        tx,
        "extraction_complete",
        &ExtractionCompleteEvent {
            total_refs: refs.len(),
            skip_stats: SkipStatsJson {
                total_raw: skip_stats.total_raw,
                skipped_url: skip_stats.url_only,
                skipped_short_title: skip_stats.short_title,
                skipped_no_authors: skip_stats.no_authors,
            },
        },
    )
    .await?;

    let config = build_config(state, fields);
    let cancel_clone = cancel.clone();
    let cancel_disconnect = cancel_for_disconnect.clone();
    let tx_progress = tx.clone();
    let tx_closed = tx.clone();
    let filename = pdf.filename.clone();

    let validation_handle = tokio::spawn(async move {
        hallucinator_core::check_references(
            refs,
            config,
            move |event| {
                send_progress_event(&tx_progress, &event, Some(&filename));
            },
            cancel_clone,
        )
        .await
    });

    // Race between validation completing and client disconnecting
    let results = tokio::select! {
        result = validation_handle => {
            result.map_err(|e| format!("Validation task error: {}", e))?
        }
        _ = tx_closed.closed() => {
            cancel_disconnect.cancel();
            return Err("Client disconnected".to_string());
        }
    };

    Ok((results, skip_stats))
}

/// Extract references from a PDF using blocking I/O (MuPDF is not async).
async fn extract_pdf_blocking(path: &std::path::Path) -> Result<ExtractionResult, String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        hallucinator_pdf::extract_references(&path)
            .map_err(|e| format!("PDF extraction failed: {}", e))
    })
    .await
    .map_err(|e| format!("Task join error: {}", e))?
}

/// Build a Config from AppState and FormFields.
fn build_config(state: &AppState, fields: &FormFields) -> Config {
    Config {
        openalex_key: fields.openalex_key.clone(),
        s2_api_key: fields.s2_api_key.clone(),
        dblp_offline_path: state.dblp_offline_path.clone(),
        dblp_offline_db: state.dblp_offline_db.clone(),
        disabled_dbs: fields.disabled_dbs.clone(),
        check_openalex_authors: fields.check_openalex_authors,
        ..Config::default()
    }
}

/// Send a progress event from the sync callback via try_send (non-blocking).
fn send_progress_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    event: &ProgressEvent,
    filename: Option<&str>,
) {
    let sse = match event {
        ProgressEvent::Checking {
            index,
            total,
            title,
        } => sse_event(
            "checking",
            &CheckingEvent {
                index: *index,
                total: *total,
                title: title.clone(),
                filename: filename.map(String::from),
            },
        ),
        ProgressEvent::Result {
            index,
            total,
            result,
        } => sse_event(
            "result",
            &ResultEvent {
                result: ResultJson::from(result.as_ref()),
                index: *index,
                total: *total,
                filename: filename.map(String::from),
            },
        ),
        ProgressEvent::Warning {
            index,
            total,
            title,
            failed_dbs,
            message,
        } => sse_event(
            "warning",
            &WarningEvent {
                index: *index,
                total: *total,
                title: title.clone(),
                failed_dbs: failed_dbs.clone(),
                message: message.clone(),
                filename: filename.map(String::from),
            },
        ),
        ProgressEvent::RetryPass { count } => {
            sse_event("retry_pass", &RetryPassEvent { count: *count })
        }
        ProgressEvent::DatabaseQueryComplete { .. } => {
            // Not sent via SSE (detail only needed in TUI)
            return;
        }
    };

    // Use try_send since we're in a sync context.
    // If the channel is full or closed (client disconnected), we just drop the event.
    let _ = tx.try_send(Ok(sse));
}

/// Send an SSE event, returning Err if the client disconnected.
async fn send<T: serde::Serialize>(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    event_type: &str,
    data: &T,
) -> Result<(), String> {
    tx.send(Ok(sse_event(event_type, data)))
        .await
        .map_err(|_| "Client disconnected".to_string())
}

/// Compute aggregate summary from flattened ResultJson list.
fn compute_aggregate_summary(results: &[ResultJson], skip_stats: &SkipStats) -> SummaryJson {
    let verified = results.iter().filter(|r| r.status == "verified").count();
    let not_found = results.iter().filter(|r| r.status == "not_found").count();
    let mismatched = results
        .iter()
        .filter(|r| r.status == "author_mismatch")
        .count();

    SummaryJson {
        total_raw: skip_stats.total_raw,
        total: results.len(),
        verified,
        not_found,
        mismatched,
        skipped: skip_stats.url_only + skip_stats.short_title,
        skipped_url: skip_stats.url_only,
        skipped_short_title: skip_stats.short_title,
        title_only: skip_stats.no_authors,
    }
}
