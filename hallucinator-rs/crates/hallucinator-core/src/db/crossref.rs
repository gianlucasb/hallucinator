use super::{DatabaseBackend, DbQueryResult};
use crate::matching::titles_match;
use hallucinator_pdf::identifiers::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct CrossRef;

impl DatabaseBackend for CrossRef {
    fn name(&self) -> &str {
        "CrossRef"
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
                "https://api.crossref.org/works?query.title={}&rows=5",
                urlencoding::encode(&query)
            );

            let resp = client
                .get(&url)
                .header("User-Agent", "Academic Reference Parser")
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

                    let doi = item["DOI"].as_str();
                    let paper_url = doi.map(|d| format!("https://doi.org/{}", d));

                    return Ok((Some(found_title.to_string()), authors, paper_url));
                }
            }

            Ok((None, vec![], None))
        })
    }
}
