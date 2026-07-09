//! S3 download + JSON parsing + Tantivy indexing for OpenAlex works.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use flate2::read::GzDecoder;
use futures_util::StreamExt;
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

/// Number of files to download and parse concurrently.
///
/// Kept modest: the S3 endpoint starts refusing connections
/// ("error sending request for url") under heavier fan-out, and each
/// dropped file leaves a hole in the index (see watermark handling in
/// [`build`]). Fewer parallel streams trade a little throughput for a
/// much lower skip rate.
const DOWNLOAD_CONCURRENCY: usize = 4;

/// Number of retry attempts per file before skipping.
const MAX_RETRIES: u32 = 6;

/// Result from downloading and parsing a single gz file.
enum FileResult {
    Ok {
        partition_date: String,
        filename: String,
        records: Vec<(u64, String, Vec<String>)>,
    },
    /// All retries exhausted — skip this file. Carries the partition date
    /// so the caller can hold the sync watermark below the failed
    /// partition and re-fetch it next run.
    Failed {
        partition_date: String,
        filename: String,
        error: String,
    },
}

/// Extract a short display name from an S3 key.
/// `"data/jsonl/works/updated_date=2026-01-15/part_003.gz"` → `"2026-01-15/part_003.gz"`
fn short_filename(key: &str) -> String {
    let full_prefix = format!("{}updated_date=", crate::s3::WORKS_PREFIX);
    key.strip_prefix(&full_prefix).unwrap_or(key).to_string()
}

/// Choose the sync watermark to persist after a run.
///
/// The watermark must only advance across the *contiguous fully-successful
/// prefix* of partitions. If any partition had a skipped file, we cap the
/// watermark just below the earliest such partition, so the next run
/// re-fetches that partition (and everything after) instead of stepping over
/// the hole forever — the next incremental only pulls partitions strictly
/// newer than the stored watermark. Later partitions that happened to succeed
/// get re-downloaded and upserted (deduped by work id), so no data is lost.
///
/// With no failures, the watermark is simply the newest partition seen.
fn safe_watermark(
    ok_partition_dates: &HashSet<String>,
    failed_partition_dates: &HashSet<String>,
    newest_date: &str,
    prev_watermark: &str,
) -> String {
    match failed_partition_dates.iter().min() {
        Some(earliest_failed) => ok_partition_dates
            .iter()
            .filter(|d| d.as_str() < earliest_failed.as_str())
            .max()
            .cloned()
            .unwrap_or_else(|| prev_watermark.to_string()),
        None => newest_date.to_string(),
    }
}

/// Build or incrementally update the OpenAlex Tantivy index.
///
/// Downloads are parallelised (up to [`DOWNLOAD_CONCURRENCY`] files at a time)
/// so the network is saturated while the indexer writes to Tantivy.
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
            failed_files: Vec::new(),
        });
        return Ok(false);
    }

    // Step 2: List all files across all partitions (concurrently)
    progress(BuildProgress::ListingPartitions {
        message: format!("Listing files across {} partitions...", partitions.len()),
    });

    let listing_futures: Vec<_> = partitions
        .iter()
        .map(|partition| {
            let client = client.clone();
            let prefix = partition.prefix.clone();
            let date = partition.date.clone();
            async move {
                let files = s3::list_partition_files(&client, &prefix).await?;
                Ok::<_, OpenAlexError>((date, files))
            }
        })
        .collect();

    let listing_results: Vec<Result<_, OpenAlexError>> =
        futures_util::stream::iter(listing_futures)
            .buffer_unordered(16)
            .collect()
            .await;

    let mut all_files: Vec<(String, s3::PartitionFile)> = Vec::new();
    for result in listing_results {
        let (date, files) = result?;
        for file in files {
            all_files.push((date.clone(), file));
        }
    }

    if all_files.is_empty() {
        progress(BuildProgress::Complete {
            publications: 0,
            skipped: true,
            failed_files: Vec::new(),
        });
        return Ok(false);
    }

    let files_total = all_files.len() as u64;

    // Step 3: Open or create Tantivy index
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

    let writer: IndexWriter = index
        .writer(256_000_000) // 256MB heap
        .map_err(|e| OpenAlexError::Index(e.to_string()))?;

    let mut newest_date = last_sync_date.clone().unwrap_or_default();

    // Shared counters for live progress
    let live_bytes = Arc::new(AtomicU64::new(0));
    let records_indexed = Arc::new(AtomicU64::new(0));
    let mut file_counters: HashMap<String, Arc<AtomicU64>> = HashMap::new();

    // Spawn dedicated indexer task so Tantivy writes don't stall the
    // download futures (FuturesUnordered only polls children when the
    // main select! loop is free).
    let (index_tx, index_rx) =
        tokio::sync::mpsc::channel::<Vec<(u64, String, Vec<String>)>>(DOWNLOAD_CONCURRENCY * 2);
    let indexer_records = records_indexed.clone();
    let index_handle = tokio::task::spawn_blocking(move || -> Result<(), OpenAlexError> {
        let mut index_rx = index_rx;
        let mut writer = writer;
        let mut uncommitted: u64 = 0;
        while let Some(batch) = index_rx.blocking_recv() {
            for (openalex_id, title, authors) in batch {
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
                uncommitted += 1;
                indexer_records.fetch_add(1, Ordering::Relaxed);
                if uncommitted >= 100_000 {
                    writer
                        .commit()
                        .map_err(|e| OpenAlexError::Index(e.to_string()))?;
                    uncommitted = 0;
                }
            }
        }
        if uncommitted > 0 {
            writer
                .commit()
                .map_err(|e| OpenAlexError::Index(e.to_string()))?;
        }
        writer
            .wait_merging_threads()
            .map_err(|e| OpenAlexError::Index(e.to_string()))?;
        Ok(())
    });

    // Step 4: Concurrent download + parse, index as results arrive.
    // Each download is tokio::spawn'd so they run on independent runtime
    // threads — gzip decompression in one task can't stall another's HTTP stream.
    let mut in_flight = tokio::task::JoinSet::new();
    let mut file_iter = all_files.into_iter();

    // Seed the initial batch of concurrent downloads
    for _ in 0..DOWNLOAD_CONCURRENCY {
        if let Some((partition_date, file)) = file_iter.next() {
            let filename = short_filename(&file.key);
            let file_bytes = Arc::new(AtomicU64::new(0));
            file_counters.insert(filename.clone(), file_bytes.clone());
            progress(BuildProgress::FileStarted {
                filename: filename.clone(),
            });
            in_flight.spawn(make_download_future(
                client.clone(),
                file.key,
                partition_date,
                min_year,
                live_bytes.clone(),
                file_bytes,
            ));
        }
    }

    let mut files_done: u64 = 0;
    let mut failed_files: Vec<String> = Vec::new();
    // Track which partitions fully succeeded vs. had any skipped file, so the
    // sync watermark only advances across the contiguous fully-downloaded
    // prefix. A partition with even one dropped file must be re-fetched next
    // run, so we never let the watermark move to or past it.
    let mut ok_partition_dates: HashSet<String> = HashSet::new();
    let mut failed_partition_dates: HashSet<String> = HashSet::new();
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            result = in_flight.join_next() => {
                let file_result = match result {
                    Some(Ok(r)) => r,
                    Some(Err(e)) => {
                        return Err(OpenAlexError::Index(
                            format!("download task panicked: {e}")
                        ));
                    }
                    None => break,
                };
                match file_result {
                    FileResult::Ok { partition_date, filename, records } => {
                        file_counters.remove(&filename);
                        progress(BuildProgress::FileComplete { filename });

                        if !records.is_empty() {
                            index_tx.send(records).await
                                .map_err(|_| OpenAlexError::Index("indexer task failed".into()))?;
                        }

                        files_done += 1;
                        if partition_date > newest_date {
                            newest_date = partition_date.clone();
                        }
                        ok_partition_dates.insert(partition_date);
                    }
                    FileResult::Failed {
                        partition_date,
                        filename,
                        error,
                    } => {
                        file_counters.remove(&filename);
                        progress(BuildProgress::FileSkipped {
                            filename: filename.clone(),
                            error,
                        });
                        failed_files.push(filename);
                        failed_partition_dates.insert(partition_date);
                        files_done += 1;
                    }
                }

                // Replenish: start the next download
                if let Some((partition_date, file)) = file_iter.next() {
                    let filename = short_filename(&file.key);
                    let file_bytes = Arc::new(AtomicU64::new(0));
                    file_counters.insert(filename.clone(), file_bytes.clone());
                    progress(BuildProgress::FileStarted {
                        filename: filename.clone(),
                    });
                    in_flight.spawn(make_download_future(
                        client.clone(),
                        file.key,
                        partition_date,
                        min_year,
                        live_bytes.clone(),
                        file_bytes,
                    ));
                }

                // Emit progress after file completion
                progress(BuildProgress::Downloading {
                    files_done,
                    files_total,
                    bytes_downloaded: live_bytes.load(Ordering::Relaxed),
                    records_indexed: records_indexed.load(Ordering::Relaxed),
                });
            }
            _ = tick.tick() => {
                // Live progress: main bar + per-file spinners
                progress(BuildProgress::Downloading {
                    files_done,
                    files_total,
                    bytes_downloaded: live_bytes.load(Ordering::Relaxed),
                    records_indexed: records_indexed.load(Ordering::Relaxed),
                });
                for (filename, counter) in &file_counters {
                    progress(BuildProgress::FileProgress {
                        filename: filename.clone(),
                        bytes_downloaded: counter.load(Ordering::Relaxed),
                    });
                }
            }
        }
    }

    // Step 6: Signal indexer to finish, then wait for commit + merge
    drop(index_tx);
    progress(BuildProgress::Committing {
        records_indexed: records_indexed.load(Ordering::Relaxed),
    });
    progress(BuildProgress::Merging);
    index_handle
        .await
        .map_err(|e| OpenAlexError::Index(e.to_string()))??;

    let total_records = records_indexed.load(Ordering::Relaxed);

    // Step 7: Write updated metadata
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let total_in_index =
        existing_meta.and_then(|m| m.publication_count).unwrap_or(0) + total_records;

    let sync_watermark = safe_watermark(
        &ok_partition_dates,
        &failed_partition_dates,
        &newest_date,
        &last_sync_date.clone().unwrap_or_default(),
    );

    metadata::write_metadata(
        db_path,
        &IndexMetadata {
            schema_version: "1".to_string(),
            build_date: Some(now.to_string()),
            publication_count: Some(total_in_index),
            last_sync_date: Some(sync_watermark),
        },
    )?;

    progress(BuildProgress::Complete {
        publications: total_records,
        skipped: false,
        failed_files,
    });

    Ok(true)
}

/// Create a boxed future that downloads and parses one S3 file.
///
/// Retries up to [`MAX_RETRIES`] times with exponential backoff. If all
/// attempts fail, returns [`FileResult::Failed`] instead of an error so
/// the build continues with the remaining files.
fn make_download_future(
    client: reqwest::Client,
    key: String,
    partition_date: String,
    min_year: Option<u32>,
    total_bytes: Arc<AtomicU64>,
    file_bytes: Arc<AtomicU64>,
) -> Pin<Box<dyn Future<Output = FileResult> + Send>> {
    let filename = short_filename(&key);
    Box::pin(async move {
        let mut last_err = String::new();
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                // Reset per-file counter for the retry
                file_bytes.store(0, Ordering::Relaxed);
                // Exponential backoff, capped so a late retry doesn't stall
                // for minutes on a flaky connection.
                let backoff = Duration::from_secs(2u64.pow(attempt).min(30));
                tokio::time::sleep(backoff).await;
            }
            match download_and_parse(&client, &key, min_year, &total_bytes, &file_bytes).await {
                Ok(records) => {
                    return FileResult::Ok {
                        partition_date,
                        filename,
                        records,
                    };
                }
                Err(e) => {
                    // Undo the bytes this failed attempt added to the global counter
                    let file_so_far = file_bytes.load(Ordering::Relaxed);
                    total_bytes.fetch_sub(file_so_far, Ordering::Relaxed);
                    last_err = e.to_string();
                }
            }
        }
        FileResult::Failed {
            partition_date,
            filename,
            error: last_err,
        }
    })
}

/// Stream-download a gzipped S3 file, updating byte counters as chunks
/// arrive, then decompress and parse the JSON lines.
async fn download_and_parse(
    client: &reqwest::Client,
    key: &str,
    min_year: Option<u32>,
    total_bytes: &AtomicU64,
    file_bytes: &AtomicU64,
) -> Result<Vec<(u64, String, Vec<String>)>, OpenAlexError> {
    let url = format!("{}/{}", s3::BUCKET_URL, key);
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

    // Stream chunks so the byte counters update in real-time
    let mut gz_bytes = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| OpenAlexError::Download(e.to_string()))?;
        let len = chunk.len() as u64;
        total_bytes.fetch_add(len, Ordering::Relaxed);
        file_bytes.fetch_add(len, Ordering::Relaxed);
        gz_bytes.extend_from_slice(&chunk);
    }

    // Decompress and parse JSON lines
    let decoder = GzDecoder::new(gz_bytes.as_slice());
    let buf_reader = BufReader::new(decoder);
    let mut records = Vec::new();

    for line_result in buf_reader.lines() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(record) = parse_work_json(&line, min_year) {
            records.push(record);
        }
    }

    Ok(records)
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

    fn dates(items: &[&str]) -> HashSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn watermark_advances_to_newest_when_all_succeed() {
        let ok = dates(&["2026-04-01", "2026-04-02", "2026-06-26"]);
        let failed = dates(&[]);
        assert_eq!(
            safe_watermark(&ok, &failed, "2026-06-26", "2026-03-30"),
            "2026-06-26"
        );
    }

    #[test]
    fn watermark_holds_below_earliest_failure() {
        // Later partitions succeeded, but 2026-04-03 had a dropped file:
        // the watermark must not pass it, so the next run re-fetches from there.
        let ok = dates(&["2026-04-01", "2026-04-02", "2026-05-10", "2026-06-26"]);
        let failed = dates(&["2026-04-03", "2026-05-11"]);
        assert_eq!(
            safe_watermark(&ok, &failed, "2026-06-26", "2026-03-30"),
            "2026-04-02"
        );
    }

    #[test]
    fn watermark_stays_at_prev_when_first_partition_fails() {
        // No fully-successful partition precedes the earliest failure, so the
        // watermark can't advance at all — the whole range re-runs next time.
        let ok = dates(&["2026-04-05", "2026-06-26"]);
        let failed = dates(&["2026-04-01"]);
        assert_eq!(
            safe_watermark(&ok, &failed, "2026-06-26", "2026-03-30"),
            "2026-03-30"
        );
    }

    #[test]
    fn works_prefix_points_at_live_jsonl_path() {
        // Regression: the old `data/works/` prefix was retired in the 2026
        // bucket restructure (moved to `legacy-data/works/`), which silently
        // returned zero partitions and made every update a no-op. The live
        // snapshot is under `data/jsonl/works/`.
        assert_eq!(crate::s3::WORKS_PREFIX, "data/jsonl/works/");
    }

    #[test]
    fn short_filename_strips_current_prefix() {
        assert_eq!(
            short_filename("data/jsonl/works/updated_date=2026-06-26/part_0003.gz"),
            "2026-06-26/part_0003.gz"
        );
        // Unknown keys pass through unchanged.
        assert_eq!(short_filename("something/else.gz"), "something/else.gz");
    }

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
