use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use crate::text_utils::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct PubMed;

impl DatabaseBackend for PubMed {
    fn name(&self) -> &str {
        "PubMed"
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

            // Step 1: Search for matching articles
            let search_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi";
            let term = format!("{}[Title]", query);

            let resp = client
                .get(search_url)
                .query(&[
                    ("db", "pubmed"),
                    ("term", &term),
                    ("retmode", "json"),
                    ("retmax", "10"),
                ])
                .header("User-Agent", "Academic Reference Parser")
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
            let id_list: Vec<String> = data["esearchresult"]["idlist"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            if id_list.is_empty() {
                return Ok((None, vec![], None));
            }

            // Step 2: Fetch details
            let fetch_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi";
            let ids = id_list.join(",");

            let resp = client
                .get(fetch_url)
                .query(&[("db", "pubmed"), ("id", ids.as_str()), ("retmode", "json")])
                .header("User-Agent", "Academic Reference Parser")
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!(
                    "HTTP {} on fetch",
                    resp.status()
                )));
            }

            let data: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;
            let results = &data["result"];

            for pmid in &id_list {
                let item = &results[pmid];
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

                    let paper_url = format!("https://pubmed.ncbi.nlm.nih.gov/{}/", pmid);

                    return Ok((Some(found_title.to_string()), authors, Some(paper_url)));
                }
            }

            Ok((None, vec![], None))
        })
    }
}
