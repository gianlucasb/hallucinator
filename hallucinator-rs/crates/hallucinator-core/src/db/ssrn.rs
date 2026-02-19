use super::{DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::rate_limit::check_rate_limit_response;
use crate::text_utils::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct Ssrn;

impl DatabaseBackend for Ssrn {
    fn name(&self) -> &str {
        "SSRN"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async move {
            let words = get_query_words(title, 6);
            let query = words.join(" ");

            let resp = client
                .get("https://papers.ssrn.com/sol3/results.cfm")
                .query(&[("txtKey_Words", query.as_str())])
                .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
                .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8")
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            check_rate_limit_response(&resp)?;
            if !resp.status().is_success() {
                return Err(DbQueryError::Other(format!("HTTP {}", resp.status())));
            }

            let body = resp
                .text()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;
            let title_owned = title.to_string();

            // Parse in spawn_blocking to avoid !Send scraper types
            tokio::task::spawn_blocking(move || parse_ssrn_results(&body, &title_owned))
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?
        })
    }
}

fn parse_ssrn_results(html: &str, title: &str) -> Result<DbQueryResult, DbQueryError> {
    let document = scraper::Html::parse_document(html);
    let title_sel = scraper::Selector::parse("a.title").unwrap();

    for link in document.select(&title_sel).take(10) {
        let found_title: String = link.text().collect();
        let found_title = found_title.trim();
        if !found_title.is_empty() && titles_match(title, found_title) {
            let href = link.value().attr("href").unwrap_or("");
            let paper_url = if href.starts_with("http") {
                Some(href.to_string())
            } else if !href.is_empty() {
                Some(format!("https://papers.ssrn.com{}", href))
            } else {
                None
            };

            // Try to find authors nearby
            let authors = Vec::new();
            // Note: scraper's tree traversal is limited; author extraction
            // from SSRN's complex DOM is best-effort here

            return Ok((Some(found_title.to_string()), authors, paper_url));
        }
    }

    Ok((None, vec![], None))
}
