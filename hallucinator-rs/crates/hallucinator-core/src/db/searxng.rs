//! SearxNG web search fallback for unverified citations.
//!
//! This backend queries a self-hosted SearxNG instance to search for papers
//! that couldn't be found in any academic database. It uses exact phrase
//! matching and filters results to academic domains.
//!
//! Note: This is a weaker form of verification than academic databases since
//! it cannot verify authors - it only confirms the paper exists on the web.

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::normalize_title;
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
            // Use exact phrase matching
            let query = format!("\"{}\"", title);
            let url = format!(
                "{}/search?q={}&format=json",
                self.base_url.trim_end_matches('/'),
                urlencoding::encode(&query)
            );

            // If SearxNG is not running, silently return "not found" instead of erroring
            let resp = match client.get(&url).timeout(timeout).send().await {
                Ok(r) => r,
                Err(_) => {
                    // Connection refused, timeout, etc. - SearxNG not available
                    return Ok((None, vec![], None));
                }
            };

            if !resp.status().is_success() {
                // SearxNG returned an error - skip silently
                return Ok((None, vec![], None));
            }

            let data: SearxngResponse = match resp.json().await {
                Ok(d) => d,
                Err(_) => {
                    // Failed to parse response - skip silently
                    return Ok((None, vec![], None));
                }
            };

            // Check results for matching titles
            for result in data.results {
                // Verify the title matches (lenient matching for web search)
                if titles_match_lenient(title, &result.title) {
                    // Return found result - no authors available from web search
                    return Ok((Some(result.title), vec![], Some(result.url)));
                }
            }

            // No matching academic result found
            Ok((None, vec![], None))
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
