//! Offline DBLP database builder and querier.
//!
//! Provides a normalized SQLite-backed DBLP index with FTS5 full-text search,
//! streaming N-Triples parsing, ETag-based conditional downloads, and fuzzy
//! title matching via rapidfuzz.

mod builder;
mod db;
pub mod parser;
mod query;
pub mod xml_parser;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

// Re-export for convenience
pub use builder::DEFAULT_DBLP_URL;
pub use query::DEFAULT_THRESHOLD;

#[derive(Error, Debug)]
pub enum DblpError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("download error: {0}")]
    Download(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A publication record from the offline DBLP database.
#[derive(Debug, Clone)]
pub struct DblpRecord {
    pub title: String,
    pub authors: Vec<String>,
    pub url: Option<String>,
}

/// Query result with fuzzy match score.
#[derive(Debug, Clone)]
pub struct DblpQueryResult {
    pub record: DblpRecord,
    pub score: f64,
}

/// Database build/download statistics.
#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub build_date: Option<String>,
    pub schema_version: Option<String>,
    pub publication_count: Option<String>,
    pub author_count: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Progress events emitted during database building.
#[derive(Debug, Clone)]
pub enum BuildProgress {
    Downloading {
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
        bytes_decompressed: u64,
    },
    Parsing {
        /// Publications found by the XML parser.
        records_parsed: u64,
        /// DB operations committed to SQLite.
        records_inserted: u64,
        /// Compressed bytes consumed from the .xml.gz file.
        bytes_read: u64,
        /// Total compressed file size (for ETA calculation).
        bytes_total: u64,
    },
    RebuildingIndex,
    Complete {
        publications: u64,
        authors: u64,
        skipped: bool,
    },
}

/// Result of a staleness check.
#[derive(Debug, Clone)]
pub struct StalenessCheck {
    pub is_stale: bool,
    pub age_days: Option<u64>,
    pub build_date: Option<String>,
}

/// Handle to an opened offline DBLP database.
pub struct DblpDatabase {
    conn: Connection,
    path: PathBuf,
}

impl DblpDatabase {
    /// Open an existing offline DBLP database.
    ///
    /// Verifies that the schema tables exist.
    pub fn open(path: &Path) -> Result<Self, DblpError> {
        let conn = Connection::open(path)?;

        // Verify the database has been initialized by checking for the publications table
        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='publications'",
            [],
            |row| row.get(0),
        )?;

        if !table_exists {
            return Err(DblpError::Database(rusqlite::Error::QueryReturnedNoRows));
        }

        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Query for a title, returning the best fuzzy match above the default threshold.
    pub fn query(&self, title: &str) -> Result<Option<DblpQueryResult>, DblpError> {
        query::query_fts(&self.conn, title, DEFAULT_THRESHOLD)
    }

    /// Query with a custom similarity threshold.
    pub fn query_with_threshold(
        &self,
        title: &str,
        threshold: f64,
    ) -> Result<Option<DblpQueryResult>, DblpError> {
        query::query_fts(&self.conn, title, threshold)
    }

    /// Get database metadata/info.
    pub fn info(&self) -> Result<DatabaseInfo, DblpError> {
        Ok(DatabaseInfo {
            build_date: db::get_metadata(&self.conn, "last_updated")?,
            schema_version: db::get_metadata(&self.conn, "schema_version")?,
            publication_count: db::get_metadata(&self.conn, "publication_count")?,
            author_count: db::get_metadata(&self.conn, "author_count")?,
            etag: db::get_metadata(&self.conn, "etag")?,
            last_modified: db::get_metadata(&self.conn, "last_modified")?,
        })
    }

    /// Check if the database is stale (older than `threshold_days`).
    pub fn check_staleness(&self, threshold_days: u64) -> Result<StalenessCheck, DblpError> {
        let build_date = db::get_metadata(&self.conn, "last_updated")?;

        let age_days = build_date.as_ref().and_then(|ts| {
            let build_secs: u64 = ts.parse().ok()?;
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some((now_secs.saturating_sub(build_secs)) / 86400)
        });

        let is_stale = age_days.is_none_or(|days| days >= threshold_days);

        Ok(StalenessCheck {
            is_stale,
            age_days,
            build_date,
        })
    }

    /// Convenience: check staleness with the default 30-day threshold.
    pub fn is_stale(&self) -> Result<bool, DblpError> {
        Ok(self.check_staleness(30)?.is_stale)
    }

    /// Get the path to the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Download and build (or update) the offline DBLP database.
///
/// Uses ETag/Last-Modified for conditional requests. Returns `false` if the
/// remote file hasn't changed since the last build (no work done).
pub async fn build_database(
    db_path: &Path,
    progress: impl FnMut(BuildProgress),
) -> Result<bool, DblpError> {
    builder::build(db_path, progress).await
}

/// Build the offline DBLP database from a local `.xml.gz` file.
pub fn build_database_from_file(
    db_path: &Path,
    xml_gz_path: &Path,
    progress: impl FnMut(BuildProgress),
) -> Result<(), DblpError> {
    builder::build_from_file(db_path, xml_gz_path, progress)
}
