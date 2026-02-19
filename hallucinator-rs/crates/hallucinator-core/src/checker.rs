use crate::db::DatabaseBackend;
use crate::db::DbQueryResult;
use crate::db::searxng::Searxng;
use crate::doi::{DoiMatchResult, DoiValidation, check_doi_match, validate_doi};
use crate::orchestrator::query_all_databases;
use crate::pool::{RefJob, ValidationPool};
use crate::retraction::check_retraction;
use crate::{
    ArxivInfo, Config, DbResult, DbStatus, DoiInfo, ProgressEvent, Reference, RetractionInfo,
    Status, ValidationResult,
};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Check a list of references against academic databases.
///
/// Creates an internal ValidationPool with `num_workers` workers.
/// Submits all refs, collects results via oneshot channels.
/// Progress events are emitted via the callback. Cancellation is supported.
pub async fn check_references(
    refs: Vec<Reference>,
    config: Config,
    progress: impl Fn(ProgressEvent) + Send + Sync + 'static,
    cancel: CancellationToken,
) -> Vec<ValidationResult> {
    let total = refs.len();
    if total == 0 {
        return vec![];
    }

    let num_workers = config.num_workers.max(1);
    let config = Arc::new(config);
    let progress = Arc::new(progress);

    // Create the pool
    let pool = ValidationPool::new(config.clone(), cancel.clone(), num_workers);

    // Submit all refs and collect oneshot receivers
    let mut receivers = Vec::with_capacity(total);
    for (i, reference) in refs.iter().enumerate() {
        if cancel.is_cancelled() {
            break;
        }

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let job = RefJob {
            reference: reference.clone(),
            result_tx,
            ref_index: i,
            total,
            progress: progress.clone(),
        };

        pool.submit(job).await;
        receivers.push((i, result_rx));
    }

    // Collect results
    let mut results: Vec<Option<ValidationResult>> = vec![None; total];
    for (i, rx) in receivers {
        if let Ok(result) = rx.await {
            results[i] = Some(result);
        }
    }

    pool.shutdown().await;

    results.into_iter().flatten().collect()
}

/// Check a single reference against all databases.
pub async fn check_single_reference(
    reference: &Reference,
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
) -> ValidationResult {
    let title = reference.title.as_deref().unwrap_or("");
    let timeout = Duration::from_secs(config.db_timeout_secs);

    // Step 1: Validate DOI if present (with cache support)
    let mut doi_info = None;
    if let Some(ref doi) = reference.doi {
        // Check cache first
        let cached = config
            .query_cache
            .as_ref()
            .and_then(|cache| cache.get(title, "DOI"));

        let (doi_result, match_result) = if let Some(ref cached_result) = cached {
            if cached_result.is_found() {
                // Cache hit with Found — reconstruct DOI validation and match
                let doi_val = DoiValidation {
                    valid: true,
                    title: cached_result.found_title.clone(),
                    authors: cached_result.authors.clone(),
                    error: None,
                };
                let match_res = check_doi_match(&doi_val, title, &reference.authors);
                (doi_val, match_res)
            } else {
                // Cache hit with NotFound — skip DOI validation entirely,
                // fall through to DB search by jumping past DOI block
                (
                    DoiValidation {
                        valid: false,
                        title: None,
                        authors: vec![],
                        error: Some("Cached as not found".into()),
                    },
                    DoiMatchResult::Invalid {
                        error: "Cached as not found".into(),
                    },
                )
            }
        } else {
            // Cache miss — call doi.org
            let doi_val = validate_doi(doi, client, timeout).await;
            let match_res = check_doi_match(&doi_val, title, &reference.authors);

            // Cache the result
            if let Some(ref cache) = config.query_cache {
                let cache_entry = match &match_res {
                    DoiMatchResult::Verified {
                        doi_title,
                        doi_authors,
                    }
                    | DoiMatchResult::AuthorMismatch {
                        doi_title,
                        doi_authors,
                    } => DbQueryResult::found(
                        doi_title.clone(),
                        doi_authors.clone(),
                        Some(format!("https://doi.org/{}", doi)),
                    ),
                    _ => DbQueryResult::not_found(),
                };
                cache.insert(title, "DOI", &cache_entry);
            }

            (doi_val, match_res)
        };

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
                let retraction =
                    check_retraction(doi, client, timeout, config.crossref_mailto.as_deref()).await;
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
                    ref_authors: reference.authors.clone(),
                    status: Status::Verified,
                    source: Some("DOI".into()),
                    found_authors: doi_authors,
                    paper_url: Some(format!("https://doi.org/{}", doi)),
                    failed_dbs: vec![],
                    db_results: vec![DbResult {
                        db_name: "DOI".into(),
                        status: DbStatus::Match,
                        elapsed: None,
                        found_authors: vec![],
                        paper_url: Some(format!("https://doi.org/{}", doi)),
                        error_message: None,
                    }],
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
                    ref_authors: reference.authors.clone(),
                    status: Status::AuthorMismatch,
                    source: Some("DOI".into()),
                    found_authors: doi_authors,
                    paper_url: Some(format!("https://doi.org/{}", doi)),
                    failed_dbs: vec![],
                    db_results: vec![DbResult {
                        db_name: "DOI".into(),
                        status: DbStatus::AuthorMismatch,
                        elapsed: None,
                        found_authors: vec![],
                        paper_url: Some(format!("https://doi.org/{}", doi)),
                        error_message: None,
                    }],
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
    let mut db_result = query_all_databases(
        title,
        &reference.authors,
        config,
        client,
        longer_timeout,
        None,
        on_db_complete,
    )
    .await;

    // Step 2.5: SearxNG fallback for NotFound references
    if db_result.status == Status::NotFound
        && let Some(ref searxng_url) = config.searxng_url
    {
        let searxng = Searxng::new(searxng_url.clone());
        let searxng_timeout = Duration::from_secs(config.db_timeout_secs);

        let start = std::time::Instant::now();
        let searxng_result = searxng.query(title, client, searxng_timeout).await;
        let elapsed = start.elapsed();

        if let Ok(ref qr) = searxng_result
            && qr.is_found()
        {
            let paper_url = qr.paper_url.clone();
            // Web search found the paper - update result
            let web_db_result = DbResult {
                db_name: "Web Search".into(),
                status: DbStatus::Match,
                elapsed: Some(elapsed),
                found_authors: vec![], // Web search cannot verify authors
                paper_url: paper_url.clone(),
                error_message: None,
            };
            if let Some(cb) = on_db_complete {
                cb(web_db_result.clone());
            }
            db_result.db_results.push(web_db_result);

            db_result.status = Status::Verified;
            db_result.source = Some("Web Search".into());
            db_result.found_authors = vec![];
            db_result.paper_url = paper_url;
        } else if let Some(cb) = on_db_complete {
            cb(DbResult {
                db_name: "Web Search".into(),
                status: DbStatus::NoMatch,
                elapsed: Some(elapsed),
                found_authors: vec![],
                paper_url: None,
                error_message: None,
            });
        }
    }

    // Step 2.6: OpenAlex API fallback when offline DB is active but didn't find it
    if db_result.status == Status::NotFound
        && config.openalex_offline_db.is_some()
        && let Some(ref api_key) = config.openalex_key
    {
        let openalex = crate::db::openalex::OpenAlex {
            api_key: api_key.clone(),
        };
        let openalex_timeout = Duration::from_secs(config.db_timeout_secs);

        let start = std::time::Instant::now();
        let openalex_result: Result<crate::db::DbQueryResult, crate::rate_limit::DbQueryError> =
            openalex.query(title, client, openalex_timeout).await;
        let elapsed = start.elapsed();

        if let Ok(ref qr) = openalex_result
            && qr.is_found()
        {
            let found_authors = qr.authors.clone();
            let paper_url = qr.paper_url.clone();
            let api_db_result = DbResult {
                db_name: "OpenAlex API".into(),
                status: DbStatus::Match,
                elapsed: Some(elapsed),
                found_authors: found_authors.clone(),
                paper_url: paper_url.clone(),
                error_message: None,
            };
            if let Some(cb) = on_db_complete {
                cb(api_db_result.clone());
            }
            db_result.db_results.push(api_db_result);

            db_result.status = Status::Verified;
            db_result.source = Some("OpenAlex API".into());
            db_result.found_authors = found_authors;
            db_result.paper_url = paper_url;
        } else if let Some(cb) = on_db_complete {
            cb(DbResult {
                db_name: "OpenAlex API".into(),
                status: DbStatus::NoMatch,
                elapsed: Some(elapsed),
                found_authors: vec![],
                paper_url: None,
                error_message: None,
            });
        }
    }

    // Step 3: Check retraction using inline data from DB results.
    // CrossRef populates retraction info in its DbQueryResult, which flows
    // through the cache. No separate API call needed.
    let retraction_info = if db_result.status == Status::Verified {
        db_result.retraction.take().and_then(|r| {
            if r.retracted {
                Some(RetractionInfo {
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
        doi_info,
        arxiv_info: reference.arxiv_id.as_ref().map(|id| ArxivInfo {
            arxiv_id: id.clone(),
            valid: false, // Will be validated separately if needed
            title: None,
        }),
        retraction_info,
    }
}

/// Retry a reference check targeting only the previously failed databases.
pub async fn check_single_reference_retry(
    reference: &Reference,
    config: &Config,
    client: &reqwest::Client,
    failed_dbs: &[String],
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
) -> ValidationResult {
    let title = reference.title.as_deref().unwrap_or("");

    let db_result = query_all_databases(
        title,
        &reference.authors,
        config,
        client,
        true, // longer timeout for retries
        Some(failed_dbs),
        on_db_complete,
    )
    .await;

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
        arxiv_info: None,
        retraction_info: None,
    }
}
