//! Offline arXiv backend: a local SQLite index built from the Kaggle
//! `Cornell-University/arxiv` weekly snapshot.
//!
//! Parallels `DblpOffline` / `AclOffline`: same `DatabaseBackend`
//! contract, same `is_local = true` so the orchestrator runs it
//! inline in the local phase before any remote DBs. Reports the
//! same `name() = "arXiv"` as the online backend so the offline
//! DB fully replaces the online one when configured (same pattern
//! as `DblpOffline`).
//!
//! The snapshot carries only the latest-version title per paper;
//! retitled-paper edge cases (reference cites an older version's
//! title, the paper was renamed in a later version) are not caught
//! by this backend. Callers who care about those can temporarily
//! remove the offline DB config to fall back to online arXiv,
//! which walks `/abs/{id}v{1..N}`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hallucinator_arxiv_offline::ArxivDatabase;

use super::{ArxivIdQueryResult, DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;

/// Offline arXiv backend backed by a local SQLite database.
pub struct ArxivOffline {
    pub db: Arc<Mutex<ArxivDatabase>>,
}

impl ArxivOffline {
    pub fn new(db: Arc<Mutex<ArxivDatabase>>) -> Self {
        Self { db }
    }
}

impl DatabaseBackend for ArxivOffline {
    fn name(&self) -> &str {
        // Same name as the online backend — offline replaces online
        // entirely when configured, matching DBLP/ACL semantics.
        // Cache keys / UI / on_db_complete events all see "arXiv"
        // regardless of which backend answered.
        "arXiv"
    }

    fn is_local(&self) -> bool {
        true
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        self.query_with_authors(title, &[], client, timeout)
    }

    fn query_with_authors<'a>(
        &'a self,
        title: &'a str,
        _ref_authors: &'a [String],
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        let db = Arc::clone(&self.db);
        let title = title.to_string();
        Box::pin(async move {
            let maybe_record = tokio::task::spawn_blocking(move || {
                let db = db.lock().map_err(|e| DbQueryError::Other(e.to_string()))?;
                // Batch-hydrate in a single mutex hold. The older
                // `search_by_title` + per-ID `lookup_by_id` loop
                // issued up to 1 + 5×3 = 16 sequential SQL queries
                // inside the lock — 4 concurrent workers queued
                // behind each other's arxiv mutex hold and slowed
                // the local phase noticeably. This variant always
                // does exactly 3 round-trips regardless of how
                // many candidates match.
                let candidates = db
                    .search_by_title_hydrated(&title, 5)
                    .map_err(|e| DbQueryError::Other(e.to_string()))?;
                drop(db); // release mutex before the (in-memory) title match
                for rec in candidates {
                    if titles_match(&title, &rec.title) {
                        return Ok::<_, DbQueryError>(Some(rec));
                    }
                }
                Ok(None)
            })
            .await
            .map_err(|e| DbQueryError::Other(e.to_string()))??;

            match maybe_record {
                Some(r) => Ok(DbQueryResult::found(
                    r.title,
                    r.authors,
                    Some(format!("https://arxiv.org/abs/{}", r.id)),
                )),
                None => Ok(DbQueryResult::not_found()),
            }
        })
    }

    fn query_arxiv_id<'a>(
        &'a self,
        arxiv_id: &'a str,
        title: &'a str,
        _authors: &'a [String],
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> ArxivIdQueryResult<'a> {
        let db = Arc::clone(&self.db);
        let arxiv_id = arxiv_id.to_string();
        let title = title.to_string();
        Box::pin(async move {
            let lookup = tokio::task::spawn_blocking(move || {
                let db = db.lock().map_err(|e| DbQueryError::Other(e.to_string()))?;
                db.lookup_by_id(&arxiv_id)
                    .map_err(|e| DbQueryError::Other(e.to_string()))
            })
            .await;

            let rec = match lookup {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => return Some(Err(e)),
                Err(e) => return Some(Err(DbQueryError::Other(e.to_string()))),
            };

            let Some(r) = rec else {
                // No match by ID in the offline index — fall through
                // to the title-search path inside `execute_query`.
                // That almost always also misses, but keeps the
                // control flow simple (single exit via title search).
                return None;
            };

            if titles_match(&title, &r.title) {
                Some(Ok(DbQueryResult::found(
                    r.title.clone(),
                    r.authors.clone(),
                    Some(format!("https://arxiv.org/abs/{}", r.id)),
                )))
            } else {
                // Title mismatch on a known ID. Could be a retitled
                // paper the snapshot caught only at its latest form.
                // Fall through to title search; if that also misses,
                // the ref surfaces as NotFound. Users who want to
                // catch retitled papers temporarily unset the
                // offline DB config and re-run with online arXiv.
                None
            }
        })
    }
}
