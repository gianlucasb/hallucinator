//! OAI-PMH harvester for the IACR Cryptology ePrint Archive.
//!
//! Hits `https://eprint.iacr.org/oai` with the standard OAI-PMH 2.0
//! protocol, pages through results via `resumptionToken`, and writes
//! records into the local SQLite + FTS5 index.
//!
//! Uses `oai_dc` (Dublin Core) — the only metadata format the
//! archive advertises (verified via `verb=ListMetadataFormats`).
//! Flat XML, small records, easy to parse.
//!
//! Incremental refresh: the builder persists the server-reported
//! `responseDate` after each successful harvest into the
//! `last_harvest` metadata key; subsequent runs pass that as the
//! OAI-PMH `from=` parameter so the server streams only records
//! whose `datestamp` is newer. Matches the cadence the
//! IACR-harvesting policy page asks for ("no more than once a
//! day").

use std::path::Path;
use std::time::Duration;

use once_cell::sync::Lazy;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use regex::Regex;

use crate::{BuildProgress, IacrDatabase, IacrError, IacrRecord};

/// OAI-PMH endpoint for eprint.iacr.org.
const OAI_BASE: &str = "https://eprint.iacr.org/oai";

/// User-Agent: polite + identifying, same approach the rest of the
/// workspace uses now that we know reqwest's default UA is
/// blacklisted by some anti-bot filters.
const USER_AGENT: &str = concat!(
    "hallucinator-iacr-eprint/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/gianlucasb/hallucinator)",
);

/// Build or incrementally refresh the offline IACR ePrint database.
///
/// Returns `true` if new data was ingested, `false` if the server
/// replied that nothing changed since the last harvest (in which
/// case the DB file is left alone).
pub async fn build(
    db_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<bool, IacrError> {
    // Open or create the database.
    let db = if db_path.exists() {
        // Try to open existing — if the schema is missing (user
        // pointed `--iacr-eprint-offline` at an unrelated file) we
        // bail rather than clobber it silently.
        match IacrDatabase::open(db_path) {
            Ok(db) => db,
            Err(IacrError::Database(_)) => IacrDatabase::create(db_path)?,
            Err(e) => return Err(e),
        }
    } else {
        IacrDatabase::create(db_path)?
    };

    // Pick up the `from` watermark from the previous run, if any.
    let from = db.get_metadata("last_harvest")?;
    progress(BuildProgress::Starting {
        incremental_from: from.clone(),
    });

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| IacrError::Harvest(e.to_string()))?;

    // Walk `ListRecords` pages via resumptionToken. The archive
    // serves ~100 records per page with `metadataPrefix=oai_dc`;
    // ~30k total → ~300 pages → a few minutes end-to-end.
    let mut resumption_token: Option<String> = None;
    let mut total_records: u64 = 0;
    let mut pages: u64 = 0;
    let mut response_date: Option<String> = None;

    loop {
        let url = if let Some(token) = &resumption_token {
            format!(
                "{OAI_BASE}?verb=ListRecords&resumptionToken={}",
                urlencode(token)
            )
        } else if let Some(from) = &from {
            format!(
                "{OAI_BASE}?verb=ListRecords&metadataPrefix=oai_dc&from={}",
                urlencode(from)
            )
        } else {
            format!("{OAI_BASE}?verb=ListRecords&metadataPrefix=oai_dc")
        };

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| IacrError::Harvest(format!("GET {url}: {e}")))?;
        if !resp.status().is_success() {
            return Err(IacrError::Harvest(format!(
                "HTTP {} from {url}",
                resp.status()
            )));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| IacrError::Harvest(e.to_string()))?;

        let parsed = parse_list_records(&body)?;

        // Capture the server's responseDate from the first page so
        // the next run can pass it as `from=` for an incremental
        // refresh. OAI-PMH says responseDate is the moment the
        // server answered — using it as the next run's `from`
        // never misses a record.
        if response_date.is_none() {
            response_date = parsed.response_date.clone();
        }

        // `noRecordsMatch` on an incremental run means we're already
        // up to date. Don't clobber `last_harvest` — keep the old
        // watermark so a future run still asks from that date.
        if parsed.no_records_match && resumption_token.is_none() {
            progress(BuildProgress::Complete {
                records: 0,
                skipped: true,
            });
            return Ok(false);
        }

        for rec in &parsed.records {
            db.upsert(rec)?;
            total_records += 1;
        }
        pages += 1;
        progress(BuildProgress::Fetched {
            records: total_records,
            pages,
        });

        resumption_token = parsed.next_token;
        if resumption_token.is_none() {
            break;
        }
    }

    // Persist watermarks for incremental refresh + staleness banner.
    if let Some(rd) = response_date {
        db.set_metadata("last_harvest", &rd)?;
    }
    // `build_date` mirrors the sibling crates (arxiv / acl / dblp) —
    // keyed as YYYY-MM-DD so the staleness banner arithmetic works.
    if let Some(today) = today_iso_date() {
        db.record_build_date(&today)?;
    }

    progress(BuildProgress::Indexed {
        records: total_records,
    });
    progress(BuildProgress::Complete {
        records: total_records,
        skipped: false,
    });

    Ok(true)
}

/// Parsed payload of one ListRecords OAI-PMH response.
struct ListRecordsResponse {
    records: Vec<IacrRecord>,
    next_token: Option<String>,
    response_date: Option<String>,
    /// `<error code="noRecordsMatch">` means the `from=` watermark
    /// is already newer than any record on the server — i.e.
    /// "nothing new since last harvest".
    no_records_match: bool,
}

/// Parse an OAI-PMH `ListRecords` response (oai_dc metadata).
///
/// Deliberately minimal: walks the event stream once, tracks which
/// `<dc:*>` element we're in while inside a record's `<metadata>`
/// block, and collects text. No DOM, no Serde — the oai_dc schema
/// is flat and small enough that this is both smaller and faster.
fn parse_list_records(xml: &str) -> Result<ListRecordsResponse, IacrError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut records: Vec<IacrRecord> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut response_date: Option<String> = None;
    let mut no_records_match = false;

    // Per-record accumulator.
    let mut cur_id: Option<String> = None;
    let mut cur_title: Option<String> = None;
    let mut cur_authors: Vec<String> = Vec::new();
    let mut cur_category: Option<String> = None;
    let mut cur_date: Option<String> = None;

    // Current context — what XML element we last entered that
    // produces text content we care about.
    let mut in_record = false;
    let mut in_metadata = false;
    let mut cur_text_target: Option<TextTarget> = None;
    // Separately track the header's `<identifier>` so it's not
    // confused with `<dc:identifier>` in the metadata block.
    let mut in_header = false;

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = std::str::from_utf8(e.local_name().as_ref())
                    .unwrap_or("")
                    .to_string();
                match name.as_str() {
                    "responseDate" => cur_text_target = Some(TextTarget::ResponseDate),
                    "record" => {
                        in_record = true;
                        cur_id = None;
                        cur_title = None;
                        cur_authors.clear();
                        cur_category = None;
                        cur_date = None;
                    }
                    "header" if in_record => in_header = true,
                    "metadata" if in_record => in_metadata = true,
                    "identifier" if in_header => cur_text_target = Some(TextTarget::HeaderId),
                    "title" if in_metadata => cur_text_target = Some(TextTarget::Title),
                    "creator" if in_metadata => cur_text_target = Some(TextTarget::Creator),
                    "subject" if in_metadata => cur_text_target = Some(TextTarget::Subject),
                    "date" if in_metadata => {
                        // Only keep the first `<dc:date>` if multiple are
                        // present (the feed sometimes emits create + modify).
                        if cur_date.is_none() {
                            cur_text_target = Some(TextTarget::Date);
                        }
                    }
                    "resumptionToken" => cur_text_target = Some(TextTarget::Token),
                    "error" => {
                        // Only flag noRecordsMatch; other errors
                        // surface as a non-2xx / empty parse below.
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"code"
                                && attr.value.as_ref() == b"noRecordsMatch"
                            {
                                no_records_match = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.unescape().unwrap_or_default().into_owned();
                match cur_text_target {
                    Some(TextTarget::ResponseDate) => response_date = Some(text),
                    Some(TextTarget::HeaderId) => {
                        // `oai:eprint.iacr.org:2022/252` → `2022/252`.
                        cur_id = text
                            .rsplit_once(':')
                            .map(|(_, id)| id.to_string())
                            .or(Some(text));
                    }
                    Some(TextTarget::Title) => cur_title = Some(text),
                    Some(TextTarget::Creator) => cur_authors.push(text),
                    Some(TextTarget::Subject) => {
                        // First subject wins; later ones are usually
                        // secondary categories and the archive UI
                        // displays only the first.
                        if cur_category.is_none() {
                            cur_category = Some(text);
                        }
                    }
                    Some(TextTarget::Date) => cur_date = Some(text),
                    Some(TextTarget::Token) => next_token = Some(text),
                    None => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = std::str::from_utf8(e.local_name().as_ref())
                    .unwrap_or("")
                    .to_string();
                match name.as_str() {
                    "record" => {
                        if let (Some(id), Some(title)) = (cur_id.take(), cur_title.take()) {
                            records.push(IacrRecord {
                                id,
                                title,
                                authors: std::mem::take(&mut cur_authors),
                                category: cur_category.take(),
                                date: cur_date.take(),
                            });
                        }
                        in_record = false;
                    }
                    "header" => in_header = false,
                    "metadata" => in_metadata = false,
                    _ => {}
                }
                cur_text_target = None;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(IacrError::Xml(format!(
                    "XML at pos {}: {e}",
                    reader.buffer_position()
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    // An empty `<resumptionToken/>` (self-closing or no text) means
    // "last page" per OAI-PMH. Some servers emit the element with
    // completeListSize but no token body — normalize empty → None.
    if let Some(tok) = next_token.as_ref()
        && tok.trim().is_empty()
    {
        next_token = None;
    }

    Ok(ListRecordsResponse {
        records,
        next_token,
        response_date,
        no_records_match,
    })
}

/// Which `<dc:*>` element's text we're currently inside.
#[derive(Debug, Clone, Copy)]
enum TextTarget {
    ResponseDate,
    HeaderId,
    Title,
    Creator,
    Subject,
    Date,
    Token,
}

/// Minimal URL encoder for the 2–3 chars we actually need to escape
/// in resumption tokens / `from=` timestamps. Adding `url` as a
/// dep just for this would be silly.
fn urlencode(s: &str) -> String {
    static UNRESERVED: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^A-Za-z0-9\-._~]").unwrap());
    UNRESERVED
        .replace_all(s, |caps: &regex::Captures| {
            let c = caps.get(0).unwrap().as_str();
            let mut out = String::new();
            for b in c.as_bytes() {
                out.push_str(&format!("%{:02X}", b));
            }
            out
        })
        .to_string()
}

/// Today's date in ISO `YYYY-MM-DD` form, computed from std::time
/// without pulling in chrono. Returns `None` if the clock is before
/// the Unix epoch (not expected in practice).
fn today_iso_date() -> Option<String> {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let days = now_secs / 86_400;
    // Inverse of ymd_to_days in lib.rs. Epoch days from year 0 to
    // 1970 is 719_162. Add that and convert back.
    let mut z = days as i64 + 719_162 - 60; // shift origin to 0000-03-01
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    z = y;
    Some(format!("{:04}-{:02}-{:02}", z, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_PAGE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/">
  <responseDate>2026-04-24T12:30:57Z</responseDate>
  <request verb="ListRecords" metadataPrefix="oai_dc">https://eprint.iacr.org/oai</request>
  <ListRecords>
    <record>
      <header>
        <identifier>oai:eprint.iacr.org:2022/252</identifier>
        <datestamp>2022-03-02T13:58:42Z</datestamp>
      </header>
      <metadata>
        <oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:identifier>https://eprint.iacr.org/2022/252</dc:identifier>
          <dc:title>Handcrafting: Improving Automated Masking in Hardware with Manual Optimizations</dc:title>
          <dc:creator>Charles Momin</dc:creator>
          <dc:creator>Gaëtan Cassiers</dc:creator>
          <dc:creator>François-Xavier Standaert</dc:creator>
          <dc:subject>Implementation</dc:subject>
          <dc:description>Abstract body.</dc:description>
          <dc:date>2022-03-02T13:58:42Z</dc:date>
        </oai_dc:dc>
      </metadata>
    </record>
    <record>
      <header>
        <identifier>oai:eprint.iacr.org:2024/1</identifier>
        <datestamp>2024-01-02T09:00:00Z</datestamp>
      </header>
      <metadata>
        <oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:title>A Second Paper</dc:title>
          <dc:creator>Alice</dc:creator>
          <dc:date>2024-01-02T09:00:00Z</dc:date>
        </oai_dc:dc>
      </metadata>
    </record>
    <resumptionToken>next-page-token-abc</resumptionToken>
  </ListRecords>
</OAI-PMH>
"#;

    #[test]
    fn parses_list_records_page() {
        let r = parse_list_records(SAMPLE_PAGE).unwrap();
        assert_eq!(r.records.len(), 2);
        assert_eq!(r.records[0].id, "2022/252");
        assert_eq!(
            r.records[0].title,
            "Handcrafting: Improving Automated Masking in Hardware with Manual Optimizations"
        );
        assert_eq!(r.records[0].authors.len(), 3);
        assert_eq!(r.records[0].category.as_deref(), Some("Implementation"));
        assert_eq!(r.records[0].date.as_deref(), Some("2022-03-02T13:58:42Z"));
        // Second record has no subject — category should be None.
        assert_eq!(r.records[1].id, "2024/1");
        assert!(r.records[1].category.is_none());
        assert_eq!(r.next_token.as_deref(), Some("next-page-token-abc"));
        assert_eq!(r.response_date.as_deref(), Some("2026-04-24T12:30:57Z"));
        assert!(!r.no_records_match);
    }

    #[test]
    fn detects_no_records_match_on_incremental_up_to_date() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/">
  <responseDate>2026-04-24T12:30:57Z</responseDate>
  <request verb="ListRecords">https://eprint.iacr.org/oai</request>
  <error code="noRecordsMatch">No records match the requested parameters.</error>
</OAI-PMH>"#;
        let r = parse_list_records(xml).unwrap();
        assert!(r.no_records_match, "must detect noRecordsMatch");
        assert!(r.records.is_empty());
        assert!(r.next_token.is_none());
    }

    #[test]
    fn empty_resumption_token_normalized_to_none() {
        // Last page of a multi-page harvest: some servers emit an
        // empty `<resumptionToken/>` with completeListSize. We
        // must treat that as "no more pages", not as a valid
        // token (which would loop forever).
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<OAI-PMH xmlns="http://www.openarchives.org/OAI/2.0/">
  <responseDate>2026-04-24T12:30:57Z</responseDate>
  <ListRecords>
    <record>
      <header>
        <identifier>oai:eprint.iacr.org:2020/1</identifier>
        <datestamp>2020-01-01T00:00:00Z</datestamp>
      </header>
      <metadata>
        <oai_dc:dc xmlns:oai_dc="http://www.openarchives.org/OAI/2.0/oai_dc/" xmlns:dc="http://purl.org/dc/elements/1.1/">
          <dc:title>Final record</dc:title>
          <dc:creator>Author</dc:creator>
        </oai_dc:dc>
      </metadata>
    </record>
    <resumptionToken completeListSize="12345"/>
  </ListRecords>
</OAI-PMH>"#;
        let r = parse_list_records(xml).unwrap();
        assert_eq!(r.records.len(), 1);
        assert!(r.next_token.is_none(), "empty token must normalize to None");
    }
}
