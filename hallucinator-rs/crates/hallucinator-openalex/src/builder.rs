//! S3 download + JSON parsing + Tantivy indexing for OpenAlex works.

use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use tantivy::doc;
use tantivy::schema::*;
use tantivy::{Index, IndexWriter};

use crate::metadata::{self, IndexMetadata};
use crate::s3;
use crate::{BuildProgress, OpenAlexError};

/// Work types we index (skip datasets, components, etc.).
const ALLOWED_TYPES: &[&str] = &[
    "article",
    "book-chapter",
    "preprint",
    "review",
    "dissertation",
];

/// Build or incrementally update the OpenAlex Tantivy index.
///
/// - `since_override`: if set, only download S3 partitions newer than this date (YYYY-MM-DD).
///   Overrides the metadata `last_sync_date`.
/// - `min_year`: if set, skip works with `publication_year` before this year during indexing.
///
/// Returns `true` if new data was indexed, `false` if already up to date.
pub async fn build(
    db_path: &Path,
    since_override: Option<String>,
    min_year: Option<u32>,
    mut progress: impl FnMut(BuildProgress),
) -> Result<bool, OpenAlexError> {
    let client = reqwest::Client::builder()
        .user_agent("hallucinator/openalex-offline (https://github.com/gianlucasb/hallucinator)")
        .build()
        .map_err(|e| OpenAlexError::Download(e.to_string()))?;

    // Read existing metadata for incremental updates
    let existing_meta = if db_path.exists() {
        metadata::read_metadata(db_path).ok()
    } else {
        None
    };
    // since_override takes priority over stored last_sync_date
    let last_sync_date = since_override.or_else(|| {
        existing_meta
            .as_ref()
            .and_then(|m| m.last_sync_date.clone())
    });

    // Step 1: List date partitions from S3
    progress(BuildProgress::ListingPartitions {
        message: "Listing OpenAlex S3 partitions...".to_string(),
    });

    let all_partitions = s3::list_date_partitions(&client).await?;

    // Filter to partitions newer than the cutoff date
    let partitions: Vec<_> = if let Some(ref since) = last_sync_date {
        all_partitions
            .into_iter()
            .filter(|p| p.date.as_str() > since.as_str())
            .collect()
    } else {
        all_partitions
    };

    if partitions.is_empty() {
        progress(BuildProgress::Complete {
            publications: 0,
            skipped: true,
        });
        return Ok(false);
    }

    let partitions_total = partitions.len() as u64;

    // Step 2: Open or create Tantivy index
    std::fs::create_dir_all(db_path)?;

    let (index, schema) = open_or_create_index(db_path)?;
    let title_field = schema
        .get_field("title")
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;
    let authors_field = schema
        .get_field("authors")
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;
    let id_field = schema
        .get_field("openalex_id")
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    let mut writer: IndexWriter = index
        .writer(256_000_000) // 256MB heap
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    let mut total_records: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut newest_date = last_sync_date.clone().unwrap_or_default();
    let mut uncommitted_count: u64 = 0;

    // Step 3: Process each partition
    for (part_idx, partition) in partitions.iter().enumerate() {
        progress(BuildProgress::Downloading {
            partitions_done: part_idx as u64,
            partitions_total,
            bytes_downloaded: total_bytes,
            records_indexed: total_records,
        });

        // List files in this partition
        let files = s3::list_partition_files(&client, &partition.prefix).await?;

        for file in &files {
            // Download the gzipped file
            let gz_bytes: Vec<u8> = s3::download_gz(&client, &file.key).await?;
            total_bytes += gz_bytes.len() as u64;

            // Decompress and parse JSON lines
            let decoder = GzDecoder::new(gz_bytes.as_slice());
            let buf_reader = BufReader::new(decoder);

            for line_result in buf_reader.lines() {
                let line: String = match line_result {
                    Ok(l) => l,
                    Err(_) => continue,
                };

                if line.trim().is_empty() {
                    continue;
                }

                if let Some((openalex_id, title, authors)) = parse_work_json(&line, min_year) {
                    // Upsert: delete existing, then add
                    let id_term = tantivy::Term::from_field_u64(id_field, openalex_id);
                    writer.delete_term(id_term);

                    let authors_str = authors.join("|");
                    writer
                        .add_document(doc!(
                            title_field => title,
                            authors_field => authors_str,
                            id_field => openalex_id,
                        ))
                        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

                    total_records += 1;
                    uncommitted_count += 1;

                    // Commit periodically
                    if uncommitted_count >= 100_000 {
                        progress(BuildProgress::Committing {
                            records_indexed: total_records,
                        });
                        writer
                            .commit()
                            .map_err(|e| OpenAlexError::Index(e.to_string()))?;
                        uncommitted_count = 0;
                    }
                }
            }

            // Update progress after each file
            progress(BuildProgress::Downloading {
                partitions_done: part_idx as u64,
                partitions_total,
                bytes_downloaded: total_bytes,
                records_indexed: total_records,
            });
        }

        if partition.date > newest_date {
            newest_date = partition.date.clone();
        }
    }

    // Step 4: Final commit
    if uncommitted_count > 0 {
        progress(BuildProgress::Committing {
            records_indexed: total_records,
        });
        writer
            .commit()
            .map_err(|e| OpenAlexError::Index(e.to_string()))?;
    }

    // Step 5: Wait for merge threads
    progress(BuildProgress::Merging);
    writer
        .wait_merging_threads()
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    // Step 6: Write updated metadata
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let total_in_index =
        existing_meta.and_then(|m| m.publication_count).unwrap_or(0) + total_records;

    metadata::write_metadata(
        db_path,
        &IndexMetadata {
            schema_version: "1".to_string(),
            build_date: Some(now.to_string()),
            publication_count: Some(total_in_index),
            last_sync_date: Some(newest_date),
        },
    )?;

    progress(BuildProgress::Complete {
        publications: total_records,
        skipped: false,
    });

    Ok(true)
}

/// Open an existing Tantivy index or create a new one with our schema.
fn open_or_create_index(path: &Path) -> Result<(Index, Schema), OpenAlexError> {
    // Check if this is already a Tantivy index directory
    let meta_path = path.join("meta.json");
    if meta_path.exists() {
        let index = Index::open_in_dir(path)?;
        let schema = index.schema();
        return Ok((index, schema));
    }

    // Create new index with schema
    let schema = build_schema();
    let index = Index::create_in_dir(path, schema.clone())?;
    Ok((index, schema))
}

fn build_schema() -> Schema {
    let mut schema_builder = Schema::builder();
    schema_builder.add_text_field("title", TEXT | STORED);
    schema_builder.add_text_field("authors", STORED);
    schema_builder.add_u64_field("openalex_id", INDEXED | STORED | FAST);
    schema_builder.build()
}

/// Parse a single OpenAlex JSON line into (openalex_id, title, authors).
///
/// Returns `None` if the work type is not in `ALLOWED_TYPES` or required
/// fields are missing.
fn parse_work_json(line: &str, min_year: Option<u32>) -> Option<(u64, String, Vec<String>)> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;

    // Filter by type
    let work_type = value.get("type")?.as_str()?;
    if !ALLOWED_TYPES.contains(&work_type) {
        return None;
    }

    // Filter by publication year
    if let Some(min) = min_year {
        let year = value.get("publication_year").and_then(|y| y.as_u64());
        if year.is_none_or(|y| y < min as u64) {
            return None;
        }
    }

    // Extract title
    let title = value.get("display_name")?.as_str()?;
    if title.is_empty() {
        return None;
    }

    // Extract numeric ID from "https://openalex.org/W1234567"
    let id_str = value.get("id")?.as_str()?;
    let openalex_id = extract_numeric_id(id_str)?;

    // Extract authors
    let authors: Vec<String> = value
        .get("authorships")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    a.get("author")
                        .and_then(|auth| auth.get("display_name"))
                        .and_then(|name| name.as_str())
                        .map(String::from)
                })
                .collect()
        })
        .unwrap_or_default();

    Some((openalex_id, title.to_string(), authors))
}

/// Extract numeric ID from OpenAlex URL: "https://openalex.org/W1234567" → 1234567
fn extract_numeric_id(id_str: &str) -> Option<u64> {
    id_str
        .rsplit('/')
        .next()
        .and_then(|s| s.strip_prefix('W'))
        .and_then(|s| s.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_work_json_article() {
        let json = r#"{"id":"https://openalex.org/W2741809807","display_name":"Attention is All you Need","type":"article","authorships":[{"author":{"display_name":"Ashish Vaswani"}},{"author":{"display_name":"Noam Shazeer"}}]}"#;
        let result = parse_work_json(json, None);
        assert!(result.is_some());
        let (id, title, authors) = result.unwrap();
        assert_eq!(id, 2741809807);
        assert_eq!(title, "Attention is All you Need");
        assert_eq!(authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
    }

    #[test]
    fn test_parse_work_json_filtered_type() {
        let json = r#"{"id":"https://openalex.org/W123","display_name":"Some Dataset","type":"dataset","authorships":[]}"#;
        assert!(parse_work_json(json, None).is_none());
    }

    #[test]
    fn test_parse_work_json_missing_title() {
        let json = r#"{"id":"https://openalex.org/W123","type":"article","authorships":[]}"#;
        assert!(parse_work_json(json, None).is_none());
    }

    #[test]
    fn test_extract_numeric_id() {
        assert_eq!(
            extract_numeric_id("https://openalex.org/W2741809807"),
            Some(2741809807)
        );
        assert_eq!(extract_numeric_id("https://openalex.org/W1"), Some(1));
        assert_eq!(extract_numeric_id("invalid"), None);
        assert_eq!(extract_numeric_id("https://openalex.org/A123"), None);
    }

    #[test]
    fn test_allowed_types() {
        for t in &[
            "article",
            "book-chapter",
            "preprint",
            "review",
            "dissertation",
        ] {
            let json = format!(
                r#"{{"id":"https://openalex.org/W1","display_name":"Test","type":"{}","authorships":[]}}"#,
                t
            );
            assert!(
                parse_work_json(&json, None).is_some(),
                "type {} should be allowed",
                t
            );
        }
        for t in &["dataset", "component", "grant", "standard", "editorial"] {
            let json = format!(
                r#"{{"id":"https://openalex.org/W1","display_name":"Test","type":"{}","authorships":[]}}"#,
                t
            );
            assert!(
                parse_work_json(&json, None).is_none(),
                "type {} should be filtered",
                t
            );
        }
    }
}
