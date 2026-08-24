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
                    // Skip book reviews: CrossRef often returns review articles
                    // about a book instead of the book itself. The review has
                    // different authors, causing false AuthorMismatch.
                    let item_type = item["type"].as_str().unwrap_or("");
                    let item_container = item["container-title"]
                        .as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let item_subtype = item["subtype"].as_str().unwrap_or("");

                    // Detect review articles: explicit CrossRef relation, "peer-review"
                    // type, or "review-article" subtype.
                    let is_review = item["relation"]["is-review-of"].is_array()
                        || item_type == "peer-review"
                        || item_subtype == "review-article";

                    // Detect book reviews CrossRef doesn't tag via the relation above:
                    // these still show up as plain "journal-article" items, but the
                    // *returned* title or container announces it's a review (e.g.
                    // "Review of ...", container "Book Reviews"). We only trust signals
                    // on the matched item here, never on the query title's wording — a
                    // prior version of this check flagged any short journal-article
                    // title lacking words like "journal"/"transactions" as "likely a
                    // book review", which wrongly discarded real short-titled papers
                    // (e.g. IEEE Transactions articles) that happened to match.
                    let found_title_lower = found_title.to_lowercase();
                    let container_lower = item_container.to_lowercase();
                    let is_likely_book_review = item_type == "journal-article"
                        && (found_title_lower.starts_with("review of ")
                            || found_title_lower.starts_with("book review")
                            || found_title_lower.contains(": review of ")
                            || container_lower.contains("book review"));

                    if is_review || is_likely_book_review {
                        continue;
                    }

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
                        source_label: None,
                    });
                }
            }

            Ok(DbQueryResult::not_found())
        })
    }
}
