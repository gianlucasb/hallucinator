use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use once_cell::sync::Lazy;
use regex::Regex;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct EuropePmc;

impl DatabaseBackend for EuropePmc {
    fn name(&self) -> &str {
        "Europe PMC"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            // Clean title for search
            static SPECIAL: Lazy<Regex> = Lazy::new(|| Regex::new(r#"["\'\[\](){}:;]"#).unwrap());
            let clean_title = SPECIAL.replace_all(title, " ");
            static WS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
            let clean_title = WS.replace_all(&clean_title, " ");
            let query: String = clean_title.chars().take(100).collect();

            let url = "https://www.ebi.ac.uk/europepmc/webservices/rest/search";

            let resp = client
                .get(url)
                .query(&[
                    ("query", query.as_str()),
                    ("format", "json"),
                    ("pageSize", "15"),
                ])
                .header("User-Agent", "Academic Reference Parser")
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            check_rate_limit_response(&resp)?;
            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let data: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;
            let results = data["resultList"]["result"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            for item in results {
                let found_title = item["title"].as_str().unwrap_or("");
                if !found_title.is_empty() && titles_match(title, found_title) {
                    let author_string = item["authorString"].as_str().unwrap_or("");
                    let authors: Vec<String> = if author_string.is_empty() {
                        vec![]
                    } else {
                        author_string
                            .split(',')
                            .map(|a| a.trim().to_string())
                            .filter(|a| !a.is_empty())
                            .collect()
                    };

                    let paper_url = if let Some(doi) = item["doi"].as_str() {
                        Some(format!("https://doi.org/{}", doi))
                    } else if let Some(pmcid) = item["pmcid"].as_str() {
                        Some(format!("https://europepmc.org/article/PMC/{}", pmcid))
                    } else {
                        item["pmid"]
                            .as_str()
                            .map(|pmid| format!("https://europepmc.org/article/MED/{}", pmid))
                    };

                    return Ok(DbQueryResult::found(found_title, authors, paper_url));
                }
            }

            Ok(DbQueryResult::not_found())
        })
    }
}
