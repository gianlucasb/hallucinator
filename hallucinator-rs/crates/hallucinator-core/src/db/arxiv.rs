use super::{DatabaseBackend, DbQueryResult};
use crate::matching::titles_match;
use hallucinator_pdf::identifiers::get_query_words;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub struct Arxiv;

impl DatabaseBackend for Arxiv {
    fn name(&self) -> &str {
        "arXiv"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, String>> + Send + 'a>> {
        Box::pin(async move {
            let words = get_query_words(title, 6);
            let query = words.join(" ");
            let url = format!(
                "http://export.arxiv.org/api/query?search_query=all:{}&start=0&max_results=5",
                urlencoding::encode(&query)
            );

            let resp = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| e.to_string())?;

            if !resp.status().is_success() {
                return Err(format!("HTTP {}", resp.status()));
            }

            let body = resp.text().await.map_err(|e| e.to_string())?;

            // Parse Atom XML feed
            parse_arxiv_response(&body, title)
        })
    }
}

/// Parse arXiv Atom XML response and find matching entries.
fn parse_arxiv_response(xml: &str, title: &str) -> Result<DbQueryResult, String> {
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
                            let link = if current_link.is_empty() {
                                None
                            } else {
                                Some(current_link.clone())
                            };
                            return Ok((Some(entry_title), current_authors.clone(), link));
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
            Err(e) => return Err(format!("XML parse error: {}", e)),
            _ => {}
        }
        buf.clear();
    }

    Ok((None, vec![], None))
}
