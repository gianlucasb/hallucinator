//! SearxNG web search fallback for unverified citations.
//!
//! This backend queries a self-hosted SearxNG instance to search for papers
//! that couldn't be found in any academic database. It tries exact phrase
//! matching first, then falls back to keyword search.
//!
//! Note: This is a weaker form of verification than academic databases since
//! it cannot verify authors - it only confirms the paper exists on the web.

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::normalize_title;
use crate::text_utils::get_query_words;
use once_cell::sync::Lazy;
use regex::Regex;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// SearxNG web search backend.
pub struct Searxng {
    /// Base URL of the SearxNG instance (e.g., "http://localhost:8080")
    pub base_url: String,
}

impl Searxng {
    /// Create a new SearxNG backend with the given base URL.
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }

    /// Check if SearxNG is reachable. Returns Ok(()) if reachable, Err with message otherwise.
    pub async fn check_connectivity(&self) -> Result<(), String> {
        let client = reqwest::Client::new();
        let url = format!("{}/", self.base_url.trim_end_matches('/'));

        match client
            .get(&url)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => Ok(()),
            Ok(resp) => Err(format!("SearxNG returned HTTP {}", resp.status())),
            Err(e) if e.is_connect() => Err(format!(
                "Cannot connect to SearxNG at {} - is the container running?",
                self.base_url
            )),
            Err(e) if e.is_timeout() => Err(format!(
                "SearxNG at {} timed out - is the container running?",
                self.base_url
            )),
            Err(e) => Err(format!("SearxNG error: {}", e)),
        }
    }
}

/// Lenient title matching for web search results.
/// More permissive than `titles_match` since we just need to confirm the paper exists.
/// Uses substring matching and a lower similarity threshold.
fn titles_match_lenient(reference_title: &str, search_title: &str) -> bool {
    let norm_ref = normalize_title(reference_title);
    let norm_search = normalize_title(search_title);

    if norm_ref.is_empty() || norm_search.is_empty() {
        return false;
    }

    // Exact match
    if norm_ref == norm_search {
        return true;
    }

    // Fuzzy match with lower threshold (85% instead of 95%)
    let score = rapidfuzz::fuzz::ratio(norm_ref.chars(), norm_search.chars());
    if score >= 0.85 {
        return true;
    }

    // Substring match: if the search result contains the reference title
    // (handles "Date Paper Title - Venue" patterns)
    if norm_ref.len() >= 15 && norm_search.contains(&norm_ref) {
        return true;
    }

    // Also check if reference contains the search result
    if norm_search.len() >= 15 && norm_ref.contains(&norm_search) {
        return true;
    }

    false
}

/// Response from SearxNG JSON API.
#[derive(Debug, serde::Deserialize)]
struct SearxngResponse {
    results: Vec<SearxngResult>,
}

#[derive(Debug, serde::Deserialize)]
struct SearxngResult {
    title: String,
    url: String,
}

impl Searxng {
    /// Execute a single search query and check results for matching titles.
    async fn search_and_match(
        &self,
        query: &str,
        title: &str,
        client: &reqwest::Client,
        timeout: Duration,
    ) -> Result<Option<DbQueryResult>, ()> {
        let url = format!(
            "{}/search?q={}&format=json",
            self.base_url.trim_end_matches('/'),
            urlencoding::encode(query)
        );

        let resp = match client.get(&url).timeout(timeout).send().await {
            Ok(r) => r,
            Err(_) => return Err(()), // Connection error
        };

        if !resp.status().is_success() {
            return Err(());
        }

        let data: SearxngResponse = match resp.json().await {
            Ok(d) => d,
            Err(_) => return Err(()),
        };

        // Check results for matching titles
        for result in data.results {
            if titles_match_lenient(title, &result.title) {
                return Ok(Some(DbQueryResult::found(
                    result.title,
                    vec![],
                    Some(result.url),
                )));
            }
        }

        Ok(None) // No match found in results
    }
}

impl DatabaseBackend for Searxng {
    fn name(&self) -> &str {
        "Web Search"
    }

    fn is_local(&self) -> bool {
        // SearxNG is self-hosted, no external rate limiting needed
        true
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            // Strategy 1: Exact phrase matching (most precise)
            // Normalize smart quotes/apostrophes for better matching
            let clean_title = title
                .replace('\u{2019}', "'") // Right single quote
                .replace('\u{2018}', "'") // Left single quote
                .replace('\u{201C}', "\"") // Left double quote
                .replace('\u{201D}', "\""); // Right double quote
            let exact_query = format!("\"{}\"", clean_title);

            match self
                .search_and_match(&exact_query, title, client, timeout)
                .await
            {
                Ok(Some(result)) => return Ok(result),
                Err(_) => return Ok(DbQueryResult::not_found()), // SearxNG unavailable
                Ok(None) => {} // No match, try fallback
            }

            // Strategy 2: Keyword search with significant words
            // This catches cases where the indexed title differs slightly
            let keywords = get_query_words(title, 6);
            if keywords.len() >= 3 {
                let keyword_query = keywords.join(" ");

                match self
                    .search_and_match(&keyword_query, title, client, timeout)
                    .await
                {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) | Err(_) => {}
                }
            }

            // Strategy 3: For RFC/standard document titles, try "RFC NNNN" exact match
            // RFC titles often appear as "RFC 5480" in search results
            static RFC_PATTERN: Lazy<Regex> =
                Lazy::new(|| Regex::new(r"(?i)\brfc\s*(\d+)\b").unwrap());
            if let Some(caps) = RFC_PATTERN.captures(title) {
                let rfc_num = caps.get(1).unwrap().as_str();
                let rfc_query = format!("\"RFC {}\"", rfc_num);

                match self
                    .search_and_match(&rfc_query, title, client, timeout)
                    .await
                {
                    Ok(Some(result)) => return Ok(result),
                    Ok(None) | Err(_) => {}
                }
            }

            // No match found
            Ok(DbQueryResult::not_found())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_titles_match_lenient_exact() {
        assert!(titles_match_lenient(
            "Attention Is All You Need",
            "Attention Is All You Need"
        ));
    }

    #[test]
    fn test_titles_match_lenient_with_suffix() {
        // Reference title should match search result with venue suffix
        assert!(titles_match_lenient(
            "Oasis: A universe in a transformer",
            "Oasis: A Universe in a Transformer - OpenReview"
        ));
    }

    #[test]
    fn test_titles_match_lenient_with_prefix() {
        // Search result with date prefix
        assert!(titles_match_lenient(
            "Oasis: A universe in a transformer",
            "October 31, 2024 Oasis: A Universe in a Transformer"
        ));
    }

    #[test]
    fn test_titles_match_lenient_case_insensitive() {
        assert!(titles_match_lenient(
            "attention is all you need",
            "ATTENTION IS ALL YOU NEED"
        ));
    }

    #[test]
    fn test_titles_match_lenient_rejects_different() {
        assert!(!titles_match_lenient(
            "Attention Is All You Need",
            "BERT: Pre-training of Deep Bidirectional Transformers"
        ));
    }
}
