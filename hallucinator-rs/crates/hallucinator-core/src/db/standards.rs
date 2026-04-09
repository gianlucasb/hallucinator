//! Standards document verifier backend.
//!
//! Recognizes references to technical standards (RFCs, 3GPP specs, IEEE standards,
//! ITU-T recommendations, etc.) and verifies them via pattern matching and, where
//! available, free public registry APIs.
//!
//! Two tiers of verification:
//! - **Tier 2** (pattern + registry lookup): IETF RFCs and Internet-Drafts use
//!   the RFC Editor / IETF Datatracker JSON APIs for strong verification.
//! - **Tier 1** (pattern only): 3GPP, IEEE, ITU-T, ISO, ETSI, and W3C specs are
//!   recognized by their rigid identifier format. This is weak verification
//!   (similar to URL liveness) but has near-zero false positives.

use once_cell::sync::Lazy;
use regex::Regex;

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Standards document verifier backend.
pub struct StandardsVerifier;

/// Type of standards document detected from a citation.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StandardType {
    /// IETF RFC (e.g., "RFC 8446", "RFC 791")
    Rfc(u32),
    /// IETF Internet-Draft (e.g., "draft-ietf-tls-esni-22")
    InternetDraft(String),
    /// 3GPP Technical Specification or Report (e.g., "TS 38.300", "TR 22.926")
    ThreeGpp { kind: String, number: String },
    /// 3GPP working group contribution (e.g., "R1-1913017")
    ThreeGppContrib(String),
    /// IEEE Standard (e.g., "IEEE 802.11ax-2021", "IEEE 1588")
    Ieee(String),
    /// ITU-T Recommendation (e.g., "ITU-T G.711", "ITU-T X.509")
    ItuT { series: String, number: String },
    /// ISO or ISO/IEC standard (e.g., "ISO 27001", "ISO/IEC 14882:2020")
    Iso(String),
    /// ETSI standard (e.g., "ETSI TS 103 645", "ETSI EN 300 328")
    Etsi { kind: String, number: String },
    /// W3C specification referenced by URL
    W3c(String),
    /// NIST Special Publication (e.g., "NIST SP 800-53")
    NistSp(String),
    /// NIST FIPS (e.g., "FIPS 140-3")
    NistFips(String),
}

/// Detect whether a title or raw citation looks like a standards document.
///
/// Returns `Some(StandardType)` with the parsed identifier if recognized.
pub(crate) fn detect_standard(text: &str) -> Option<StandardType> {
    // IETF RFC: "RFC 8446", "RFC8446", "rfc 791"
    static RFC_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bRFC\s*(\d{1,5})\b").unwrap());
    if let Some(m) = RFC_RE.captures(text)
        && let Ok(num) = m[1].parse::<u32>()
    {
        return Some(StandardType::Rfc(num));
    }

    // IETF Internet-Draft: "draft-ietf-tls-esni-22"
    static DRAFT_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(draft-[a-z0-9][-a-z0-9]+)\b").unwrap());
    if let Some(m) = DRAFT_RE.captures(text) {
        return Some(StandardType::InternetDraft(m[1].to_lowercase()));
    }

    // 3GPP TS/TR: "3GPP TS 38.300", "TS38.300", "TR 22.926", "3GPP. TR22.926:"
    // Also matches "3GPP. TS36.321:" format from parsed references
    static THREEGPP_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b(?:3GPP\.?\s+)?(TS|TR)\s*(\d{2}\.\d{3})\b").unwrap());
    if let Some(m) = THREEGPP_RE.captures(text) {
        return Some(StandardType::ThreeGpp {
            kind: m[1].to_uppercase(),
            number: m[2].to_string(),
        });
    }

    // 3GPP working group contributions: "R1-1913017", "S2-2100001"
    static THREEGPP_CONTRIB_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b([RS]\d)-(\d{6,7})\b").unwrap());
    if let Some(m) = THREEGPP_CONTRIB_RE.captures(text) {
        return Some(StandardType::ThreeGppContrib(format!(
            "{}-{}",
            m[1].to_uppercase(),
            &m[2]
        )));
    }

    // IEEE Standards: "IEEE 802.11", "IEEE Std 1588-2019", "IEEE 802.11ax-2021"
    static IEEE_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\bIEEE\s+(?:Std\.?\s+)?(\d{3,5}(?:\.\d+[a-z]*)?(?:-\d{4})?)\b").unwrap()
    });
    if let Some(m) = IEEE_RE.captures(text) {
        return Some(StandardType::Ieee(m[1].to_string()));
    }

    // ITU-T Recommendations: "ITU-T G.711", "ITU-T X.509"
    static ITU_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\bITU-?T\s+([A-Z])\.(\d{1,4}(?:\.\d+)?)\b").unwrap());
    if let Some(m) = ITU_RE.captures(text) {
        return Some(StandardType::ItuT {
            series: m[1].to_uppercase(),
            number: m[2].to_string(),
        });
    }

    // ISO/IEC: "ISO 27001", "ISO/IEC 14882:2020", "ISO 8601-1:2019"
    static ISO_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\bISO(?:/IEC)?\s+(\d{4,5}(?:[-:]\d{1,4})?(?:[-:]\d{4})?)\b").unwrap()
    });
    if let Some(m) = ISO_RE.captures(text) {
        return Some(StandardType::Iso(m[1].to_string()));
    }

    // ETSI: "ETSI TS 103 645", "ETSI EN 300 328", "ETSI TR 101 112"
    static ETSI_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\bETSI\s+(TS|TR|EN|ES|EG|GS|GR)\s+(\d{3}\s*\d{3})\b").unwrap()
    });
    if let Some(m) = ETSI_RE.captures(text) {
        return Some(StandardType::Etsi {
            kind: m[1].to_uppercase(),
            number: m[2].to_string(),
        });
    }

    // W3C spec URL: "https://www.w3.org/TR/css-grid-1/"
    static W3C_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)https?://www\.w3\.org/TR/([\w-]+)/?").unwrap());
    if let Some(m) = W3C_RE.captures(text) {
        return Some(StandardType::W3c(m[1].to_string()));
    }

    // NIST SP: "NIST SP 800-53", "SP 800-171"
    static NIST_SP_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)(?:NIST\s+)?SP\s+(\d{3,4}[-‐]\d+[A-Za-z]?(?:\s*[Rr]ev\.?\s*\d*)?)")
            .unwrap()
    });
    if let Some(m) = NIST_SP_RE.captures(text) {
        return Some(StandardType::NistSp(m[1].to_string()));
    }

    // NIST FIPS: "FIPS 140-3", "FIPS PUB 180-4"
    static NIST_FIPS_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(?:NIST\s+)?FIPS\s+(?:PUB\s+)?(\d{2,4}[-‐]?\d*)").unwrap());
    if let Some(m) = NIST_FIPS_RE.captures(text) {
        return Some(StandardType::NistFips(m[1].to_string()));
    }

    None
}

/// Build a human-readable label for the standard type.
fn standard_label(st: &StandardType) -> String {
    match st {
        StandardType::Rfc(n) => format!("RFC {}", n),
        StandardType::InternetDraft(name) => format!("Internet-Draft {}", name),
        StandardType::ThreeGpp { kind, number } => format!("3GPP {} {}", kind, number),
        StandardType::ThreeGppContrib(id) => format!("3GPP {}", id),
        StandardType::Ieee(num) => format!("IEEE {}", num),
        StandardType::ItuT { series, number } => format!("ITU-T {}.{}", series, number),
        StandardType::Iso(num) => format!("ISO {}", num),
        StandardType::Etsi { kind, number } => format!("ETSI {} {}", kind, number),
        StandardType::W3c(name) => format!("W3C {}", name),
        StandardType::NistSp(num) => format!("NIST SP {}", num),
        StandardType::NistFips(num) => format!("FIPS {}", num),
    }
}

/// Query the RFC Editor JSON API for an RFC.
async fn query_rfc(
    number: u32,
    _title: &str,
    client: &reqwest::Client,
    timeout: Duration,
) -> Result<DbQueryResult, DbQueryError> {
    let url = format!("https://www.rfc-editor.org/rfc/rfc{}.json", number);

    let resp = client
        .get(&url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| DbQueryError::Other(e.to_string()))?;

    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(DbQueryResult::not_found());
    }
    if status == 429 || status == 503 {
        return Err(DbQueryError::RateLimited {
            retry_after: Some(Duration::from_secs(2)),
        });
    }
    if !resp.status().is_success() {
        return Err(DbQueryError::Other(format!("HTTP {}", status)));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| DbQueryError::Other(e.to_string()))?;

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| DbQueryError::Other(e.to_string()))?;

    let rfc_title = json["title"].as_str().unwrap_or("");

    // Accept if the RFC exists (we already matched by number); optionally verify title
    if rfc_title.is_empty() {
        return Ok(DbQueryResult::not_found());
    }

    // Extract authors
    let authors: Vec<String> = json["authors"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let paper_url = format!("https://www.rfc-editor.org/rfc/rfc{}", number);

    // RFC matched by number — always accept if the registry returned a title.
    // `source_label` intentionally omitted (uses default `None`) so this displays
    // as plain "Standards" (registry-verified), unlike Tier 1 pattern matches
    // which show "Standards (pattern)".
    Ok(DbQueryResult::found(rfc_title, authors, Some(paper_url)))
}

/// Query the IETF Datatracker for an Internet-Draft.
async fn query_internet_draft(
    name: &str,
    client: &reqwest::Client,
    timeout: Duration,
) -> Result<DbQueryResult, DbQueryError> {
    let url = format!(
        "https://datatracker.ietf.org/doc/{}/doc.json",
        urlencoding::encode(name)
    );

    let resp = client
        .get(&url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| DbQueryError::Other(e.to_string()))?;

    let status = resp.status().as_u16();
    if status == 404 {
        return Ok(DbQueryResult::not_found());
    }
    if status == 429 || status == 503 {
        return Err(DbQueryError::RateLimited {
            retry_after: Some(Duration::from_secs(2)),
        });
    }
    if !resp.status().is_success() {
        return Err(DbQueryError::Other(format!("HTTP {}", status)));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| DbQueryError::Other(e.to_string()))?;

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| DbQueryError::Other(e.to_string()))?;

    let draft_title = json["title"].as_str().unwrap_or("");
    if draft_title.is_empty() {
        return Ok(DbQueryResult::not_found());
    }

    // Extract authors from the Datatracker response (same structure as RFC editor API)
    let authors: Vec<String> = json["authors"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a["name"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let paper_url = format!("https://datatracker.ietf.org/doc/{}/", name);

    // `source_label` intentionally omitted (uses default `None`) so this displays
    // as plain "Standards" (registry-verified), unlike Tier 1 pattern matches
    // which show "Standards (pattern)".
    Ok(DbQueryResult::found(draft_title, authors, Some(paper_url)))
}

impl DatabaseBackend for StandardsVerifier {
    fn name(&self) -> &str {
        "Standards"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            // Pre-filter: only process standards-like references
            let std_type = match detect_standard(title) {
                Some(st) => st,
                None => return Ok(DbQueryResult::not_found()),
            };

            // Tier 2: query public registries for strong verification
            match &std_type {
                StandardType::Rfc(num) => {
                    return query_rfc(*num, title, client, timeout).await;
                }
                StandardType::InternetDraft(name) => {
                    return query_internet_draft(name, client, timeout).await;
                }
                _ => {}
            }

            // Tier 1: pattern-based verification (no network call)
            // The identifier format is rigid enough that a regex match is strong
            // evidence the document is real. Return the standard label as the
            // "found title" so the user sees what was matched.
            let label = standard_label(&std_type);
            Ok(DbQueryResult::found_with_source(
                label,
                vec![],
                None,
                "Standards (pattern)",
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Pattern detection tests ──

    #[test]
    fn test_detect_rfc() {
        assert_eq!(detect_standard("RFC 8446"), Some(StandardType::Rfc(8446)));
        assert_eq!(detect_standard("RFC8446"), Some(StandardType::Rfc(8446)));
        assert_eq!(detect_standard("rfc 791"), Some(StandardType::Rfc(791)));
        assert_eq!(
            detect_standard("HTTP Semantics, RFC 9110"),
            Some(StandardType::Rfc(9110))
        );
    }

    #[test]
    fn test_detect_internet_draft() {
        match detect_standard("draft-ietf-tls-esni-22") {
            Some(StandardType::InternetDraft(name)) => {
                assert_eq!(name, "draft-ietf-tls-esni-22");
            }
            other => panic!("Expected InternetDraft, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_3gpp_ts() {
        match detect_standard("3GPP TS 38.300") {
            Some(StandardType::ThreeGpp { kind, number }) => {
                assert_eq!(kind, "TS");
                assert_eq!(number, "38.300");
            }
            other => panic!("Expected ThreeGpp, got {:?}", other),
        }

        // Without "3GPP" prefix — common in citations where author is "3GPP"
        match detect_standard("TS38.300: 5G NR: Overall Description") {
            Some(StandardType::ThreeGpp { kind, number }) => {
                assert_eq!(kind, "TS");
                assert_eq!(number, "38.300");
            }
            other => panic!("Expected ThreeGpp, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_3gpp_tr() {
        match detect_standard("3GPP TR 22.926: Guidelines for Extraterritorial 5G") {
            Some(StandardType::ThreeGpp { kind, number }) => {
                assert_eq!(kind, "TR");
                assert_eq!(number, "22.926");
            }
            other => panic!("Expected ThreeGpp TR, got {:?}", other),
        }

        // With dot-separated author format: "3GPP. TR22.926:"
        match detect_standard("TR22.926: Guidelines for Extraterritorial 5G Systems") {
            Some(StandardType::ThreeGpp { kind, number }) => {
                assert_eq!(kind, "TR");
                assert_eq!(number, "22.926");
            }
            other => panic!("Expected ThreeGpp TR, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_3gpp_contrib() {
        match detect_standard("R1-1913017: Doppler Compensation") {
            Some(StandardType::ThreeGppContrib(id)) => {
                assert_eq!(id, "R1-1913017");
            }
            other => panic!("Expected ThreeGppContrib, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_ieee() {
        match detect_standard("IEEE 802.11ax-2021") {
            Some(StandardType::Ieee(num)) => assert_eq!(num, "802.11ax-2021"),
            other => panic!("Expected Ieee, got {:?}", other),
        }
        match detect_standard("IEEE Std 1588-2019") {
            Some(StandardType::Ieee(num)) => assert_eq!(num, "1588-2019"),
            other => panic!("Expected Ieee, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_itu_t() {
        match detect_standard("ITU-T G.711") {
            Some(StandardType::ItuT { series, number }) => {
                assert_eq!(series, "G");
                assert_eq!(number, "711");
            }
            other => panic!("Expected ItuT, got {:?}", other),
        }
        match detect_standard("ITU-T X.509") {
            Some(StandardType::ItuT { series, number }) => {
                assert_eq!(series, "X");
                assert_eq!(number, "509");
            }
            other => panic!("Expected ItuT, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_iso() {
        match detect_standard("ISO/IEC 14882:2020") {
            Some(StandardType::Iso(num)) => assert_eq!(num, "14882:2020"),
            other => panic!("Expected Iso, got {:?}", other),
        }
        match detect_standard("ISO 27001") {
            Some(StandardType::Iso(num)) => assert_eq!(num, "27001"),
            other => panic!("Expected Iso, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_etsi() {
        match detect_standard("ETSI TS 103 645") {
            Some(StandardType::Etsi { kind, number }) => {
                assert_eq!(kind, "TS");
                assert_eq!(number, "103 645");
            }
            other => panic!("Expected Etsi, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_w3c() {
        match detect_standard("https://www.w3.org/TR/css-grid-1/") {
            Some(StandardType::W3c(name)) => assert_eq!(name, "css-grid-1"),
            other => panic!("Expected W3c, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_nist() {
        match detect_standard("NIST SP 800-53 Rev. 5") {
            Some(StandardType::NistSp(num)) => assert!(num.starts_with("800-53")),
            other => panic!("Expected NistSp, got {:?}", other),
        }
        match detect_standard("FIPS 140-3") {
            Some(StandardType::NistFips(num)) => assert_eq!(num, "140-3"),
            other => panic!("Expected NistFips, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_no_match() {
        assert!(detect_standard("A Study of Network Security").is_none());
        assert!(detect_standard("Machine Learning for Beginners").is_none());
    }

    // ── Group 1: StandardsVerifier::query() end-to-end (Tier 1, no network) ──

    #[tokio::test]
    async fn test_query_tier1_ieee_returns_found_with_source() {
        let verifier = StandardsVerifier;
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(5);
        let result = verifier
            .query("IEEE 802.11ax-2021", &client, timeout)
            .await
            .unwrap();
        assert!(result.is_found());
        assert_eq!(result.source_label.as_deref(), Some("Standards (pattern)"));
        assert_eq!(result.found_title.as_deref(), Some("IEEE 802.11ax-2021"));
        assert!(result.authors.is_empty());
        assert!(result.paper_url.is_none());
    }

    #[tokio::test]
    async fn test_query_tier1_3gpp_returns_found_with_source() {
        let verifier = StandardsVerifier;
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(5);
        let result = verifier
            .query("3GPP TS 38.300", &client, timeout)
            .await
            .unwrap();
        assert!(result.is_found());
        assert_eq!(result.source_label.as_deref(), Some("Standards (pattern)"));
        assert_eq!(result.found_title.as_deref(), Some("3GPP TS 38.300"));
        assert!(result.authors.is_empty());
        assert!(result.paper_url.is_none());
    }

    #[tokio::test]
    async fn test_query_tier1_iso_returns_found_with_source() {
        let verifier = StandardsVerifier;
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(5);
        let result = verifier
            .query("ISO/IEC 14882:2020", &client, timeout)
            .await
            .unwrap();
        assert!(result.is_found());
        assert_eq!(result.source_label.as_deref(), Some("Standards (pattern)"));
        assert_eq!(result.found_title.as_deref(), Some("ISO 14882:2020"));
        assert!(result.authors.is_empty());
        assert!(result.paper_url.is_none());
    }

    #[tokio::test]
    async fn test_query_non_standard_returns_not_found() {
        let verifier = StandardsVerifier;
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(5);
        let result = verifier
            .query("A Study of Network Security", &client, timeout)
            .await
            .unwrap();
        assert!(!result.is_found());
        assert!(result.source_label.is_none());
    }

    #[tokio::test]
    async fn test_query_empty_string_returns_not_found() {
        let verifier = StandardsVerifier;
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(5);
        let result = verifier.query("", &client, timeout).await.unwrap();
        assert!(!result.is_found());
    }

    #[tokio::test]
    async fn test_query_name_is_standards() {
        let verifier = StandardsVerifier;
        assert_eq!(verifier.name(), "Standards");
    }

    // ── Group 2: DbQueryResult constructor tests ──

    #[test]
    fn test_found_with_source_sets_all_fields() {
        let r = DbQueryResult::found_with_source(
            "Title",
            vec!["Auth".into()],
            Some("http://x".into()),
            "Custom",
        );
        assert_eq!(r.found_title.as_deref(), Some("Title"));
        assert_eq!(r.authors, vec!["Auth"]);
        assert_eq!(r.paper_url.as_deref(), Some("http://x"));
        assert_eq!(r.source_label.as_deref(), Some("Custom"));
        assert!(r.retraction.is_none());
        assert!(r.is_found());
    }

    #[test]
    fn test_found_has_no_source_label() {
        let r = DbQueryResult::found("T", vec![], None);
        assert!(r.source_label.is_none());
    }

    #[test]
    fn test_not_found_has_all_none() {
        let r = DbQueryResult::not_found();
        assert!(!r.is_found());
        assert!(r.found_title.is_none());
        assert!(r.authors.is_empty());
        assert!(r.paper_url.is_none());
        assert!(r.source_label.is_none());
        assert!(r.retraction.is_none());
    }

    // ── Group 3: Pattern detection edge cases ──

    #[test]
    fn test_detect_empty_string() {
        assert!(detect_standard("").is_none());
    }

    #[test]
    fn test_detect_whitespace_only() {
        assert!(detect_standard("   ").is_none());
    }

    #[test]
    fn test_detect_rfc_in_sentence() {
        assert_eq!(
            detect_standard("As described in RFC 8446, the protocol..."),
            Some(StandardType::Rfc(8446))
        );
    }

    #[test]
    fn test_detect_rfc_too_large() {
        // 6 digits exceeds \d{1,5}
        assert!(detect_standard("RFC 999999").is_none());
    }

    #[test]
    fn test_detect_rfc_no_false_positive_on_prefix() {
        // "RFCA" won't match \bRFC\s*\d because "A" breaks it
        assert!(detect_standard("RFCA 8446").is_none());
    }

    #[test]
    fn test_detect_ieee_in_sentence() {
        match detect_standard("The IEEE 802.11 standard defines...") {
            Some(StandardType::Ieee(num)) => assert_eq!(num, "802.11"),
            other => panic!("Expected Ieee, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_case_insensitive_iso() {
        assert!(detect_standard("iso 27001").is_some());
    }

    #[test]
    fn test_detect_case_insensitive_etsi() {
        assert!(detect_standard("etsi ts 103 645").is_some());
    }

    #[test]
    fn test_detect_multiple_standards_returns_first() {
        // RFC checked before IEEE, so "RFC 8446 and IEEE 802.11" returns Rfc
        assert_eq!(
            detect_standard("RFC 8446 and IEEE 802.11"),
            Some(StandardType::Rfc(8446))
        );
    }

    #[test]
    fn test_detect_w3c_without_trailing_slash() {
        match detect_standard("https://www.w3.org/TR/css-grid-1") {
            Some(StandardType::W3c(name)) => assert_eq!(name, "css-grid-1"),
            other => panic!("Expected W3c, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_fips_pub_variant() {
        match detect_standard("FIPS PUB 180-4") {
            Some(StandardType::NistFips(num)) => assert_eq!(num, "180-4"),
            other => panic!("Expected NistFips, got {:?}", other),
        }
    }

    #[test]
    fn test_detect_nist_sp_without_prefix() {
        // NIST prefix is optional in the regex
        assert!(detect_standard("SP 800-53").is_some());
    }

    // ── Group 4: Remaining standard_label coverage ──

    #[test]
    fn test_standard_label_all_variants() {
        assert_eq!(
            standard_label(&StandardType::InternetDraft(
                "draft-ietf-tls-esni-22".into()
            )),
            "Internet-Draft draft-ietf-tls-esni-22"
        );
        assert_eq!(
            standard_label(&StandardType::ThreeGppContrib("R1-1913017".into())),
            "3GPP R1-1913017"
        );
        assert_eq!(
            standard_label(&StandardType::ItuT {
                series: "G".into(),
                number: "711".into()
            }),
            "ITU-T G.711"
        );
        assert_eq!(
            standard_label(&StandardType::Iso("27001".into())),
            "ISO 27001"
        );
        assert_eq!(
            standard_label(&StandardType::Etsi {
                kind: "TS".into(),
                number: "103 645".into()
            }),
            "ETSI TS 103 645"
        );
        assert_eq!(
            standard_label(&StandardType::W3c("css-grid-1".into())),
            "W3C css-grid-1"
        );
        assert_eq!(
            standard_label(&StandardType::NistSp("800-53".into())),
            "NIST SP 800-53"
        );
        assert_eq!(
            standard_label(&StandardType::NistFips("140-3".into())),
            "FIPS 140-3"
        );
    }

    #[test]
    fn test_detect_3gpp_from_sigcomm_paper() {
        // Real references from the SIGCOMM '25 thesis paper
        let refs = [
            "TR22.926: Guidelines for Extraterritorial 5G Systems, Dec",
            "TR23.737: Study on Architecture Aspects for Using Satellite Access in 5G, Jun",
            "TR38.811: Study on New Radio (NR) to Support Non-terrestrial Networks",
            "TS38.300: 5G NR: Overall Description; Stage-2, Mar",
            "TS36.331: E-UTRA; Radio Resource Control (RRC), Jun",
            "TS38.331: 5G NR: Radio Resource Control (RRC), Jun",
            "TS24.301: Non-Access-Stratum (NAS) for EPS, Apr",
            "TS24.501: Non-Access-Stratum (NAS) for 5G, Apr",
            "TS36.321: Evolved Universal Terrestrial Radio Access (E-UTRA); Medium Access Control (MAC) Protocol Specification, Mar",
            "TS38.321: 5G NR; Medium Access Control (MAC) Protocol Specification, Jun",
            "TS33.501: Security Architecture and Procedures for 5G System, Jul",
            "TS33.401: 3GPP System Architecture Evolution (SAE); Security Architecture, Jul",
            "TR33.809: Study on 5G Security Enhancement against False Base Stations (FBS), Jun",
            "TS22.042: Network Identity and TimeZone (NITZ)",
            "TR22.872: Study on Positioning Use Cases, Jun",
        ];
        for r in &refs {
            assert!(
                detect_standard(r).is_some(),
                "Should detect standard in: {}",
                r
            );
        }
    }

    #[test]
    fn test_standard_label() {
        assert_eq!(standard_label(&StandardType::Rfc(8446)), "RFC 8446");
        assert_eq!(
            standard_label(&StandardType::ThreeGpp {
                kind: "TS".to_string(),
                number: "38.300".to_string()
            }),
            "3GPP TS 38.300"
        );
        assert_eq!(
            standard_label(&StandardType::Ieee("802.11".to_string())),
            "IEEE 802.11"
        );
    }
}
