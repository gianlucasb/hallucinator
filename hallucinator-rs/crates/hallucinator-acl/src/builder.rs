//! Download and build pipeline for the offline ACL Anthology database.
//!
//! Downloads the ACL Anthology GitHub repo tarball, extracts XML files from
//! `data/xml/`, parses them, and builds a normalized SQLite database with
//! FTS5 full-text search.

use std::io::BufReader;
use std::path::Path;

use futures_util::StreamExt;
use rusqlite::Connection;

use crate::db::{self, InsertBatch};
use crate::xml_parser;
use crate::{AclError, BuildProgress};

/// GitHub API URL for the ACL Anthology tarball.
const TARBALL_URL: &str = "https://api.github.com/repos/acl-org/acl-anthology/tarball/master";

/// GitHub API URL for the latest commit SHA on master.
const COMMITS_URL: &str = "https://api.github.com/repos/acl-org/acl-anthology/commits/master";

/// Batch size for database inserts.
const BATCH_SIZE: usize = 10_000;

/// Build (or update) the offline ACL Anthology database.
///
/// Downloads the GitHub tarball, extracts `data/xml/*.xml`, parses them,
/// and builds the SQLite database. Uses commit SHA to skip if unchanged.
pub async fn build(
    db_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<bool, AclError> {
    let conn = Connection::open(db_path)?;
    db::init_database(&conn)?;

    // Check stored commit SHA for conditional update
    let stored_sha = db::get_metadata(&conn, "commit_sha")?;

    // Build HTTP client
    let client = reqwest::Client::builder()
        .user_agent("hallucinator-acl/0.1.0")
        .build()
        .map_err(|e| AclError::Download(e.to_string()))?;

    // Check latest commit SHA
    let current_sha = get_latest_commit_sha(&client).await?;

    if let Some(ref stored) = stored_sha
        && stored == &current_sha
    {
        progress(BuildProgress::Complete {
            publications: 0,
            authors: 0,
            skipped: true,
        });
        return Ok(false);
    }

    // Download tarball
    progress(BuildProgress::Downloading {
        bytes_downloaded: 0,
        total_bytes: None,
    });

    let response = client
        .get(TARBALL_URL)
        .send()
        .await
        .map_err(|e| AclError::Download(e.to_string()))?;

    if !response.status().is_success() {
        return Err(AclError::Download(format!(
            "HTTP error: {}",
            response.status()
        )));
    }

    let total_bytes = response.content_length();

    // Download to temp file
    let db_dir = db_path.parent().unwrap_or(Path::new("."));
    let tmp_dir = tempfile::TempDir::new_in(db_dir).map_err(AclError::Io)?;
    let tarball_path = tmp_dir.path().join("acl-anthology.tar.gz");

    {
        let mut out = std::fs::File::create(&tarball_path)?;
        let mut stream = response.bytes_stream();
        let mut bytes_downloaded: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| AclError::Download(e.to_string()))?;
            std::io::Write::write_all(&mut out, &chunk)?;
            bytes_downloaded += chunk.len() as u64;

            progress(BuildProgress::Downloading {
                bytes_downloaded,
                total_bytes,
            });
        }
        std::io::Write::flush(&mut out)?;
    }

    // Extract XML files and parse into DB
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<BuildProgress>(64);
    let sha_for_meta = current_sha.clone();

    let parse_handle = tokio::task::spawn_blocking(move || {
        let _tmp_dir = tmp_dir; // keep alive

        // Extract XML files from tarball
        let xml_dir = _tmp_dir.path().join("xml");
        std::fs::create_dir_all(&xml_dir)?;

        let file = std::fs::File::open(&tarball_path)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let mut xml_files: Vec<std::path::PathBuf> = Vec::new();
        let mut files_extracted: u64 = 0;

        for entry in archive.entries().map_err(AclError::Io)? {
            let mut entry = entry.map_err(AclError::Io)?;
            let path = entry.path().map_err(AclError::Io)?.into_owned();
            let path_str = path.to_string_lossy();

            // Only extract data/xml/*.xml files
            if path_str.contains("data/xml/") && path_str.ends_with(".xml") {
                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let dest = xml_dir.join(&filename);
                entry.unpack(&dest).map_err(AclError::Io)?;
                xml_files.push(dest);
                files_extracted += 1;

                if files_extracted.is_multiple_of(100) {
                    let _ =
                        progress_tx.blocking_send(BuildProgress::Extracting { files_extracted });
                }
            }
        }

        let _ = progress_tx.blocking_send(BuildProgress::Extracting { files_extracted });

        let files_total = xml_files.len() as u64;

        // Parse XML files and insert into database
        db::begin_bulk_load(&conn)?;

        let mut batch = InsertBatch::new();
        let mut records_parsed: u64 = 0;
        let mut records_inserted: u64 = 0;
        let mut files_processed: u64 = 0;

        for xml_path in &xml_files {
            let file = match std::fs::File::open(xml_path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            let reader = BufReader::new(file);

            xml_parser::parse_xml(reader, |paper| {
                records_parsed += 1;

                // Add authors
                for author in &paper.authors {
                    batch.authors.push(author.clone());
                }

                // Add publication
                batch.publications.push((
                    paper.anthology_id.clone(),
                    paper.title,
                    paper.url,
                    paper.doi,
                ));

                // Add relationships
                for (pos, author) in paper.authors.iter().enumerate() {
                    batch.publication_authors.push((
                        paper.anthology_id.clone(),
                        author.clone(),
                        pos,
                    ));
                }

                if batch.len() >= BATCH_SIZE {
                    records_inserted += batch.len() as u64;
                    if let Err(_e) = db::insert_batch(&conn, &batch) {
                        // Log error but continue
                    }
                    batch.clear();

                    let _ = progress_tx.blocking_send(BuildProgress::Parsing {
                        records_parsed,
                        records_inserted,
                        files_processed,
                        files_total,
                    });
                }
            });

            files_processed += 1;

            if files_processed.is_multiple_of(50) {
                let _ = progress_tx.blocking_send(BuildProgress::Parsing {
                    records_parsed,
                    records_inserted,
                    files_processed,
                    files_total,
                });
            }
        }

        // Flush remaining
        if !batch.is_empty() {
            records_inserted += batch.len() as u64;
            db::insert_batch(&conn, &batch)?;
        }

        let _ = progress_tx.blocking_send(BuildProgress::Parsing {
            records_parsed,
            records_inserted,
            files_processed: files_total,
            files_total,
        });

        let _ = progress_tx.blocking_send(BuildProgress::RebuildingIndex);
        db::end_bulk_load(&conn)?;
        db::rebuild_fts_index(&conn)?;

        // Update metadata
        let timestamp = now_unix_timestamp();
        db::set_metadata(&conn, "last_updated", &timestamp)?;
        db::set_metadata(&conn, "schema_version", "1")?;
        db::set_metadata(&conn, "commit_sha", &sha_for_meta)?;

        let (pubs, authors, _) = db::get_counts(&conn)?;
        db::set_metadata(&conn, "publication_count", &pubs.to_string())?;
        db::set_metadata(&conn, "author_count", &authors.to_string())?;

        Ok::<(i64, i64), AclError>((pubs, authors))
    });

    // Forward progress events
    while let Some(evt) = progress_rx.recv().await {
        progress(evt);
    }

    let (pubs, authors) = parse_handle
        .await
        .map_err(|e| AclError::Download(format!("parse task panicked: {}", e)))??;

    progress(BuildProgress::Complete {
        publications: pubs as u64,
        authors: authors as u64,
        skipped: false,
    });

    Ok(true)
}

/// Get the latest commit SHA for the master branch.
async fn get_latest_commit_sha(client: &reqwest::Client) -> Result<String, AclError> {
    let response = client
        .get(COMMITS_URL)
        .send()
        .await
        .map_err(|e| AclError::Download(e.to_string()))?;

    if !response.status().is_success() {
        return Err(AclError::Download(format!(
            "Failed to fetch commit info: HTTP {}",
            response.status()
        )));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| AclError::Download(e.to_string()))?;

    body["sha"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| AclError::Download("No SHA in commit response".into()))
}

/// Unix timestamp as a string (seconds since epoch).
fn now_unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}
