use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Offline OpenAlex backend backed by a local Tantivy index.
pub struct OpenAlexOffline {
    pub db: Arc<Mutex<hallucinator_openalex::OpenAlexDatabase>>,
}

impl DatabaseBackend for OpenAlexOffline {
    fn name(&self) -> &str {
        "OpenAlex"
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
                let db = db.lock().map_err(|e| DbQueryError::Other(e.to_string()))?;
                db.query(&title)
                    .map_err(|e| DbQueryError::Other(e.to_string()))
            })
            .await
            .map_err(|e| DbQueryError::Other(e.to_string()))??;

            match result {
                Some(qr) => Ok(DbQueryResult::found(
                    qr.record.title,
                    qr.record.authors,
                    qr.record.url,
                )),
                None => Ok(DbQueryResult::not_found()),
            }
        })
    }
}
