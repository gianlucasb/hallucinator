//! Offline ACL Anthology database builder and querier.
//!
//! Provides a normalized SQLite-backed ACL Anthology index with FTS5 full-text
//! search, GitHub tarball download/extraction, and fuzzy title matching via
//! rapidfuzz. Mirrors the `hallucinator-dblp` crate's architecture.

mod builder;
mod db;
mod query;
mod xml_parser;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

pub use query::DEFAULT_THRESHOLD;

#[derive(Error, Debug)]
pub enum AclError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("download error: {0}")]
    Download(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A publication record from the offline ACL Anthology database.
#[derive(Debug, Clone)]
pub struct AclRecord {
    pub title: String,
    pub authors: Vec<String>,
    pub url: Option<String>,
}

/// Query result with fuzzy match score.
#[derive(Debug, Clone)]
pub struct AclQueryResult {
    pub record: AclRecord,
    pub score: f64,
}

/// Database build/download statistics.
#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub build_date: Option<String>,
    pub schema_version: Option<String>,
    pub publication_count: Option<String>,
    pub author_count: Option<String>,
    pub commit_sha: Option<String>,
}

/// Progress events emitted during database building.
#[derive(Debug, Clone)]
pub enum BuildProgress {
    Downloading {
        bytes_downloaded: u64,
        total_bytes: Option<u64>,
    },
    Extracting {
        files_extracted: u64,
    },
    Parsing {
        records_parsed: u64,
        records_inserted: u64,
        files_processed: u64,
        files_total: u64,
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

/// Handle to an opened offline ACL Anthology database.
pub struct AclDatabase {
    conn: Connection,
    path: PathBuf,
}

impl AclDatabase {
    /// Open an existing offline ACL Anthology database.
    pub fn open(path: &Path) -> Result<Self, AclError> {
        let conn = Connection::open(path)?;

        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='publications'",
            [],
            |row| row.get(0),
        )?;

        if !table_exists {
            return Err(AclError::Database(rusqlite::Error::QueryReturnedNoRows));
        }

        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Query for a title, returning the best fuzzy match above the default threshold.
    pub fn query(&self, title: &str) -> Result<Option<AclQueryResult>, AclError> {
        query::query_fts(&self.conn, title, DEFAULT_THRESHOLD)
    }

    /// Query with a custom similarity threshold.
    pub fn query_with_threshold(
        &self,
        title: &str,
        threshold: f64,
    ) -> Result<Option<AclQueryResult>, AclError> {
        query::query_fts(&self.conn, title, threshold)
    }

    /// Get database metadata/info.
    pub fn info(&self) -> Result<DatabaseInfo, AclError> {
        Ok(DatabaseInfo {
            build_date: db::get_metadata(&self.conn, "last_updated")?,
            schema_version: db::get_metadata(&self.conn, "schema_version")?,
            publication_count: db::get_metadata(&self.conn, "publication_count")?,
            author_count: db::get_metadata(&self.conn, "author_count")?,
            commit_sha: db::get_metadata(&self.conn, "commit_sha")?,
        })
    }

    /// Check if the database is stale (older than `threshold_days`).
    pub fn check_staleness(&self, threshold_days: u64) -> Result<StalenessCheck, AclError> {
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
    pub fn is_stale(&self) -> Result<bool, AclError> {
        Ok(self.check_staleness(30)?.is_stale)
    }

    /// Get the path to the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Download and build (or update) the offline ACL Anthology database.
///
/// Downloads the GitHub tarball, extracts XML files, parses them, and builds
/// an FTS5-indexed SQLite database. Uses commit SHA for conditional updates.
/// Returns `false` if the remote hasn't changed since the last build.
pub async fn build_database(
    db_path: &Path,
    progress: impl FnMut(BuildProgress),
) -> Result<bool, AclError> {
    builder::build(db_path, progress).await
}
