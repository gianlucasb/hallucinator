//! Two-tier cache for remote database query results.
//!
//! **L1** – [`DashMap`] in-memory map (lock-free concurrent reads, sub-µs).
//! **L2** – Optional SQLite database on disk (persists across process restarts).
//!
//! On [`get`](QueryCache::get): check L1 first; on miss, fall through to L2 and
//! promote the result back into L1 on hit. On [`insert`](QueryCache::insert):
//! write-through to both tiers.
//!
//! Cache keys use [`normalize_title`](crate::matching::normalize_title) so that
//! minor variations (diacritics, HTML entities, Greek letters) produce the same
//! key. Only successful results are cached; transient errors (timeouts, network
//! failures) are never cached.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rusqlite::{Connection, OpenFlags, params};

use crate::db::DbQueryResult;
use crate::matching::normalize_title;
use crate::retraction::RetractionResult;

/// Default time-to-live for positive (found) cache entries: 7 days.
pub const DEFAULT_POSITIVE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default time-to-live for negative (not found) cache entries: 24 hours.
pub const DEFAULT_NEGATIVE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Cache key: normalized title + database name.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct CacheKey {
    normalized_title: String,
    db_name: String,
}

/// What we store: either a found result or a not-found marker.
#[derive(Clone, Debug)]
enum CachedResult {
    /// Paper found: title, authors, url, and optional retraction info.
    Found {
        title: String,
        authors: Vec<String>,
        url: Option<String>,
        retraction: Option<RetractionResult>,
    },
    /// Paper not found in this database.
    NotFound,
}

/// A timestamped cache entry (L1 only — uses monotonic `Instant`).
#[derive(Clone, Debug)]
struct CacheEntry {
    result: CachedResult,
    inserted_at: Instant,
    /// Wall-clock timestamp stored for L2 round-trips (written but not
    /// actively read back from L1 — SQLite uses it on promotion).
    #[allow(dead_code)]
    inserted_epoch: u64,
}

/// Open a SQLite connection with WAL mode and standard pragmas.
fn open_sqlite(path: &Path, read_only: bool) -> Result<Connection, rusqlite::Error> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let conn = Connection::open_with_flags(path, flags)?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;",
    )?;
    Ok(conn)
}

/// SQLite writer connection (L2 writes: insert, clear, evict).
struct SqliteWriter {
    conn: Connection,
}

impl SqliteWriter {
    fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = open_sqlite(path, false)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS query_cache (
                 normalized_title TEXT NOT NULL,
                 db_name          TEXT NOT NULL,
                 found            INTEGER NOT NULL,
                 found_title      TEXT,
                 authors          TEXT,
                 paper_url        TEXT,
                 inserted_at      INTEGER NOT NULL,
                 retraction_json  TEXT,
                 PRIMARY KEY (normalized_title, db_name)
             );",
        )?;
        // Migration: add retraction_json column to existing databases.
        // ALTER TABLE ADD COLUMN is a no-op if the column already exists (SQLite
        // returns "duplicate column name" error which we silently ignore).
        let _ = conn.execute_batch("ALTER TABLE query_cache ADD COLUMN retraction_json TEXT");
        Ok(Self { conn })
    }

    /// Insert or replace a cache entry. Returns what was previously stored:
    /// `None` = new entry, `Some(true)` = replaced a Found, `Some(false)` = replaced a NotFound.
    fn insert(
        &self,
        norm_title: &str,
        db_name: &str,
        result: &CachedResult,
        epoch: u64,
    ) -> Option<bool> {
        // Check what (if anything) is being replaced
        let previous: Option<bool> = self
            .conn
            .query_row(
                "SELECT found FROM query_cache WHERE normalized_title = ?1 AND db_name = ?2",
                params![norm_title, db_name],
                |row| {
                    let f: i32 = row.get(0)?;
                    Ok(f != 0)
                },
            )
            .ok();

        let (found, found_title, authors_json, paper_url, retraction_json) = match result {
            CachedResult::Found {
                title,
                authors,
                url,
                retraction,
            } => (
                1i32,
                Some(title.as_str()),
                Some(serde_json::to_string(authors).unwrap_or_default()),
                url.as_deref(),
                retraction
                    .as_ref()
                    .and_then(|r| serde_json::to_string(r).ok()),
            ),
            CachedResult::NotFound => (0i32, None, None, None, None),
        };

        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO query_cache
                 (normalized_title, db_name, found, found_title, authors, paper_url, inserted_at, retraction_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                norm_title,
                db_name,
                found,
                found_title,
                authors_json,
                paper_url,
                epoch,
                retraction_json
            ],
        );

        previous
    }

    fn clear(&self) {
        let _ = self.conn.execute("DELETE FROM query_cache", []);
        // Reclaim disk space — without VACUUM the deleted pages stay as free pages.
        let _ = self.conn.execute_batch("VACUUM");
    }

    fn clear_not_found(&self) -> usize {
        let deleted = self
            .conn
            .execute("DELETE FROM query_cache WHERE found = 0", [])
            .unwrap_or(0);
        if deleted > 0 {
            let _ = self.conn.execute_batch("VACUUM");
        }
        deleted
    }

    fn evict_expired(&self, positive_ttl: Duration, negative_ttl: Duration) {
        let now = now_epoch();
        let pos_cutoff = now.saturating_sub(positive_ttl.as_secs());
        let neg_cutoff = now.saturating_sub(negative_ttl.as_secs());
        let _ = self.conn.execute(
            "DELETE FROM query_cache WHERE
                 (found = 1 AND inserted_at < ?1) OR
                 (found = 0 AND inserted_at < ?2)",
            params![pos_cutoff, neg_cutoff],
        );
    }

    /// Count of (found, not_found) entries in the SQLite table.
    fn counts_by_type(&self) -> (usize, usize) {
        let found: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM query_cache WHERE found = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let not_found: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM query_cache WHERE found = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        (found, not_found)
    }
}

/// Pool of read-only SQLite connections for concurrent L2 lookups.
///
/// Each reader gets its own connection (SQLite WAL mode allows concurrent reads).
/// Connections are returned to the pool after use. If the pool is empty, a new
/// connection is opened.
struct ReadPool {
    pool: Mutex<Vec<Connection>>,
    path: PathBuf,
}

impl ReadPool {
    fn new(path: &Path) -> Self {
        Self {
            pool: Mutex::new(Vec::new()),
            path: path.to_path_buf(),
        }
    }

    fn acquire(&self) -> Option<Connection> {
        // Try to reuse a pooled connection
        if let Ok(mut pool) = self.pool.lock()
            && let Some(conn) = pool.pop()
        {
            return Some(conn);
        }
        // Pool empty — open a new read-only connection
        open_sqlite(&self.path, true).ok()
    }

    fn release(&self, conn: Connection) {
        if let Ok(mut pool) = self.pool.lock() {
            pool.push(conn);
        }
    }

    fn get(
        &self,
        norm_title: &str,
        db_name: &str,
        positive_ttl: Duration,
        negative_ttl: Duration,
    ) -> Option<(CachedResult, u64)> {
        let conn = self.acquire()?;
        let result = Self::query(&conn, norm_title, db_name, positive_ttl, negative_ttl);
        self.release(conn);
        result
    }

    fn query(
        conn: &Connection,
        norm_title: &str,
        db_name: &str,
        positive_ttl: Duration,
        negative_ttl: Duration,
    ) -> Option<(CachedResult, u64)> {
        let now = now_epoch();
        let mut stmt = conn
            .prepare_cached(
                "SELECT found, found_title, authors, paper_url, inserted_at, retraction_json
                 FROM query_cache
                 WHERE normalized_title = ?1 AND db_name = ?2",
            )
            .ok()?;

        let row = stmt
            .query_row(params![norm_title, db_name], |row| {
                let found: i32 = row.get(0)?;
                let found_title: Option<String> = row.get(1)?;
                let authors_json: Option<String> = row.get(2)?;
                let paper_url: Option<String> = row.get(3)?;
                let inserted_at: u64 = row.get(4)?;
                let retraction_json: Option<String> = row.get(5)?;
                Ok((
                    found,
                    found_title,
                    authors_json,
                    paper_url,
                    inserted_at,
                    retraction_json,
                ))
            })
            .ok()?;

        let (found, found_title, authors_json, paper_url, inserted_at, retraction_json) = row;

        let result = if found != 0 {
            CachedResult::Found {
                title: found_title.unwrap_or_default(),
                authors: authors_json
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default(),
                url: paper_url,
                retraction: retraction_json.and_then(|j| serde_json::from_str(&j).ok()),
            }
        } else {
            CachedResult::NotFound
        };

        // Check TTL — if expired, return None (writer evicts on next startup)
        let ttl = match &result {
            CachedResult::Found { .. } => positive_ttl,
            CachedResult::NotFound => negative_ttl,
        };
        let age = Duration::from_secs(now.saturating_sub(inserted_at));
        if age > ttl {
            return None;
        }

        Some((result, inserted_at))
    }
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Thread-safe two-tier cache for database query results.
///
/// L1: [`DashMap`] for lock-free concurrent access from multiple drainer tasks.
/// L2: Optional SQLite database — reads use a [`ReadPool`] of concurrent connections,
///     writes go through a single [`SqliteWriter`] behind a [`Mutex`].
pub struct QueryCache {
    entries: DashMap<CacheKey, CacheEntry>,
    /// Writer connection for inserts, clears, eviction (serialized).
    sqlite_writer: Option<Mutex<SqliteWriter>>,
    /// Pool of read-only connections for concurrent L2 lookups.
    read_pool: Option<ReadPool>,
    positive_ttl: Duration,
    negative_ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
    /// Running sum of lookup durations in microseconds (for computing average).
    total_lookup_us: AtomicU64,
    /// Total number of lookups (hits + misses) for average calculation.
    total_lookups: AtomicU64,
    // ── Counters kept in sync on insert/remove/clear (no per-frame queries) ──
    l1_found_count: AtomicU64,
    l1_not_found_count: AtomicU64,
    l2_found_count: AtomicU64,
    l2_not_found_count: AtomicU64,
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new(DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL)
    }
}

impl QueryCache {
    /// Create an in-memory-only cache with custom TTLs (no disk persistence).
    pub fn new(positive_ttl: Duration, negative_ttl: Duration) -> Self {
        Self {
            entries: DashMap::new(),
            sqlite_writer: None,
            read_pool: None,
            positive_ttl,
            negative_ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            total_lookup_us: AtomicU64::new(0),
            total_lookups: AtomicU64::new(0),
            l1_found_count: AtomicU64::new(0),
            l1_not_found_count: AtomicU64::new(0),
            l2_found_count: AtomicU64::new(0),
            l2_not_found_count: AtomicU64::new(0),
        }
    }

    /// Open a persistent cache backed by a SQLite database at `path`.
    ///
    /// On startup, expired entries are evicted from SQLite. The L1 DashMap
    /// starts empty and is populated lazily as entries are accessed.
    pub fn open(
        path: &Path,
        positive_ttl: Duration,
        negative_ttl: Duration,
    ) -> Result<Self, String> {
        let writer = SqliteWriter::open(path)
            .map_err(|e| format!("Failed to open cache database at {}: {}", path.display(), e))?;
        writer.evict_expired(positive_ttl, negative_ttl);
        let (l2_found, l2_nf) = writer.counts_by_type();
        Ok(Self {
            entries: DashMap::new(),
            sqlite_writer: Some(Mutex::new(writer)),
            read_pool: Some(ReadPool::new(path)),
            positive_ttl,
            negative_ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            total_lookup_us: AtomicU64::new(0),
            total_lookups: AtomicU64::new(0),
            l1_found_count: AtomicU64::new(0),
            l1_not_found_count: AtomicU64::new(0),
            l2_found_count: AtomicU64::new(l2_found as u64),
            l2_not_found_count: AtomicU64::new(l2_nf as u64),
        })
    }

    /// Look up a cached result for the given title and database.
    ///
    /// Returns `Some(result)` on cache hit (within TTL), `None` on miss.
    /// The title is normalized before lookup.
    pub fn get(&self, title: &str, db_name: &str) -> Option<DbQueryResult> {
        let start = Instant::now();
        let norm = normalize_title(title);
        let key = CacheKey {
            normalized_title: norm.clone(),
            db_name: db_name.to_string(),
        };

        // L1 check
        if let Some(entry) = self.entries.get(&key) {
            let ttl = match &entry.result {
                CachedResult::Found { .. } => self.positive_ttl,
                CachedResult::NotFound => self.negative_ttl,
            };
            if entry.inserted_at.elapsed() > ttl {
                let is_found = matches!(entry.result, CachedResult::Found { .. });
                drop(entry);
                self.entries.remove(&key);
                // Adjust L1 counters for the expired eviction
                if is_found {
                    self.l1_found_count.fetch_sub(1, Ordering::Relaxed);
                } else {
                    self.l1_not_found_count.fetch_sub(1, Ordering::Relaxed);
                }
                // Fall through to L2
            } else {
                self.hits.fetch_add(1, Ordering::Relaxed);
                self.record_lookup(start);
                tracing::trace!(db = db_name, title, "cache L1 hit");
                return Some(cached_to_query_result(&entry.result));
            }
        }

        // L2 check (concurrent read — no writer lock needed)
        if let Some(ref pool) = self.read_pool
            && let Some((result, epoch)) =
                pool.get(&norm, db_name, self.positive_ttl, self.negative_ttl)
        {
            // Promote to L1
            tracing::trace!(db = db_name, title, "cache L2 hit, promoting to L1");
            let query_result = cached_to_query_result(&result);
            match &result {
                CachedResult::Found { .. } => {
                    self.l1_found_count.fetch_add(1, Ordering::Relaxed);
                }
                CachedResult::NotFound => {
                    self.l1_not_found_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            self.entries.insert(
                key,
                CacheEntry {
                    result,
                    inserted_at: epoch_to_instant(epoch),
                    inserted_epoch: epoch,
                },
            );
            self.hits.fetch_add(1, Ordering::Relaxed);
            self.record_lookup(start);
            return Some(query_result);
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        self.record_lookup(start);
        tracing::trace!(db = db_name, title, "cache miss");
        None
    }

    fn record_lookup(&self, start: Instant) {
        let us = start.elapsed().as_micros() as u64;
        self.total_lookup_us.fetch_add(us, Ordering::Relaxed);
        self.total_lookups.fetch_add(1, Ordering::Relaxed);
    }

    /// Insert a query result into the cache.
    ///
    /// Only caches successful results (found or not-found). Errors should NOT
    /// be passed to this method. Write-through: updates both L1 and L2.
    ///
    /// If `negative_ttl` is zero, not-found results are never cached.
    pub fn insert(&self, title: &str, db_name: &str, result: &DbQueryResult) {
        let is_not_found = !result.is_found();
        tracing::trace!(db = db_name, title, found = !is_not_found, "cache insert");

        // Skip not-found entries entirely when negative TTL is zero
        if is_not_found && self.negative_ttl.is_zero() {
            return;
        }

        let norm = normalize_title(title);
        let key = CacheKey {
            normalized_title: norm.clone(),
            db_name: db_name.to_string(),
        };

        let cached = if let Some(ref found_title) = result.found_title {
            CachedResult::Found {
                title: found_title.clone(),
                authors: result.authors.clone(),
                url: result.paper_url.clone(),
                retraction: result.retraction.clone(),
            }
        } else {
            CachedResult::NotFound
        };

        let now_is_found = matches!(cached, CachedResult::Found { .. });
        let epoch = now_epoch();

        // L1 — DashMap::insert returns the old value if the key existed
        let old_l1 = self.entries.insert(
            key,
            CacheEntry {
                result: cached.clone(),
                inserted_at: Instant::now(),
                inserted_epoch: epoch,
            },
        );

        // Adjust L1 counters: decrement for old, increment for new
        if let Some(old_entry) = old_l1 {
            if matches!(old_entry.result, CachedResult::Found { .. }) {
                self.l1_found_count.fetch_sub(1, Ordering::Relaxed);
            } else {
                self.l1_not_found_count.fetch_sub(1, Ordering::Relaxed);
            }
        }
        if now_is_found {
            self.l1_found_count.fetch_add(1, Ordering::Relaxed);
        } else {
            self.l1_not_found_count.fetch_add(1, Ordering::Relaxed);
        }

        // L2 — write-through to SQLite for persistence across restarts.
        if let Some(ref sqlite_mutex) = self.sqlite_writer
            && let Ok(store) = sqlite_mutex.lock()
        {
            let previous = store.insert(&norm, db_name, &cached, epoch);

            // Adjust L2 counters: decrement old type, increment new type
            match previous {
                Some(true) => {
                    self.l2_found_count.fetch_sub(1, Ordering::Relaxed);
                }
                Some(false) => {
                    self.l2_not_found_count.fetch_sub(1, Ordering::Relaxed);
                }
                None => {} // new entry
            }
            if now_is_found {
                self.l2_found_count.fetch_add(1, Ordering::Relaxed);
            } else {
                self.l2_not_found_count.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Remove all not-found entries from L1 (in-memory) and L2 (SQLite).
    ///
    /// Returns the total number of entries removed across both tiers.
    pub fn clear_not_found(&self) -> usize {
        // L1: retain only Found entries
        let mut l1_removed = 0usize;
        self.entries.retain(|_, entry| {
            if matches!(entry.result, CachedResult::NotFound) {
                l1_removed += 1;
                false
            } else {
                true
            }
        });
        self.l1_not_found_count.store(0, Ordering::Relaxed);

        // L2: delete not-found rows from SQLite
        let l2_removed = if let Some(ref sqlite_mutex) = self.sqlite_writer
            && let Ok(store) = sqlite_mutex.lock()
        {
            store.clear_not_found()
        } else {
            0
        };
        self.l2_not_found_count.store(0, Ordering::Relaxed);

        l1_removed + l2_removed
    }

    /// Remove all entries from both L1 and L2.
    pub fn clear(&self) {
        self.entries.clear();
        self.l1_found_count.store(0, Ordering::Relaxed);
        self.l1_not_found_count.store(0, Ordering::Relaxed);
        if let Some(ref sqlite_mutex) = self.sqlite_writer
            && let Ok(store) = sqlite_mutex.lock()
        {
            store.clear();
        }
        self.l2_found_count.store(0, Ordering::Relaxed);
        self.l2_not_found_count.store(0, Ordering::Relaxed);
    }

    /// Number of cache hits since creation.
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Number of cache misses since creation.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
    }

    /// Average lookup time in milliseconds (hits and misses).
    pub fn avg_lookup_ms(&self) -> f64 {
        let count = self.total_lookups.load(Ordering::Relaxed);
        if count == 0 {
            return 0.0;
        }
        let us = self.total_lookup_us.load(Ordering::Relaxed);
        us as f64 / count as f64 / 1000.0
    }

    /// Number of entries currently in the L1 in-memory cache.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the L1 cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Total entries in the persistent L2 store (0 if no SQLite backing).
    pub fn disk_len(&self) -> usize {
        let f = self.l2_found_count.load(Ordering::Relaxed) as usize;
        let nf = self.l2_not_found_count.load(Ordering::Relaxed) as usize;
        f + nf
    }

    /// Whether this cache has a persistent SQLite backing store.
    pub fn has_persistence(&self) -> bool {
        self.sqlite_writer.is_some()
    }

    /// Count of (found, not_found) entries in L2 (SQLite).
    /// Returns (0, 0) if no persistence. Uses cached atomic counters (no SQL query).
    pub fn l2_counts(&self) -> (usize, usize) {
        (
            self.l2_found_count.load(Ordering::Relaxed) as usize,
            self.l2_not_found_count.load(Ordering::Relaxed) as usize,
        )
    }

    /// Count of found vs not-found entries in L1 (in-memory).
    /// Uses cached atomic counters (no DashMap iteration).
    pub fn l1_counts(&self) -> (usize, usize) {
        (
            self.l1_found_count.load(Ordering::Relaxed) as usize,
            self.l1_not_found_count.load(Ordering::Relaxed) as usize,
        )
    }

    /// The positive (found) TTL.
    pub fn positive_ttl(&self) -> Duration {
        self.positive_ttl
    }

    /// The negative (not found) TTL.
    pub fn negative_ttl(&self) -> Duration {
        self.negative_ttl
    }
}

fn cached_to_query_result(cached: &CachedResult) -> DbQueryResult {
    match cached {
        CachedResult::Found {
            title,
            authors,
            url,
            retraction,
        } => DbQueryResult {
            found_title: Some(title.clone()),
            authors: authors.clone(),
            paper_url: url.clone(),
            retraction: retraction.clone(),
        },
        CachedResult::NotFound => DbQueryResult::not_found(),
    }
}

/// Convert a wall-clock epoch to a monotonic `Instant` approximation.
///
/// We compute the age from `now_epoch - epoch` and subtract from `Instant::now()`.
/// This is approximate but sufficient for TTL checks on L2 → L1 promotion.
fn epoch_to_instant(epoch: u64) -> Instant {
    let now = now_epoch();
    let age_secs = now.saturating_sub(epoch);
    Instant::now() - Duration::from_secs(age_secs)
}

impl std::fmt::Debug for QueryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryCache")
            .field("l1_entries", &self.entries.len())
            .field("l2_entries", &self.disk_len())
            .field("hits", &self.hits())
            .field("misses", &self.misses())
            .field("positive_ttl", &self.positive_ttl)
            .field("negative_ttl", &self.negative_ttl)
            .field("persistent", &self.has_persistence())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn cache_miss_on_empty() {
        let cache = QueryCache::default();
        assert!(cache.get("Some Title", "CrossRef").is_none());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);
    }

    #[test]
    fn cache_hit_after_insert_found() {
        let cache = QueryCache::default();
        let result = DbQueryResult::found(
            "Attention Is All You Need",
            vec!["Vaswani".into()],
            Some("https://doi.org/10.1234".into()),
        );
        cache.insert("Attention Is All You Need", "CrossRef", &result);
        let cached = cache.get("Attention Is All You Need", "CrossRef");
        assert!(cached.is_some());
        let r = cached.unwrap();
        assert_eq!(r.found_title.unwrap(), "Attention Is All You Need");
        assert_eq!(r.authors, vec!["Vaswani"]);
        assert_eq!(r.paper_url.unwrap(), "https://doi.org/10.1234");
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn cache_hit_after_insert_not_found() {
        let cache = QueryCache::default();
        let result = DbQueryResult::not_found();
        cache.insert("Nonexistent Paper", "arXiv", &result);
        let cached = cache.get("Nonexistent Paper", "arXiv");
        assert!(cached.is_some());
        let r = cached.unwrap();
        assert!(r.found_title.is_none());
        assert!(r.authors.is_empty());
        assert!(r.paper_url.is_none());
    }

    #[test]
    fn cache_miss_different_db() {
        let cache = QueryCache::default();
        let result = DbQueryResult::found("A Paper", vec![], None);
        cache.insert("A Paper", "CrossRef", &result);
        assert!(cache.get("A Paper", "arXiv").is_none());
    }

    #[test]
    fn cache_normalized_key() {
        let cache = QueryCache::default();
        let result = DbQueryResult::found("Résumé of Methods", vec![], None);
        // Insert with accented title
        cache.insert("Résumé of Methods", "CrossRef", &result);
        // Look up with ASCII equivalent (normalization strips accents)
        let cached = cache.get("Resume of Methods", "CrossRef");
        assert!(cached.is_some());
    }

    #[test]
    fn cache_expired_positive() {
        let cache = QueryCache::new(Duration::from_millis(1), Duration::from_secs(3600));
        let result = DbQueryResult::found("Paper", vec![], None);
        cache.insert("Paper", "CrossRef", &result);
        // Sleep briefly to let TTL expire
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("Paper", "CrossRef").is_none());
    }

    #[test]
    fn cache_expired_negative() {
        let cache = QueryCache::new(Duration::from_secs(3600), Duration::from_millis(1));
        let result = DbQueryResult::not_found();
        cache.insert("Paper", "CrossRef", &result);
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("Paper", "CrossRef").is_none());
    }

    #[test]
    fn cache_len_and_empty() {
        let cache = QueryCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_clear() {
        let cache = QueryCache::default();
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert_eq!(cache.len(), 1);
        cache.clear();
        assert!(cache.is_empty());
        assert!(cache.get("Paper", "DB").is_none());
    }

    // ── SQLite persistence tests ──────────────────────────────────────

    use std::sync::atomic::AtomicU32;
    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_cache_path() -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "hallucinator_test_cache_{}_{}",
            std::process::id(),
            id,
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("test_cache.db")
    }

    #[test]
    fn sqlite_write_and_read() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let result = DbQueryResult::found(
            "Deep Learning",
            vec!["LeCun".into(), "Bengio".into()],
            Some("https://doi.org/10.1234".into()),
        );
        cache.insert("Deep Learning", "CrossRef", &result);
        assert_eq!(cache.disk_len(), 1);

        // Read back from a fresh cache instance (simulating restart)
        drop(cache);
        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        // L1 should be empty
        assert!(cache2.is_empty());
        // But get() should find it in L2
        let cached = cache2.get("Deep Learning", "CrossRef");
        assert!(cached.is_some());
        let r = cached.unwrap();
        assert_eq!(r.found_title.unwrap(), "Deep Learning");
        assert_eq!(r.authors, vec!["LeCun", "Bengio"]);
        assert_eq!(r.paper_url.unwrap(), "https://doi.org/10.1234");
        // Should have promoted to L1
        assert_eq!(cache2.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sqlite_not_found_persists() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let result = DbQueryResult::not_found();
        cache.insert("Fake Paper", "arXiv", &result);
        assert!(cache.get("Fake Paper", "arXiv").is_some());
        assert_eq!(cache.disk_len(), 1);

        drop(cache);
        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let cached = cache2.get("Fake Paper", "arXiv");
        assert!(cached.is_some());
        assert!(cached.unwrap().found_title.is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sqlite_not_found_skipped_when_negative_ttl_zero() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        // Zero negative TTL = don't cache not-found at all
        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, Duration::ZERO).unwrap();
        cache.insert("Fake Paper", "arXiv", &DbQueryResult::not_found());
        // Not in L1 or L2
        assert!(cache.get("Fake Paper", "arXiv").is_none());
        assert_eq!(cache.disk_len(), 0);

        // Found results still work
        cache.insert(
            "Real Paper",
            "arXiv",
            &DbQueryResult::found("Real Paper", vec![], None),
        );
        assert!(cache.get("Real Paper", "arXiv").is_some());
        assert_eq!(cache.disk_len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sqlite_clear() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert_eq!(cache.disk_len(), 1);
        cache.clear();
        assert_eq!(cache.disk_len(), 0);
        assert!(cache.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sqlite_expired_evicted_on_open() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        // Insert with 1-second TTL (SQLite uses epoch-second resolution)
        {
            let cache =
                QueryCache::open(&path, Duration::from_secs(1), Duration::from_secs(1)).unwrap();
            cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
            cache.insert("Missing", "DB", &DbQueryResult::not_found());
        }

        std::thread::sleep(Duration::from_secs(2));

        // Re-open — eviction should remove expired entries
        let cache2 =
            QueryCache::open(&path, Duration::from_secs(1), Duration::from_secs(1)).unwrap();
        assert_eq!(cache2.disk_len(), 0);

        let _ = std::fs::remove_file(&path);
    }

    // ── Two-tier interaction tests ────────────────────────────────────

    #[test]
    fn l1_expired_l2_valid_promotes() {
        // L1 has a very short TTL, L2 has a long TTL.
        // After L1 expires, get() should still find the entry in L2 and promote it.
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let positive_ttl = DEFAULT_POSITIVE_TTL;
        let negative_ttl = DEFAULT_NEGATIVE_TTL;
        let cache = QueryCache::open(&path, positive_ttl, negative_ttl).unwrap();

        let result = DbQueryResult::found("Persistent Paper", vec!["Author".into()], None);
        cache.insert("Persistent Paper", "CrossRef", &result);

        // Manually expire L1 by removing the entry, simulating L1 eviction
        let norm = normalize_title("Persistent Paper");
        let key = CacheKey {
            normalized_title: norm,
            db_name: "CrossRef".to_string(),
        };
        cache.entries.remove(&key);
        assert!(cache.is_empty()); // L1 is empty

        // get() should fall through to L2 and find it
        let cached = cache.get("Persistent Paper", "CrossRef");
        assert!(cached.is_some());
        let r = cached.unwrap();
        assert_eq!(r.found_title.unwrap(), "Persistent Paper");
        assert_eq!(r.authors, vec!["Author"]);

        // Should be promoted back to L1
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.hits(), 1);
        assert_eq!(cache.misses(), 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn l2_miss_increments_miss_counter_once() {
        // When both L1 and L2 miss, misses should increment exactly once.
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        assert!(cache.get("Nonexistent", "DB").is_none());
        assert_eq!(cache.misses(), 1); // exactly one miss, not two
        assert_eq!(cache.hits(), 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clear_both_tiers_then_restart() {
        // Insert entries, clear both tiers, restart — should be empty.
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert(
            "Paper A",
            "DB1",
            &DbQueryResult::found("Paper A", vec![], None),
        );
        cache.insert("Paper B", "DB2", &DbQueryResult::not_found());
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.disk_len(), 2);

        cache.clear();
        assert!(cache.is_empty());
        assert_eq!(cache.disk_len(), 0);

        // Restart — should still be empty
        drop(cache);
        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        assert!(cache2.is_empty());
        assert_eq!(cache2.disk_len(), 0);
        assert!(cache2.get("Paper A", "DB1").is_none());
        assert!(cache2.get("Paper B", "DB2").is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn concurrent_reads_and_writes() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = std::sync::Arc::new(
            QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap(),
        );

        let mut handles = vec![];
        for i in 0..10 {
            let c = cache.clone();
            handles.push(std::thread::spawn(move || {
                let title = format!("Paper {}", i);
                let db = format!("DB{}", i % 3);
                // Write
                c.insert(
                    &title,
                    &db,
                    &DbQueryResult::found(title.clone(), vec!["Author".into()], None),
                );
                // Read back
                let result = c.get(&title, &db);
                assert!(result.is_some(), "concurrent read failed for {}", title);
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // All 10 entries should be present
        assert_eq!(cache.len(), 10);
        assert_eq!(cache.disk_len(), 10);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupted_authors_json_in_sqlite() {
        // Manually corrupt the authors JSON in SQLite, verify graceful recovery.
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        // First insert a valid entry
        {
            let cache =
                QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
            cache.insert(
                "Test Paper",
                "DB",
                &DbQueryResult::found("Test Paper", vec!["Author".into()], None),
            );
        }

        // Corrupt the authors JSON directly in SQLite
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE query_cache SET authors = '{not valid json!!!' WHERE db_name = 'DB'",
                [],
            )
            .unwrap();
        }

        // Re-open and read — should fall back to empty authors, not panic
        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let cached = cache2.get("Test Paper", "DB");
        assert!(cached.is_some());
        let r = cached.unwrap();
        assert_eq!(r.found_title.unwrap(), "Test Paper");
        assert!(r.authors.is_empty()); // corrupted JSON → empty fallback

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn zero_ttl_entries_expire_immediately() {
        let cache = QueryCache::new(Duration::ZERO, Duration::ZERO);
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        // With zero positive TTL, the entry is inserted but expires immediately
        // on get (elapsed > Duration::ZERO).
        // Zero negative TTL means not-found entries are never cached at all:
        cache.insert("Missing", "DB", &DbQueryResult::not_found());
        // Only the found entry is in L1 (not-found was skipped)
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn multiple_dbs_same_title() {
        // Same title cached across multiple databases should be independent.
        let cache = QueryCache::default();
        let found = DbQueryResult::found("Paper X", vec!["A".into()], None);
        let not_found = DbQueryResult::not_found();

        cache.insert("Paper X", "CrossRef", &found);
        cache.insert("Paper X", "arXiv", &not_found);
        cache.insert("Paper X", "DBLP", &found);

        assert_eq!(cache.len(), 3);

        let cr = cache.get("Paper X", "CrossRef").unwrap();
        assert!(cr.is_found());

        let arxiv = cache.get("Paper X", "arXiv").unwrap();
        assert!(!arxiv.is_found());

        let dblp = cache.get("Paper X", "DBLP").unwrap();
        assert!(dblp.is_found());
    }

    #[test]
    fn overwrite_existing_entry() {
        // Inserting the same key twice should overwrite the first entry.
        let cache = QueryCache::default();
        cache.insert("Paper", "DB", &DbQueryResult::not_found());
        assert!(!cache.get("Paper", "DB").unwrap().is_found());

        // Now overwrite with a found result
        cache.insert(
            "Paper",
            "DB",
            &DbQueryResult::found("Paper", vec!["Author".into()], None),
        );
        let cached = cache.get("Paper", "DB").unwrap();
        assert_eq!(cached.found_title.unwrap(), "Paper");
        assert_eq!(cached.authors, vec!["Author"]);
        assert_eq!(cache.len(), 1); // still one entry, not two
    }

    #[test]
    fn sqlite_overwrite_existing_entry() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("Paper", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.disk_len(), 1);

        // Overwrite with found result
        cache.insert(
            "Paper",
            "DB",
            &DbQueryResult::found("Paper", vec!["Author".into()], None),
        );
        assert_eq!(cache.disk_len(), 1);

        // Restart and verify the overwritten value persisted
        drop(cache);
        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let cached = cache2.get("Paper", "DB").unwrap();
        assert_eq!(cached.found_title.unwrap(), "Paper");
        assert_eq!(cached.authors, vec!["Author"]);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn has_persistence_flag() {
        // In-memory cache reports no persistence
        let mem = QueryCache::default();
        assert!(!mem.has_persistence());

        // SQLite-backed cache reports persistence
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);
        let persistent =
            QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        assert!(persistent.has_persistence());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn ttl_accessors() {
        let cache = QueryCache::new(Duration::from_secs(42), Duration::from_secs(7));
        assert_eq!(cache.positive_ttl(), Duration::from_secs(42));
        assert_eq!(cache.negative_ttl(), Duration::from_secs(7));
    }

    #[test]
    fn clear_not_found_l1_only() {
        let cache = QueryCache::default();
        cache.insert(
            "Found Paper",
            "DB",
            &DbQueryResult::found("Found Paper", vec![], None),
        );
        cache.insert("Missing Paper", "DB", &DbQueryResult::not_found());
        cache.insert("Also Missing", "DB2", &DbQueryResult::not_found());
        assert_eq!(cache.len(), 3);

        let removed = cache.clear_not_found();
        assert_eq!(removed, 2);
        assert_eq!(cache.len(), 1);
        // Found paper should still be there
        assert!(cache.get("Found Paper", "DB").is_some());
        // Not-found papers should be gone
        assert!(cache.get("Missing Paper", "DB").is_none());
        assert!(cache.get("Also Missing", "DB2").is_none());
    }

    #[test]
    fn clear_not_found_with_sqlite() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert(
            "Found Paper",
            "DB",
            &DbQueryResult::found("Found Paper", vec!["Author".into()], None),
        );
        cache.insert("Missing Paper", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.len(), 2);

        let removed = cache.clear_not_found();
        assert!(removed >= 1); // at least the L1 not-found entry
        assert_eq!(cache.len(), 1);

        // Found paper should survive in both tiers
        assert!(cache.get("Found Paper", "DB").is_some());
        assert!(cache.disk_len() >= 1);

        let _ = std::fs::remove_file(&path);
    }

    // ── Atomic counter tests ────────────────────────────────────────

    #[test]
    fn l1_counts_after_inserts() {
        let cache = QueryCache::default();
        assert_eq!(cache.l1_counts(), (0, 0));

        cache.insert("A", "DB1", &DbQueryResult::found("A", vec![], None));
        assert_eq!(cache.l1_counts(), (1, 0));

        cache.insert("B", "DB1", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (1, 1));

        cache.insert("C", "DB2", &DbQueryResult::found("C", vec![], None));
        assert_eq!(cache.l1_counts(), (2, 1));
    }

    #[test]
    fn l2_counts_after_inserts() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        assert_eq!(cache.l2_counts(), (0, 0));

        cache.insert("A", "DB1", &DbQueryResult::found("A", vec![], None));
        assert_eq!(cache.l2_counts(), (1, 0));

        cache.insert("B", "DB1", &DbQueryResult::not_found());
        assert_eq!(cache.l2_counts(), (1, 1));

        assert_eq!(cache.disk_len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn l2_counts_initialized_on_open() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        // Populate cache, close, reopen — counters should reflect disk contents.
        {
            let cache =
                QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
            cache.insert("Found", "DB", &DbQueryResult::found("Found", vec![], None));
            cache.insert("Missing1", "DB", &DbQueryResult::not_found());
            cache.insert("Missing2", "DB2", &DbQueryResult::not_found());
        }

        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        assert_eq!(cache2.l2_counts(), (1, 2));
        assert_eq!(cache2.disk_len(), 3);
        // L1 starts empty after restart
        assert_eq!(cache2.l1_counts(), (0, 0));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overwrite_not_found_to_found_adjusts_counters() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("Paper", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (0, 1));
        assert_eq!(cache.l2_counts(), (0, 1));

        // Overwrite not-found → found
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert_eq!(cache.l1_counts(), (1, 0));
        assert_eq!(cache.l2_counts(), (1, 0));
        assert_eq!(cache.disk_len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn overwrite_found_to_not_found_adjusts_counters() {
        let cache = QueryCache::default();
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert_eq!(cache.l1_counts(), (1, 0));

        cache.insert("Paper", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (0, 1));
    }

    #[test]
    fn overwrite_same_type_no_double_count() {
        let cache = QueryCache::default();
        cache.insert("Paper", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (0, 1));

        // Overwrite not-found → not-found: should still be (0, 1)
        cache.insert("Paper", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (0, 1));

        // Same for found → found
        cache.insert(
            "Paper2",
            "DB",
            &DbQueryResult::found("Paper2", vec![], None),
        );
        assert_eq!(cache.l1_counts(), (1, 1));
        cache.insert(
            "Paper2",
            "DB",
            &DbQueryResult::found("Paper2", vec!["X".into()], None),
        );
        assert_eq!(cache.l1_counts(), (1, 1));
    }

    #[test]
    fn clear_resets_all_counters() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("A", "DB", &DbQueryResult::found("A", vec![], None));
        cache.insert("B", "DB", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (1, 1));
        assert_eq!(cache.l2_counts(), (1, 1));

        cache.clear();
        assert_eq!(cache.l1_counts(), (0, 0));
        assert_eq!(cache.l2_counts(), (0, 0));
        assert_eq!(cache.disk_len(), 0);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn clear_not_found_adjusts_counters() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("Found", "DB", &DbQueryResult::found("Found", vec![], None));
        cache.insert("NF1", "DB", &DbQueryResult::not_found());
        cache.insert("NF2", "DB2", &DbQueryResult::not_found());
        assert_eq!(cache.l1_counts(), (1, 2));
        assert_eq!(cache.l2_counts(), (1, 2));

        cache.clear_not_found();
        assert_eq!(cache.l1_counts(), (1, 0));
        assert_eq!(cache.l2_counts(), (1, 0));
        assert_eq!(cache.disk_len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn l1_counter_adjusts_on_ttl_expiry() {
        let cache = QueryCache::new(Duration::from_millis(1), Duration::from_millis(1));
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert_eq!(cache.l1_counts(), (1, 0));

        std::thread::sleep(Duration::from_millis(10));

        // get() should evict the expired entry and adjust counter
        assert!(cache.get("Paper", "DB").is_none());
        assert_eq!(cache.l1_counts(), (0, 0));
    }

    #[test]
    fn l1_counter_adjusts_on_l2_promotion() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("Paper", "DB", &DbQueryResult::found("Paper", vec![], None));
        assert_eq!(cache.l1_counts(), (1, 0));

        // Remove from L1, simulating eviction
        let norm = normalize_title("Paper");
        let key = CacheKey {
            normalized_title: norm,
            db_name: "DB".to_string(),
        };
        cache.entries.remove(&key);
        // Manually adjust L1 counter since we bypassed the normal path
        cache.l1_found_count.fetch_sub(1, Ordering::Relaxed);
        assert_eq!(cache.l1_counts(), (0, 0));

        // get() should promote from L2 and increment L1 counter
        let result = cache.get("Paper", "DB");
        assert!(result.is_some());
        assert_eq!(cache.l1_counts(), (1, 0));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn concurrent_inserts_counters_consistent() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = std::sync::Arc::new(
            QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap(),
        );

        let mut handles = vec![];
        for i in 0..20 {
            let c = cache.clone();
            handles.push(std::thread::spawn(move || {
                let title = format!("Paper {}", i);
                let db = format!("DB{}", i % 4);
                if i % 3 == 0 {
                    c.insert(&title, &db, &DbQueryResult::not_found());
                } else {
                    c.insert(
                        &title,
                        &db,
                        &DbQueryResult::found(title.clone(), vec![], None),
                    );
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let (l1_f, l1_nf) = cache.l1_counts();
        let (l2_f, l2_nf) = cache.l2_counts();
        // Total L1 entries should match DashMap len
        assert_eq!(l1_f + l1_nf, cache.len());
        // Total L2 entries should match disk_len
        assert_eq!(l2_f + l2_nf, cache.disk_len());
        // L1 and L2 should agree (all entries are in both tiers)
        assert_eq!(l1_f, l2_f);
        assert_eq!(l1_nf, l2_nf);

        let _ = std::fs::remove_file(&path);
    }
}
