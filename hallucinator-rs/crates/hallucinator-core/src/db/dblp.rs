use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use crate::text_utils::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Strip DBLP disambiguation suffixes from author names.
///
/// DBLP appends exactly 4 digits (e.g. " 0001") to distinguish authors with
/// the same name. See <https://dblp.org/faq/1474704.html>.
fn strip_dblp_suffix(name: &str) -> String {
    let name = name.trim();
    if name.len() > 5 {
        let (prefix, suffix) = name.split_at(name.len() - 5);
        if suffix.starts_with(' ')
            && suffix[1..].len() == 4
            && suffix[1..].chars().all(|c| c.is_ascii_digit())
        {
            return prefix.to_string();
        }
    }
    name.to_string()
}

pub struct DblpOnline;

/// Offline DBLP backend backed by a local SQLite database with FTS5.
pub struct DblpOffline {
    pub db: Arc<Mutex<hallucinator_dblp::DblpDatabase>>,
}

impl DatabaseBackend for DblpOffline {
    fn name(&self) -> &str {
        "DBLP"
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
                    qr.record
                        .authors
                        .into_iter()
                        .map(|a| strip_dblp_suffix(&a))
                        .collect(),
                    qr.record.url,
                )),
                // Skip results with empty authors - let other DBs verify
                _ => Ok(DbQueryResult::not_found()),
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
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
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
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            check_rate_limit_response(&resp)?;
            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let data: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;
            let hits = data["result"]["hits"]["hit"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            for hit in hits {
                let info = &hit["info"];
                let found_title = info["title"].as_str().unwrap_or("");

                if titles_match(title, found_title) {
                    let authors: Vec<String> = match &info["authors"]["author"] {
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
                            vec![
                                obj.get("text")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            ]
                        }
                        _ => vec![],
                    };

                    // Skip results with empty authors - let other DBs verify
                    if authors.is_empty() {
                        continue;
                    }

                    let authors: Vec<String> =
                        authors.into_iter().map(|a| strip_dblp_suffix(&a)).collect();
                    let paper_url = info["url"].as_str().map(String::from);

                    return Ok(DbQueryResult::found(found_title, authors, paper_url));
                }
            }

            Ok(DbQueryResult::not_found())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_suffix_removes_4_digit_disambiguation() {
        assert_eq!(strip_dblp_suffix("Nuno Santos 0001"), "Nuno Santos");
        assert_eq!(strip_dblp_suffix("Wei Wang 0042"), "Wei Wang");
        assert_eq!(strip_dblp_suffix("John Smith 0002"), "John Smith");
    }

    #[test]
    fn strip_suffix_preserves_normal_names() {
        assert_eq!(strip_dblp_suffix("Alice Johnson"), "Alice Johnson");
        assert_eq!(strip_dblp_suffix("Bob"), "Bob");
        assert_eq!(strip_dblp_suffix(""), "");
    }

    #[test]
    fn strip_suffix_ignores_non_4_digit_patterns() {
        // 3 digits — not a DBLP suffix
        assert_eq!(strip_dblp_suffix("Name 123"), "Name 123");
        // 5 digits — not a DBLP suffix
        assert_eq!(strip_dblp_suffix("Name 12345"), "Name 12345");
        // No space before digits
        assert_eq!(strip_dblp_suffix("Name0001"), "Name0001");
    }

    #[test]
    fn strip_suffix_handles_whitespace() {
        assert_eq!(strip_dblp_suffix("  Nuno Santos 0001  "), "Nuno Santos");
        assert_eq!(strip_dblp_suffix("  Alice  "), "Alice");
    }
}
