//! PatentsView API backend for US patent lookup.
//!
//! Uses the PatentsView API (search.patentsview.org) to search for patents.
//! Requires an API key (X-Api-Key header).

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// PatentsView database backend.
pub struct PatentsView {
    pub api_key: String,
}

impl PatentsView {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }
}

impl DatabaseBackend for PatentsView {
    fn name(&self) -> &str {
        "PatentsView"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            let url = "https://search.patentsview.org/api/v1/patent/";

            let body = serde_json::json!({
                "q": {"_text_any": {"patent_title": title}},
                "f": ["patent_number", "patent_title", "patent_date",
                      "inventors.inventor_name_first", "inventors.inventor_name_last"],
                "o": {"per_page": 10}
            });

            let resp = client
                .post(url)
                .header("X-Api-Key", &self.api_key)
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

            parse_patentsview_response(&text, title)
        })
    }
}

/// Parse PatentsView JSON response and find matching patents.
fn parse_patentsview_response(json: &str, title: &str) -> Result<DbQueryResult, DbQueryError> {
    let data: serde_json::Value =
        serde_json::from_str(json).map_err(|e| DbQueryError::Other(format!("JSON error: {}", e)))?;

    let patents = data["patents"].as_array();

    if let Some(patents) = patents {
        for patent in patents {
            let patent_title = patent["patent_title"].as_str().unwrap_or_default();

            if titles_match(title, patent_title) {
                // Extract inventors
                let mut authors: Vec<String> = Vec::new();
                if let Some(inventors) = patent["inventors"].as_array() {
                    for inventor in inventors {
                        let first = inventor["inventor_name_first"].as_str().unwrap_or("");
                        let last = inventor["inventor_name_last"].as_str().unwrap_or("");
                        if !last.is_empty() {
                            if !first.is_empty() {
                                authors.push(format!("{} {}", first, last));
                            } else {
                                authors.push(last.to_string());
                            }
                        }
                    }
                }

                // Build patent URL
                let patent_number = patent["patent_number"].as_str().unwrap_or_default();
                let url = if !patent_number.is_empty() {
                    Some(format!(
                        "https://patents.google.com/patent/US{}",
                        patent_number
                    ))
                } else {
                    None
                };

                return Ok(DbQueryResult::found(patent_title, authors, url));
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
        let json = r#"{"patents": [], "count": 0, "total_patent_count": 0}"#;
        let result = parse_patentsview_response(json, "Some Patent").unwrap();
        assert!(!result.is_found());
    }

    #[test]
    fn parse_matching_result() {
        let json = r#"{
            "patents": [{
                "patent_number": "10123456",
                "patent_title": "Method for Testing Software",
                "patent_date": "2020-01-01",
                "inventors": [
                    {"inventor_name_first": "John", "inventor_name_last": "Smith"},
                    {"inventor_name_first": "Jane", "inventor_name_last": "Doe"}
                ]
            }],
            "count": 1,
            "total_patent_count": 1
        }"#;
        let result = parse_patentsview_response(json, "Method for Testing Software").unwrap();
        assert!(result.is_found());
        assert_eq!(result.found_title.unwrap(), "Method for Testing Software");
        assert_eq!(result.authors.len(), 2);
        assert!(result.authors.contains(&"John Smith".to_string()));
        assert!(result.authors.contains(&"Jane Doe".to_string()));
        assert_eq!(
            result.paper_url.unwrap(),
            "https://patents.google.com/patent/US10123456"
        );
    }

    #[test]
    fn parse_no_match() {
        let json = r#"{
            "patents": [{
                "patent_number": "10123456",
                "patent_title": "Different Patent Title",
                "inventors": []
            }],
            "count": 1
        }"#;
        let result = parse_patentsview_response(json, "Completely Unrelated Title").unwrap();
        assert!(!result.is_found());
    }

    #[test]
    fn parse_missing_inventors() {
        let json = r#"{
            "patents": [{
                "patent_number": "10123456",
                "patent_title": "Some Patent Title"
            }],
            "count": 1
        }"#;
        let result = parse_patentsview_response(json, "Some Patent Title").unwrap();
        assert!(result.is_found());
        assert!(result.authors.is_empty());
    }
}
