use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use crate::retraction::extract_retraction_from_item;
use crate::text_utils::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct CrossRef {
    pub mailto: Option<String>,
}

impl DatabaseBackend for CrossRef {
    fn name(&self) -> &str {
        "CrossRef"
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
            let mut url = format!(
                "https://api.crossref.org/works?query.title={}&rows=5",
                urlencoding::encode(&query)
            );

            let user_agent = if let Some(ref email) = self.mailto {
                url.push_str(&format!("&mailto={}", urlencoding::encode(email)));
                format!("HallucinatedReferenceChecker/1.0 (mailto:{})", email)
            } else {
                "Academic Reference Parser".to_string()
            };

            let resp = client
                .get(&url)
                .header("User-Agent", user_agent)
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
            let items = data["message"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            for item in items {
                let found_title = item["title"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if titles_match(title, found_title) {
                    let authors: Vec<String> = item["author"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .map(|a| {
                                    let given = a["given"].as_str().unwrap_or("");
                                    let family = a["family"].as_str().unwrap_or("");
                                    format!("{} {}", given, family).trim().to_string()
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    // Skip results with empty authors - let other DBs verify (issue #188)
                    // CrossRef sometimes returns title matches without author data, which
                    // causes false AuthorMismatch when we can't verify authors
                    if authors.is_empty() {
                        continue;
                    }

                    let doi = item["DOI"].as_str();
                    let paper_url = doi.map(|d| format!("https://doi.org/{}", d));

                    // Extract retraction info inline from the same CrossRef response
                    let retraction = extract_retraction_from_item(&item);

                    return Ok(DbQueryResult {
                        found_title: Some(found_title.to_string()),
                        authors,
                        paper_url,
                        retraction: Some(retraction),
                    });
                }
            }

            Ok(DbQueryResult::not_found())
        })
    }
}
