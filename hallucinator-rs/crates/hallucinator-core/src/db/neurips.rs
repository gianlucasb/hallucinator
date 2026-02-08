use super::{DatabaseBackend, DbQueryResult};
use crate::matching::titles_match;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct NeurIPS;

impl DatabaseBackend for NeurIPS {
    fn name(&self) -> &str {
        "NeurIPS"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let years = [2023, 2022, 2021, 2020, 2019, 2018];
            let title_owned = title.to_string();

            for year in years {
                let url = format!(
                    "https://papers.nips.cc/paper_files/paper/{}/hash/index.html",
                    year
                );

                let resp = client
                    .get(&url)
                    .timeout(timeout)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;

                if !resp.status().is_success() {
                    continue;
                }

                let body = resp.text().await.map_err(|e| e.to_string())?;

                // Parse in spawn_blocking to avoid !Send scraper types in async context
                let title_clone = title_owned.clone();
                let match_result = tokio::task::spawn_blocking(move || {
                    parse_neurips_index(&body, &title_clone)
                })
                .await
                .map_err(|e| e.to_string())?;

                if let Some((found_title, href)) = match_result {
                    let paper_url = format!("https://papers.nips.cc{}", href);

                    // Fetch author page
                    let authors = match client.get(&paper_url).timeout(timeout).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            let body = resp.text().await.unwrap_or_default();
                            tokio::task::spawn_blocking(move || parse_neurips_authors(&body))
                                .await
                                .unwrap_or_default()
                        }
                        _ => vec![],
                    };

                    return Ok((Some(found_title), authors, Some(paper_url)));
                }
            }

            Ok((None, vec![], None))
        })
    }
}

fn parse_neurips_index(html: &str, title: &str) -> Option<(String, String)> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("a").unwrap();

    for element in document.select(&selector) {
        let link_text = element.text().collect::<String>();
        if titles_match(title, &link_text) {
            let href = element.value().attr("href").unwrap_or("").to_string();
            return Some((link_text.trim().to_string(), href));
        }
    }
    None
}

fn parse_neurips_authors(html: &str) -> Vec<String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("li.author").unwrap();

    document
        .select(&selector)
        .map(|el| el.text().collect::<String>().trim().to_string())
        .collect()
}
