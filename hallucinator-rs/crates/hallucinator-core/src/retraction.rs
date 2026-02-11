use crate::matching::titles_match;
use std::time::Duration;

/// Result of a retraction check.
#[derive(Debug, Clone, Default)]
pub struct RetractionResult {
    pub retracted: bool,
    pub retraction_doi: Option<String>,
    pub retraction_type: Option<String>,
    pub error: Option<String>,
}

/// Check if a paper with the given DOI has been retracted via CrossRef.
pub async fn check_retraction(
    doi: &str,
    client: &reqwest::Client,
    timeout: Duration,
    mailto: Option<&str>,
) -> RetractionResult {
    if doi.is_empty() {
        return RetractionResult::default();
    }

    let user_agent = match mailto {
        Some(email) => format!("HallucinatedReferenceChecker/1.0 (mailto:{})", email),
        None => "HallucinatedReferenceChecker/1.0 (mailto:hallucination-checker@example.com)"
            .to_string(),
    };

    let url = format!("https://api.crossref.org/works/{}", doi);
    let resp = match client
        .get(&url)
        .header("User-Agent", &user_agent)
        .timeout(timeout)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return RetractionResult {
                error: Some(format!("Retraction check failed: {}", e)),
                ..Default::default()
            }
        }
    };

    if resp.status().as_u16() == 404 {
        return RetractionResult::default();
    }
    if !resp.status().is_success() {
        return RetractionResult {
            error: Some(format!("CrossRef lookup failed: HTTP {}", resp.status())),
            ..Default::default()
        };
    }

    let data: serde_json::Value = match resp.json().await {
        Ok(d) => d,
        Err(e) => {
            return RetractionResult {
                error: Some(format!("Failed to parse: {}", e)),
                ..Default::default()
            }
        }
    };

    let work = &data["message"];

    // Check update-to relations
    if let Some(updates) = work["update-to"].as_array() {
        for update in updates {
            let update_type = update["type"].as_str().unwrap_or("").to_lowercase();
            if update_type == "retraction" || update_type == "removal" {
                return RetractionResult {
                    retracted: true,
                    retraction_doi: update["DOI"].as_str().map(String::from),
                    retraction_type: Some(
                        update["type"].as_str().unwrap_or("Retraction").to_string(),
                    ),
                    error: None,
                };
            }
        }
    }

    // Check relation field
    let relation = &work["relation"];
    if let Some(retracted_by) = relation["is-retracted-by"].as_array() {
        if let Some(first) = retracted_by.first() {
            return RetractionResult {
                retracted: true,
                retraction_doi: first["id"].as_str().map(String::from),
                retraction_type: Some("Retraction".into()),
                error: None,
            };
        }
    }

    if let Some(concerns) = relation["has-expression-of-concern"].as_array() {
        if let Some(first) = concerns.first() {
            return RetractionResult {
                retracted: true,
                retraction_doi: first["id"].as_str().map(String::from),
                retraction_type: Some("Expression of Concern".into()),
                error: None,
            };
        }
    }

    RetractionResult::default()
}

/// Check if a paper has been retracted by searching CrossRef by title.
pub async fn check_retraction_by_title(
    title: &str,
    client: &reqwest::Client,
    timeout: Duration,
    mailto: Option<&str>,
) -> RetractionResult {
    if title.len() < 10 {
        return RetractionResult::default();
    }

    let url = format!(
        "https://api.crossref.org/works?query.title={}&filter=has-update:true&rows=5",
        urlencoding::encode(title)
    );

    let user_agent = match mailto {
        Some(email) => format!("HallucinatedReferenceChecker/1.0 (mailto:{})", email),
        None => "HallucinatedReferenceChecker/1.0 (mailto:hallucination-checker@example.com)"
            .to_string(),
    };

    let resp = match client
        .get(&url)
        .header("User-Agent", &user_agent)
        .timeout(timeout)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return RetractionResult {
                error: Some(format!("Retraction search failed: {}", e)),
                ..Default::default()
            }
        }
    };

    if !resp.status().is_success() {
        return RetractionResult::default();
    }

    let data: serde_json::Value = match resp.json().await {
        Ok(d) => d,
        Err(_) => return RetractionResult::default(),
    };

    let items = data["message"]["items"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    for item in items {
        let found_title = item["title"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|v| v.as_str())
            .unwrap_or("");

        if !titles_match(title, found_title) {
            continue;
        }

        // Check update-to relations
        if let Some(updates) = item["update-to"].as_array() {
            for update in updates {
                let update_type = update["type"].as_str().unwrap_or("").to_lowercase();
                if update_type == "retraction" || update_type == "removal" {
                    return RetractionResult {
                        retracted: true,
                        retraction_doi: update["DOI"].as_str().map(String::from),
                        retraction_type: Some(
                            update["type"].as_str().unwrap_or("Retraction").to_string(),
                        ),
                        error: None,
                    };
                }
            }
        }

        // Check relation field
        let relation = &item["relation"];
        if let Some(retracted_by) = relation["is-retracted-by"].as_array() {
            if let Some(first) = retracted_by.first() {
                return RetractionResult {
                    retracted: true,
                    retraction_doi: first["id"].as_str().map(String::from),
                    retraction_type: Some("Retraction".into()),
                    error: None,
                };
            }
        }

        if let Some(concerns) = relation["has-expression-of-concern"].as_array() {
            if let Some(first) = concerns.first() {
                return RetractionResult {
                    retracted: true,
                    retraction_doi: first["id"].as_str().map(String::from),
                    retraction_type: Some("Expression of Concern".into()),
                    error: None,
                };
            }
        }
    }

    RetractionResult::default()
}
