use crate::authors::validate_authors;
use crate::matching::normalize_title;
use std::time::Duration;

/// Result of DOI validation.
#[derive(Debug, Clone)]
pub struct DoiValidation {
    pub valid: bool,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub error: Option<String>,
}

/// Result of checking DOI match against a reference.
#[derive(Debug, Clone)]
pub enum DoiMatchResult {
    Verified {
        doi_title: String,
        doi_authors: Vec<String>,
    },
    TitleMismatch {
        doi_title: String,
        doi_authors: Vec<String>,
    },
    AuthorMismatch {
        doi_title: String,
        doi_authors: Vec<String>,
    },
    Invalid {
        error: String,
    },
}

/// Validate a DOI by querying doi.org for metadata.
pub async fn validate_doi(
    doi: &str,
    client: &reqwest::Client,
    timeout: Duration,
) -> DoiValidation {
    if doi.is_empty() {
        return DoiValidation {
            valid: false,
            title: None,
            authors: vec![],
            error: Some("No DOI provided".into()),
        };
    }

    let url = format!("https://doi.org/{}", doi);
    let result = client
        .get(&url)
        .header("Accept", "application/vnd.citationstyles.csl+json")
        .header("User-Agent", "HallucinatedReferenceChecker/1.0")
        .timeout(timeout)
        .send()
        .await;

    match result {
        Ok(resp) => {
            if resp.status().is_success() {
                match resp.json::<serde_json::Value>().await {
                    Ok(data) => {
                        let title = match &data["title"] {
                            serde_json::Value::Array(arr) => {
                                arr.first().and_then(|v| v.as_str()).map(String::from)
                            }
                            serde_json::Value::String(s) => Some(s.clone()),
                            _ => None,
                        };

                        let authors: Vec<String> = data["author"]
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|a| {
                                        if let Some(family) = a["family"].as_str() {
                                            let given = a["given"].as_str().unwrap_or("");
                                            Some(
                                                format!("{} {}", given, family)
                                                    .trim()
                                                    .to_string(),
                                            )
                                        } else {
                                            a["literal"].as_str().map(String::from)
                                        }
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();

                        DoiValidation {
                            valid: true,
                            title,
                            authors,
                            error: None,
                        }
                    }
                    Err(e) => DoiValidation {
                        valid: false,
                        title: None,
                        authors: vec![],
                        error: Some(format!("Failed to parse DOI metadata: {}", e)),
                    },
                }
            } else if resp.status().as_u16() == 404 {
                DoiValidation {
                    valid: false,
                    title: None,
                    authors: vec![],
                    error: Some("DOI not found".into()),
                }
            } else {
                DoiValidation {
                    valid: false,
                    title: None,
                    authors: vec![],
                    error: Some(format!("DOI lookup failed: HTTP {}", resp.status())),
                }
            }
        }
        Err(e) => DoiValidation {
            valid: false,
            title: None,
            authors: vec![],
            error: Some(format!("DOI lookup failed: {}", e)),
        },
    }
}

/// Check if DOI metadata matches the reference title and authors.
pub fn check_doi_match(
    doi_result: &DoiValidation,
    ref_title: &str,
    ref_authors: &[String],
) -> DoiMatchResult {
    if !doi_result.valid {
        return DoiMatchResult::Invalid {
            error: doi_result.error.clone().unwrap_or_default(),
        };
    }

    let doi_title = doi_result.title.as_deref().unwrap_or("");
    let doi_authors = &doi_result.authors;

    let ref_norm = normalize_title(ref_title);
    let doi_norm = normalize_title(doi_title);

    // Multiple matching strategies (using only fuzz::ratio + manual checks)
    let title_ratio = rapidfuzz::fuzz::ratio(ref_norm.chars(), doi_norm.chars());

    // Check if DOI title is a prefix of ref title (handles subtitles)
    let is_prefix = doi_norm.len() >= 8 && ref_norm.starts_with(&doi_norm);

    // Check if ref title starts with DOI title and DOI title is reasonably long
    let is_contained_prefix = doi_norm.len() >= 8 && ref_norm.starts_with(&doi_norm);

    // Tool name match: "ReCon: Subtitle" vs "ReCon"
    let is_tool_name_match = if doi_norm.len() >= 4 && ref_title.contains(':') {
        let before_colon = ref_title.split(':').next().unwrap_or("").trim();
        normalize_title(before_colon) == doi_norm
    } else {
        false
    };

    // For longer DOI titles, check if one contains the other
    let is_long_substring_match = doi_norm.len() >= 20
        && (ref_norm.contains(&doi_norm) || doi_norm.contains(&ref_norm));

    let title_match = title_ratio >= 0.95
        || is_prefix
        || is_contained_prefix
        || is_long_substring_match
        || is_tool_name_match;

    if !title_match {
        return DoiMatchResult::TitleMismatch {
            doi_title: doi_title.to_string(),
            doi_authors: doi_authors.clone(),
        };
    }

    // Check author match
    if !ref_authors.is_empty() && !doi_authors.is_empty() {
        if validate_authors(ref_authors, doi_authors) {
            DoiMatchResult::Verified {
                doi_title: doi_title.to_string(),
                doi_authors: doi_authors.clone(),
            }
        } else {
            DoiMatchResult::AuthorMismatch {
                doi_title: doi_title.to_string(),
                doi_authors: doi_authors.clone(),
            }
        }
    } else {
        DoiMatchResult::Verified {
            doi_title: doi_title.to_string(),
            doi_authors: doi_authors.clone(),
        }
    }
}
