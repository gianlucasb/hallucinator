use super::{DatabaseBackend, DbQueryResult};
use crate::matching::titles_match;
use hallucinator_pdf::identifiers::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct DblpOnline;

/// Offline DBLP backend backed by a local SQLite database with FTS5.
pub struct DblpOffline {
    pub db: Arc<Mutex<hallucinator_dblp::DblpDatabase>>,
}

impl DatabaseBackend for DblpOffline {
    fn name(&self) -> &str {
        "DBLP"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, String>> + Send + 'a>> {
        let db = Arc::clone(&self.db);
        let title = title.to_string();
        Box::pin(async move {
            let result = tokio::task::spawn_blocking(move || {
                let db = db.lock().map_err(|e| e.to_string())?;
                db.query(&title).map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;

            match result {
                Some(qr) => Ok((Some(qr.record.title), qr.record.authors, qr.record.url)),
                None => Ok((None, vec![], None)),
            }
        })
    }
}

impl DatabaseBackend for DblpOnline {
    fn name(&self) -> &str {
        "DBLP"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let words = get_query_words(title, 6);
            let query = words.join(" ");
            let url = format!(
                "https://dblp.org/search/publ/api?q={}&format=json",
                urlencoding::encode(&query)
            );

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            let status = resp.status();
            if status.as_u16() == 429 {
                return Err("Rate limited (429)".into());
            }
            if !status.is_success() {
                return Err(format!("HTTP {}", status));
            }

            let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let hits = data["result"]["hits"]["hit"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            for hit in hits {
                let info = &hit["info"];
                let found_title = info["title"].as_str().unwrap_or("");

                if titles_match(title, found_title) {
                    let authors = match &info["authors"]["author"] {
                        serde_json::Value::Array(arr) => arr
                            .iter()
                            .filter_map(|a| {
                                if let Some(text) = a["text"].as_str() {
                                    Some(text.to_string())
                                } else {
                                    a.as_str().map(|s| s.to_string())
                                }
                            })
                            .collect(),
                        serde_json::Value::Object(obj) => {
                            vec![obj
                                .get("text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string()]
                        }
                        _ => vec![],
                    };

                    let paper_url = info["url"].as_str().map(String::from);

                    return Ok((Some(found_title.to_string()), authors, paper_url));
                }
            }

            Ok((None, vec![], None))
        })
    }
}
