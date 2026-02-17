use crate::authors::validate_authors;
use crate::db::DatabaseBackend;
use crate::rate_limit;
use crate::{Config, DbResult, DbStatus, Status};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

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

/// Query all databases for a single reference (local first, then remote).
///
/// This is a convenience wrapper that calls [`query_local_databases`] followed by
/// [`query_remote_databases`]. For the pool's split architecture, use those
/// functions directly.
pub async fn query_all_databases(
    title: &str,
    ref_authors: &[String],
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
    only_dbs: Option<&[String]>,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
) -> DbSearchResult {
    let local_result = query_local_databases(
        title,
        ref_authors,
        config,
        client,
        longer_timeout,
        only_dbs,
        on_db_complete,
    )
    .await;

    if local_result.status == Status::Verified {
        return local_result;
    }

    query_remote_databases(
        title,
        ref_authors,
        config,
        client,
        longer_timeout,
        only_dbs,
        on_db_complete,
        local_result,
    )
    .await
}

/// Query only local/offline databases (DBLP offline, ACL offline).
///
/// Returns immediately (<1ms). If a local DB matches, the result has
/// `status == Verified` and remaining DBs are marked Skipped.
pub async fn query_local_databases(
    title: &str,
    ref_authors: &[String],
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
    only_dbs: Option<&[String]>,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
) -> DbSearchResult {
    let timeout = compute_timeout(config, longer_timeout);

    let all_databases: Vec<Arc<dyn DatabaseBackend>> = build_database_list(config, only_dbs)
        .into_iter()
        .map(Arc::from)
        .collect();

    if all_databases.is_empty() {
        return empty_result();
    }

    let (local_dbs, remote_dbs): (Vec<_>, Vec<_>) =
        all_databases.into_iter().partition(|db| db.is_local());

    // All DB names for Skipped tracking on early exit
    let all_db_names: HashSet<String> = local_dbs
        .iter()
        .chain(remote_dbs.iter())
        .map(|db| db.name().to_string())
        .collect();

    let rate_limiters = config.rate_limiters.clone();
    let max_retries = config.max_rate_limit_retries;

    let mut first_mismatch: Option<DbSearchResult> = None;
    let mut failed_dbs = Vec::new();
    let mut db_results: Vec<DbResult> = Vec::new();
    let mut completed_db_names: HashSet<String> = HashSet::new();

    for db in &local_dbs {
        let name = db.name().to_string();
        let rl_result = rate_limit::query_with_retry(
            db.as_ref(),
            title,
            client,
            timeout,
            &rate_limiters,
            max_retries,
        )
        .await;
        let elapsed = rl_result.elapsed;
        completed_db_names.insert(name.clone());

        match process_query_result(
            name,
            rl_result.result,
            elapsed,
            ref_authors,
            config.check_openalex_authors,
            on_db_complete,
            &mut db_results,
            &mut failed_dbs,
            &mut first_mismatch,
        ) {
            Some(verified) => {
                // Mark all remaining DBs as Skipped
                emit_skipped(
                    &all_db_names,
                    &completed_db_names,
                    on_db_complete,
                    &mut db_results,
                );
                return DbSearchResult {
                    db_results,
                    ..verified
                };
            }
            None => continue,
        }
    }

    // No local match — return partial result for remote phase to continue from
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

/// Query only remote/online databases concurrently, continuing from local results.
///
/// The `local_result` carries any db_results, failed_dbs, and first_mismatch from
/// the local phase. Remote results are merged in.
#[allow(clippy::too_many_arguments)]
pub async fn query_remote_databases(
    title: &str,
    ref_authors: &[String],
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
    only_dbs: Option<&[String]>,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
    local_result: DbSearchResult,
) -> DbSearchResult {
    let check_openalex_authors = config.check_openalex_authors;
    let timeout = compute_timeout(config, longer_timeout);

    let all_databases: Vec<Arc<dyn DatabaseBackend>> = build_database_list(config, only_dbs)
        .into_iter()
        .map(Arc::from)
        .collect();

    let (local_dbs, remote_dbs): (Vec<_>, Vec<_>) =
        all_databases.into_iter().partition(|db| db.is_local());

    // All DB names for Skipped tracking
    let all_db_names: HashSet<String> = local_dbs
        .iter()
        .chain(remote_dbs.iter())
        .map(|db| db.name().to_string())
        .collect();

    let rate_limiters = config.rate_limiters.clone();
    let max_retries = config.max_rate_limit_retries;

    // Carry forward state from local phase
    let mut first_mismatch: Option<DbSearchResult> =
        if local_result.status == Status::AuthorMismatch {
            Some(DbSearchResult {
                db_results: vec![], // filled in at return
                ..local_result.clone()
            })
        } else {
            None
        };
    let mut failed_dbs = local_result.failed_dbs;
    let mut db_results = local_result.db_results;
    let mut completed_db_names: HashSet<String> =
        db_results.iter().map(|r| r.db_name.clone()).collect();

    if remote_dbs.is_empty() {
        if let Some(mut mismatch) = first_mismatch {
            mismatch.db_results = db_results;
            return mismatch;
        }
        return DbSearchResult {
            status: Status::NotFound,
            source: None,
            found_authors: vec![],
            paper_url: None,
            failed_dbs,
            db_results,
        };
    }

    // Spawn remote DB queries concurrently
    let mut join_set = tokio::task::JoinSet::new();

    for db in &remote_dbs {
        let db = Arc::clone(db);
        let title = title.to_string();
        let client = client.clone();
        let ref_authors = ref_authors.to_vec();
        let rate_limiters = rate_limiters.clone();

        join_set.spawn(async move {
            let name = db.name().to_string();
            let rl_result = rate_limit::query_with_retry(
                db.as_ref(),
                &title,
                &client,
                timeout,
                &rate_limiters,
                max_retries,
            )
            .await;
            (name, rl_result.result, ref_authors, rl_result.elapsed)
        });
    }

    while let Some(result) = join_set.join_next().await {
        let (name, query_result, ref_authors, elapsed) = match result {
            Ok(r) => r,
            Err(_) => continue,
        };

        completed_db_names.insert(name.clone());

        match process_query_result(
            name,
            query_result,
            elapsed,
            &ref_authors,
            check_openalex_authors,
            on_db_complete,
            &mut db_results,
            &mut failed_dbs,
            &mut first_mismatch,
        ) {
            Some(verified) => {
                join_set.abort_all();
                emit_skipped(
                    &all_db_names,
                    &completed_db_names,
                    on_db_complete,
                    &mut db_results,
                );
                return DbSearchResult {
                    db_results,
                    ..verified
                };
            }
            None => continue,
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

// ── Helpers ───────────────────────────────────────────────────────────────

fn compute_timeout(config: &Config, longer: bool) -> Duration {
    if longer {
        Duration::from_secs(config.db_timeout_secs * 2)
    } else {
        Duration::from_secs(config.db_timeout_secs)
    }
}

fn empty_result() -> DbSearchResult {
    DbSearchResult {
        status: Status::NotFound,
        source: None,
        found_authors: vec![],
        paper_url: None,
        failed_dbs: vec![],
        db_results: vec![],
    }
}

/// Process a single DB query result. Returns `Some(verified_result)` on match,
/// `None` to continue checking other DBs.
#[allow(clippy::too_many_arguments)]
fn process_query_result(
    name: String,
    result: Result<crate::db::DbQueryResult, crate::rate_limit::DbQueryError>,
    elapsed: Duration,
    ref_authors: &[String],
    check_openalex_authors: bool,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
    db_results: &mut Vec<DbResult>,
    failed_dbs: &mut Vec<String>,
    first_mismatch: &mut Option<DbSearchResult>,
) -> Option<DbSearchResult> {
    match result {
        Ok((Some(_found_title), found_authors, paper_url)) => {
            if ref_authors.is_empty() || validate_authors(ref_authors, &found_authors) {
                let db_result = DbResult {
                    db_name: name.clone(),
                    status: DbStatus::Match,
                    elapsed: Some(elapsed),
                    found_authors: found_authors.clone(),
                    paper_url: paper_url.clone(),
                    error_message: None,
                };
                if let Some(cb) = on_db_complete {
                    cb(db_result.clone());
                }
                db_results.push(db_result);

                return Some(DbSearchResult {
                    status: Status::Verified,
                    source: Some(name),
                    found_authors,
                    paper_url,
                    failed_dbs: vec![],
                    db_results: vec![], // caller fills this in
                });
            } else {
                let db_result = DbResult {
                    db_name: name.clone(),
                    status: DbStatus::AuthorMismatch,
                    elapsed: Some(elapsed),
                    found_authors: found_authors.clone(),
                    paper_url: paper_url.clone(),
                    error_message: None,
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
                        db_results: vec![],
                    });
                }
            }
        }
        Ok((None, _, _)) => {
            let db_result = DbResult {
                db_name: name,
                status: DbStatus::NoMatch,
                elapsed: Some(elapsed),
                found_authors: vec![],
                paper_url: None,
                error_message: None,
            };
            if let Some(cb) = on_db_complete {
                cb(db_result.clone());
            }
            db_results.push(db_result);
        }
        Err(err) => {
            let db_result = DbResult {
                db_name: name.clone(),
                status: DbStatus::Error,
                elapsed: Some(elapsed),
                found_authors: vec![],
                paper_url: None,
                error_message: Some(err.to_string()),
            };
            if let Some(cb) = on_db_complete {
                cb(db_result.clone());
            }
            db_results.push(db_result);
            log::debug!("{}: {}", name, err);
            failed_dbs.push(name);
        }
    }
    None
}

/// Emit Skipped events for DBs that weren't queried due to early exit.
fn emit_skipped(
    all_db_names: &HashSet<String>,
    completed_db_names: &HashSet<String>,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
    db_results: &mut Vec<DbResult>,
) {
    for db_name in all_db_names {
        if !completed_db_names.contains(db_name) {
            let skipped = DbResult {
                db_name: db_name.clone(),
                status: DbStatus::Skipped,
                elapsed: None,
                found_authors: vec![],
                paper_url: None,
                error_message: None,
            };
            if let Some(cb) = on_db_complete {
                cb(skipped.clone());
            }
            db_results.push(skipped);
        }
    }
}

/// Build the list of database backends based on config.
pub(crate) fn build_database_list(
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
    if should_include("ACL Anthology") {
        if let Some(ref db) = config.acl_offline_db {
            databases.push(Box::new(acl::AclOffline {
                db: std::sync::Arc::clone(db),
            }));
        } else {
            databases.push(Box::new(acl::AclAnthology));
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::mock::{MockDb, MockResponse};

    fn config_all_disabled() -> Config {
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
            ..Config::default()
        }
    }

    #[test]
    fn default_includes_active_dbs() {
        let config = Config::default();
        let dbs = build_database_list(&config, None);
        let names: Vec<&str> = dbs.iter().map(|db| db.name()).collect();
        for expected in [
            "CrossRef",
            "arXiv",
            "DBLP",
            "Semantic Scholar",
            "ACL Anthology",
            "Europe PMC",
            "PubMed",
        ] {
            assert!(names.contains(&expected), "missing {expected}");
        }
    }

    #[test]
    fn disabled_dbs_excluded() {
        let config = Config {
            disabled_dbs: vec!["CrossRef".into()],
            ..Config::default()
        };
        let dbs = build_database_list(&config, None);
        let names: Vec<&str> = dbs.iter().map(|db| db.name()).collect();
        assert!(!names.contains(&"CrossRef"));
    }

    #[test]
    fn only_dbs_filters() {
        let config = Config::default();
        let only = vec!["arXiv".into()];
        let dbs = build_database_list(&config, Some(&only));
        assert_eq!(dbs.len(), 1);
        assert_eq!(dbs[0].name(), "arXiv");
    }

    #[test]
    fn openalex_requires_key() {
        let config = Config::default();
        let dbs = build_database_list(&config, None);
        let names: Vec<&str> = dbs.iter().map(|db| db.name()).collect();
        assert!(!names.contains(&"OpenAlex"));

        let config_with_key = Config {
            openalex_key: Some("test-key".into()),
            ..Config::default()
        };
        let dbs = build_database_list(&config_with_key, None);
        assert_eq!(dbs[0].name(), "OpenAlex");
    }

    #[tokio::test]
    async fn empty_db_list_returns_not_found() {
        let config = config_all_disabled();
        let client = reqwest::Client::new();
        let result =
            query_all_databases("Some Title", &[], &config, &client, false, None, None).await;
        assert_eq!(result.status, Status::NotFound);
        assert!(result.db_results.is_empty());
    }

    async fn query_single_mock_db(
        mock: Arc<dyn DatabaseBackend>,
        ref_authors: &[String],
    ) -> DbSearchResult {
        let config = config_all_disabled();
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(config.db_timeout_secs);
        let rate_limiters = config.rate_limiters.clone();
        let max_retries = config.max_rate_limit_retries;

        let title = "Test Paper Title";
        let mut join_set = tokio::task::JoinSet::new();
        let db = mock;
        let ref_authors_owned = ref_authors.to_vec();
        let rate_limiters_clone = rate_limiters.clone();

        join_set.spawn(async move {
            let name = db.name().to_string();
            let rl_result = crate::rate_limit::query_with_retry(
                db.as_ref(),
                title,
                &client,
                timeout,
                &rate_limiters_clone,
                max_retries,
            )
            .await;
            (name, rl_result.result, ref_authors_owned, rl_result.elapsed)
        });

        let mut failed_dbs = Vec::new();
        let mut db_results: Vec<DbResult> = Vec::new();
        let mut first_mismatch: Option<DbSearchResult> = None;

        while let Some(result) = join_set.join_next().await {
            let (name, query_result, ref_authors, elapsed) = result.unwrap();
            match process_query_result(
                name,
                query_result,
                elapsed,
                &ref_authors,
                false,
                None,
                &mut db_results,
                &mut failed_dbs,
                &mut first_mismatch,
            ) {
                Some(verified) => {
                    return DbSearchResult {
                        db_results,
                        ..verified
                    };
                }
                None => continue,
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

    #[tokio::test]
    async fn single_match_returns_verified() {
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "TestDB",
            MockResponse::Found {
                title: "Test Paper Title".into(),
                authors: vec!["Smith".into()],
                url: Some("https://example.com".into()),
            },
        ));
        let result = query_single_mock_db(mock, &["Smith".into()]).await;
        assert_eq!(result.status, Status::Verified);
        assert_eq!(result.source.as_deref(), Some("TestDB"));
    }

    #[tokio::test]
    async fn author_mismatch_tracked() {
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "TestDB",
            MockResponse::Found {
                title: "Test Paper Title".into(),
                authors: vec!["Jones".into()],
                url: None,
            },
        ));
        let result = query_single_mock_db(mock, &["CompletelyDifferentAuthor".into()]).await;
        assert_eq!(result.status, Status::AuthorMismatch);
    }

    #[tokio::test]
    async fn error_tracked_in_failed_dbs() {
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "FailDB",
            MockResponse::Error("connection refused".into()),
        ));
        let result = query_single_mock_db(mock, &[]).await;
        assert_eq!(result.status, Status::NotFound);
        assert!(result.failed_dbs.contains(&"FailDB".to_string()));
    }
}
