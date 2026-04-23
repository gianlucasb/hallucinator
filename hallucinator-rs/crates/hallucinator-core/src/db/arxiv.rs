use super::{ArxivIdQueryResult, DatabaseBackend, DbQueryError, DbQueryResult};
use crate::matching::titles_match;
use crate::text_utils::get_query_words;
use once_cell::sync::Lazy;
use regex::Regex;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// Parsed subset of one `<entry>` block from an arXiv Atom XML response.
#[derive(Debug, Clone)]
struct ArxivEntry {
    title: String,
    authors: Vec<String>,
    link: Option<String>,
    /// Version number parsed from the entry's `<id>` URL suffix, e.g.,
    /// `2` for `http://arxiv.org/abs/2403.00108v2`. `None` on old-format
    /// IDs without a version suffix, or when `<id>` was missing.
    version: Option<u32>,
}

/// Match a trailing `v\d+` version suffix on an arXiv ID.
static ARXIV_VERSION_SUFFIX: Lazy<Regex> = Lazy::new(|| Regex::new(r"v(\d+)$").unwrap());

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
            // Step 1: fetch the latest version (what `id_list=<id>` without
            // a version suffix resolves to). This is what the old code
            // did, and for papers whose title didn't change across
            // versions it's still the happy path.
            let latest_entries = match fetch_arxiv_id_entries(arxiv_id, client, timeout).await {
                Ok(e) => e,
                Err(e) => return Some(Err(e)),
            };
            let latest = latest_entries.into_iter().next();

            if let Some(entry) = &latest
                && !entry.authors.is_empty()
                && titles_match(title, &entry.title)
            {
                return Some(Ok(DbQueryResult::found(
                    entry.title.clone(),
                    entry.authors.clone(),
                    entry.link.clone(),
                )));
            }

            // Step 2: arXiv papers sometimes get retitled between
            // versions. A citation like `arXiv:2403.00108` (no explicit
            // version) typically captures the title as it was at
            // submission time (v1), which may differ from the current
            // latest-version title. Example: NDSS 2026 paper f168 ref 8
            // cites "Lora-as-an-attack! piercing llm safety under the
            // share-and-play scenario" (v1), but arXiv 2403.00108's
            // latest version is titled "LoRATK: LoRA Once, Backdoor
            // Everywhere in the Share-and-Play Ecosystem". Walk earlier
            // versions when the latest doesn't match.
            //
            // Skip the fallback when:
            //   * the citation already specifies an explicit version
            //     (the user chose v1, honor that — don't fall back to
            //     "any version with a matching title"), or
            //   * the latest version is v1 (nothing earlier to try), or
            //   * we couldn't parse the latest version number from the
            //     Atom `<id>` URL (unexpected response shape).
            if ARXIV_VERSION_SUFFIX.is_match(arxiv_id) {
                return Some(Ok(DbQueryResult::not_found()));
            }
            let Some(latest_version) = latest.as_ref().and_then(|e| e.version) else {
                return Some(Ok(DbQueryResult::not_found()));
            };
            if latest_version < 2 {
                return Some(Ok(DbQueryResult::not_found()));
            }

            // Batch-fetch v1..v{latest-1} in one id_list call (arXiv API
            // accepts comma-separated IDs). This keeps the extra cost at
            // exactly one additional request even when a paper has many
            // versions, so we don't multiply rate-limit pressure.
            let older_ids: Vec<String> = (1..latest_version)
                .map(|v| format!("{}v{}", arxiv_id, v))
                .collect();
            let joined = older_ids.join(",");
            let older_entries = match fetch_arxiv_id_entries(&joined, client, timeout).await {
                Ok(e) => e,
                Err(_) => return Some(Ok(DbQueryResult::not_found())),
            };
            for entry in older_entries {
                if !entry.authors.is_empty() && titles_match(title, &entry.title) {
                    return Some(Ok(DbQueryResult::found(
                        entry.title,
                        entry.authors,
                        entry.link,
                    )));
                }
            }

            Some(Ok(DbQueryResult::not_found()))
        })
    }
}

/// Fetch one or more arXiv IDs via the `id_list` endpoint and parse the
/// Atom XML response into a list of entries.
///
/// Accepts a comma-separated list (the arXiv API natively supports
/// `id_list=id1,id2,id3`), so callers can batch version lookups into a
/// single request.
async fn fetch_arxiv_id_entries(
    id_list: &str,
    client: &reqwest::Client,
    timeout: Duration,
) -> Result<Vec<ArxivEntry>, DbQueryError> {
    let url = format!(
        "https://export.arxiv.org/api/query?id_list={}&max_results=10",
        urlencoding::encode(id_list)
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
    parse_arxiv_id_entries(&body)
}

/// Parse an arXiv Atom XML `id_list` response into one entry per
/// returned paper. Title-matching is deliberately left to the caller —
/// a single response may carry multiple versions of the same paper, and
/// which version we consider "the match" depends on caller intent.
fn parse_arxiv_id_entries(xml: &str) -> Result<Vec<ArxivEntry>, DbQueryError> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);

    let mut in_entry = false;
    let mut in_title = false;
    let mut in_author = false;
    let mut in_name = false;
    let mut in_id = false;

    let mut current_title = String::new();
    let mut current_authors: Vec<String> = Vec::new();
    let mut current_name = String::new();
    let mut current_link = String::new();
    let mut current_id = String::new();

    let mut out: Vec<ArxivEntry> = Vec::new();
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
                        current_id.clear();
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
                    b"id" if in_entry => {
                        in_id = true;
                        current_id.clear();
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
                if in_id {
                    current_id.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => {
                        let title = current_title.trim().to_string();
                        if !title.is_empty() {
                            let link = if current_link.is_empty() {
                                None
                            } else {
                                Some(current_link.clone())
                            };
                            let version = ARXIV_VERSION_SUFFIX
                                .captures(current_id.trim())
                                .and_then(|c| c.get(1))
                                .and_then(|m| m.as_str().parse::<u32>().ok());
                            out.push(ArxivEntry {
                                title,
                                authors: std::mem::take(&mut current_authors),
                                link,
                                version,
                            });
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
                    b"id" => in_id = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(DbQueryError::Other(format!("XML parse error: {}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn one_entry_xml(id_url: &str, title: &str, authors: &[&str]) -> String {
        let authors_xml: String = authors
            .iter()
            .map(|a| format!("<author><name>{}</name></author>", a))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>{id_url}</id>
    <title>{title}</title>
    {authors_xml}
    <link href="{id_url}" rel="alternate" type="text/html"/>
  </entry>
</feed>"#
        )
    }

    #[test]
    fn parses_single_entry_with_version() {
        // Typical id_list=<bare-id> response: one <entry> whose <id>
        // carries the latest version suffix.
        let xml = one_entry_xml(
            "http://arxiv.org/abs/2403.00108v2",
            "LoRATK: LoRA Once, Backdoor Everywhere in the Share-and-Play Ecosystem",
            &["Hongyi Liu", "Shaochen Zhong"],
        );
        let entries = parse_arxiv_id_entries(&xml).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].title,
            "LoRATK: LoRA Once, Backdoor Everywhere in the Share-and-Play Ecosystem"
        );
        assert_eq!(entries[0].authors, vec!["Hongyi Liu", "Shaochen Zhong"]);
        assert_eq!(entries[0].version, Some(2));
        assert_eq!(
            entries[0].link.as_deref(),
            Some("http://arxiv.org/abs/2403.00108v2")
        );
    }

    #[test]
    fn parses_multiple_entries_for_batched_id_list() {
        // When we issue `id_list=X v1, X v2, X v3`, the response is a
        // multi-entry feed. The parser must return all entries in order
        // so the caller can scan for a title match across versions.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <entry>
    <id>http://arxiv.org/abs/2403.00108v1</id>
    <title>Lora-as-an-attack! piercing llm safety under the share-and-play scenario</title>
    <author><name>Hongyi Liu</name></author>
    <link href="http://arxiv.org/abs/2403.00108v1" rel="alternate"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2403.00108v2</id>
    <title>LoRATK: LoRA Once, Backdoor Everywhere in the Share-and-Play Ecosystem</title>
    <author><name>Hongyi Liu</name></author>
    <link href="http://arxiv.org/abs/2403.00108v2" rel="alternate"/>
  </entry>
</feed>"#;
        let entries = parse_arxiv_id_entries(xml).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].title.starts_with("Lora-as-an-attack"));
        assert_eq!(entries[0].version, Some(1));
        assert!(entries[1].title.starts_with("LoRATK:"));
        assert_eq!(entries[1].version, Some(2));
    }

    #[test]
    fn version_none_for_id_without_suffix() {
        // Old-format or otherwise version-less `<id>` URL — the caller
        // then skips the earlier-versions fallback because it doesn't
        // know how many to try.
        let xml = one_entry_xml(
            "http://arxiv.org/abs/hep-th/9901001",
            "Some old paper",
            &["A. Einstein"],
        );
        let entries = parse_arxiv_id_entries(&xml).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].version, None);
    }

    #[test]
    fn arxiv_id_explicit_version_suffix_detected() {
        // `ARXIV_VERSION_SUFFIX` gates whether the fallback fires:
        // citations that already pinned a version shouldn't fall back to
        // "any version", and old-format IDs don't carry a suffix at all.
        assert!(ARXIV_VERSION_SUFFIX.is_match("2403.00108v1"));
        assert!(ARXIV_VERSION_SUFFIX.is_match("2403.00108v42"));
        assert!(ARXIV_VERSION_SUFFIX.is_match("hep-th/9901001v1"));
        assert!(!ARXIV_VERSION_SUFFIX.is_match("2403.00108"));
        assert!(!ARXIV_VERSION_SUFFIX.is_match("hep-th/9901001"));
    }

    #[test]
    fn returns_empty_on_no_matches_feed() {
        // Arxiv returns a <feed> with no <entry> when the id_list
        // doesn't resolve (e.g., all fabricated IDs). Parser must not
        // panic.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>ArXiv Query: search_query=&amp;id_list=2403.99999</title>
  <id>http://arxiv.org/api/fake</id>
</feed>"#;
        let entries = parse_arxiv_id_entries(xml).unwrap();
        assert!(entries.is_empty());
    }
}
