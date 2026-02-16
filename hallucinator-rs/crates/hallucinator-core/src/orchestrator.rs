use crate::authors::validate_authors;
use crate::db::DatabaseBackend;
use crate::rate_limit::{query_with_backoff, RateLimiter};
use crate::{Config, DbResult, DbStatus, Status};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Result of querying all databases for a single reference.
#[derive(Debug, Clone)]
pub struct DbSearchResult {
    pub status: Status,
    pub source: Option<String>,
    pub found_authors: Vec<String>,
    pub paper_url: Option<String>,
    pub failed_dbs: Vec<String>,
    pub db_results: Vec<DbResult>,
}

/// Query all databases for a single reference, with early exit on match.
///
/// Offline databases are queried first. If any returns a verified match,
/// online databases are skipped entirely. Otherwise, online databases are
/// queried concurrently with rate limiting and exponential backoff on 429s.
///
/// If `on_db_complete` is provided, it is called for each database as it finishes.
pub async fn query_all_databases(
    title: &str,
    ref_authors: &[String],
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
    only_dbs: Option<&[String]>,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
    rate_limiter: &Arc<RateLimiter>,
) -> DbSearchResult {
    let check_openalex_authors = config.check_openalex_authors;
    let timeout = if longer_timeout {
        Duration::from_secs(config.db_timeout_secs * 2)
    } else {
        Duration::from_secs(config.db_timeout_secs)
    };

    // Build the list of databases to query
    let databases: Vec<Arc<dyn DatabaseBackend>> = build_database_list(config, only_dbs)
        .into_iter()
        .map(Arc::from)
        .collect();

    if databases.is_empty() {
        return DbSearchResult {
            status: Status::NotFound,
            source: None,
            found_authors: vec![],
            paper_url: None,
            failed_dbs: vec![],
            db_results: vec![],
        };
    }

    // Partition into offline and online databases
    let (offline_dbs, online_dbs): (Vec<_>, Vec<_>) =
        databases.into_iter().partition(|db| db.is_offline());

    // Collect all DB names upfront for tracking skipped on early exit
    let all_db_names: HashSet<String> = offline_dbs
        .iter()
        .chain(online_dbs.iter())
        .map(|db| db.name().to_string())
        .collect();

    let mut first_mismatch: Option<DbSearchResult> = None;
    let mut failed_dbs = Vec::new();
    let mut db_results: Vec<DbResult> = Vec::new();
    let mut completed_db_names: HashSet<String> = HashSet::new();

    // Phase 1: Query offline databases (no rate limiting needed)
    if !offline_dbs.is_empty() {
        let phase_result = run_db_phase(
            &offline_dbs,
            title,
            ref_authors,
            client,
            timeout,
            check_openalex_authors,
            on_db_complete,
            &all_db_names,
            &mut completed_db_names,
            &mut first_mismatch,
            &mut failed_dbs,
            &mut db_results,
            None, // no rate limiter for offline
        )
        .await;

        if let Some(result) = phase_result {
            return result;
        }
    }

    // Phase 2: Query online databases with rate limiting
    if !online_dbs.is_empty() {
        let phase_result = run_db_phase(
            &online_dbs,
            title,
            ref_authors,
            client,
            timeout,
            check_openalex_authors,
            on_db_complete,
            &all_db_names,
            &mut completed_db_names,
            &mut first_mismatch,
            &mut failed_dbs,
            &mut db_results,
            Some(rate_limiter),
        )
        .await;

        if let Some(result) = phase_result {
            return result;
        }
    }

    if let Some(mut mismatch) = first_mismatch {
        mismatch.db_results = db_results;
        return mismatch;
    }

    DbSearchResult {
        status: Status::NotFound,
        source: None,
        found_authors: vec![],
        paper_url: None,
        failed_dbs,
        db_results,
    }
}

/// Run a phase of database queries (offline or online), returning early if a match is found.
///
/// Returns `Some(DbSearchResult)` if a verified match was found (early exit),
/// or `None` to continue to the next phase.
#[allow(clippy::too_many_arguments)]
async fn run_db_phase(
    databases: &[Arc<dyn DatabaseBackend>],
    title: &str,
    ref_authors: &[String],
    client: &reqwest::Client,
    timeout: Duration,
    check_openalex_authors: bool,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
    all_db_names: &HashSet<String>,
    completed_db_names: &mut HashSet<String>,
    first_mismatch: &mut Option<DbSearchResult>,
    failed_dbs: &mut Vec<String>,
    db_results: &mut Vec<DbResult>,
    rate_limiter: Option<&Arc<RateLimiter>>,
) -> Option<DbSearchResult> {
    let mut join_set = tokio::task::JoinSet::new();

    for db in databases {
        let db = Arc::clone(db);
        let title = title.to_string();
        let client = client.clone();
        let ref_authors = ref_authors.to_vec();
        let rate_limiter = rate_limiter.cloned();

        join_set.spawn(async move {
            let name = db.name().to_string();
            let start = Instant::now();
            let result = if let Some(rl) = &rate_limiter {
                query_with_backoff(&db, &title, &client, timeout, rl).await
            } else {
                db.query(&title, &client, timeout).await
            };
            let elapsed = start.elapsed();
            (name, result, ref_authors, elapsed)
        });
    }

    while let Some(result) = join_set.join_next().await {
        let (name, query_result, ref_authors, elapsed) = match result {
            Ok(r) => r,
            Err(_) => continue,
        };

        completed_db_names.insert(name.clone());

        match query_result {
            Ok((Some(_found_title), found_authors, paper_url)) => {
                if ref_authors.is_empty() || validate_authors(&ref_authors, &found_authors) {
                    // Found and verified — record this DB, mark remaining as Skipped
                    let db_result = DbResult {
                        db_name: name.clone(),
                        status: DbStatus::Match,
                        elapsed: Some(elapsed),
                        found_authors: found_authors.clone(),
                        paper_url: paper_url.clone(),
                    };
                    if let Some(cb) = on_db_complete {
                        cb(db_result.clone());
                    }
                    db_results.push(db_result);

                    // Abort remaining tasks
                    join_set.abort_all();

                    // Mark unfinished DBs as Skipped
                    for db_name in all_db_names {
                        if !completed_db_names.contains(db_name) {
                            let skipped = DbResult {
                                db_name: db_name.clone(),
                                status: DbStatus::Skipped,
                                elapsed: None,
                                found_authors: vec![],
                                paper_url: None,
                            };
                            if let Some(cb) = on_db_complete {
                                cb(skipped.clone());
                            }
                            db_results.push(skipped);
                        }
                    }

                    return Some(DbSearchResult {
                        status: Status::Verified,
                        source: Some(name),
                        found_authors,
                        paper_url,
                        failed_dbs: vec![],
                        db_results: db_results.clone(),
                    });
                } else {
                    let db_result = DbResult {
                        db_name: name.clone(),
                        status: DbStatus::AuthorMismatch,
                        elapsed: Some(elapsed),
                        found_authors: found_authors.clone(),
                        paper_url: paper_url.clone(),
                    };
                    if let Some(cb) = on_db_complete {
                        cb(db_result.clone());
                    }
                    db_results.push(db_result);

                    if first_mismatch.is_none() && (name != "OpenAlex" || check_openalex_authors) {
                        *first_mismatch = Some(DbSearchResult {
                            status: Status::AuthorMismatch,
                            source: Some(name),
                            found_authors,
                            paper_url,
                            failed_dbs: vec![],
                            db_results: vec![], // filled in at return
                        });
                    }
                }
            }
            Ok((None, _, _)) => {
                // Not found in this DB
                let db_result = DbResult {
                    db_name: name.clone(),
                    status: DbStatus::NoMatch,
                    elapsed: Some(elapsed),
                    found_authors: vec![],
                    paper_url: None,
                };
                if let Some(cb) = on_db_complete {
                    cb(db_result.clone());
                }
                db_results.push(db_result);
            }
            Err(_e) => {
                let db_result = DbResult {
                    db_name: name.clone(),
                    status: DbStatus::Error,
                    elapsed: Some(elapsed),
                    found_authors: vec![],
                    paper_url: None,
                };
                if let Some(cb) = on_db_complete {
                    cb(db_result.clone());
                }
                db_results.push(db_result);
                failed_dbs.push(name);
            }
        }
    }

    None
}

/// Build the list of database backends based on config.
fn build_database_list(
    config: &Config,
    only_dbs: Option<&[String]>,
) -> Vec<Box<dyn DatabaseBackend>> {
    use crate::db::*;

    let mut databases: Vec<Box<dyn DatabaseBackend>> = Vec::new();

    let should_include = |name: &str| -> bool {
        if config
            .disabled_dbs
            .iter()
            .any(|d| d.eq_ignore_ascii_case(name))
        {
            return false;
        }
        match only_dbs {
            Some(dbs) => dbs.iter().any(|d| d == name),
            None => true,
        }
    };

    if should_include("CrossRef") {
        databases.push(Box::new(crossref::CrossRef {
            mailto: config.crossref_mailto.clone(),
        }));
    }
    if should_include("arXiv") {
        databases.push(Box::new(arxiv::Arxiv));
    }
    if should_include("DBLP") {
        // Use offline DBLP if available, otherwise online
        if let Some(ref db) = config.dblp_offline_db {
            databases.push(Box::new(dblp::DblpOffline {
                db: std::sync::Arc::clone(db),
            }));
        } else {
            databases.push(Box::new(dblp::DblpOnline));
        }
    }
    if should_include("Semantic Scholar") {
        databases.push(Box::new(semantic_scholar::SemanticScholar {
            api_key: config.s2_api_key.clone(),
        }));
    }
    // SSRN disabled: papers.ssrn.com blocks automated requests (403/Cloudflare).
    // Most SSRN papers are indexed by OpenAlex and CrossRef anyway.
    // if should_include("SSRN") {
    //     databases.push(Box::new(ssrn::Ssrn));
    // }
    if should_include("ACL Anthology") {
        // Use offline ACL if available, otherwise online (scraping)
        if let Some(ref db) = config.acl_offline_db {
            databases.push(Box::new(acl::AclOffline {
                db: std::sync::Arc::clone(db),
            }));
        } else {
            databases.push(Box::new(acl::AclAnthology));
        }
    }
    // NeurIPS disabled: papers.nips.cc returns 404 and the HTML structure has changed.
    // DBLP already indexes NeurIPS papers, so this source is redundant for now.
    // if should_include("NeurIPS") {
    //     databases.push(Box::new(neurips::NeurIPS));
    // }
    if should_include("Europe PMC") {
        databases.push(Box::new(europe_pmc::EuropePmc));
    }
    if should_include("PubMed") {
        databases.push(Box::new(pubmed::PubMed));
    }
    if let Some(ref key) = config.openalex_key
        && should_include("OpenAlex")
    {
        databases.insert(
            0,
            Box::new(openalex::OpenAlex {
                api_key: key.clone(),
            }),
        );
    }

    databases
}
