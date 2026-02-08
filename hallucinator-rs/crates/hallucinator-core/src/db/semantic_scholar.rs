use super::{DatabaseBackend, DbQueryResult};
use crate::matching::titles_match;
use hallucinator_pdf::identifiers::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct SemanticScholar {
    pub api_key: Option<String>,
}

impl DatabaseBackend for SemanticScholar {
    fn name(&self) -> &str {
        "Semantic Scholar"
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
                "https://api.semanticscholar.org/graph/v1/paper/search?query={}&limit=10&fields=title,authors,url",
                urlencoding::encode(&query)
            );

            let mut req = client
                .get(&url)
                .header("User-Agent", "Academic Reference Parser")
                .timeout(timeout);

            if let Some(ref key) = self.api_key {
                req = req.header("x-api-key", key);
            }

            let resp = req.send().await.map_err(|e| e.to_string())?;

            let status = resp.status();
            if status.as_u16() == 429 {
                return Err("Rate limited (429)".into());
            }
            if !status.is_success() {
                return Err(format!("HTTP {}", status));
            }

            let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
            let results = data["data"].as_array().cloned().unwrap_or_default();

            for item in results {
                let found_title = item["title"].as_str().unwrap_or("");
                if !found_title.is_empty() && titles_match(title, found_title) {
                    let authors: Vec<String> = item["authors"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|a| a["name"].as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();

                    let paper_url = item["url"].as_str().map(String::from);

                    return Ok((Some(found_title.to_string()), authors, paper_url));
                }
            }

            Ok((None, vec![], None))
        })
    }
}
