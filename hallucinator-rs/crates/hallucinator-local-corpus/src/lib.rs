// Licensed under either AGPL-3.0-or-later or MIT license, at your option.

//! Local corpus of manually-verified and not-yet-indexed papers.
//!
//! Two problems, one mechanism:
//!
//! 1. **Recent conferences aren't indexed anywhere yet.** CrossRef, DBLP,
//!    Semantic Scholar etc. only pick up a conference weeks to months after
//!    it runs, so a real citation to a just-published NDSS/USENIX paper
//!    looks identical to a hallucination until then.
//! 2. **`fp_overrides` (cache.db's "mark safe" table) matches by exact
//!    identity key** (normalized title + author fingerprint set) — brittle
//!    against citation-style drift: a subtitle dropped, an "(Extended
//!    Version)" suffix added, an et-al-truncated author list. Two citations
//!    of the literal same paper can fail to share an identity key.
//!
//! Both are fixed by the same offline SQLite + FTS5 index used elsewhere in
//! this codebase (mirroring `hallucinator-acl`'s architecture): entries are
//! tagged by provenance (`source` column — `"ndss2026"`, `"usenix2026"`,
//! `"marked_safe:known_good"`, ...) and matched via fuzzy title search
//! (`rapidfuzz` ratio, 95% threshold) instead of an exact key, so wording
//! drift across citations doesn't matter. Registered as a `DatabaseBackend`
//! in `hallucinator-core`, it gets real Verified/Author-Mismatch
//! classification from the existing generic pipeline for free.

mod db;
pub mod ingest;
mod insert;
mod query;

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rusqlite::Connection;
use thiserror::Error;

pub use db::{NewPublication, count_by_source, total_count};
pub use ingest::{HtmlSource, ImportStats};
pub use insert::InsertOutcome;
pub use query::DEFAULT_THRESHOLD;

#[derive(Error, Debug)]
pub enum CorpusError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("fetch error: {0}")]
    Fetch(String),
    #[error("parse error: {0}")]
    Parse(String),
}

/// A publication record returned from a query.
#[derive(Debug, Clone)]
pub struct CorpusRecord {
    pub title: String,
    pub authors: Vec<String>,
    pub url: Option<String>,
    /// Provenance tag, e.g. `"ndss2026"` or `"marked_safe:known_good"`.
    pub source: String,
}

/// Query result with fuzzy match score.
#[derive(Debug, Clone)]
pub struct CorpusQueryResult {
    pub record: CorpusRecord,
    pub score: f64,
}

/// Open (creating if necessary) a corpus database and ensure its schema
/// exists. Used by import commands, which need read+write access.
pub fn open_or_create(path: &Path) -> Result<Connection, CorpusError> {
    let conn = Connection::open(path)?;
    db::init_database(&conn)?;
    Ok(conn)
}

/// Open an existing corpus database read-only, verifying it has the
/// expected schema. Used when querying during a `check` run.
fn open_and_verify(path: &Path) -> Result<Connection, CorpusError> {
    let conn = Connection::open(path)?;

    let table_exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='publications'",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Err(CorpusError::Database(rusqlite::Error::QueryReturnedNoRows));
    }

    let _ = conn.pragma_update(None, "cache_size", -64000);
    let _ = conn.pragma_update(None, "mmap_size", 268_435_456i64);

    Ok(conn)
}

/// Default number of read connections held by [`CorpusPool`]. Matches
/// `hallucinator_acl::DEFAULT_POOL_SIZE` — same rationale (one connection
/// per concurrent reference-check worker avoids serializing on a single
/// mutex-guarded connection).
pub const DEFAULT_POOL_SIZE: usize = 4;

/// A small fixed pool of read connections to a local corpus database.
/// Mirrors `hallucinator_acl::AclPool`.
pub struct CorpusPool {
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
    path: PathBuf,
}

impl CorpusPool {
    /// Open a pool of [`DEFAULT_POOL_SIZE`] read connections.
    pub fn open(path: &Path) -> Result<Self, CorpusError> {
        Self::open_with_size(path, DEFAULT_POOL_SIZE)
    }

    /// Open a pool of `size` read connections (minimum 1).
    pub fn open_with_size(path: &Path, size: usize) -> Result<Self, CorpusError> {
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

    fn with_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, CorpusError>,
    ) -> Result<T, CorpusError> {
        let idx = self.next.fetch_add(1, Ordering::Relaxed) % self.conns.len();
        let conn = self.conns[idx]
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&conn)
    }

    /// Query for a title, returning the best fuzzy match above the default threshold.
    pub fn query(&self, title: &str) -> Result<Option<CorpusQueryResult>, CorpusError> {
        self.with_conn(|conn| query::query_fts(conn, title, DEFAULT_THRESHOLD))
    }

    /// Query with a custom similarity threshold.
    pub fn query_with_threshold(
        &self,
        title: &str,
        threshold: f64,
    ) -> Result<Option<CorpusQueryResult>, CorpusError> {
        self.with_conn(|conn| query::query_fts(conn, title, threshold))
    }

    /// Total publication count across all sources.
    pub fn total_count(&self) -> Result<i64, CorpusError> {
        self.with_conn(db::total_count)
    }

    /// Get the path to the database file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}
