// Licensed under either AGPL-3.0-or-later or MIT license, at your option.

//! Offline IACR Cryptology ePrint Archive database.
//!
//! IACR ePrint exposes an OAI-PMH 2.0 feed at
//! `https://eprint.iacr.org/oai` with `oai_dc` (Dublin Core) records
//! for every paper. The archive is small (~25–30k papers as of
//! 2026), so a full harvest is a minutes-scale build producing a
//! SQLite + FTS5 index of similar shape to `hallucinator-acl` and
//! `hallucinator-arxiv-offline`.
//!
//! Unlike those cousins, IACR ePrint has *no* public search API:
//! title search is only possible via a local FTS index, so the
//! offline backend is the only way to resolve title-only references
//! against the archive. ID-based lookup (`YYYY/N`) is also served
//! from the same local index — extracting the ID from the raw
//! citation and calling [`IacrDatabase::lookup_by_id`] skips title
//! matching entirely.
//!
//! Metadata is CC0 per the IACR harvesting-policy page
//! (<https://eprint.iacr.org/rss>), and the site asks harvesters to
//! refresh no more than once per day — the builder supports
//! incremental updates via OAI-PMH `from=` to honour that.

pub mod builder;
pub mod db;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IacrError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("harvest error: {0}")]
    Harvest(String),
    #[error("XML parse error: {0}")]
    Xml(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// One IACR ePrint paper. The OAI-PMH feed emits `oai_dc` (Dublin
/// Core), which is flat and string-typed — no separate forename /
/// surname splits, no DOIs (ePrint preprints generally aren't
/// DOI-registered until they're republished in a venue), no per-
/// revision metadata. We store just what's needed for title +
/// author matching.
#[derive(Debug, Clone)]
pub struct IacrRecord {
    /// Canonical ePrint identifier, e.g. `"2022/252"`.
    pub id: String,
    /// Paper title (latest revision).
    pub title: String,
    /// Authors in listed order — free-form strings, exactly as the
    /// submitter entered them.
    pub authors: Vec<String>,
    /// IACR category (`Applications`, `Attacks`, `Foundations`,
    /// `Implementation`, `Protocols`, `Public-key cryptography`,
    /// `Secret-key cryptography`, …), or `None` if unclassified.
    pub category: Option<String>,
    /// ISO-8601 submission / last-revision timestamp as reported by
    /// the feed. Free-form; not parsed downstream, used for
    /// diagnostics and as a tiebreaker if multiple records match.
    pub date: Option<String>,
}

/// Result of a staleness check on an offline IACR database.
#[derive(Debug, Clone)]
pub struct StalenessCheck {
    pub is_stale: bool,
    pub age_days: Option<u64>,
    pub build_date: Option<String>,
}

/// Progress events emitted by the builder so the CLI / TUI can show
/// a live status line during the harvest.
#[derive(Debug, Clone)]
pub enum BuildProgress {
    Starting { incremental_from: Option<String> },
    Fetched { records: u64, pages: u64 },
    Indexed { records: u64 },
    Complete { records: u64, skipped: bool },
}

/// Handle to an opened offline IACR ePrint database.
pub struct IacrDatabase {
    conn: Connection,
    #[allow(dead_code)] // Retained for parity with sibling crates (dblp/arxiv/acl).
    path: PathBuf,
}

impl IacrDatabase {
    /// Open an existing offline database. Fails if the schema hasn't
    /// been initialized — callers get a clear error rather than
    /// silent empty results.
    pub fn open(path: &Path) -> Result<Self, IacrError> {
        let conn = Connection::open(path)?;
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='papers'",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(IacrError::Database(rusqlite::Error::QueryReturnedNoRows));
        }
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -64000;",
        )?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Create a fresh database with the IACR schema applied.
    pub fn create(path: &Path) -> Result<Self, IacrError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        db::create_schema(&conn)?;
        db::set_metadata(&conn, "schema_version", db::SCHEMA_VERSION)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Exact-ID lookup (e.g. `"2022/252"`). Returns `None` when the
    /// archive doesn't carry that paper.
    pub fn lookup_by_id(&self, id: &str) -> Result<Option<IacrRecord>, IacrError> {
        db::lookup_by_id(&self.conn, id)
    }

    /// Title search. Returns up to `limit` full records in BM25-rank
    /// order; caller does its own fuzzy title + author match.
    pub fn search_by_title(&self, query: &str, limit: usize) -> Result<Vec<IacrRecord>, IacrError> {
        db::search_by_title(&self.conn, query, limit)
    }

    /// Insert / replace a single record. Used by the OAI-PMH
    /// harvester; keeps the FTS5 index in sync on every write.
    pub fn upsert(&self, rec: &IacrRecord) -> Result<(), IacrError> {
        db::upsert_record(&self.conn, rec)
    }

    /// Borrow the underlying SQLite connection — useful for the
    /// builder's resumption-token bookkeeping.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Record a metadata value (used by the builder to persist the
    /// OAI-PMH `from` timestamp for incremental refreshes).
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<(), IacrError> {
        db::set_metadata(&self.conn, key, value)
    }

    /// Read a metadata value previously stored via [`set_metadata`].
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>, IacrError> {
        db::get_metadata(&self.conn, key)
    }

    /// Record the last-successful-harvest ISO-8601 timestamp. Used
    /// by [`staleness`](Self::staleness) and as the starting point
    /// for incremental refreshes.
    pub fn record_build_date(&self, iso_date: &str) -> Result<(), IacrError> {
        db::set_metadata(&self.conn, "build_date", iso_date)
    }

    /// Number of papers in the database.
    pub fn paper_count(&self) -> Result<u64, IacrError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM papers", [], |row| row.get(0))?;
        Ok(n as u64)
    }

    /// Whether the database is older than `threshold_days`. Matches
    /// the shape used by the sibling offline DBs so the CLI staleness
    /// banner code can treat them uniformly.
    pub fn staleness(&self, threshold_days: u64) -> Result<StalenessCheck, IacrError> {
        let build_date = db::get_metadata(&self.conn, "build_date")?;
        let Some(date) = &build_date else {
            return Ok(StalenessCheck {
                is_stale: false,
                age_days: None,
                build_date: None,
            });
        };
        let age_days = iso_date_age_days(date);
        Ok(StalenessCheck {
            is_stale: age_days.is_some_and(|d| d >= threshold_days),
            age_days,
            build_date,
        })
    }
}

/// Days between an ISO `YYYY-MM-DD` string and today. Returns `None`
/// on parse failure so the caller treats the age as unknown rather
/// than warning spuriously.
fn iso_date_age_days(s: &str) -> Option<u64> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let year: i32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    let then = ymd_to_days(year, month, day)?;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let now_days = (now_secs / 86_400) as i64;
    let epoch_days_to_then = then - 719_162;
    Some((now_days - epoch_days_to_then).max(0) as u64)
}

fn ymd_to_days(year: i32, month: u32, day: u32) -> Option<i64> {
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    let (y, m) = if month > 2 {
        (year as i64, month as i64 - 3)
    } else {
        (year as i64 - 1, month as i64 + 9)
    };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let doy = (153 * m as u64 + 2) / 5 + (day as u64 - 1);
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_then_open_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("iacr.db");
        {
            let db = IacrDatabase::create(&path).unwrap();
            db.upsert(&IacrRecord {
                id: "2022/252".into(),
                title: "Handcrafting Masked AES".into(),
                authors: vec!["Charles Momin".into(), "Gaëtan Cassiers".into()],
                category: Some("Implementation".into()),
                date: Some("2022-03-02T13:58:42Z".into()),
            })
            .unwrap();
            db.record_build_date("2026-04-24").unwrap();
        }
        let reopened = IacrDatabase::open(&path).unwrap();
        assert_eq!(reopened.paper_count().unwrap(), 1);
        let got = reopened.lookup_by_id("2022/252").unwrap().unwrap();
        assert_eq!(got.title, "Handcrafting Masked AES");
        assert_eq!(got.authors.len(), 2);
    }

    #[test]
    fn fts_finds_records_by_title_tokens() {
        // Minimal end-to-end: FTS5 must return a seeded record for a
        // non-exact title query so the DatabaseBackend's fuzzy match
        // has candidates to compare against.
        let dir = tempdir().unwrap();
        let path = dir.path().join("iacr.db");
        let db = IacrDatabase::create(&path).unwrap();
        db.upsert(&IacrRecord {
            id: "2024/100".into(),
            title: "Efficient zero-knowledge proofs of knowledge".into(),
            authors: vec!["A. Author".into()],
            category: None,
            date: None,
        })
        .unwrap();
        let results = db.search_by_title("zero knowledge proofs", 5).unwrap();
        assert!(
            results.iter().any(|r| r.id == "2024/100"),
            "expected ID 2024/100, got {:?}",
            results.iter().map(|r| &r.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn open_rejects_fresh_file_without_schema() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.db");
        let _ = rusqlite::Connection::open(&path).unwrap();
        match IacrDatabase::open(&path) {
            Err(IacrError::Database(_)) => {}
            Err(other) => panic!("expected Database error, got {other:?}"),
            Ok(_) => panic!("open() on an empty file should fail"),
        }
    }
}
