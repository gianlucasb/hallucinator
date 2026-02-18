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

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use rusqlite::{Connection, params};

use crate::db::DbQueryResult;
use crate::matching::normalize_title;

/// Default time-to-live for positive (found) cache entries: 7 days.
const DEFAULT_POSITIVE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Default time-to-live for negative (not found) cache entries: 24 hours.
const DEFAULT_NEGATIVE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Cache key: normalized title + database name.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
struct CacheKey {
    normalized_title: String,
    db_name: String,
}

/// What we store: either a found result or a not-found marker.
#[derive(Clone, Debug)]
enum CachedResult {
    /// Paper found: (title, authors, url).
    Found {
        title: String,
        authors: Vec<String>,
        url: Option<String>,
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

/// SQLite-backed persistent store (L2).
struct SqliteStore {
    conn: Connection,
}

impl SqliteStore {
    fn open(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;",
        )?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS query_cache (
                 normalized_title TEXT NOT NULL,
                 db_name          TEXT NOT NULL,
                 found            INTEGER NOT NULL,
                 found_title      TEXT,
                 authors          TEXT,
                 paper_url        TEXT,
                 inserted_at      INTEGER NOT NULL,
                 PRIMARY KEY (normalized_title, db_name)
             );",
        )?;
        Ok(Self { conn })
    }

    fn get(
        &self,
        norm_title: &str,
        db_name: &str,
        positive_ttl: Duration,
        negative_ttl: Duration,
    ) -> Option<(CachedResult, u64)> {
        let now = now_epoch();
        let mut stmt = self
            .conn
            .prepare_cached(
                "SELECT found, found_title, authors, paper_url, inserted_at
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
                Ok((found, found_title, authors_json, paper_url, inserted_at))
            })
            .ok()?;

        let (found, found_title, authors_json, paper_url, inserted_at) = row;

        let result = if found != 0 {
            CachedResult::Found {
                title: found_title.unwrap_or_default(),
                authors: authors_json
                    .and_then(|j| serde_json::from_str(&j).ok())
                    .unwrap_or_default(),
                url: paper_url,
            }
        } else {
            CachedResult::NotFound
        };

        // Check TTL
        let ttl = match &result {
            CachedResult::Found { .. } => positive_ttl,
            CachedResult::NotFound => negative_ttl,
        };
        let age = Duration::from_secs(now.saturating_sub(inserted_at));
        if age > ttl {
            // Expired — lazily remove
            let _ = self.conn.execute(
                "DELETE FROM query_cache WHERE normalized_title = ?1 AND db_name = ?2",
                params![norm_title, db_name],
            );
            return None;
        }

        Some((result, inserted_at))
    }

    fn insert(&self, norm_title: &str, db_name: &str, result: &CachedResult, epoch: u64) {
        let (found, found_title, authors_json, paper_url) = match result {
            CachedResult::Found {
                title,
                authors,
                url,
            } => (
                1i32,
                Some(title.as_str()),
                Some(serde_json::to_string(authors).unwrap_or_default()),
                url.as_deref(),
            ),
            CachedResult::NotFound => (0i32, None, None, None),
        };

        let _ = self.conn.execute(
            "INSERT OR REPLACE INTO query_cache
                 (normalized_title, db_name, found, found_title, authors, paper_url, inserted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                norm_title,
                db_name,
                found,
                found_title,
                authors_json,
                paper_url,
                epoch
            ],
        );
    }

    fn clear(&self) {
        let _ = self.conn.execute("DELETE FROM query_cache", []);
    }

    /// Remove all expired entries from the database.
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

    fn count(&self) -> usize {
        self.conn
            .query_row("SELECT COUNT(*) FROM query_cache", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap_or(0) as usize
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
/// L2: Optional [`SqliteStore`] for persistence across restarts.
pub struct QueryCache {
    entries: DashMap<CacheKey, CacheEntry>,
    sqlite: Option<Mutex<SqliteStore>>,
    positive_ttl: Duration,
    negative_ttl: Duration,
    hits: AtomicU64,
    misses: AtomicU64,
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
            sqlite: None,
            positive_ttl,
            negative_ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
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
        let store = SqliteStore::open(path)
            .map_err(|e| format!("Failed to open cache database at {}: {}", path.display(), e))?;
        store.evict_expired(positive_ttl, negative_ttl);
        Ok(Self {
            entries: DashMap::new(),
            sqlite: Some(Mutex::new(store)),
            positive_ttl,
            negative_ttl,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        })
    }

    /// Look up a cached result for the given title and database.
    ///
    /// Returns `Some(result)` on cache hit (within TTL), `None` on miss.
    /// The title is normalized before lookup.
    pub fn get(&self, title: &str, db_name: &str) -> Option<DbQueryResult> {
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
                drop(entry);
                self.entries.remove(&key);
                // Fall through to L2
            } else {
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(cached_to_query_result(&entry.result));
            }
        }

        // L2 check
        if let Some(ref sqlite_mutex) = self.sqlite {
            if let Ok(store) = sqlite_mutex.lock() {
                if let Some((result, epoch)) =
                    store.get(&norm, db_name, self.positive_ttl, self.negative_ttl)
                {
                    // Promote to L1
                    let query_result = cached_to_query_result(&result);
                    self.entries.insert(
                        key,
                        CacheEntry {
                            result,
                            inserted_at: epoch_to_instant(epoch),
                            inserted_epoch: epoch,
                        },
                    );
                    self.hits.fetch_add(1, Ordering::Relaxed);
                    return Some(query_result);
                }
            }
        }

        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Insert a query result into the cache.
    ///
    /// Only caches successful results (found or not-found). Errors should NOT
    /// be passed to this method. Write-through: updates both L1 and L2.
    pub fn insert(&self, title: &str, db_name: &str, result: &DbQueryResult) {
        let norm = normalize_title(title);
        let key = CacheKey {
            normalized_title: norm.clone(),
            db_name: db_name.to_string(),
        };

        let cached = match result {
            (Some(found_title), authors, url) => CachedResult::Found {
                title: found_title.clone(),
                authors: authors.clone(),
                url: url.clone(),
            },
            (None, _, _) => CachedResult::NotFound,
        };

        let epoch = now_epoch();

        // L1
        self.entries.insert(
            key,
            CacheEntry {
                result: cached.clone(),
                inserted_at: Instant::now(),
                inserted_epoch: epoch,
            },
        );

        // L2
        if let Some(ref sqlite_mutex) = self.sqlite {
            if let Ok(store) = sqlite_mutex.lock() {
                store.insert(&norm, db_name, &cached, epoch);
            }
        }
    }

    /// Remove all entries from both L1 and L2.
    pub fn clear(&self) {
        self.entries.clear();
        if let Some(ref sqlite_mutex) = self.sqlite {
            if let Ok(store) = sqlite_mutex.lock() {
                store.clear();
            }
        }
    }

    /// Number of cache hits since creation.
    pub fn hits(&self) -> u64 {
        self.hits.load(Ordering::Relaxed)
    }

    /// Number of cache misses since creation.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::Relaxed)
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
        self.sqlite
            .as_ref()
            .and_then(|m| m.lock().ok())
            .map(|s| s.count())
            .unwrap_or(0)
    }

    /// Whether this cache has a persistent SQLite backing store.
    pub fn has_persistence(&self) -> bool {
        self.sqlite.is_some()
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
        } => (Some(title.clone()), authors.clone(), url.clone()),
        CachedResult::NotFound => (None, vec![], None),
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
        let result: DbQueryResult = (
            Some("Attention Is All You Need".into()),
            vec!["Vaswani".into()],
            Some("https://doi.org/10.1234".into()),
        );
        cache.insert("Attention Is All You Need", "CrossRef", &result);
        let cached = cache.get("Attention Is All You Need", "CrossRef");
        assert!(cached.is_some());
        let (title, authors, url) = cached.unwrap();
        assert_eq!(title.unwrap(), "Attention Is All You Need");
        assert_eq!(authors, vec!["Vaswani"]);
        assert_eq!(url.unwrap(), "https://doi.org/10.1234");
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn cache_hit_after_insert_not_found() {
        let cache = QueryCache::default();
        let result: DbQueryResult = (None, vec![], None);
        cache.insert("Nonexistent Paper", "arXiv", &result);
        let cached = cache.get("Nonexistent Paper", "arXiv");
        assert!(cached.is_some());
        let (title, authors, url) = cached.unwrap();
        assert!(title.is_none());
        assert!(authors.is_empty());
        assert!(url.is_none());
    }

    #[test]
    fn cache_miss_different_db() {
        let cache = QueryCache::default();
        let result: DbQueryResult = (Some("A Paper".into()), vec![], None);
        cache.insert("A Paper", "CrossRef", &result);
        assert!(cache.get("A Paper", "arXiv").is_none());
    }

    #[test]
    fn cache_normalized_key() {
        let cache = QueryCache::default();
        let result: DbQueryResult = (Some("Résumé of Methods".into()), vec![], None);
        // Insert with accented title
        cache.insert("Résumé of Methods", "CrossRef", &result);
        // Look up with ASCII equivalent (normalization strips accents)
        let cached = cache.get("Resume of Methods", "CrossRef");
        assert!(cached.is_some());
    }

    #[test]
    fn cache_expired_positive() {
        let cache = QueryCache::new(Duration::from_millis(1), Duration::from_secs(3600));
        let result: DbQueryResult = (Some("Paper".into()), vec![], None);
        cache.insert("Paper", "CrossRef", &result);
        // Sleep briefly to let TTL expire
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("Paper", "CrossRef").is_none());
    }

    #[test]
    fn cache_expired_negative() {
        let cache = QueryCache::new(Duration::from_secs(3600), Duration::from_millis(1));
        let result: DbQueryResult = (None, vec![], None);
        cache.insert("Paper", "CrossRef", &result);
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("Paper", "CrossRef").is_none());
    }

    #[test]
    fn cache_len_and_empty() {
        let cache = QueryCache::default();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
        cache.insert("Paper", "DB", &(Some("Paper".into()), vec![], None));
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_clear() {
        let cache = QueryCache::default();
        cache.insert("Paper", "DB", &(Some("Paper".into()), vec![], None));
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
        let result: DbQueryResult = (
            Some("Deep Learning".into()),
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
        let (title, authors, url) = cached.unwrap();
        assert_eq!(title.unwrap(), "Deep Learning");
        assert_eq!(authors, vec!["LeCun", "Bengio"]);
        assert_eq!(url.unwrap(), "https://doi.org/10.1234");
        // Should have promoted to L1
        assert_eq!(cache2.len(), 1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sqlite_not_found_persists() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let result: DbQueryResult = (None, vec![], None);
        cache.insert("Fake Paper", "arXiv", &result);

        drop(cache);
        let cache2 = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        let cached = cache2.get("Fake Paper", "arXiv");
        assert!(cached.is_some());
        let (title, _, _) = cached.unwrap();
        assert!(title.is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sqlite_clear() {
        let path = temp_cache_path();
        let _ = std::fs::remove_file(&path);

        let cache = QueryCache::open(&path, DEFAULT_POSITIVE_TTL, DEFAULT_NEGATIVE_TTL).unwrap();
        cache.insert("Paper", "DB", &(Some("Paper".into()), vec![], None));
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
            let cache = QueryCache::open(&path, Duration::from_secs(1), Duration::from_secs(1))
                .unwrap();
            cache.insert("Paper", "DB", &(Some("Paper".into()), vec![], None));
            cache.insert("Missing", "DB", &(None, vec![], None));
        }

        std::thread::sleep(Duration::from_secs(2));

        // Re-open — eviction should remove expired entries
        let cache2 =
            QueryCache::open(&path, Duration::from_secs(1), Duration::from_secs(1)).unwrap();
        assert_eq!(cache2.disk_len(), 0);

        let _ = std::fs::remove_file(&path);
    }
}
