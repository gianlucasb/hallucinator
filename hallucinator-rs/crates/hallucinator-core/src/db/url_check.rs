//! URL liveness check for non-academic references.
//!
//! This module provides a fallback verification method for references that contain
//! URLs (e.g., GitHub repositories, blog posts) but couldn't be verified through
//! academic databases. If the URL is live, we consider the reference verified.
//!
//! Note: This is a weaker form of verification than academic databases since
//! it only confirms the URL is reachable, not that the content matches.

use std::time::Duration;

/// Result of checking a URL's liveness.
#[derive(Debug, Clone)]
pub struct UrlCheckResult {
    /// The original URL that was checked.
    pub url: String,
    /// Whether the URL is live (returned 2xx or 3xx).
    pub is_live: bool,
    /// HTTP status code if a response was received.
    pub status_code: Option<u16>,
    /// Final URL after following redirects (if different from original).
    pub final_url: Option<String>,
}

/// URL liveness checker.
pub struct UrlChecker;

impl UrlChecker {
    /// Check if a single URL is live.
    ///
    /// Uses HEAD request first (cheaper), falls back to GET if the server
    /// returns 405 Method Not Allowed.
    pub async fn check_url(
        url: &str,
        client: &reqwest::Client,
        timeout: Duration,
    ) -> UrlCheckResult {
        // Try HEAD first (cheaper, no body download)
        let head_result = client.head(url).timeout(timeout).send().await;

        match head_result {
            Ok(resp) => {
                let status = resp.status();
                let final_url = resp.url().as_str();
                let final_url = if final_url != url {
                    Some(final_url.to_string())
                } else {
                    None
                };

                if status.is_success() || status.is_redirection() {
                    return UrlCheckResult {
                        url: url.to_string(),
                        is_live: true,
                        status_code: Some(status.as_u16()),
                        final_url,
                    };
                }

                // If HEAD returns 405 Method Not Allowed, try GET
                if status.as_u16() == 405 {
                    return Self::check_url_get(url, client, timeout).await;
                }

                // Other non-success status
                UrlCheckResult {
                    url: url.to_string(),
                    is_live: false,
                    status_code: Some(status.as_u16()),
                    final_url,
                }
            }
            Err(_) => {
                // HEAD failed (timeout, connection error, etc.) - try GET as fallback
                Self::check_url_get(url, client, timeout).await
            }
        }
    }

    /// Check URL using GET request (fallback when HEAD fails or returns 405).
    async fn check_url_get(
        url: &str,
        client: &reqwest::Client,
        timeout: Duration,
    ) -> UrlCheckResult {
        let get_result = client.get(url).timeout(timeout).send().await;

        match get_result {
            Ok(resp) => {
                let status = resp.status();
                let final_url = resp.url().as_str();
                let final_url = if final_url != url {
                    Some(final_url.to_string())
                } else {
                    None
                };

                UrlCheckResult {
                    url: url.to_string(),
                    is_live: status.is_success() || status.is_redirection(),
                    status_code: Some(status.as_u16()),
                    final_url,
                }
            }
            Err(_) => {
                // Connection failed
                UrlCheckResult {
                    url: url.to_string(),
                    is_live: false,
                    status_code: None,
                    final_url: None,
                }
            }
        }
    }

    /// Check multiple URLs and return the first one that is live.
    ///
    /// Checks URLs sequentially (to minimize network traffic) and returns
    /// as soon as one is found to be live.
    pub async fn check_first_live(
        urls: &[String],
        client: &reqwest::Client,
        timeout: Duration,
    ) -> Option<UrlCheckResult> {
        for url in urls {
            let result = Self::check_url(url, client, timeout).await;
            if result.is_live {
                return Some(result);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These are integration tests that require network access.
    // They're marked #[ignore] by default and can be run with:
    // cargo test --package hallucinator-core url_check -- --ignored

    #[tokio::test]
    #[ignore]
    async fn test_check_url_live_site() {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap();
        let timeout = Duration::from_secs(10);

        let result = UrlChecker::check_url("https://www.example.com", &client, timeout).await;
        assert!(result.is_live);
        assert!(result.status_code.is_some());
    }

    #[tokio::test]
    #[ignore]
    async fn test_check_url_dead_site() {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap();
        let timeout = Duration::from_secs(5);

        // Non-existent domain
        let result = UrlChecker::check_url(
            "https://this-domain-definitely-does-not-exist-12345.com",
            &client,
            timeout,
        )
        .await;
        assert!(!result.is_live);
    }

    #[tokio::test]
    #[ignore]
    async fn test_check_url_follows_redirects() {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap();
        let timeout = Duration::from_secs(10);

        // http://example.com redirects to https://example.com
        let result = UrlChecker::check_url("http://example.com", &client, timeout).await;
        assert!(result.is_live);
        // final_url may be set if redirect happened
    }

    #[tokio::test]
    #[ignore]
    async fn test_check_first_live() {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .unwrap();
        let timeout = Duration::from_secs(10);

        let urls = vec![
            "https://this-does-not-exist-12345.com".to_string(),
            "https://www.example.com".to_string(),
            "https://www.google.com".to_string(),
        ];

        let result = UrlChecker::check_first_live(&urls, &client, timeout).await;
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.is_live);
        assert!(result.url.contains("example.com"));
    }
}
