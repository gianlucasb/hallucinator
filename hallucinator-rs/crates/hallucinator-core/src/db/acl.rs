use super::{DatabaseBackend, DbQueryResult};
use crate::matching::titles_match;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct AclAnthology;

impl DatabaseBackend for AclAnthology {
    fn name(&self) -> &str {
        "ACL Anthology"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!(
                "https://aclanthology.org/search/?q={}",
                urlencoding::encode(title)
            );

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            let status = resp.status();
            if status.as_u16() == 429 {
                return Err("Rate limited (429)".into());
            }
            if !status.is_success() {
                return Err(format!("HTTP {}", status));
            }

            let body = resp.text().await.map_err(|e| e.to_string())?;
            let title_owned = title.to_string();

            // Parse in spawn_blocking to avoid !Send scraper types
            tokio::task::spawn_blocking(move || parse_acl_results(&body, &title_owned))
                .await
                .map_err(|e| e.to_string())?
        })
    }
}

fn parse_acl_results(html: &str, title: &str) -> Result<DbQueryResult, String> {
    let document = scraper::Html::parse_document(html);

    let entry_sel = scraper::Selector::parse(".d-sm-flex.align-items-stretch.p-2").unwrap();
    let title_sel = scraper::Selector::parse("h5").unwrap();
    let author_sel = scraper::Selector::parse("span.badge.badge-light").unwrap();
    let link_sel = scraper::Selector::parse("a[href*='/papers/']").unwrap();

    for entry in document.select(&entry_sel) {
        if let Some(title_el) = entry.select(&title_sel).next() {
            let found_title: String = title_el.text().collect();
            if titles_match(title, &found_title) {
                let authors: Vec<String> = entry
                    .select(&author_sel)
                    .map(|a| a.text().collect::<String>().trim().to_string())
                    .collect();

                let paper_url = entry
                    .select(&link_sel)
                    .next()
                    .and_then(|a| a.value().attr("href"))
                    .map(|href| format!("https://aclanthology.org{}", href));

                return Ok((Some(found_title.trim().to_string()), authors, paper_url));
            }
        }
    }

    Ok((None, vec![], None))
}
