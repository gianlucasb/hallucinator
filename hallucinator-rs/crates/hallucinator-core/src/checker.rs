use crate::doi::{check_doi_match, validate_doi, DoiMatchResult};
use crate::orchestrator::query_all_databases;
use crate::retraction::{check_retraction, check_retraction_by_title};
use crate::{
    ArxivInfo, Config, DoiInfo, ProgressEvent, Reference, RetractionInfo, Status,
    ValidationResult,
};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

/// Check a list of references against academic databases.
///
/// Validates each reference concurrently (up to `max_concurrent_refs` at a time),
/// querying multiple databases in parallel per reference.
/// Progress events are emitted via the callback. Cancellation is supported.
pub async fn check_references(
    refs: Vec<Reference>,
    config: Config,
    progress: impl Fn(ProgressEvent) + Send + Sync + 'static,
    cancel: CancellationToken,
) -> Vec<ValidationResult> {
    let total = refs.len();
    let client = reqwest::Client::new();
    let semaphore = Arc::new(Semaphore::new(config.max_concurrent_refs));
    let config = Arc::new(config);
    let progress = Arc::new(progress);

    let mut results: Vec<Option<ValidationResult>> = vec![None; total];
    let mut retry_candidates: Vec<(usize, Reference)> = Vec::new();

    // First pass: check all references
    let mut handles = Vec::new();

    for (i, reference) in refs.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }

        let permit = semaphore.clone().acquire_owned().await.unwrap();
        let reference = reference.clone();
        let client = client.clone();
        let config = Arc::clone(&config);
        let progress = Arc::clone(&progress);
        let cancel = cancel.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit; // Hold until done

            if cancel.is_cancelled() {
                return (i, None);
            }

            let title = reference.title.as_deref().unwrap_or("");
            progress(ProgressEvent::Checking {
                index: i,
                total,
                title: title.to_string(),
            });

            let result =
                check_single_reference(&reference, &config, &client, false).await;

            progress(ProgressEvent::Result {
                index: i,
                total,
                result: result.clone(),
            });

            (i, Some(result))
        });

        handles.push(handle);
    }

    // Collect results
    for handle in handles {
        if let Ok((i, Some(result))) = handle.await {
            // Track retry candidates
            if result.status == Status::NotFound && !result.failed_dbs.is_empty() {
                retry_candidates.push((i, refs[i].clone()));
            }
            results[i] = Some(result);
        }
    }

    // Retry pass: re-check references that had failed DBs
    if !retry_candidates.is_empty() && !cancel.is_cancelled() {
        progress(ProgressEvent::RetryPass {
            count: retry_candidates.len(),
        });

        for (i, reference) in retry_candidates {
            if cancel.is_cancelled() {
                break;
            }

            let prev_result = results[i].as_ref().unwrap();
            let failed_dbs = prev_result.failed_dbs.clone();

            let result = check_single_reference_retry(
                &reference,
                &config,
                &client,
                &failed_dbs,
            )
            .await;

            // Only update if retry found something better
            if result.status != Status::NotFound {
                progress(ProgressEvent::Result {
                    index: i,
                    total,
                    result: result.clone(),
                });
                results[i] = Some(result);
            }
        }
    }

    results.into_iter().filter_map(|r| r).collect()
}

/// Check a single reference against all databases.
async fn check_single_reference(
    reference: &Reference,
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
) -> ValidationResult {
    let title = reference.title.as_deref().unwrap_or("");
    let timeout = Duration::from_secs(config.db_timeout_secs);

    // Step 1: Validate DOI if present
    let mut doi_info = None;
    if let Some(ref doi) = reference.doi {
        let doi_result = validate_doi(doi, client, timeout).await;
        let match_result = check_doi_match(&doi_result, title, &reference.authors);

        doi_info = Some(DoiInfo {
            doi: doi.clone(),
            valid: doi_result.valid,
            title: doi_result.title.clone(),
        });

        match match_result {
            DoiMatchResult::Verified {
                doi_title: _,
                doi_authors,
            } => {
                // Check retraction
                let retraction = check_retraction(doi, client, timeout).await;
                let retraction_info = if retraction.retracted {
                    Some(RetractionInfo {
                        is_retracted: true,
                        retraction_doi: retraction.retraction_doi,
                        retraction_source: retraction.retraction_type,
                    })
                } else {
                    None
                };

                return ValidationResult {
                    title: title.to_string(),
                    raw_citation: reference.raw_citation.clone(),
                    status: if retraction_info.is_some() {
                        Status::Verified // Still verified, but flagged
                    } else {
                        Status::Verified
                    },
                    source: Some("DOI".into()),
                    found_authors: doi_authors,
                    paper_url: Some(format!("https://doi.org/{}", doi)),
                    failed_dbs: vec![],
                    doi_info,
                    arxiv_info: None,
                    retraction_info,
                };
            }
            DoiMatchResult::AuthorMismatch {
                doi_title: _,
                doi_authors,
            } => {
                return ValidationResult {
                    title: title.to_string(),
                    raw_citation: reference.raw_citation.clone(),
                    status: Status::AuthorMismatch,
                    source: Some("DOI".into()),
                    found_authors: doi_authors,
                    paper_url: Some(format!("https://doi.org/{}", doi)),
                    failed_dbs: vec![],
                    doi_info,
                    arxiv_info: None,
                    retraction_info: None,
                };
            }
            _ => {
                // DOI invalid or title mismatch — fall through to DB search
            }
        }
    }

    // Step 2: Query all databases concurrently
    let db_result =
        query_all_databases(title, &reference.authors, config, client, longer_timeout, None).await;

    // Step 3: Check retraction by title if verified
    let retraction_info = if db_result.status == Status::Verified {
        let retraction = check_retraction_by_title(title, client, timeout).await;
        if retraction.retracted {
            Some(RetractionInfo {
                is_retracted: true,
                retraction_doi: retraction.retraction_doi,
                retraction_source: retraction.retraction_type,
            })
        } else {
            None
        }
    } else {
        None
    };

    ValidationResult {
        title: title.to_string(),
        raw_citation: reference.raw_citation.clone(),
        status: db_result.status,
        source: db_result.source,
        found_authors: db_result.found_authors,
        paper_url: db_result.paper_url,
        failed_dbs: db_result.failed_dbs,
        doi_info,
        arxiv_info: reference
            .arxiv_id
            .as_ref()
            .map(|id| ArxivInfo {
                arxiv_id: id.clone(),
                valid: false, // Will be validated separately if needed
                title: None,
            }),
        retraction_info,
    }
}

/// Retry a reference check targeting only the previously failed databases.
async fn check_single_reference_retry(
    reference: &Reference,
    config: &Config,
    client: &reqwest::Client,
    failed_dbs: &[String],
) -> ValidationResult {
    let title = reference.title.as_deref().unwrap_or("");

    let db_result = query_all_databases(
        title,
        &reference.authors,
        config,
        client,
        true, // longer timeout for retries
        Some(failed_dbs),
    )
    .await;

    ValidationResult {
        title: title.to_string(),
        raw_citation: reference.raw_citation.clone(),
        status: db_result.status,
        source: db_result.source,
        found_authors: db_result.found_authors,
        paper_url: db_result.paper_url,
        failed_dbs: db_result.failed_dbs,
        doi_info: None,
        arxiv_info: None,
        retraction_info: None,
    }
}
