// Licensed under either AGPL-3.0-or-later or MIT license, at your option.

//! Offline arXiv metadata database builder and querier.
//!
//! Ingests the weekly Kaggle `Cornell-University/arxiv` snapshot into
//! a SQLite database with an FTS5 title index. Sibling crate to
//! `hallucinator-dblp` and `hallucinator-acl`; same build/query/
//! staleness idioms, different upstream source (Kaggle JSONL dump
//! instead of RDF / XML).
//!
//! Architecture:
//! - [`download`] fetches the Kaggle zip via the public dataset API.
//! - [`ingest`] streams JSONL records out of the zip into [`ArxivRecord`]s.
//! - [`db`] owns the SQLite schema and the single-row lookup /
//!   FTS title search used by the online backend.
//! - [`ArxivDatabase`] is the high-level handle the rest of the
//!   workspace holds (similar to `DblpDatabase` / `AclDatabase`).

pub mod db;
pub mod download;
pub mod ingest;

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ArxivError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("harvest error: {0}")]
    Harvest(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A single parsed record from the arXiv OAI-PMH feed.
#[derive(Debug, Clone)]
pub struct ArxivRecord {
    /// Canonical arXiv identifier, e.g., `"2403.00108"` or `"hep-th/9901001"`.
    pub id: String,
    /// Latest-version title. arXiv retitles papers between versions
    /// sometimes (see f168 ref 8 on the NDSS 2026 corpus); `arXivRaw`
    /// only carries the latest title, so offline lookups use this and
    /// the earlier-version fallback still needs the live API.
    pub title: String,
    /// Authors in listed order, each as a single string (full name as
    /// arXiv publishes it — whatever split the submitter chose).
    pub authors: Vec<String>,
    /// Space-separated category list, e.g. `"cs.CR cs.AI cs.CL"`.
    pub categories: Option<String>,
    /// DOI, if the author declared one in the submission.
    pub doi: Option<String>,
    /// License URL as reported by arXiv.
    pub license: Option<String>,
    /// Per-version submission metadata (latest version last).
    pub versions: Vec<ArxivVersion>,
}

impl ArxivRecord {
    /// Latest version number recorded. 1 by default when the feed
    /// omitted version info for this record.
    pub fn latest_version(&self) -> u32 {
        self.versions.iter().map(|v| v.version).max().unwrap_or(1)
    }
}

/// One entry in an arXiv paper's version history.
#[derive(Debug, Clone)]
pub struct ArxivVersion {
    pub version: u32,
    /// Submitted-on date as reported by the feed (free-form string,
    /// usually an RFC-2822 timestamp). Left as `Option<String>` because
    /// downstream uses are informational — no callers parse this yet.
    pub submitted: Option<String>,
}

/// Result of a staleness check on an offline arXiv database.
#[derive(Debug, Clone)]
pub struct StalenessCheck {
    pub is_stale: bool,
    pub age_days: Option<u64>,
    pub build_date: Option<String>,
}

/// Handle to an opened offline arXiv database.
pub struct ArxivDatabase {
    conn: Connection,
    #[allow(dead_code)] // Retained for future commands (e.g. compaction) — matches dblp / acl.
    path: PathBuf,
}

impl ArxivDatabase {
    /// Open an existing offline arXiv database. Fails if the schema
    /// hasn't been initialized (i.e. this file hasn't been populated
    /// by an `update-arxiv` run).
    pub fn open(path: &Path) -> Result<Self, ArxivError> {
        let conn = Connection::open(path)?;
        // Fail loud when the file exists but hasn't been built — much
        // friendlier than silently returning "not found" for every
        // query.
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='papers'",
            [],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(ArxivError::Database(rusqlite::Error::QueryReturnedNoRows));
        }
        // Pragmas for read-heavy workload (matches dblp/acl setup).
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA cache_size = -64000;",
        )?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Create a fresh (empty) database at `path`, initializing the
    /// schema and recording the schema version.
    pub fn create(path: &Path) -> Result<Self, ArxivError> {
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

    /// Exact-ID lookup. Strips any trailing `vN` version suffix, so
    /// `2403.00108v1` and `2403.00108` both resolve to the same paper.
    pub fn lookup_by_id(&self, arxiv_id: &str) -> Result<Option<ArxivRecord>, ArxivError> {
        db::lookup_by_id(&self.conn, arxiv_id)
    }

    /// Title search. Returns up to `limit` candidate arXiv IDs in
    /// BM25-rank order; the caller resolves each ID with
    /// [`lookup_by_id`](Self::lookup_by_id) and applies its own fuzzy
    /// title / author validation.
    pub fn search_by_title(&self, query: &str, limit: usize) -> Result<Vec<String>, ArxivError> {
        db::search_by_title(&self.conn, query, limit)
    }

    /// Insert or replace a record. Used by the harvester.
    pub fn upsert(&self, rec: &ArxivRecord) -> Result<(), ArxivError> {
        db::upsert_record(&self.conn, rec)
    }

    /// Fast-path upsert that skips the FTS5 index update. Use inside
    /// a bulk ingest loop and call [`rebuild_fts`](Self::rebuild_fts)
    /// once at the end. Typically 5-10× faster than `upsert` for
    /// 2M+ record runs because FTS5 inverted-index maintenance
    /// dominates per-record cost.
    pub fn upsert_bulk(&self, rec: &ArxivRecord) -> Result<(), ArxivError> {
        db::upsert_record_no_fts(&self.conn, rec)
    }

    /// Rebuild the FTS5 title index from the `papers` table in one
    /// shot. Call after a bulk ingest that used `upsert_bulk`.
    pub fn rebuild_fts(&self) -> Result<(), ArxivError> {
        db::rebuild_fts_from_papers(&self.conn)
    }

    /// Open an explicit transaction for bulk ingest. Without this,
    /// each `upsert` runs in its own implicit BEGIN/COMMIT — fine for
    /// individual writes, catastrophic for ~2.5M of them (fsync
    /// per record drops throughput to ~150 rows/s).
    ///
    /// Usage pattern:
    /// ```ignore
    /// db.begin_bulk()?;
    /// for rec in records {
    ///     db.upsert(&rec)?;
    ///     // Every N records, checkpoint to bound WAL growth:
    ///     if i % 50_000 == 0 { db.commit_and_continue()?; }
    /// }
    /// db.commit_bulk()?;
    /// ```
    pub fn begin_bulk(&self) -> Result<(), ArxivError> {
        // Bulk-tuned PRAGMAs. Applied before BEGIN so they stick for
        // the lifetime of the ingest run:
        //   - cache_size = -1048576 : 1 GiB page cache (SQLite
        //     interprets negative values as kibibytes). Keeps the
        //     FTS b-tree and hot papers pages resident, turning
        //     random-access writes into in-memory updates.
        //   - synchronous = OFF : no fsync on commit. Acceptable
        //     here because the whole DB is re-buildable from the
        //     Kaggle dump — a crash mid-ingest just means re-run.
        //   - temp_store = MEMORY : FTS5 rebuild uses temp tables;
        //     keeping them in RAM avoids spurious disk traffic.
        //   - mmap_size = 1 GiB : cheaper read path for the hot
        //     pages of the papers table during FTS rebuild.
        // These are process-local — they don't persist in the DB.
        self.conn.execute_batch(
            "PRAGMA cache_size = -1048576; \
             PRAGMA synchronous = OFF; \
             PRAGMA temp_store = MEMORY; \
             PRAGMA mmap_size = 1073741824;",
        )?;
        // IMMEDIATE acquires the write lock up front so a concurrent
        // reader can't squeeze in between our statements and force
        // SQLITE_BUSY. On a single-writer ingest it's free.
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        Ok(())
    }

    /// Commit the current bulk transaction. Call once at the end of
    /// a successful ingest. Also restores `synchronous = NORMAL` so
    /// subsequent writes to this connection are crash-safe again
    /// (begin_bulk weakens it to OFF for throughput).
    pub fn commit_bulk(&self) -> Result<(), ArxivError> {
        self.conn.execute_batch("COMMIT")?;
        self.conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
        Ok(())
    }

    /// Roll back the current bulk transaction. For abort paths — the
    /// rusqlite transaction guard would handle this automatically
    /// but we're using raw statements to work around Connection
    /// borrow limitations inside `Arc<Mutex<_>>`.
    pub fn rollback_bulk(&self) -> Result<(), ArxivError> {
        self.conn.execute_batch("ROLLBACK")?;
        Ok(())
    }

    /// Commit the current transaction and open a new one. Call every
    /// N records to cap WAL file growth (SQLite's WAL can balloon to
    /// the size of all pending writes if never checkpointed mid-run).
    pub fn commit_and_continue(&self) -> Result<(), ArxivError> {
        self.conn.execute_batch("COMMIT; BEGIN IMMEDIATE;")?;
        Ok(())
    }

    /// Borrow the underlying SQLite connection — useful for callers
    /// that need to stash metadata (e.g. resumption tokens) that
    /// this handle's public API doesn't expose directly.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Record the last-successful-harvest ISO-8601 timestamp. Used
    /// by [`staleness`](Self::staleness) and as the starting point
    /// for incremental refreshes.
    pub fn record_build_date(&self, iso_date: &str) -> Result<(), ArxivError> {
        db::set_metadata(&self.conn, "build_date", iso_date)
    }

    /// Number of papers in the database.
    pub fn paper_count(&self) -> Result<u64, ArxivError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM papers", [], |row| row.get(0))?;
        Ok(n as u64)
    }

    /// Check whether the database is older than `threshold_days`.
    /// Returns `is_stale = false` and no age when no build date is
    /// recorded (fresh / never-built / corrupted metadata).
    pub fn staleness(&self, threshold_days: u64) -> Result<StalenessCheck, ArxivError> {
        let build_date = db::get_metadata(&self.conn, "build_date")?;
        let Some(date) = &build_date else {
            return Ok(StalenessCheck {
                is_stale: false,
                age_days: None,
                build_date: None,
            });
        };
        // Parse YYYY-MM-DD and compare to today. Uses std::time — we
        // don't want a chrono dep just for this.
        let age_days = iso_date_age_days(date);
        Ok(StalenessCheck {
            is_stale: age_days.is_some_and(|d| d >= threshold_days),
            age_days,
            build_date,
        })
    }
}

/// Compute age in days between an ISO `YYYY-MM-DD` string and today.
/// Returns `None` when the string can't be parsed — the caller treats
/// that as "unknown age, don't warn".
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
    let epoch_days_to_then = then - 719_162; // days from year 0000 to 1970
    Some((now_days - epoch_days_to_then).max(0) as u64)
}

/// Convert a Gregorian date to days since year 0000-03-01. Zeller-ish
/// arithmetic; accurate enough for staleness reports over a human
/// time horizon.
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
        let path = dir.path().join("arxiv.db");
        {
            let db = ArxivDatabase::create(&path).unwrap();
            db.upsert(&ArxivRecord {
                id: "2101.00001".into(),
                title: "Test paper".into(),
                authors: vec!["A".into(), "B".into()],
                categories: None,
                doi: None,
                license: None,
                versions: vec![ArxivVersion {
                    version: 1,
                    submitted: None,
                }],
            })
            .unwrap();
            db.record_build_date("2025-01-15").unwrap();
        }
        let reopened = ArxivDatabase::open(&path).unwrap();
        assert_eq!(reopened.paper_count().unwrap(), 1);
        let got = reopened.lookup_by_id("2101.00001").unwrap().unwrap();
        assert_eq!(got.title, "Test paper");
    }

    #[test]
    fn open_rejects_fresh_file_without_schema() {
        // Caller handed us an empty SQLite file (e.g. `touch foo.db`);
        // we should fail fast rather than pretend everything is fine
        // and return "not found" on every query.
        let dir = tempdir().unwrap();
        let path = dir.path().join("empty.db");
        let _ = rusqlite::Connection::open(&path).unwrap(); // create empty file
        match ArxivDatabase::open(&path) {
            Err(ArxivError::Database(_)) => {}
            Err(other) => panic!("expected Database error, got {other:?}"),
            Ok(_) => panic!("open() on an empty file should fail"),
        }
    }

    #[test]
    fn iso_date_parsing_plausible() {
        // We can't assert an exact day count without freezing "now", but
        // we can verify monotonicity: an older date has a larger age.
        let older = iso_date_age_days("2020-01-01").unwrap_or(0);
        let newer = iso_date_age_days("2025-01-01").unwrap_or(0);
        assert!(older > newer, "older={older} newer={newer}");
        assert!(iso_date_age_days("not-a-date").is_none());
    }
}
