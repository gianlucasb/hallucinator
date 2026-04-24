//! Offline IACR Cryptology ePrint Archive backend.
//!
//! Queries a local SQLite + FTS5 index harvested via OAI-PMH (see
//! `hallucinator-iacr-eprint`). There's no online counterpart —
//! `eprint.iacr.org` exposes OAI-PMH for bulk harvesting but no
//! search API, so this backend only fires when the user has run
//! `hallucinator-cli update-iacr-eprint <path>` and passed
//! `--iacr-eprint-offline <path>` to `check`.
//!
//! Local backend: `is_local() = true` so the orchestrator runs it
//! in the inline phase before fanning out to remote DBs. Title
//! search is the primary code path (FTS5 BM25 → fuzzy title match
//! → author validation); ID-based lookup (`YYYY/N`) is served from
//! the same index when the reference extractor has captured one.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hallucinator_iacr_eprint::IacrDatabase;

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;

/// Offline IACR ePrint backend. Wraps the shared `IacrDatabase`
/// handle under an `Arc<Mutex<_>>` so the orchestrator can clone
/// it cheaply per ref without sharing the underlying SQLite
/// connection across threads directly (rusqlite connections aren't
/// `Sync`).
pub struct IacrEprintOffline {
    pub db: Arc<Mutex<IacrDatabase>>,
}

impl IacrEprintOffline {
    pub fn new(db: Arc<Mutex<IacrDatabase>>) -> Self {
        Self { db }
    }
}

impl DatabaseBackend for IacrEprintOffline {
    fn name(&self) -> &str {
        // Distinct name from any online backend — there isn't one,
        // but having a stable identifier makes the cache / UI / on-
        // db-complete events unambiguous.
        "IACR ePrint"
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
            // SQLite work off the async runtime — `spawn_blocking`
            // keeps the executor free for other backends.
            let maybe_record = tokio::task::spawn_blocking(move || {
                let db = db.lock().map_err(|e| DbQueryError::Other(e.to_string()))?;
                let candidates = db
                    .search_by_title(&title, 5)
                    .map_err(|e| DbQueryError::Other(e.to_string()))?;
                drop(db);
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
                    Some(format!("https://eprint.iacr.org/{}", r.id)),
                )),
                None => Ok(DbQueryResult::not_found()),
            }
        })
    }
}
