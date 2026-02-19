//! S3 listing and streaming download for the public OpenAlex bucket.
//!
//! Uses raw reqwest against the public bucket — no AWS SDK needed.

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::OpenAlexError;

const BUCKET_URL: &str = "https://openalex.s3.us-east-1.amazonaws.com";

/// A date partition in the OpenAlex S3 bucket (e.g., `data/works/updated_date=2025-01-15/`).
#[derive(Debug, Clone)]
pub struct DatePartition {
    pub prefix: String,
    pub date: String,
}

/// A gzip file within a partition.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct PartitionFile {
    pub key: String,
    pub size: u64,
}

/// List all date partitions under `data/works/`.
pub async fn list_date_partitions(
    client: &reqwest::Client,
) -> Result<Vec<DatePartition>, OpenAlexError> {
    let mut partitions = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let mut url = format!("{}/?list-type=2&prefix=data/works/&delimiter=/", BUCKET_URL);
        if let Some(ref token) = continuation_token {
            url.push_str(&format!(
                "&continuation-token={}",
                urlencoding::encode(token)
            ));
        }

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| OpenAlexError::Download(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(OpenAlexError::Download(format!(
                "S3 list failed: HTTP {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| OpenAlexError::Download(e.to_string()))?;

        let (new_partitions, next_token) = parse_partition_list_xml(&body)?;
        partitions.extend(new_partitions);

        if let Some(token) = next_token {
            continuation_token = Some(token);
        } else {
            break;
        }
    }

    partitions.sort_by(|a, b| a.date.cmp(&b.date));
    Ok(partitions)
}

/// List all `.gz` files within a partition prefix.
pub async fn list_partition_files(
    client: &reqwest::Client,
    prefix: &str,
) -> Result<Vec<PartitionFile>, OpenAlexError> {
    let mut files = Vec::new();
    let mut continuation_token: Option<String> = None;

    loop {
        let mut url = format!("{}/?list-type=2&prefix={}", BUCKET_URL, prefix);
        if let Some(ref token) = continuation_token {
            url.push_str(&format!(
                "&continuation-token={}",
                urlencoding::encode(token)
            ));
        }

        let resp = client
            .get(&url)
            .send()
            .await
            .map_err(|e| OpenAlexError::Download(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(OpenAlexError::Download(format!(
                "S3 list files failed: HTTP {}",
                resp.status()
            )));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| OpenAlexError::Download(e.to_string()))?;

        let (new_files, next_token) = parse_file_list_xml(&body)?;
        files.extend(new_files);

        if let Some(token) = next_token {
            continuation_token = Some(token);
        } else {
            break;
        }
    }

    Ok(files)
}

/// Download a gzipped file and return the raw bytes.
pub async fn download_gz(client: &reqwest::Client, key: &str) -> Result<Vec<u8>, OpenAlexError> {
    let url = format!("{}/{}", BUCKET_URL, key);
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| OpenAlexError::Download(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(OpenAlexError::Download(format!(
            "S3 download failed for {}: HTTP {}",
            key,
            resp.status()
        )));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| OpenAlexError::Download(e.to_string()))?;
    Ok(bytes.to_vec())
}

/// URL-encode a continuation token for S3 queries.
mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::with_capacity(s.len());
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    result.push(b as char);
                }
                _ => {
                    result.push_str(&format!("%{:02X}", b));
                }
            }
        }
        result
    }
}

// ── XML Parsing ──────────────────────────────────────────────────────────

fn parse_partition_list_xml(
    xml: &str,
) -> Result<(Vec<DatePartition>, Option<String>), OpenAlexError> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut partitions = Vec::new();
    let mut next_token = None;
    let mut in_prefix = false;
    let mut in_next_token = false;
    let mut current_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "Prefix" => {
                        in_prefix = true;
                        current_text.clear();
                    }
                    "NextContinuationToken" => {
                        in_next_token = true;
                        current_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if (in_prefix || in_next_token)
                    && let Ok(text) = e.unescape()
                {
                    current_text.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "Prefix" => {
                        if in_prefix {
                            if let Some(date) = extract_date_from_prefix(&current_text) {
                                partitions.push(DatePartition {
                                    prefix: current_text.clone(),
                                    date,
                                });
                            }
                            in_prefix = false;
                        }
                    }
                    "NextContinuationToken" => {
                        if in_next_token {
                            next_token = Some(current_text.clone());
                            in_next_token = false;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OpenAlexError::Parse(format!("XML parse error: {}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok((partitions, next_token))
}

fn parse_file_list_xml(xml: &str) -> Result<(Vec<PartitionFile>, Option<String>), OpenAlexError> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut files = Vec::new();
    let mut next_token = None;

    let mut in_contents = false;
    let mut in_key = false;
    let mut in_size = false;
    let mut in_next_token = false;
    let mut current_key = String::new();
    let mut current_size = String::new();
    let mut current_text = String::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "Contents" => {
                        in_contents = true;
                        current_key.clear();
                        current_size.clear();
                    }
                    "Key" if in_contents => {
                        in_key = true;
                        current_text.clear();
                    }
                    "Size" if in_contents => {
                        in_size = true;
                        current_text.clear();
                    }
                    "NextContinuationToken" => {
                        in_next_token = true;
                        current_text.clear();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if (in_key || in_size || in_next_token)
                    && let Ok(text) = e.unescape()
                {
                    current_text.push_str(&text);
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let name = std::str::from_utf8(qname.as_ref()).unwrap_or("");
                match name {
                    "Key" if in_key => {
                        current_key = current_text.clone();
                        in_key = false;
                    }
                    "Size" if in_size => {
                        current_size = current_text.clone();
                        in_size = false;
                    }
                    "Contents" if in_contents => {
                        if current_key.ends_with(".gz") {
                            let size = current_size.parse::<u64>().unwrap_or(0);
                            files.push(PartitionFile {
                                key: current_key.clone(),
                                size,
                            });
                        }
                        in_contents = false;
                    }
                    "NextContinuationToken" => {
                        if in_next_token {
                            next_token = Some(current_text.clone());
                            in_next_token = false;
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(OpenAlexError::Parse(format!("XML parse error: {}", e))),
            _ => {}
        }
        buf.clear();
    }

    Ok((files, next_token))
}

fn extract_date_from_prefix(prefix: &str) -> Option<String> {
    // "data/works/updated_date=2025-01-15/" → "2025-01-15"
    let parts: Vec<&str> = prefix.trim_end_matches('/').split('=').collect();
    if parts.len() == 2 {
        let date = parts[1];
        // Basic validation: YYYY-MM-DD
        if date.len() == 10 && date.chars().nth(4) == Some('-') && date.chars().nth(7) == Some('-')
        {
            return Some(date.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_date_from_prefix() {
        assert_eq!(
            extract_date_from_prefix("data/works/updated_date=2025-01-15/"),
            Some("2025-01-15".to_string())
        );
        assert_eq!(
            extract_date_from_prefix("data/works/updated_date=2024-12-31/"),
            Some("2024-12-31".to_string())
        );
        assert_eq!(extract_date_from_prefix("data/works/"), None);
        assert_eq!(extract_date_from_prefix("invalid"), None);
    }

    #[test]
    fn test_parse_partition_list_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <CommonPrefixes><Prefix>data/works/updated_date=2025-01-15/</Prefix></CommonPrefixes>
  <CommonPrefixes><Prefix>data/works/updated_date=2025-01-16/</Prefix></CommonPrefixes>
</ListBucketResult>"#;
        let (partitions, token) = parse_partition_list_xml(xml).unwrap();
        assert_eq!(partitions.len(), 2);
        assert_eq!(partitions[0].date, "2025-01-15");
        assert_eq!(partitions[1].date, "2025-01-16");
        assert!(token.is_none());
    }

    #[test]
    fn test_parse_partition_list_with_continuation() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <IsTruncated>true</IsTruncated>
  <CommonPrefixes><Prefix>data/works/updated_date=2025-01-15/</Prefix></CommonPrefixes>
  <NextContinuationToken>abc123</NextContinuationToken>
</ListBucketResult>"#;
        let (partitions, token) = parse_partition_list_xml(xml).unwrap();
        assert_eq!(partitions.len(), 1);
        assert_eq!(token, Some("abc123".to_string()));
    }

    #[test]
    fn test_parse_file_list_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<ListBucketResult>
  <Contents>
    <Key>data/works/updated_date=2025-01-15/part_000.gz</Key>
    <Size>12345678</Size>
  </Contents>
  <Contents>
    <Key>data/works/updated_date=2025-01-15/manifest</Key>
    <Size>100</Size>
  </Contents>
</ListBucketResult>"#;
        let (files, token) = parse_file_list_xml(xml).unwrap();
        assert_eq!(files.len(), 1); // only .gz files
        assert_eq!(
            files[0].key,
            "data/works/updated_date=2025-01-15/part_000.gz"
        );
        assert_eq!(files[0].size, 12345678);
        assert!(token.is_none());
    }
}
