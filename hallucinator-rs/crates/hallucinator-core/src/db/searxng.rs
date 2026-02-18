//! SearxNG web search fallback for unverified citations.
//!
//! This backend queries a self-hosted SearxNG instance to search for papers
//! that couldn't be found in any academic database. It uses exact phrase
//! matching and filters results to academic domains.
//!
//! Note: This is a weaker form of verification than academic databases since
//! it cannot verify authors - it only confirms the paper exists on the web.

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
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

/// Check if a URL is from an academic domain.
fn is_academic_url(url: &str) -> bool {
    const ACADEMIC_DOMAINS: &[&str] = &[
        "scholar.google",
        "arxiv.org",
        "semanticscholar.org",
        "researchgate.net",
        "academia.edu",
        "acm.org",
        "ieee.org",
        "springer.com",
        "sciencedirect.com",
        "wiley.com",
        "nature.com",
        "pnas.org",
        "nih.gov",
        "pubmed",
        "jstor.org",
        "aclanthology.org",
        "aclweb.org",
        "openreview.net",
        "neurips.cc",
        "proceedings.mlr.press",
        "jmlr.org",
        "ssrn.com",
        "europepmc.org",
        "ncbi.nlm.nih.gov",
        "biorxiv.org",
        "medrxiv.org",
        "plos.org",
        "frontiersin.org",
        "mdpi.com",
        "tandfonline.com",
        "cambridge.org",
        "oup.com",
        "sagepub.com",
        "dblp.org",
        ".edu/",
        ".ac.uk/",
    ];

    let url_lower = url.to_lowercase();
    ACADEMIC_DOMAINS.iter().any(|d| url_lower.contains(d))
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

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let data: SearxngResponse = resp
                .json()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            // Check results for academic URLs with matching titles
            for result in data.results {
                // Only accept results from academic domains
                if !is_academic_url(&result.url) {
                    continue;
                }

                // Verify the title matches (fuzzy matching)
                if titles_match(title, &result.title) {
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
    fn test_is_academic_url() {
        assert!(is_academic_url("https://arxiv.org/abs/1234.5678"));
        assert!(is_academic_url("https://www.semanticscholar.org/paper/..."));
        assert!(is_academic_url("https://dl.acm.org/doi/10.1145/123456"));
        assert!(is_academic_url("https://ieeexplore.ieee.org/document/123"));
        assert!(is_academic_url("https://cs.stanford.edu/papers/paper.pdf"));
        assert!(is_academic_url("https://www.cam.ac.uk/research/paper.pdf"));

        assert!(!is_academic_url("https://github.com/user/repo"));
        assert!(!is_academic_url("https://medium.com/article"));
        assert!(!is_academic_url("https://random-blog.com/post"));
    }
}
