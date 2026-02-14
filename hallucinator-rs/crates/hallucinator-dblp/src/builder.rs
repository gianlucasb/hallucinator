//! Download and build pipeline for the offline DBLP database.
//!
//! Downloads `dblp.xml.gz` from dblp.org (~1 GB), parses it with a SAX-style
//! XML parser, and builds a normalized SQLite database with FTS5 full-text search.

use std::cell::Cell;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::rc::Rc;

use futures_util::StreamExt;
use rusqlite::Connection;

use crate::db::{self, InsertBatch};
use crate::xml_parser;
use crate::{BuildProgress, DblpError};

/// Default DBLP XML dump URL (~1 GB compressed).
pub const DEFAULT_DBLP_URL: &str = "https://dblp.uni-trier.de/xml/dblp.xml.gz";

/// Batch size for database inserts — large batches reduce transaction overhead.
const BATCH_SIZE: usize = 100_000;

/// Build (or update) the offline DBLP database by downloading from dblp.org.
///
/// Phase 1: Downloads `dblp.xml.gz` to a temporary file with progress reporting.
/// Phase 2: Parses the XML and inserts into SQLite (runs in a blocking thread).
///
/// Uses ETag/Last-Modified headers for conditional requests — if the remote
/// file hasn't changed since the last build, returns `Ok(false)`.
pub async fn build(
    db_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<bool, DblpError> {
    let conn = Connection::open(db_path)?;
    db::init_database(&conn)?;

    // Check stored ETag/Last-Modified for conditional request
    let stored_etag = db::get_metadata(&conn, "etag")?;
    let stored_last_modified = db::get_metadata(&conn, "last_modified")?;

    // Build HTTP client
    let client = reqwest::Client::builder()
        .user_agent("hallucinator-dblp/0.1.0")
        .build()
        .map_err(|e| DblpError::Download(e.to_string()))?;

    // Conditional GET
    let mut request = client.get(DEFAULT_DBLP_URL);
    if let Some(ref etag) = stored_etag {
        request = request.header("If-None-Match", etag.as_str());
    }
    if let Some(ref lm) = stored_last_modified {
        request = request.header("If-Modified-Since", lm.as_str());
    }

    let response = request
        .send()
        .await
        .map_err(|e| DblpError::Download(e.to_string()))?;

    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        progress(BuildProgress::Complete {
            publications: 0,
            authors: 0,
            skipped: true,
        });
        return Ok(false);
    }

    if !response.status().is_success() {
        return Err(DblpError::Download(format!(
            "HTTP error: {}",
            response.status()
        )));
    }

    // Capture new ETag/Last-Modified from response
    let new_etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let new_last_modified = response
        .headers()
        .get("last-modified")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let total_bytes = response.content_length();

    // Phase 1: Download .xml.gz to a temporary file
    let db_dir = db_path.parent().unwrap_or(Path::new("."));
    let tmp_dir = tempfile::TempDir::new_in(db_dir).map_err(DblpError::Io)?;
    let gz_path = tmp_dir.path().join("dblp.xml.gz");

    progress(BuildProgress::Downloading {
        bytes_downloaded: 0,
        total_bytes,
        bytes_decompressed: 0,
    });

    {
        let mut out = File::create(&gz_path)?;
        let mut stream = response.bytes_stream();
        let mut bytes_downloaded: u64 = 0;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| DblpError::Download(e.to_string()))?;
            out.write_all(&chunk)?;
            bytes_downloaded += chunk.len() as u64;

            progress(BuildProgress::Downloading {
                bytes_downloaded,
                total_bytes,
                bytes_decompressed: 0,
            });
        }
        out.flush()?;
    }

    // Phase 2: Parse XML and insert into SQLite.
    // Runs in a blocking thread since XML parsing and SQLite writes are sync I/O.
    let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<BuildProgress>(64);

    let parse_handle = tokio::task::spawn_blocking(move || {
        let _tmp_dir = tmp_dir; // keep temp directory alive until parsing is done

        db::begin_bulk_load(&conn)?;

        parse_and_insert(&conn, &gz_path, |evt| {
            let _ = progress_tx.blocking_send(evt);
        })?;

        let _ = progress_tx.blocking_send(BuildProgress::RebuildingIndex);
        db::end_bulk_load(&conn)?;
        db::rebuild_fts_index(&conn)?;

        // Update metadata
        let timestamp = now_unix_timestamp();
        db::set_metadata(&conn, "last_updated", &timestamp)?;
        db::set_metadata(&conn, "schema_version", "2")?;

        if let Some(etag) = new_etag {
            db::set_metadata(&conn, "etag", &etag)?;
        }
        if let Some(lm) = new_last_modified {
            db::set_metadata(&conn, "last_modified", &lm)?;
        }

        let (pubs, authors, _) = db::get_counts(&conn)?;
        db::set_metadata(&conn, "publication_count", &pubs.to_string())?;
        db::set_metadata(&conn, "author_count", &authors.to_string())?;

        Ok::<(i64, i64), DblpError>((pubs, authors))
    });

    // Forward progress events from the blocking task to the caller
    while let Some(evt) = progress_rx.recv().await {
        progress(evt);
    }

    let (pubs, authors) = parse_handle
        .await
        .map_err(|e| DblpError::Download(format!("parse task panicked: {}", e)))??;

    progress(BuildProgress::Complete {
        publications: pubs as u64,
        authors: authors as u64,
        skipped: false,
    });

    Ok(true)
}

/// Build the offline DBLP database from a local `.xml.gz` file.
pub fn build_from_file(
    db_path: &Path,
    xml_gz_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<(), DblpError> {
    let conn = Connection::open(db_path)?;
    db::init_database(&conn)?;
    db::begin_bulk_load(&conn)?;

    parse_and_insert(&conn, xml_gz_path, &mut progress)?;

    progress(BuildProgress::RebuildingIndex);
    db::end_bulk_load(&conn)?;
    db::rebuild_fts_index(&conn)?;

    let timestamp = now_unix_timestamp();
    db::set_metadata(&conn, "last_updated", &timestamp)?;
    db::set_metadata(&conn, "schema_version", "2")?;

    let (pubs, authors, _) = db::get_counts(&conn)?;
    db::set_metadata(&conn, "publication_count", &pubs.to_string())?;
    db::set_metadata(&conn, "author_count", &authors.to_string())?;

    progress(BuildProgress::Complete {
        publications: pubs as u64,
        authors: authors as u64,
        skipped: false,
    });

    Ok(())
}

/// Wrapper around a `Read` that tracks how many bytes have been consumed.
struct CountingReader<R> {
    inner: R,
    bytes_read: Rc<Cell<u64>>,
}

impl<R: Read> Read for CountingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.bytes_read.set(self.bytes_read.get() + n as u64);
        Ok(n)
    }
}

/// Parse a `.xml.gz` file and insert publications into the database.
fn parse_and_insert(
    conn: &Connection,
    gz_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<(), DblpError> {
    let file = File::open(gz_path)?;
    let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);

    let bytes_read_counter = Rc::new(Cell::new(0u64));
    let bytes_read_for_progress = Rc::clone(&bytes_read_counter);

    let counting = CountingReader {
        inner: file,
        bytes_read: bytes_read_counter,
    };
    let decoder = flate2::read::GzDecoder::new(counting);
    let reader = BufReader::with_capacity(1024 * 1024, decoder);

    let mut batch = InsertBatch::new();
    let mut records_parsed: u64 = 0;
    let mut records_inserted: u64 = 0;
    let mut insert_error: Option<DblpError> = None;

    xml_parser::parse_xml(reader, |pub_record| {
        if insert_error.is_some() {
            return;
        }

        records_parsed += 1;

        let uri = format!("https://dblp.org/rec/{}", pub_record.key);

        batch.publications.push((uri.clone(), pub_record.title));

        for author in &pub_record.authors {
            batch.authors.push((author.clone(), author.clone()));
            batch
                .publication_authors
                .push((uri.clone(), author.clone()));
        }

        if batch.len() >= BATCH_SIZE {
            records_inserted += batch.len() as u64;
            if let Err(e) = db::insert_batch(conn, &batch) {
                insert_error = Some(e);
                return;
            }
            batch.clear();

            progress(BuildProgress::Parsing {
                records_parsed,
                records_inserted,
                bytes_read: bytes_read_for_progress.get(),
                bytes_total: file_size,
            });
        }
    });

    if let Some(err) = insert_error {
        return Err(err);
    }

    // Flush remaining
    if !batch.is_empty() {
        records_inserted += batch.len() as u64;
        db::insert_batch(conn, &batch)?;
    }

    progress(BuildProgress::Parsing {
        records_parsed,
        records_inserted,
        bytes_read: file_size, // done
        bytes_total: file_size,
    });

    Ok(())
}

/// Unix timestamp as a string (seconds since epoch).
fn now_unix_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a minimal .xml.gz file for testing.
    fn create_test_xml_gz() -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<article key="journals/cacm/Knuth74" mdate="2020-01-01">
  <author>Donald E. Knuth</author>
  <title>Computer Programming as an Art.</title>
  <ee>https://doi.org/10.1145/361604.361612</ee>
</article>
<inproceedings key="conf/nips/VaswaniSPUJGKP17" mdate="2023-03-30">
  <author>Ashish Vaswani</author>
  <author>Noam Shazeer</author>
  <title>Attention is All you Need.</title>
</inproceedings>
</dblp>"#;
        encoder.write_all(xml.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn test_build_from_xml_gz() {
        let gz_data = create_test_xml_gz();

        let dir = tempfile::tempdir().unwrap();
        let xml_gz_path = dir.path().join("test.xml.gz");
        let db_path = dir.path().join("test.db");

        std::fs::write(&xml_gz_path, &gz_data).unwrap();

        let mut progress_events = Vec::new();
        build_from_file(&db_path, &xml_gz_path, |evt| {
            progress_events.push(format!("{:?}", evt));
        })
        .unwrap();

        // Verify database contents
        let conn = Connection::open(&db_path).unwrap();
        let (pubs, authors, rels) = db::get_counts(&conn).unwrap();
        assert_eq!(pubs, 2);
        assert_eq!(authors, 3); // Knuth, Vaswani, Shazeer
        assert_eq!(rels, 3);

        // Verify metadata
        let schema = db::get_metadata(&conn, "schema_version").unwrap();
        assert_eq!(schema, Some("2".into()));

        let last_updated = db::get_metadata(&conn, "last_updated").unwrap();
        assert!(last_updated.is_some());

        // Verify FTS works
        let mut stmt = conn
            .prepare(
                "SELECT p.title FROM publications p \
                 WHERE p.id IN (SELECT rowid FROM publications_fts WHERE title MATCH ?1)",
            )
            .unwrap();
        let results: Vec<String> = stmt
            .query_map(["attention"], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "Attention is All you Need.");

        // Verify progress was reported
        assert!(!progress_events.is_empty());
    }

    #[test]
    fn test_parse_and_insert() {
        let gz_data = create_test_xml_gz();

        let dir = tempfile::tempdir().unwrap();
        let xml_gz_path = dir.path().join("test.xml.gz");
        std::fs::write(&xml_gz_path, &gz_data).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        db::init_database(&conn).unwrap();

        parse_and_insert(&conn, &xml_gz_path, |_| {}).unwrap();

        let (pubs, authors, rels) = db::get_counts(&conn).unwrap();
        assert_eq!(pubs, 2);
        assert_eq!(authors, 3);
        assert_eq!(rels, 3);

        // Verify author lookup works with name-based URIs
        let mut paper_authors = db::get_authors_for_publication(
            &conn,
            "https://dblp.org/rec/conf/nips/VaswaniSPUJGKP17",
        )
        .unwrap();
        paper_authors.sort();
        assert_eq!(paper_authors, vec!["Ashish Vaswani", "Noam Shazeer"]);
    }

    #[test]
    fn test_parse_proceedings_with_editors() {
        use flate2::Compression;
        use flate2::write::GzEncoder;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<dblp>
<proceedings key="conf/test/2024">
  <editor>Alice Smith</editor>
  <editor>Bob Jones</editor>
  <title>Proceedings of Test Conference 2024.</title>
</proceedings>
</dblp>"#;
        encoder.write_all(xml.as_bytes()).unwrap();
        let gz_data = encoder.finish().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let xml_gz_path = dir.path().join("test.xml.gz");
        std::fs::write(&xml_gz_path, &gz_data).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        db::init_database(&conn).unwrap();

        parse_and_insert(&conn, &xml_gz_path, |_| {}).unwrap();

        let (pubs, authors, rels) = db::get_counts(&conn).unwrap();
        assert_eq!(pubs, 1);
        assert_eq!(authors, 2);
        assert_eq!(rels, 2);
    }
}
