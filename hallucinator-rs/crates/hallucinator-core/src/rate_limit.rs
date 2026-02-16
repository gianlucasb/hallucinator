//! Per-database rate limiting and exponential backoff for 429 responses.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::db::{DatabaseBackend, DbQueryResult};

/// Enforces minimum intervals between requests to each database.
pub struct RateLimiter {
    /// Minimum interval between requests per database name.
    intervals: HashMap<String, Duration>,
    /// Last request time per database name.
    last_request: Mutex<HashMap<String, Instant>>,
}

impl RateLimiter {
    /// Create a new rate limiter with the given per-database intervals.
    pub fn new(intervals: HashMap<String, Duration>) -> Self {
        Self {
            intervals,
            last_request: Mutex::new(HashMap::new()),
        }
    }

    /// Wait until the rate limit interval has elapsed for the given database,
    /// then record the current time as the last request time.
    pub async fn acquire(&self, db_name: &str) {
        let interval = match self.intervals.get(db_name) {
            Some(d) if *d > Duration::ZERO => *d,
            _ => return, // No rate limit for this DB
        };

        loop {
            let now = Instant::now();
            let wait = {
                let mut last = self.last_request.lock().await;
                if let Some(last_time) = last.get(db_name) {
                    let elapsed = now.duration_since(*last_time);
                    if elapsed >= interval {
                        last.insert(db_name.to_string(), now);
                        Duration::ZERO
                    } else {
                        interval - elapsed
                    }
                } else {
                    last.insert(db_name.to_string(), now);
                    Duration::ZERO
                }
            };

            if wait.is_zero() {
                return;
            }

            tokio::time::sleep(wait).await;
        }
    }

    /// Record a backoff delay so other tasks also wait before hitting this DB.
    pub async fn record_backoff(&self, db_name: &str, backoff: Duration) {
        let mut last = self.last_request.lock().await;
        // Set last_request to now + backoff (future time) so other acquires will wait.
        let future_time = Instant::now() + backoff;
        last.insert(db_name.to_string(), future_time);
    }
}

/// Default rate limit intervals.
pub fn default_rate_limits() -> HashMap<String, Duration> {
    let mut m = HashMap::new();
    m.insert("Semantic Scholar".to_string(), Duration::from_millis(1000));
    m
}

const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Query a database with exponential backoff on 429 errors.
///
/// Retries up to `MAX_RETRIES` times with doubling delays (1s, 2s, 4s),
/// bounded by the overall `timeout`.
pub async fn query_with_backoff(
    db: &Arc<dyn DatabaseBackend>,
    title: &str,
    client: &reqwest::Client,
    timeout: Duration,
    rate_limiter: &Arc<RateLimiter>,
) -> Result<DbQueryResult, String> {
    let db_name = db.name();
    let mut backoff = INITIAL_BACKOFF;

    for attempt in 0..=MAX_RETRIES {
        rate_limiter.acquire(db_name).await;

        let result = db.query(title, client, timeout).await;

        match &result {
            Err(e) if attempt < MAX_RETRIES && is_rate_limited(e) => {
                log::warn!(
                    "{}: rate limited (429), retrying in {:?} (attempt {}/{})",
                    db_name,
                    backoff,
                    attempt + 1,
                    MAX_RETRIES
                );
                // Push back the rate limiter so concurrent tasks also wait
                rate_limiter.record_backoff(db_name, backoff).await;
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
            _ => return result,
        }
    }

    // Unreachable, but satisfy the compiler
    Err(format!("{}: max retries exceeded", db_name))
}

fn is_rate_limited(error: &str) -> bool {
    error.contains("429") || error.contains("rate limit")
}
