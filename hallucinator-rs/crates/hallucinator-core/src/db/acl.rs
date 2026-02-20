use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct AclAnthology;

/// Offline ACL Anthology backend backed by a local SQLite database with FTS5.
pub struct AclOffline {
    pub db: Arc<Mutex<hallucinator_acl::AclDatabase>>,
}

impl DatabaseBackend for AclOffline {
    fn name(&self) -> &str {
        "ACL Anthology"
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
                Some(qr) if !qr.record.authors.is_empty() => Ok(DbQueryResult::found(
                    qr.record.title,
                    qr.record.authors,
                    qr.record.url,
                )),
                // Skip results with empty authors - let other DBs verify
                _ => Ok(DbQueryResult::not_found()),
            }
        })
    }
}

impl DatabaseBackend for AclAnthology {
    fn name(&self) -> &str {
        "ACL Anthology"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!(
                "https://aclanthology.org/search/?q={}",
                urlencoding::encode(title)
            );

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            check_rate_limit_response(&resp)?;
            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let body = resp
                .text()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;
            let title_owned = title.to_string();

            // Parse in spawn_blocking to avoid !Send scraper types
            tokio::task::spawn_blocking(move || parse_acl_results(&body, &title_owned))
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?
        })
    }
}

fn parse_acl_results(html: &str, title: &str) -> Result<DbQueryResult, DbQueryError> {
    let document = scraper::Html::parse_document(html);

    let entry_sel = scraper::Selector::parse(".d-sm-flex.align-items-stretch.p-2").unwrap();
    let title_sel = scraper::Selector::parse("h5").unwrap();
    let author_sel = scraper::Selector::parse("span.badge.badge-light").unwrap();
    let link_sel = scraper::Selector::parse("a[href*='/papers/']").unwrap();

    for entry in document.select(&entry_sel) {
        if let Some(title_el) = entry.select(&title_sel).next() {
            let found_title: String = title_el.text().collect();
            if titles_match(title, &found_title) {
                let authors: Vec<String> = entry
                    .select(&author_sel)
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .collect();

                // Skip results with empty authors - let other DBs verify
                if authors.is_empty() {
                    continue;
                }

                let paper_url = entry
                    .select(&link_sel)
                    .next()
                    .and_then(|a| a.value().attr("href"))
                    .map(|href| format!("https://aclanthology.org{}", href));

                return Ok(DbQueryResult::found(found_title.trim(), authors, paper_url));
            }
        }
    }

    Ok(DbQueryResult::not_found())
}
