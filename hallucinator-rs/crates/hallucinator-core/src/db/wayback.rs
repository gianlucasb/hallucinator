//! Wayback Machine (Internet Archive) availability fallback.
//!
//! Some references cite URLs that were live when the paper was written
//! but have since 404'd — link rot, not fabrication. Treating these as
//! hallucinations is a false positive the user then has to hand-mark
//! safe. This module queries archive.org's Availability API so we can
//! recognise "URL is dead now but existed once" as a real citation,
//! distinct from "URL never existed".
//!
//! API: `https://archive.org/wayback/available?url=<url>` — returns
//! JSON with the closest snapshot (or an empty object if the URL was
//! never archived). Only snapshots whose capture-time HTTP status was
//! 2xx / 3xx count: a Wayback record of a 404 response doesn't prove
//! the URL was ever live.

use std::time::Duration;

/// Result of a successful Wayback lookup.
#[derive(Debug, Clone)]
pub struct WaybackResult {
    /// The original URL the user checked (useful for building the
    /// citation's own DB result row).
    pub original_url: String,
    /// The `web.archive.org/web/<timestamp>/...` URL. Clickable and
    /// serves the captured page content.
    pub snapshot_url: String,
    /// Wayback timestamp, e.g. "20230612123456". Callers can display
    /// the capture date alongside the snapshot link.
    pub timestamp: String,
}

/// Check one URL against the Wayback Machine's Availability API.
///
/// Returns `Some` only when the closest snapshot both exists
/// (`available: true`) and captured a successful HTTP response
/// (`status` = 2xx or 3xx). Network errors and missing snapshots both
/// collapse to `None`.
pub async fn check_url(
    url: &str,
    client: &reqwest::Client,
    timeout: Duration,
) -> Option<WaybackResult> {
    let api = format!(
        "https://archive.org/wayback/available?url={}",
        urlencoding::encode(url)
    );
    let resp = client.get(&api).timeout(timeout).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = resp.text().await.ok()?;
    parse_availability_response(url, &body)
}

/// Check a list of URLs and return the first successful snapshot.
///
/// Mirrors `UrlChecker::check_first_live`: sequential, short-circuits
/// on the first hit. Usually a ref has exactly one URL, so the loop is
/// either one request or a couple.
pub async fn check_first_snapshot(
    urls: &[String],
    client: &reqwest::Client,
    timeout: Duration,
) -> Option<WaybackResult> {
    for url in urls {
        if let Some(result) = check_url(url, client, timeout).await {
            return Some(result);
        }
    }
    None
}

/// Parse the Availability API's JSON response into a `WaybackResult`.
fn parse_availability_response(original_url: &str, body: &str) -> Option<WaybackResult> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let closest = value.get("archived_snapshots")?.get("closest")?;
    if !closest
        .get("available")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    // `status` is a string in the API response, e.g. "200".
    let status = closest.get("status").and_then(|v| v.as_str()).unwrap_or("");
    if !status_counts_as_captured(status) {
        return None;
    }
    let snapshot_url = closest.get("url")?.as_str()?.to_string();
    let timestamp = closest
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(WaybackResult {
        original_url: original_url.to_string(),
        snapshot_url,
        timestamp,
    })
}

/// A Wayback snapshot only counts as "the URL existed" when the
/// capture itself was a successful fetch. A snapshot of a 404 page
/// would be misleading.
fn status_counts_as_captured(status: &str) -> bool {
    let Ok(n) = status.parse::<u16>() else {
        return false;
    };
    (200..400).contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_successful_snapshot() {
        let body = r#"{
            "url": "https://example.com/page",
            "archived_snapshots": {
                "closest": {
                    "status": "200",
                    "available": true,
                    "url": "http://web.archive.org/web/20230612123456/https://example.com/page",
                    "timestamp": "20230612123456"
                }
            }
        }"#;
        let result = parse_availability_response("https://example.com/page", body).unwrap();
        assert_eq!(result.original_url, "https://example.com/page");
        assert_eq!(
            result.snapshot_url,
            "http://web.archive.org/web/20230612123456/https://example.com/page"
        );
        assert_eq!(result.timestamp, "20230612123456");
    }

    #[test]
    fn returns_none_on_empty_snapshots() {
        // Wayback shape when the URL was never archived.
        let body = r#"{"url":"https://example.com/page","archived_snapshots":{}}"#;
        assert!(parse_availability_response("https://example.com/page", body).is_none());
    }

    #[test]
    fn rejects_snapshot_of_404_capture() {
        // A capture of a 404 response doesn't prove the URL was ever
        // live — that would just let a hallucinated URL slip through
        // because archive.org crawled it once and got a 404 too.
        let body = r#"{
            "archived_snapshots": {
                "closest": {
                    "status": "404",
                    "available": true,
                    "url": "http://web.archive.org/web/20230612123456/https://example.com/page",
                    "timestamp": "20230612123456"
                }
            }
        }"#;
        assert!(parse_availability_response("https://example.com/page", body).is_none());
    }

    #[test]
    fn rejects_snapshot_with_available_false() {
        // `available: false` happens when the record exists in the
        // index but the capture itself is gone. Don't count it.
        let body = r#"{
            "archived_snapshots": {
                "closest": {
                    "status": "200",
                    "available": false,
                    "url": "http://web.archive.org/web/20230612123456/https://example.com/page",
                    "timestamp": "20230612123456"
                }
            }
        }"#;
        assert!(parse_availability_response("https://example.com/page", body).is_none());
    }

    #[test]
    fn accepts_redirect_status_captures() {
        // A 301/302 capture means the archived version was itself a
        // redirect — still evidence the URL existed.
        let body = r#"{
            "archived_snapshots": {
                "closest": {
                    "status": "301",
                    "available": true,
                    "url": "http://web.archive.org/web/20230101000000/https://example.com/page",
                    "timestamp": "20230101000000"
                }
            }
        }"#;
        assert!(parse_availability_response("https://example.com/page", body).is_some());
    }

    #[test]
    fn rejects_malformed_json() {
        // Defensive: archive.org occasionally returns HTML error pages
        // under load. Parsing failure should collapse to "no snapshot".
        let body = "<html>service unavailable</html>";
        assert!(parse_availability_response("https://example.com/page", body).is_none());
    }
}
