use super::{ArxivIdQueryResult, DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::text_utils::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct Arxiv;

/// Check arXiv response status, treating 429 and 503 as rate limiting.
/// arXiv returns 503 Service Unavailable when overloaded.
fn check_arxiv_status(resp: &reqwest::Response) -> Result<(), DbQueryError> {
    let status = resp.status().as_u16();
    if status == 429 || status == 503 {
        let retry_after = resp
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs);
        // Default to 3s backoff for arXiv (their API is rate-sensitive)
        Err(DbQueryError::RateLimited {
            retry_after: retry_after.or(Some(Duration::from_secs(3))),
        })
    } else if !resp.status().is_success() {
        Err(DbQueryError::Other(format!("HTTP {}", resp.status())))
    } else {
        Ok(())
    }
}

impl DatabaseBackend for Arxiv {
    fn name(&self) -> &str {
        "arXiv"
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
            let url = format!(
                "https://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results=5",
                urlencoding::encode(&query)
            );

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            check_arxiv_status(&resp)?;

            let body = resp
                .text()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            // Parse Atom XML feed
            parse_arxiv_response(&body, title)
        })
    }

    fn query_arxiv_id<'a>(
        &'a self,
        arxiv_id: &'a str,
        title: &'a str,
        _authors: &'a [String],
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> ArxivIdQueryResult<'a> {
        Box::pin(async move {
            // Use id_list parameter for direct lookup by arXiv ID
            let url = format!(
                "https://export.arxiv.org/api/query?id_list={}&max_results=1",
                urlencoding::encode(arxiv_id)
            );

            let resp = match client.get(&url).timeout(timeout).send().await {
                Ok(r) => r,
                Err(e) => return Some(Err(DbQueryError::Other(e.to_string()))),
            };

            if let Err(e) = check_arxiv_status(&resp) {
                return Some(Err(e));
            }

            let body = match resp.text().await {
                Ok(b) => b,
                Err(e) => return Some(Err(DbQueryError::Other(e.to_string()))),
            };

            // Validate the title at the claimed ID actually matches the citation —
            // fabricated IDs often resolve to unrelated real papers.
            Some(parse_arxiv_id_response(&body, title))
        })
    }
}

/// Parse arXiv Atom XML response for direct ID lookup.
///
/// Returns not-found if the paper at the claimed ID has a title that doesn't
/// match `expected_title`. An arXiv ID that someone fabricated will typically
/// resolve to *some* real paper, so without this check an unrelated paper gets
/// reported as merely an author mismatch rather than a hallucination.
fn parse_arxiv_id_response(xml: &str, expected_title: &str) -> Result<DbQueryResult, DbQueryError> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);

    let mut in_entry = false;
    let mut in_title = false;
    let mut in_author = false;
    let mut in_name = false;

    let mut current_title = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_name = String::new();
    let mut current_link = String::new();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => {
                        in_entry = true;
                        current_title.clear();
                        current_authors.clear();
                        current_link.clear();
                    }
                    b"title" if in_entry => {
                        in_title = true;
                        current_title.clear();
                    }
                    b"author" if in_entry => {
                        in_author = true;
                        current_name.clear();
                    }
                    b"name" if in_author => {
                        in_name = true;
                        current_name.clear();
                    }
                    _ => {}
                }
                // Handle link element (self-closing or not)
                if local.as_ref() == b"link" && in_entry {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" {
                            current_link = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                if e.local_name().as_ref() == b"link" && in_entry {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" && current_link.is_empty() {
                            current_link = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default();
                if in_title && in_entry {
                    current_title.push_str(&text);
                }
                if in_name {
                    current_name.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => {
                        // Return the first entry (direct ID lookup returns exactly one)
                        let entry_title = current_title.trim().to_string();
                        if !entry_title.is_empty()
                            && !current_authors.is_empty()
                            && titles_match(expected_title, &entry_title)
                        {
                            let link = if current_link.is_empty() {
                                None
                            } else {
                                Some(current_link.clone())
                            };
                            return Ok(DbQueryResult::found(
                                entry_title,
                                current_authors.clone(),
                                link,
                            ));
                        }
                        in_entry = false;
                    }
                    b"title" => in_title = false,
                    b"author" => {
                        if !current_name.is_empty() {
                            current_authors.push(current_name.trim().to_string());
                        }
                        in_author = false;
                    }
                    b"name" => in_name = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(DbQueryError::Other(format!("XML parse error: {}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(DbQueryResult::not_found())
}

/// Parse arXiv Atom XML response and find matching entries.
fn parse_arxiv_response(xml: &str, title: &str) -> Result<DbQueryResult, DbQueryError> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);

    let mut in_entry = false;
    let mut in_title = false;
    let mut in_author = false;
    let mut in_name = false;

    let mut current_title = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_name = String::new();
    let mut current_link = String::new();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => {
                        in_entry = true;
                        current_title.clear();
                        current_authors.clear();
                        current_link.clear();
                    }
                    b"title" if in_entry => {
                        in_title = true;
                        current_title.clear();
                    }
                    b"author" if in_entry => {
                        in_author = true;
                        current_name.clear();
                    }
                    b"name" if in_author => {
                        in_name = true;
                        current_name.clear();
                    }
                    _ => {}
                }
                // Handle link element (self-closing or not)
                if local.as_ref() == b"link" && in_entry {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" {
                            current_link = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
            }
            Ok(Event::Empty(ref e)) => {
                if e.local_name().as_ref() == b"link" && in_entry {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"href" && current_link.is_empty() {
                            current_link = String::from_utf8_lossy(&attr.value).to_string();
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = e.unescape().unwrap_or_default();
                if in_title && in_entry {
                    current_title.push_str(&text);
                }
                if in_name {
                    current_name.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => {
                        // Check if this entry matches
                        let entry_title = current_title.trim().to_string();
                        if titles_match(title, &entry_title) {
                            // Skip results with empty authors - let other DBs verify
                            if !current_authors.is_empty() {
                                let link = if current_link.is_empty() {
                                    None
                                } else {
                                    Some(current_link.clone())
                                };
                                return Ok(DbQueryResult::found(
                                    entry_title,
                                    current_authors.clone(),
                                    link,
                                ));
                            }
                        }
                        in_entry = false;
                    }
                    b"title" => in_title = false,
                    b"author" => {
                        if !current_name.is_empty() {
                            current_authors.push(current_name.trim().to_string());
                        }
                        in_author = false;
                    }
                    b"name" => in_name = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(DbQueryError::Other(format!("XML parse error: {}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(DbQueryResult::not_found())
}
