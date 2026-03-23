//! GovInfo API backend for US Federal laws and regulations.
//!
//! Uses the GovInfo API (api.govinfo.gov) to search for legal documents.
//! Requires an API key from api.data.gov.

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// GovInfo database backend.
pub struct GovInfo {
    pub api_key: String,
}

impl GovInfo {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

impl DatabaseBackend for GovInfo {
    fn name(&self) -> &str {
        "GovInfo"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!(
                "https://api.govinfo.gov/search?api_key={}",
                urlencoding::encode(&self.api_key)
            );

            let body = serde_json::json!({
                "query": title,
                "pageSize": 10,
                "offsetMark": "*",
                "sorts": [{"field": "relevancy", "sortOrder": "DESC"}]
            });

            let resp = client
                .post(&url)
                .json(&body)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            check_rate_limit_response(&resp)?;

            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let text = resp
                .text()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            parse_govinfo_response(&text, title)
        })
    }
}

/// Parse GovInfo JSON response and find matching documents.
fn parse_govinfo_response(json: &str, title: &str) -> Result<DbQueryResult, DbQueryError> {
    let data: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| DbQueryError::Other(format!("JSON error: {}", e)))?;

    let results = data["results"].as_array();

    if let Some(results) = results {
        for result in results {
            let doc_title = result["title"].as_str().unwrap_or_default();

            if titles_match(title, doc_title) {
                // GovInfo doesn't have authors in the traditional sense, but may have
                // "granuleClass" or "collectionCode" that could serve as metadata
                let granule_class = result["granuleClass"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let collection_code = result["collectionCode"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                // Build a pseudo-author from collection info
                let authors = if !collection_code.is_empty() {
                    vec![format!("{} ({})", collection_code, granule_class)]
                } else {
                    vec![]
                };

                // Get the package link
                let package_link = result["packageLink"].as_str().map(|s| s.to_string());

                return Ok(DbQueryResult::found(doc_title, authors, package_link));
            }
        }
    }

    Ok(DbQueryResult::not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_results() {
        let json = r#"{"count": 0, "results": []}"#;
        let result = parse_govinfo_response(json, "Some Title").unwrap();
        assert!(!result.is_found());
    }

    #[test]
    fn parse_matching_result() {
        let json = r#"{
            "count": 1,
            "results": [{
                "title": "Test Document Title",
                "collectionCode": "BILLS",
                "granuleClass": "HR",
                "packageLink": "https://api.govinfo.gov/packages/BILLS-117hr1234"
            }]
        }"#;
        let result = parse_govinfo_response(json, "Test Document Title").unwrap();
        assert!(result.is_found());
        assert_eq!(result.found_title.unwrap(), "Test Document Title");
        assert!(result.paper_url.is_some());
    }

    #[test]
    fn parse_no_match() {
        let json = r#"{
            "count": 1,
            "results": [{
                "title": "Different Title",
                "collectionCode": "BILLS",
                "granuleClass": "HR"
            }]
        }"#;
        let result = parse_govinfo_response(json, "Completely Unrelated").unwrap();
        assert!(!result.is_found());
    }
}
