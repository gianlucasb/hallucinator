use crate::authors::validate_authors;
use crate::db::DatabaseBackend;
use crate::rate_limit;
use crate::{Config, DbResult, DbStatus, MismatchKind, Status};
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
    pub retraction: Option<crate::retraction::RetractionResult>,
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
    let cache = config.query_cache.as_deref();

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
            cache,
        )
        .await;
        let elapsed = rl_result.elapsed;
        completed_db_names.insert(name.clone());

        match process_query_result(
            name,
            rl_result.result,
            elapsed,
            title,
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
        retraction: None,
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
    let cache = config.query_cache.clone();

    // Carry forward state from local phase
    let mut first_mismatch: Option<DbSearchResult> =
        if local_result.status == Status::Mismatch(MismatchKind::AUTHOR) {
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
            retraction: None,
        };
    }

    // --- Cache pre-check for all remote DBs ---
    // Check cache synchronously before spawning concurrent tasks to avoid
    // the race where a fast task returns Verified and aborts others before
    // they can cache their results.
    let mut cache_miss_dbs: Vec<&Arc<dyn DatabaseBackend>> = Vec::new();
    for db in &remote_dbs {
        let name = db.name().to_string();
        let cached = cache.as_ref().and_then(|c| c.get(title, &name));

        if let Some(cached_result) = cached {
            completed_db_names.insert(name.clone());
            if let Some(verified) = process_query_result(
                name,
                Ok(cached_result),
                Duration::ZERO,
                title,
                ref_authors,
                check_openalex_authors,
                on_db_complete,
                &mut db_results,
                &mut failed_dbs,
                &mut first_mismatch,
            ) {
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
        } else {
            cache_miss_dbs.push(db);
        }
    }

    // Spawn only cache-miss DBs concurrently
    let mut join_set = tokio::task::JoinSet::new();

    for db in cache_miss_dbs {
        let db = Arc::clone(db);
        let title = title.to_string();
        let client = client.clone();
        let ref_authors = ref_authors.to_vec();
        let rate_limiters = rate_limiters.clone();
        let cache = cache.clone();

        join_set.spawn(async move {
            let name = db.name().to_string();
            let rl_result = rate_limit::query_with_retry(
                db.as_ref(),
                &title,
                &client,
                timeout,
                &rate_limiters,
                max_retries,
                cache.as_deref(),
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
            title,
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
        retraction: None,
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
        retraction: None,
    }
}

/// Threshold (in words) below which a title is considered "short".
/// For short titles, author mismatches are suppressed because a title-only
/// match is unreliable — a different paper with the same short title is
/// more likely than a genuine author mismatch.
const SHORT_TITLE_WORD_THRESHOLD: usize = 6;

/// Process a single DB query result. Returns `Some(verified_result)` on match,
/// `None` to continue checking other DBs.
#[allow(clippy::too_many_arguments)]
fn process_query_result(
    name: String,
    result: Result<crate::db::DbQueryResult, crate::rate_limit::DbQueryError>,
    elapsed: Duration,
    title: &str,
    ref_authors: &[String],
    check_openalex_authors: bool,
    on_db_complete: Option<&(dyn Fn(DbResult) + Send + Sync)>,
    db_results: &mut Vec<DbResult>,
    failed_dbs: &mut Vec<String>,
    first_mismatch: &mut Option<DbSearchResult>,
) -> Option<DbSearchResult> {
    match result {
        Ok(ref qr) if qr.is_found() => {
            let found_authors = qr.authors.clone();
            let paper_url = qr.paper_url.clone();
            let retraction = qr.retraction.clone();

            // Some databases legitimately return a title match with no authors:
            //   - Web Search (SearxNG) never provides author data.
            //   - DBLP sometimes stores authorless records for handbook chapters
            //     and anonymised/organisational entries (e.g. `journals/ccr/X12`).
            // In both cases we accept the title-only verification rather than
            // forcing an AuthorMismatch that would mask a real match.
            let skip_author_check =
                (name == "Web Search" || name == "DBLP") && found_authors.is_empty();
            if ref_authors.is_empty()
                || skip_author_check
                || validate_authors(ref_authors, &found_authors)
            {
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
                    retraction,
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

                // For short/ambiguous titles, suppress author mismatch — a title-only
                // match on a short title is unreliable (likely a different paper with
                // the same common title like "Gemma", "Sentience", "Interactions").
                let is_short_title = title.split_whitespace().count() < SHORT_TITLE_WORD_THRESHOLD;

                // Also suppress mismatch when there is zero surname overlap
                // from fuzzy-matching databases. This prevents false mismatches
                // where CrossRef/Semantic Scholar/Europe PMC return a completely
                // different paper that happens to have a similar title.
                let zero_overlap = if !ref_authors.is_empty() && !found_authors.is_empty() {
                    let ref_surnames: HashSet<String> = ref_authors
                        .iter()
                        .filter_map(|a| {
                            let s = crate::authors::get_last_name_public(a);
                            if s.is_empty() { None } else { Some(s) }
                        })
                        .collect();
                    let found_surnames: HashSet<String> = found_authors
                        .iter()
                        .filter_map(|a| {
                            let s = crate::authors::get_last_name_public(a);
                            if s.is_empty() { None } else { Some(s) }
                        })
                        .collect();
                    ref_surnames.is_disjoint(&found_surnames)
                } else {
                    false
                };

                // Suppress for fuzzy DBs with zero overlap - likely wrong paper
                let is_fuzzy_db = matches!(
                    name.as_str(),
                    "CrossRef" | "Semantic Scholar" | "Europe PMC" | "PubMed"
                );
                let suppress_zero_overlap = zero_overlap && is_fuzzy_db;

                if first_mismatch.is_none()
                    && (name != "OpenAlex" || check_openalex_authors)
                    && !is_short_title
                    && !suppress_zero_overlap
                {
                    *first_mismatch = Some(DbSearchResult {
                        status: Status::Mismatch(MismatchKind::AUTHOR),
                        source: Some(name),
                        found_authors,
                        paper_url,
                        failed_dbs: vec![],
                        db_results: vec![],
                        retraction,
                    });
                }
            }
        }
        Ok(_) => {
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
            tracing::debug!(db = name, error = %err, "query error");
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
    if should_include("DOI") {
        databases.push(Box::new(doi_resolver::DoiResolver));
    }
    if should_include("OpenAlex") {
        if let Some(ref db) = config.openalex_offline_db {
            databases.push(Box::new(openalex_offline::OpenAlexOffline {
                db: std::sync::Arc::clone(db),
            }));
        } else if let Some(ref key) = config.openalex_key {
            databases.insert(
                0,
                Box::new(openalex::OpenAlex {
                    api_key: key.clone(),
                }),
            );
        }
    }
    // GovInfo (requires API key, silently skip if not configured)
    if should_include("GovInfo")
        && let Some(ref key) = config.govinfo_key
    {
        databases.push(Box::new(govinfo::GovInfo::new(key.clone())));
    }
    // PatentsView - DISABLED: API key grants are currently suspended
    // See: https://patentsview.org/apis/keyrequest
    // Uncomment when API keys become available again:
    // if should_include("PatentsView")
    //     && let Some(ref key) = config.patentsview_key
    // {
    //     databases.push(Box::new(patentsview::PatentsView::new(key.clone())));
    // }

    // Standards documents (RFCs, 3GPP, IEEE, ITU-T, ISO, ETSI, etc.)
    // No API key required; pattern-based pre-filter means zero cost for non-standards refs.
    if should_include("Standards") {
        databases.push(Box::new(standards::StandardsVerifier));
    }

    // Open Library - books and technical reports not in academic databases
    if should_include("Open Library") {
        databases.push(Box::new(openlibrary::OpenLibrary));
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
                "DOI".into(),
                "GovInfo".into(),
                "Standards".into(),
                "Open Library".into(),
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
            "DOI",
            "Standards",
            "Open Library",
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

        let title = "A Comprehensive Survey of Test Paper Methods and Approaches";
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
                None,
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
                title,
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
            retraction: None,
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
    async fn dblp_empty_authors_is_title_only_match() {
        // Regression test for BUG #3: DBLP returns authorless records for
        // some handbook chapters and anonymised entries. The orchestrator
        // should treat those as title-only verifications rather than
        // AuthorMismatch, which previously turned real matches into
        // potential hallucinations.
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "DBLP",
            MockResponse::Found {
                title: "How to Share a Secret".into(),
                authors: vec![],
                url: Some("https://dblp.org/rec/books/sp/voecking2011/Blomer11".into()),
            },
        ));
        let result =
            query_single_mock_db(mock, &["Adi Shamir".into()]).await;
        assert_eq!(result.status, Status::Verified);
        assert_eq!(result.source.as_deref(), Some("DBLP"));
        assert!(result.found_authors.is_empty());
    }

    #[tokio::test]
    async fn non_dblp_empty_authors_still_mismatches() {
        // Counterpart: empty-authors from a non-DBLP DB other than Web Search
        // must NOT get the free pass — that would be a false positive. The
        // existing filter in the individual DB backends ensures they never
        // send empty-author payloads, but this test guards the orchestrator's
        // skip_author_check condition against accidental over-broadening.
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "CrossRef",
            MockResponse::Found {
                title: "Test Paper Title".into(),
                authors: vec![],
                url: None,
            },
        ));
        // When found_authors is empty and ref_authors is non-empty,
        // validate_authors returns false → AuthorMismatch path.
        let result = query_single_mock_db(mock, &["Some Author".into()]).await;
        assert_ne!(result.status, Status::Verified);
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
        assert_eq!(result.status, Status::Mismatch(MismatchKind::AUTHOR));
    }

    #[tokio::test]
    async fn short_title_mismatch_suppressed() {
        // For short titles (<6 words), author mismatches should be suppressed
        // because the title match is unreliable (e.g., "Gemma" matching a
        // different paper with the same name).
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "TestDB",
            MockResponse::Found {
                title: "Short Title".into(),
                authors: vec!["Jones".into()],
                url: None,
            },
        ));
        // Use a short title — mismatch should be suppressed, returning NotFound
        let config = config_all_disabled();
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(config.db_timeout_secs);
        let rate_limiters = config.rate_limiters.clone();

        let short_title = "Short Title";
        let ref_authors_owned: Vec<String> = vec!["CompletelyDifferent".into()];
        let db = mock;
        let rate_limiters_clone = rate_limiters.clone();
        let ref_authors_clone = ref_authors_owned.clone();

        let mut join_set = tokio::task::JoinSet::new();
        join_set.spawn(async move {
            let name = db.name().to_string();
            let rl_result = crate::rate_limit::query_with_retry(
                db.as_ref(),
                short_title,
                &client,
                timeout,
                &rate_limiters_clone,
                config.max_rate_limit_retries,
                None,
            )
            .await;
            (name, rl_result.result, ref_authors_clone, rl_result.elapsed)
        });

        let mut failed_dbs = Vec::new();
        let mut db_results: Vec<DbResult> = Vec::new();
        let mut first_mismatch: Option<DbSearchResult> = None;

        while let Some(result) = join_set.join_next().await {
            let (name, query_result, ref_authors, elapsed) = result.unwrap();
            if let Some(verified) = process_query_result(
                name,
                query_result,
                elapsed,
                short_title,
                &ref_authors,
                false,
                None,
                &mut db_results,
                &mut failed_dbs,
                &mut first_mismatch,
            ) {
                panic!("Should not verify: {:?}", verified.status);
            }
        }

        // first_mismatch should be None because short title suppresses it
        assert!(
            first_mismatch.is_none(),
            "Short title should suppress author mismatch, got: {:?}",
            first_mismatch.as_ref().map(|m| &m.status)
        );
    }

    #[tokio::test]
    async fn fuzzy_db_zero_overlap_mismatch_suppressed() {
        // When a fuzzy DB (e.g., Europe PMC) returns a completely different paper
        // (zero author surname overlap), the mismatch should be suppressed.
        let mock: Arc<dyn DatabaseBackend> = Arc::new(MockDb::new(
            "Europe PMC",
            MockResponse::Found {
                title: "Secure multiparty quantum computation".into(),
                authors: vec!["Song X".into(), "Gou R".into(), "Wen A.".into()],
                url: None,
            },
        ));
        let config = config_all_disabled();
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(config.db_timeout_secs);
        let rate_limiters = config.rate_limiters.clone();

        let title = "Secure multiparty quantum computation";
        let ref_authors_owned: Vec<String> = vec![
            "C. Crepeau".into(),
            "D. Gottesman".into(),
            "A. Smith".into(),
        ];
        let db = mock;
        let rate_limiters_clone = rate_limiters.clone();
        let ref_authors_clone = ref_authors_owned.clone();

        let mut join_set = tokio::task::JoinSet::new();
        join_set.spawn(async move {
            let name = db.name().to_string();
            let rl_result = crate::rate_limit::query_with_retry(
                db.as_ref(),
                title,
                &client,
                timeout,
                &rate_limiters_clone,
                config.max_rate_limit_retries,
                None,
            )
            .await;
            (name, rl_result.result, ref_authors_clone, rl_result.elapsed)
        });

        let mut db_results = vec![];
        let mut failed_dbs = vec![];
        let mut first_mismatch: Option<DbSearchResult> = None;

        while let Some(result) = join_set.join_next().await {
            let (name, query_result, ref_authors, elapsed) = result.unwrap();
            if let Some(verified) = process_query_result(
                name,
                query_result,
                elapsed,
                title,
                &ref_authors,
                false,
                None,
                &mut db_results,
                &mut failed_dbs,
                &mut first_mismatch,
            ) {
                panic!("Should not verify: {:?}", verified.status);
            }
        }

        assert!(
            first_mismatch.is_none(),
            "Zero overlap from fuzzy DB should suppress mismatch, got: {:?}",
            first_mismatch.as_ref().map(|m| (&m.status, &m.source))
        );
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
