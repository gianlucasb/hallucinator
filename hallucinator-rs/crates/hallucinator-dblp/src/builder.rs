//! Download and build pipeline for the offline DBLP database.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use flate2::read::GzDecoder;
use rusqlite::Connection;

use crate::db::{self, InsertBatch};
use crate::parser;
use crate::{BuildProgress, DblpError};

/// Default DBLP RDF dump URL.
pub const DEFAULT_DBLP_URL: &str = "https://dblp.org/rdf/dblp.nt.gz";

/// Default batch size for database inserts.
const BATCH_SIZE: usize = 10_000;

/// Build (or update) the offline DBLP database by downloading from dblp.org.
///
/// Uses ETag/Last-Modified headers for conditional requests — if the remote
/// file hasn't changed since the last build, returns `Ok(false)` without
/// re-downloading.
///
/// Returns `Ok(true)` if the database was rebuilt, `Ok(false)` if skipped.
pub fn build(
    db_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<bool, DblpError> {
    let conn = Connection::open(db_path)?;
    db::init_database(&conn)?;

    // Check stored ETag/Last-Modified
    let stored_etag = db::get_metadata(&conn, "etag")?;
    let stored_last_modified = db::get_metadata(&conn, "last_modified")?;

    progress(BuildProgress::Downloading {
        bytes_downloaded: 0,
        total_bytes: None,
    });

    // Build the blocking HTTP client
    let client = reqwest::blocking::Client::builder()
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

    // Stream response through gzip decompression
    let decoder = GzDecoder::new(response);
    let reader = BufReader::with_capacity(1024 * 1024, decoder);

    process_lines(&conn, reader, total_bytes, &mut progress)?;

    // Rebuild FTS index
    progress(BuildProgress::RebuildingIndex);
    db::rebuild_fts_index(&conn)?;

    // Update metadata
    let timestamp = now_rfc3339();
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

    progress(BuildProgress::Complete {
        publications: pubs as u64,
        authors: authors as u64,
        skipped: false,
    });

    Ok(true)
}

/// Build the offline DBLP database from a local `.nt.gz` file.
pub fn build_from_file(
    db_path: &Path,
    nt_gz_path: &Path,
    mut progress: impl FnMut(BuildProgress),
) -> Result<(), DblpError> {
    let conn = Connection::open(db_path)?;
    db::init_database(&conn)?;

    let file = File::open(nt_gz_path)?;
    let file_size = file.metadata().map(|m| m.len()).ok();

    progress(BuildProgress::Parsing {
        lines_processed: 0,
        records_inserted: 0,
    });

    let decoder = GzDecoder::new(file);
    let reader = BufReader::with_capacity(1024 * 1024, decoder);

    process_lines(&conn, reader, file_size, &mut progress)?;

    // Rebuild FTS index
    progress(BuildProgress::RebuildingIndex);
    db::rebuild_fts_index(&conn)?;

    // Update metadata
    let timestamp = now_rfc3339();
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

/// Process lines from a buffered reader, routing triples into batch inserts.
fn process_lines<R: BufRead>(
    conn: &Connection,
    reader: R,
    _total_bytes: Option<u64>,
    progress: &mut impl FnMut(BuildProgress),
) -> Result<(), DblpError> {
    let mut batch = InsertBatch::new();
    let mut lines_processed: u64 = 0;
    let mut records_inserted: u64 = 0;

    for line_result in reader.lines() {
        let line = line_result?;
        lines_processed += 1;

        if lines_processed % 100_000 == 0 {
            progress(BuildProgress::Parsing {
                lines_processed,
                records_inserted,
            });
        }

        let triple = match parser::parse_line(&line) {
            Some(t) => t,
            None => continue,
        };

        // Route triple by predicate
        match triple.predicate.as_str() {
            parser::TITLE | parser::DC_TITLE => {
                if !triple.object_is_uri {
                    batch.publications.push((triple.subject, triple.object));
                }
            }
            parser::AUTHORED_BY => {
                if triple.object_is_uri {
                    batch
                        .publication_authors
                        .push((triple.subject, triple.object));
                }
            }
            parser::PRIMARY_CREATOR_NAME | parser::CREATOR_NAME => {
                if !triple.object_is_uri {
                    batch.authors.push((triple.subject, triple.object));
                }
            }
            _ => {}
        }

        // Flush batch when full
        if batch.len() >= BATCH_SIZE {
            records_inserted += batch.len() as u64;
            db::insert_batch(conn, &batch)?;
            batch.clear();
        }
    }

    // Flush remaining
    if !batch.is_empty() {
        records_inserted += batch.len() as u64;
        db::insert_batch(conn, &batch)?;
    }

    progress(BuildProgress::Parsing {
        lines_processed,
        records_inserted,
    });

    Ok(())
}

/// Simple RFC 3339 timestamp without pulling in chrono.
fn now_rfc3339() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Very simple: just store as unix timestamp string. The staleness check
    // will parse this back. For a proper RFC 3339 string we'd need chrono,
    // but storing seconds since epoch is sufficient for our purposes.
    secs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Create a minimal .nt.gz file in memory for testing.
    fn create_test_nt_gz() -> Vec<u8> {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        let data = r#"<https://dblp.org/rec/conf/test/Paper1> <https://dblp.org/rdf/schema#title> "Test Paper One" .
<https://dblp.org/rec/conf/test/Paper2> <https://dblp.org/rdf/schema#title> "Another Test Paper" .
<https://dblp.org/pid/00/1> <https://dblp.org/rdf/schema#primaryCreatorName> "Alice Smith" .
<https://dblp.org/pid/00/2> <https://dblp.org/rdf/schema#primaryCreatorName> "Bob Jones" .
<https://dblp.org/rec/conf/test/Paper1> <https://dblp.org/rdf/schema#authoredBy> <https://dblp.org/pid/00/1> .
<https://dblp.org/rec/conf/test/Paper1> <https://dblp.org/rdf/schema#authoredBy> <https://dblp.org/pid/00/2> .
<https://dblp.org/rec/conf/test/Paper2> <https://dblp.org/rdf/schema#authoredBy> <https://dblp.org/pid/00/1> .
# This is a comment
"#;
        encoder.write_all(data.as_bytes()).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn test_build_from_gz_bytes() {
        let gz_data = create_test_nt_gz();

        // Write to a temp file
        let dir = tempfile::tempdir().unwrap();
        let nt_gz_path = dir.path().join("test.nt.gz");
        let db_path = dir.path().join("test.db");

        std::fs::write(&nt_gz_path, &gz_data).unwrap();

        let mut progress_events = Vec::new();
        build_from_file(&db_path, &nt_gz_path, |evt| {
            progress_events.push(format!("{:?}", evt));
        })
        .unwrap();

        // Verify database contents
        let conn = Connection::open(&db_path).unwrap();
        let (pubs, authors, rels) = db::get_counts(&conn).unwrap();
        assert_eq!(pubs, 2);
        assert_eq!(authors, 2);
        assert_eq!(rels, 3);

        // Verify metadata was set
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
            .query_map(["test"], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(results.len(), 2);

        // Verify progress was reported
        assert!(!progress_events.is_empty());
    }

    #[test]
    fn test_process_lines_routes_triples_correctly() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_database(&conn).unwrap();

        let data = r#"<https://dblp.org/rec/1> <https://dblp.org/rdf/schema#title> "Paper Title" .
<https://dblp.org/pid/1> <https://dblp.org/rdf/schema#creatorName> "Author Name" .
<https://dblp.org/rec/1> <https://dblp.org/rdf/schema#authoredBy> <https://dblp.org/pid/1> .
<https://dblp.org/rec/1> <http://purl.org/dc/terms/title> "Alt Title" .
"#;
        let reader = BufReader::new(data.as_bytes());
        process_lines(&conn, reader, None, &mut |_| {}).unwrap();

        let (pubs, authors, rels) = db::get_counts(&conn).unwrap();
        assert_eq!(pubs, 1); // Two titles for same URI → UPSERT keeps one
        assert_eq!(authors, 1);
        assert_eq!(rels, 1);
    }
}
