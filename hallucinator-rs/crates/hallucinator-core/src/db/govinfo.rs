//! GovInfo API backend for US Federal laws and regulations.
//!
//! Uses the GovInfo API (api.govinfo.gov) to search for legal documents.
//! Requires an API key from api.data.gov.
//!
//! Improvements over naive title search:
//! - Pre-filters references: only queries the API for government-like citations
//! - Detects document numbers (NIST SP, FIPS, CFR, USC, Public Law, EO)
//! - Builds collection-filtered queries for better precision
//! - Uses relaxed matching for government documents (keyword overlap)

use once_cell::sync::Lazy;
use regex::Regex;

use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::{normalize_title, titles_match};
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

/// Type of government document detected from a citation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum GovDocType {
    /// NIST Special Publication (e.g., "NIST SP 800-82")
    NistSp(String),
    /// NIST FIPS (e.g., "FIPS 198-1", "FIPS PUB 180-4")
    NistFips(String),
    /// NIST Internal Report (e.g., "NIST IR 8214C")
    NistIr(String),
    /// Code of Federal Regulations (e.g., "21 CFR Part 11")
    Cfr(String),
    /// US Code (e.g., "47 U.S.C. § 230")
    Usc(String),
    /// Public Law (e.g., "Pub. L. 117-263")
    PublicLaw(String),
    /// Executive Order (e.g., "Executive Order 14110")
    ExecutiveOrder(String),
    /// Generic government document (matched by agency/org keywords)
    Generic(String),
}

/// Detect whether a title looks like a government document reference and extract
/// structured information for targeted querying.
fn detect_gov_doc(title: &str) -> Option<GovDocType> {
    // NIST Special Publication: "NIST SP 800-82", "SP 800-53 Rev. 5"
    static NIST_SP: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(?:NIST\s+)?SP\s+(\d{3,4}[-‐]\d+[A-Za-z]?(?:\s*[Rr]ev\.?\s*\d*)?)").unwrap());
    if let Some(m) = NIST_SP.captures(title) {
        return Some(GovDocType::NistSp(m[1].to_string()));
    }

    // NIST FIPS: "FIPS 198-1", "FIPS PUB 180-4", "NIST FIPS 140-3"
    static NIST_FIPS: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(?:NIST\s+)?FIPS\s+(?:PUB\s+)?(\d{2,4}[-‐]?\d*)").unwrap());
    if let Some(m) = NIST_FIPS.captures(title) {
        return Some(GovDocType::NistFips(m[1].to_string()));
    }

    // NIST Internal/Interagency Report: "NIST IR 8214C", "NISTIR 8413"
    static NIST_IR: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)NIST\s*(?:IR|Interagency)\s+(?:Report\s+)?(\d{4}[A-Za-z]?)").unwrap());
    if let Some(m) = NIST_IR.captures(title) {
        return Some(GovDocType::NistIr(m[1].to_string()));
    }

    // CFR: "21 CFR Part 11", "47 CFR § 1.1307", "Code of Federal Regulations"
    static CFR_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(\d+)\s+C\.?F\.?R\.?\s+(?:Part\s+|§\s*)?(\d+)").unwrap());
    if let Some(m) = CFR_RE.captures(title) {
        return Some(GovDocType::Cfr(format!("{} CFR {}", &m[1], &m[2])));
    }

    // US Code: "47 U.S.C. § 230", "15 USC 1681", "Title 47 United States Code"
    static USC_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(\d+)\s+U\.?S\.?C\.?\s+(?:§\s*)?(\d+)").unwrap());
    if let Some(m) = USC_RE.captures(title) {
        return Some(GovDocType::Usc(format!("{} USC {}", &m[1], &m[2])));
    }

    // Public Law: "Pub. L. 117-263", "Public Law 104-191"
    static PUBLAW_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(?:Pub(?:lic)?\.?\s+L(?:aw)?\.?)\s+(\d+[-‐]\d+)").unwrap());
    if let Some(m) = PUBLAW_RE.captures(title) {
        return Some(GovDocType::PublicLaw(m[1].to_string()));
    }

    // Executive Order: "Executive Order 14110", "E.O. 13960"
    static EO_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)(?:Executive\s+Order|E\.?O\.?)\s+(\d{4,5})").unwrap());
    if let Some(m) = EO_RE.captures(title) {
        return Some(GovDocType::ExecutiveOrder(m[1].to_string()));
    }

    // Generic government document: agency/institution names
    static GOV_AGENCY: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?:National\s+Institute\s+of\s+Standards|NIST|CISA|GAO|CRS|OMB|OSTP|Department\s+of\s+(?:Commerce|Defense|Energy|Homeland|Justice)|Federal\s+(?:Trade|Register|Communications)|White\s+House|Congress(?:ional)?|Government\s+Accountability)\b").unwrap()
    });
    if GOV_AGENCY.is_match(title) {
        return Some(GovDocType::Generic(title.to_string()));
    }

    // Common US law acronyms as standalone titles
    static LAW_ACRONYM: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(?:HIPAA|COPPA|CCPA|FERPA|ECPA|CFAA|DMCA|GLBA|FISMA|SOX|CIPA)\b").unwrap()
    });
    if LAW_ACRONYM.is_match(title) {
        return Some(GovDocType::Generic(title.to_string()));
    }

    None
}

/// Extract significant title keywords for GovInfo search (skip common words).
fn title_keywords(title: &str, max: usize) -> String {
    static STOP_WORDS: Lazy<std::collections::HashSet<&str>> = Lazy::new(|| {
        [
            "the", "and", "for", "with", "from", "that", "this", "are", "was",
            "not", "but", "its", "our", "into", "over", "about", "rev", "vol",
            "report", "standard", "special", "publication", "internal", "interagency",
            "department", "commerce", "institute", "national", "technology", "standards",
            "online", "available", "accessed",
        ]
        .into_iter()
        .collect()
    });
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3 && !STOP_WORDS.contains(w.to_lowercase().as_str()))
        .take(max)
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a targeted GovInfo search query based on the detected document type.
fn build_query(doc_type: &GovDocType, title: &str) -> String {
    // Combine document identifier with title keywords for best results.
    // GovInfo's full-text search works best with a mix of specific identifiers
    // and descriptive keywords from the title.
    let keywords = title_keywords(title, 6);

    match doc_type {
        GovDocType::NistSp(sp_num) => {
            if keywords.is_empty() {
                format!("\"SP {}\"", sp_num)
            } else {
                format!("\"SP {}\" {}", sp_num, keywords)
            }
        }
        GovDocType::NistFips(fips_num) => {
            if keywords.is_empty() {
                format!("\"FIPS {}\"", fips_num)
            } else {
                format!("\"FIPS {}\" {}", fips_num, keywords)
            }
        }
        GovDocType::NistIr(ir_num) => {
            format!("\"IR {}\" NIST {}", ir_num, keywords)
        }
        GovDocType::Cfr(cfr_cite) => {
            format!("collection:CFR AND \"{}\"", cfr_cite)
        }
        GovDocType::Usc(usc_cite) => {
            format!("collection:USCODE AND \"{}\"", usc_cite)
        }
        GovDocType::PublicLaw(law_num) => {
            format!("collection:PLAW AND \"{}\"", law_num)
        }
        GovDocType::ExecutiveOrder(eo_num) => {
            format!("\"Executive Order {}\"", eo_num)
        }
        GovDocType::Generic(_) => {
            if keywords.is_empty() {
                title.to_string()
            } else {
                keywords
            }
        }
    }
}

/// Check if a GovInfo result matches the query, using relaxed matching
/// appropriate for government documents where titles are often abbreviated.
///
/// For document-number types (NIST SP, FIPS, etc.), we accept the match
/// if the first result from a targeted query is from the right collection
/// (GOVPUB for NIST, CFR, USCODE, etc.) since the API's relevancy ranking
/// already filtered by the document number.
fn gov_title_matches(
    query_title: &str,
    doc_title: &str,
    doc_type: &GovDocType,
    result: &serde_json::Value,
) -> bool {
    // First try standard fuzzy matching (works for exact/near-exact titles)
    if titles_match(query_title, doc_title) {
        return true;
    }

    let collection = result["collectionCode"].as_str().unwrap_or_default();
    let gov_authors = result["governmentAuthor"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .unwrap_or_default();

    match doc_type {
        GovDocType::NistSp(_) | GovDocType::NistFips(_) | GovDocType::NistIr(_) => {
            // For NIST documents: accept if the result is from GOVPUB and authored by NIST.
            // The API search already filtered by document number, so the first NIST result
            // from a "SP 800-82" query is very likely the right document.
            (collection == "GOVPUB" || collection == "NIST")
                && gov_authors.contains("nist")
        }
        GovDocType::Cfr(_) => collection == "CFR" || collection == "ECFR",
        GovDocType::Usc(_) => collection == "USCODE",
        GovDocType::PublicLaw(_) => collection == "PLAW" || collection == "STATUTE",
        GovDocType::ExecutiveOrder(_) => {
            doc_title.to_lowercase().contains("executive order")
                || collection == "CPD"
                || collection == "DCPD"
        }
        GovDocType::Generic(_) => {
            // For generic gov docs: try relaxed keyword-based matching
            let norm_query = normalize_title(query_title);
            let norm_doc = normalize_title(doc_title);
            if norm_query.is_empty() || norm_doc.is_empty() {
                return false;
            }
            let (shorter, longer) = if norm_query.len() <= norm_doc.len() {
                (&norm_query, &norm_doc)
            } else {
                (&norm_doc, &norm_query)
            };
            // Substring containment for long enough titles
            if shorter.len() >= 20 && longer.contains(shorter.as_str()) {
                return true;
            }
            // Relaxed fuzzy match at 85%
            rapidfuzz::fuzz::ratio(norm_query.chars(), norm_doc.chars()) >= 0.85
        }
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
            // Pre-filter: only query GovInfo for government-like references.
            // This avoids wasting API calls on academic papers, and prevents
            // errors from sending formulas/garbage as search queries.
            let doc_type = match detect_gov_doc(title) {
                Some(dt) => dt,
                None => return Ok(DbQueryResult::not_found()),
            };

            let query = build_query(&doc_type, title);

            let url = format!(
                "https://api.govinfo.gov/search?api_key={}",
                urlencoding::encode(&self.api_key)
            );

            let body = serde_json::json!({
                "query": query,
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

            // Handle 429 and 503 as rate limiting
            let status = resp.status().as_u16();
            if status == 429 || status == 503 {
                return Err(DbQueryError::RateLimited {
                    retry_after: Some(Duration::from_secs(2)),
                });
            }
            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let text = resp
                .text()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            parse_govinfo_response(&text, title, &doc_type)
        })
    }
}

/// Parse GovInfo JSON response and find matching documents.
fn parse_govinfo_response(
    json: &str,
    title: &str,
    doc_type: &GovDocType,
) -> Result<DbQueryResult, DbQueryError> {
    let data: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| DbQueryError::Other(format!("JSON error: {}", e)))?;

    let results = data["results"].as_array();

    if let Some(results) = results {
        for result in results {
            let doc_title = result["title"].as_str().unwrap_or_default();

            if gov_title_matches(title, doc_title, doc_type, result) {
                // Extract government authors from the API response
                let authors: Vec<String> = result["governmentAuthor"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_else(|| {
                        // Fallback to collection/granule info
                        let collection_code = result["collectionCode"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        let granule_class = result["granuleClass"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string();
                        if !collection_code.is_empty() {
                            vec![format!("{} ({})", collection_code, granule_class)]
                        } else {
                            vec![]
                        }
                    });

                // Prefer summary link, fall back to package link
                let doc_url = result["resultLink"]
                    .as_str()
                    .or_else(|| result["packageLink"].as_str())
                    .map(|s| s.to_string());

                return Ok(DbQueryResult::found(doc_title, authors, doc_url));
            }
        }
    }

    Ok(DbQueryResult::not_found())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Detection tests ──

    #[test]
    fn detect_nist_sp() {
        assert!(matches!(detect_gov_doc("NIST SP 800-82 Rev"), Some(GovDocType::NistSp(_))));
        assert!(matches!(detect_gov_doc("SP 800-53 Rev. 5"), Some(GovDocType::NistSp(_))));
        assert!(matches!(
            detect_gov_doc("Guide to OT Security, NIST SP 800-82"),
            Some(GovDocType::NistSp(_))
        ));
    }

    #[test]
    fn detect_nist_fips() {
        assert!(matches!(detect_gov_doc("FIPS 198-1"), Some(GovDocType::NistFips(_))));
        assert!(matches!(detect_gov_doc("NIST FIPS 140-3"), Some(GovDocType::NistFips(_))));
        assert!(matches!(
            detect_gov_doc("FIPS PUB 180-4"),
            Some(GovDocType::NistFips(_))
        ));
    }

    #[test]
    fn detect_nist_ir() {
        assert!(matches!(
            detect_gov_doc("NIST Interagency Report 8214C"),
            Some(GovDocType::NistIr(_))
        ));
        assert!(matches!(detect_gov_doc("NIST IR 8413"), Some(GovDocType::NistIr(_))));
    }

    #[test]
    fn detect_cfr() {
        assert!(matches!(
            detect_gov_doc("21 CFR Part 11: Electronic Records"),
            Some(GovDocType::Cfr(_))
        ));
        assert!(matches!(detect_gov_doc("47 C.F.R. § 1.1307"), Some(GovDocType::Cfr(_))));
    }

    #[test]
    fn detect_usc() {
        assert!(matches!(detect_gov_doc("47 U.S.C. § 230"), Some(GovDocType::Usc(_))));
        assert!(matches!(detect_gov_doc("15 USC 1681"), Some(GovDocType::Usc(_))));
    }

    #[test]
    fn detect_public_law() {
        assert!(matches!(
            detect_gov_doc("Public Law 104-191"),
            Some(GovDocType::PublicLaw(_))
        ));
        assert!(matches!(detect_gov_doc("Pub. L. 117-263"), Some(GovDocType::PublicLaw(_))));
    }

    #[test]
    fn detect_executive_order() {
        assert!(matches!(
            detect_gov_doc("Executive Order 14110"),
            Some(GovDocType::ExecutiveOrder(_))
        ));
        assert!(matches!(detect_gov_doc("E.O. 13960"), Some(GovDocType::ExecutiveOrder(_))));
    }

    #[test]
    fn detect_generic_gov() {
        assert!(matches!(
            detect_gov_doc("National Institute of Standards and Technology report"),
            Some(GovDocType::Generic(_))
        ));
        assert!(matches!(
            detect_gov_doc("CISA advisory on post-quantum"),
            Some(GovDocType::Generic(_))
        ));
        assert!(matches!(
            detect_gov_doc("HIPAA compliance guide"),
            Some(GovDocType::Generic(_))
        ));
    }

    #[test]
    fn detect_non_gov() {
        assert!(detect_gov_doc("Attention is all you need").is_none());
        assert!(detect_gov_doc("BERT pre-training").is_none());
        assert!(detect_gov_doc("A survey on deep learning").is_none());
    }

    // ── Query building tests ──

    #[test]
    fn build_query_nist_sp() {
        let q = build_query(&GovDocType::NistSp("800-82".into()), "NIST SP 800-82");
        assert!(q.contains("SP 800-82"));
    }

    #[test]
    fn build_query_cfr() {
        let q = build_query(&GovDocType::Cfr("21 CFR 11".into()), "21 CFR Part 11");
        assert!(q.contains("collection:CFR"));
        assert!(q.contains("21 CFR 11"));
    }

    // ── Response parsing tests ──

    #[test]
    fn parse_empty_results() {
        let json = r#"{"count": 0, "results": []}"#;
        let result =
            parse_govinfo_response(json, "Some Title", &GovDocType::Generic("".into())).unwrap();
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
        let result = parse_govinfo_response(
            json,
            "Test Document Title",
            &GovDocType::Generic("".into()),
        )
        .unwrap();
        assert!(result.is_found());
        assert_eq!(result.found_title.unwrap(), "Test Document Title");
    }

    #[test]
    fn parse_nist_sp_by_collection() {
        // GovInfo returns the full title, but we matched by collection + author
        let json = r#"{
            "count": 1,
            "results": [{
                "title": "Guide to Operational Technology (OT) Security",
                "collectionCode": "GOVPUB",
                "granuleClass": "NIST",
                "governmentAuthor": ["Commerce Department", "National Institute of Standards and Technology (NIST)"],
                "resultLink": "https://api.govinfo.gov/packages/GOVPUB-C13-abc/summary"
            }]
        }"#;
        let result =
            parse_govinfo_response(json, "NIST SP 800-82 Rev", &GovDocType::NistSp("800-82".into()))
                .unwrap();
        assert!(result.is_found(), "Should match NIST SP by collection+author");
        assert!(result.authors.iter().any(|a| a.contains("NIST")));
    }

    #[test]
    fn parse_fips_by_collection() {
        let json = r#"{
            "count": 1,
            "results": [{
                "title": "The Keyed-Hash Message Authentication Code (Hmac)",
                "collectionCode": "GOVPUB",
                "granuleClass": "NIST",
                "governmentAuthor": ["Commerce Department", "National Institute of Standards and Technology (NIST)"],
                "resultLink": "https://api.govinfo.gov/packages/xyz/summary"
            }]
        }"#;
        let result = parse_govinfo_response(
            json,
            "The keyed-hash message authentication code (HMAC)",
            &GovDocType::NistFips("198-1".into()),
        )
        .unwrap();
        assert!(result.is_found(), "Should match FIPS by title+collection");
    }

    #[test]
    fn parse_no_match() {
        let json = r#"{
            "count": 1,
            "results": [{
                "title": "Completely Different Document",
                "collectionCode": "BILLS",
                "granuleClass": "HR"
            }]
        }"#;
        let result = parse_govinfo_response(
            json,
            "Framework for Critical Infrastructure",
            &GovDocType::Generic("".into()),
        )
        .unwrap();
        assert!(!result.is_found());
    }

    // ── Matching tests ──

    fn mock_result(collection: &str, gov_author: &str) -> serde_json::Value {
        serde_json::json!({
            "title": "Placeholder",
            "collectionCode": collection,
            "governmentAuthor": [gov_author],
        })
    }

    #[test]
    fn gov_match_relaxed_threshold() {
        let result = mock_result("GOVPUB", "");
        assert!(gov_title_matches(
            "Framework for Improving Critical Infrastructure Cybersecurity",
            "Framework for Improving Critical Infrastructure Cybersecurity Version 1.1",
            &GovDocType::Generic("".into()),
            &result,
        ));
    }

    #[test]
    fn gov_match_nist_sp_by_collection() {
        // NIST SP from GOVPUB collection authored by NIST should match
        let result = mock_result("GOVPUB", "National Institute of Standards and Technology (NIST)");
        assert!(gov_title_matches(
            "NIST SP 800-82 Rev",
            "Guide to Operational Technology (OT) Security",
            &GovDocType::NistSp("800-82".into()),
            &result,
        ));
    }

    #[test]
    fn gov_match_nist_sp_rejects_non_nist() {
        // A non-NIST result from GOVPUB should NOT match
        let result = mock_result("GOVPUB", "Department of Defense");
        assert!(!gov_title_matches(
            "NIST SP 800-82 Rev",
            "Some military document",
            &GovDocType::NistSp("800-82".into()),
            &result,
        ));
    }

    #[test]
    fn gov_match_rejects_unrelated() {
        let result = mock_result("GOVPUB", "");
        assert!(!gov_title_matches(
            "Some random paper",
            "A totally different government document",
            &GovDocType::Generic("".into()),
            &result,
        ));
    }
}
