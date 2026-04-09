use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::text_utils::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Open Library backend for matching books and technical reports.
///
/// Queries the Open Library Search API (`openlibrary.org/search.json`)
/// which covers millions of books, technical reports, and other publications
/// that aren't in academic-specific databases.
pub struct OpenLibrary;

impl DatabaseBackend for OpenLibrary {
    fn name(&self) -> &str {
        "Open Library"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            let words = get_query_words(title, 8);
            let query = words.join(" ");
            if query.is_empty() {
                return Ok(DbQueryResult::not_found());
            }

            let url = format!(
                "https://openlibrary.org/search.json?title={}&limit=5&fields=title,author_name,key",
                urlencoding::encode(&query)
            );

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            if resp.status().as_u16() == 429 || resp.status().as_u16() == 503 {
                return Err(DbQueryError::RateLimited {
                    retry_after: Some(Duration::from_secs(2)),
                });
            }
            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let body = resp
                .text()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            parse_openlibrary_response(&body, title)
        })
    }
}

fn parse_openlibrary_response(json: &str, title: &str) -> Result<DbQueryResult, DbQueryError> {
    let parsed: serde_json::Value =
        serde_json::from_str(json).map_err(|e| DbQueryError::Other(e.to_string()))?;

    let docs = parsed
        .get("docs")
        .and_then(|d| d.as_array())
        .unwrap_or(&Vec::new())
        .clone();

    for doc in &docs {
        let found_title = doc
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or_default();

        if found_title.is_empty() || !titles_match(title, found_title) {
            continue;
        }

        let authors: Vec<String> = doc
            .get("author_name")
            .and_then(|a| a.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        // Skip results with no authors
        if authors.is_empty() {
            continue;
        }

        let paper_url = doc
            .get("key")
            .and_then(|k| k.as_str())
            .map(|key| format!("https://openlibrary.org{}", key));

        return Ok(DbQueryResult::found(found_title, authors, paper_url));
    }

    Ok(DbQueryResult::not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_response() {
        let json = r#"{"numFound": 0, "docs": []}"#;
        let result = parse_openlibrary_response(json, "Some Book").unwrap();
        assert!(!result.is_found());
    }

    #[test]
    fn test_parse_matching_book() {
        let json = r#"{"numFound": 1, "docs": [{"title": "Social Engineering: The Science of Human Hacking", "author_name": ["Christopher Hadnagy"], "key": "/works/OL123W"}]}"#;
        let result =
            parse_openlibrary_response(json, "Social Engineering: The Science of Human Hacking")
                .unwrap();
        assert!(result.is_found());
        assert_eq!(result.authors, vec!["Christopher Hadnagy"]);
    }

    #[test]
    fn test_parse_no_match() {
        let json = r#"{"numFound": 1, "docs": [{"title": "Completely Different Book", "author_name": ["Someone"], "key": "/works/OL456W"}]}"#;
        let result =
            parse_openlibrary_response(json, "Social Engineering: The Science of Human Hacking")
                .unwrap();
        assert!(!result.is_found());
    }
}
