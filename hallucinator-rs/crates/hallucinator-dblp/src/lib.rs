// Licensed under either AGPL-3.0-or-later or MIT license, at your option.

//! Offline DBLP database builder and querier.
//!
//! Provides a normalized SQLite-backed DBLP index with FTS5 full-text search,
//! streaming N-Triples parsing, ETag-based conditional downloads, and fuzzy
//! title matching via rapidfuzz.

mod builder;
pub mod db;
pub mod parser;
pub mod query;
pub mod xml_parser;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

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
        /// Publications inserted into the database.
        records_inserted: u64,
        /// Compressed bytes consumed from the .xml.gz file.
        bytes_read: u64,
        /// Total compressed file size (for ETA calculation).
        bytes_total: u64,
    },
    RebuildingIndex,
    Compacting,
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

/// Open a connection to `path` and verify it is a compatible offline DBLP
/// database (publications table present, schema version 3).
///
/// Also applies read-side pragmas (`mmap_size`, `cache_size`) — the database
/// is built with `synchronous = NORMAL` / WAL, which is fine for readers, but
/// SQLite's default 2MB page cache is tiny against a multi-GB file, so every
/// query would otherwise re-fault pages from the OS cache.
fn open_and_verify(path: &Path) -> Result<Connection, DblpError> {
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

    // Check schema version — v3 uses integer IDs; older versions are incompatible
    let version = db::get_metadata(&conn, "schema_version")?;
    match version.as_deref() {
        Some("3") => {}
        Some(v) => {
            return Err(DblpError::Parse(format!(
                "DBLP database at {} has schema version {}, but version 3 is required. \
                 Please rebuild with 'hallucinator-tui update-dblp'.",
                path.display(),
                v
            )));
        }
        None => {
            return Err(DblpError::Parse(format!(
                "DBLP database at {} has no schema version. \
                 Please rebuild with 'hallucinator-tui update-dblp'.",
                path.display()
            )));
        }
    }

    // 64MB page cache and a 256MB mapped region make repeated FTS/join
    // lookups cheaper on a multi-GB database; failures here are non-fatal
    // (e.g. platforms where mmap is unsupported), so ignore errors.
    let _ = conn.pragma_update(None, "cache_size", -64000);
    let _ = conn.pragma_update(None, "mmap_size", 268_435_456i64);

    // Non-fatal: powers term-frequency-aware OR-fallback word selection in
    // `query::query_fts_with_authors`. If this fails (exotic sqlite build),
    // that code degrades to its original extraction-order behavior.
    let _ = db::ensure_vocab_table(&conn);

    Ok(conn)
}

/// Handle to an opened offline DBLP database.
pub struct DblpDatabase {
    conn: Connection,
    path: PathBuf,
}

impl DblpDatabase {
    /// Open an existing offline DBLP database.
    ///
    /// Verifies that the schema tables exist and the schema version is compatible.
    pub fn open(path: &Path) -> Result<Self, DblpError> {
        let conn = open_and_verify(path)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Query for a title, returning the best fuzzy match above the default threshold.
    pub fn query(&self, title: &str) -> Result<Option<DblpQueryResult>, DblpError> {
        query::query_fts(&self.conn, title, DEFAULT_THRESHOLD)
    }

    /// Query for a title, using the citation's authors to break ties among
    /// candidates with comparable title similarity. See
    /// [`query::query_fts_with_authors`] for the scoring details.
    pub fn query_with_authors(
        &self,
        title: &str,
        ref_authors: &[String],
    ) -> Result<Option<DblpQueryResult>, DblpError> {
        query::query_fts_with_authors(&self.conn, title, ref_authors, DEFAULT_THRESHOLD)
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

/// Default number of read connections held by [`DblpPool`], matching the
/// default reference-processing concurrency (`num_workers` in
/// `hallucinator-core`) so concurrent lookups don't serialize on one
/// connection.
pub const DEFAULT_POOL_SIZE: usize = 4;

/// A small fixed pool of read connections to an offline DBLP database.
///
/// `DblpDatabase` wraps a single `Connection`, so callers that share one
/// behind an `Arc<Mutex<_>>` end up serializing every lookup — including the
/// fuzzy-match scoring — through a single global lock, even though SQLite in
/// WAL mode supports multiple concurrent readers just fine. `DblpPool` opens
/// several independent connections up front and round-robins across them, so
/// concurrent reference checks can query DBLP in parallel.
pub struct DblpPool {
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
    path: PathBuf,
}

impl DblpPool {
    /// Open a pool of [`DEFAULT_POOL_SIZE`] read connections.
    pub fn open(path: &Path) -> Result<Self, DblpError> {
        Self::open_with_size(path, DEFAULT_POOL_SIZE)
    }

    /// Open a pool of `size` read connections (minimum 1).
    pub fn open_with_size(path: &Path, size: usize) -> Result<Self, DblpError> {
        let size = size.max(1);
        let mut conns = Vec::with_capacity(size);
        for _ in 0..size {
            conns.push(Mutex::new(open_and_verify(path)?));
        }
        Ok(Self {
            conns,
            next: AtomicUsize::new(0),
            path: path.to_path_buf(),
        })
    }

    /// Borrow the next connection in round-robin order and run `f` with it.
    ///
    /// Recovers from lock poisoning rather than propagating it: a panic
    /// while holding the lock doesn't leave the underlying SQLite connection
    /// in an inconsistent state, so continuing to use it is safe.
    fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, DblpError>,
    ) -> Result<T, DblpError> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        let conn = self.conns[idx]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&conn)
    }

    /// Query for a title, returning the best fuzzy match above the default threshold.
    pub fn query(&self, title: &str) -> Result<Option<DblpQueryResult>, DblpError> {
        self.with_conn(|conn| query::query_fts(conn, title, DEFAULT_THRESHOLD))
    }

    /// Query for a title, using the citation's authors to break ties among
    /// candidates with comparable title similarity. See
    /// [`query::query_fts_with_authors`] for the scoring details.
    pub fn query_with_authors(
        &self,
        title: &str,
        ref_authors: &[String],
    ) -> Result<Option<DblpQueryResult>, DblpError> {
        self.with_conn(|conn| {
            query::query_fts_with_authors(conn, title, ref_authors, DEFAULT_THRESHOLD)
        })
    }

    /// Query with a custom similarity threshold.
    pub fn query_with_threshold(
        &self,
        title: &str,
        threshold: f64,
    ) -> Result<Option<DblpQueryResult>, DblpError> {
        self.with_conn(|conn| query::query_fts(conn, title, threshold))
    }

    /// Get database metadata/info.
    pub fn info(&self) -> Result<DatabaseInfo, DblpError> {
        self.with_conn(|conn| {
            Ok(DatabaseInfo {
                build_date: db::get_metadata(conn, "last_updated")?,
                schema_version: db::get_metadata(conn, "schema_version")?,
                publication_count: db::get_metadata(conn, "publication_count")?,
                author_count: db::get_metadata(conn, "author_count")?,
                etag: db::get_metadata(conn, "etag")?,
                last_modified: db::get_metadata(conn, "last_modified")?,
            })
        })
    }

    /// Check if the database is stale (older than `threshold_days`).
    pub fn check_staleness(&self, threshold_days: u64) -> Result<StalenessCheck, DblpError> {
        self.with_conn(|conn| {
            let build_date = db::get_metadata(conn, "last_updated")?;

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
