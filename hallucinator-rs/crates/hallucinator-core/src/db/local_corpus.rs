//! Local corpus backend: recent conference proceedings not yet indexed
//! anywhere else, plus references manually marked safe during review. See
//! `hallucinator-local-corpus` crate docs for the full rationale.

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

/// Offline local-corpus backend, backed by a small SQLite + FTS5 database.
/// Mirrors `AclOffline` — a connection pool rather than a single
/// mutex-guarded connection so concurrent reference checks don't serialize.
pub struct LocalCorpus {
    pub db: Arc<hallucinator_local_corpus::CorpusPool>,
}

impl DatabaseBackend for LocalCorpus {
    fn name(&self) -> &str {
        "Local Corpus"
    }

    fn is_local(&self) -> bool {
        true
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        let db = Arc::clone(&self.db);
        let title = title.to_string();
        Box::pin(async move {
            let result = tokio::task::spawn_blocking(move || {
                db.query(&title)
                    .map_err(|e| DbQueryError::Other(e.to_string()))
            })
            .await
            .map_err(|e| DbQueryError::Other(e.to_string()))??;

            match result {
                // Same rule as ACL's offline backend: skip empty-author
                // matches and let another database verify — a title-only
                // hit here (e.g. a talk/panel with no byline that slipped
                // through import filtering) shouldn't count as "found".
                Some(qr) if !qr.record.authors.is_empty() => Ok(DbQueryResult::found_with_source(
                    qr.record.title,
                    qr.record.authors,
                    qr.record.url,
                    format!("Local Corpus ({})", qr.record.source),
                )),
                _ => Ok(DbQueryResult::not_found()),
            }
        })
    }
}
