//! Offline arXiv backend: a local SQLite index harvested from arXiv's
//! OAI-PMH feed.
//!
//! Parallels `DblpOffline` / `AclOffline`: same `DatabaseBackend`
//! contract, same `is_local = true` so the orchestrator runs it
//! inline in the local phase before any remote DBs.
//!
//! The offline DB carries only the latest-version title per paper
//! (that's all OAI-PMH's `arXivRaw` format surfaces), so the
//! earlier-versions title fallback still needs the live arXiv API
//! on miss. The `query_arxiv_id` path here returns `NotFound` when
//! the title doesn't match — the orchestrator then falls through
//! to the online `Arxiv` backend, which walks the version history.

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
        // Distinct from the online backend's "arXiv" on purpose: the
        // offline index carries only latest-version titles, so a
        // cached "not found" from offline does NOT imply the online
        // version-history fallback would also miss. Keeping the
        // cache keys separate lets the online backend still run
        // (and walk earlier versions) when offline misses on title.
        "arXiv (offline)"
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
                let ids = db
                    .search_by_title(&title, 5)
                    .map_err(|e| DbQueryError::Other(e.to_string()))?;
                // Iterate the top candidates and accept the first whose
                // stored title fuzz-matches the citation title. This is
                // how the online backend works too, just against a much
                // smaller top-5 instead of the ~5 results returned from
                // the API.
                for id in ids {
                    let rec = db
                        .lookup_by_id(&id)
                        .map_err(|e| DbQueryError::Other(e.to_string()))?;
                    if let Some(r) = rec
                        && titles_match(&title, &r.title)
                    {
                        return Ok::<_, DbQueryError>(Some(r));
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
                // No match by ID in the offline index. Return None so
                // the orchestrator falls through to the online arXiv
                // backend — which might have seen the paper more
                // recently than our last harvest, or walks the
                // earlier-versions fallback (the offline index
                // doesn't carry per-version titles).
                return None;
            };

            if titles_match(&title, &r.title) {
                Some(Ok(DbQueryResult::found(
                    r.title.clone(),
                    r.authors.clone(),
                    Some(format!("https://arxiv.org/abs/{}", r.id)),
                )))
            } else {
                // Title mismatch on a known ID. Same reason as above:
                // fall through to the online backend so its
                // earlier-version walk can try v1..v{N-1}.
                None
            }
        })
    }
}
