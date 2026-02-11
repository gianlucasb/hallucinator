use std::collections::{HashMap, VecDeque};

/// Health status of a database backend.
#[derive(Debug, Clone)]
pub struct DbHealth {
    pub total_queries: usize,
    pub successful: usize,
    pub failed: usize,
    pub hits: usize,
    pub avg_response_ms: f64,
}

impl DbHealth {
    pub fn new() -> Self {
        Self {
            total_queries: 0,
            successful: 0,
            failed: 0,
            hits: 0,
            avg_response_ms: 0.0,
        }
    }

    pub fn record(&mut self, success: bool, is_match: bool, elapsed_ms: f64) {
        self.total_queries += 1;
        if success {
            self.successful += 1;
        } else {
            self.failed += 1;
        }
        if is_match {
            self.hits += 1;
        }
        // Running average
        self.avg_response_ms =
            self.avg_response_ms + (elapsed_ms - self.avg_response_ms) / self.total_queries as f64;
    }

    /// Health indicator: full, half, empty.
    pub fn indicator(&self) -> char {
        if self.total_queries == 0 {
            '\u{25CB}' // ○ unknown
        } else {
            let success_rate = self.successful as f64 / self.total_queries as f64;
            if success_rate >= 0.8 {
                '\u{25CF}' // ● healthy
            } else if success_rate >= 0.4 {
                '\u{25D0}' // ◐ degraded
            } else {
                '\u{25CB}' // ○ unhealthy
            }
        }
    }
}

/// An active (in-flight) query.
#[derive(Debug, Clone)]
pub struct ActiveQuery {
    pub db_name: String,
    pub ref_title: String,
}

/// State for the activity panel.
#[derive(Debug, Clone)]
pub struct ActivityState {
    pub db_health: HashMap<String, DbHealth>,
    /// Throughput buckets: refs completed per time bucket.
    pub throughput_buckets: VecDeque<u16>,
    pub active_queries: Vec<ActiveQuery>,
    /// Total refs completed (for throughput tracking).
    pub total_completed: usize,
    /// Recent log messages: (text, is_warning).
    pub messages: VecDeque<(String, bool)>,
    /// Whether the DBLP timeout warning has already been shown.
    pub dblp_timeout_warned: bool,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            db_health: HashMap::new(),
            throughput_buckets: VecDeque::with_capacity(60),
            active_queries: Vec::new(),
            total_completed: 0,
            messages: VecDeque::new(),
            dblp_timeout_warned: false,
        }
    }
}

impl ActivityState {
    pub fn log(&mut self, msg: String) {
        if self.messages.len() >= 50 {
            self.messages.pop_front();
        }
        self.messages.push_back((msg, false));
    }

    pub fn log_warn(&mut self, msg: String) {
        if self.messages.len() >= 50 {
            self.messages.pop_front();
        }
        self.messages.push_back((msg, true));
    }

    pub fn record_db_complete(
        &mut self,
        db_name: &str,
        success: bool,
        is_match: bool,
        elapsed_ms: f64,
    ) {
        let health = self
            .db_health
            .entry(db_name.to_string())
            .or_insert_with(DbHealth::new);
        health.record(success, is_match, elapsed_ms);
    }

    pub fn push_throughput(&mut self, count: u16) {
        if self.throughput_buckets.len() >= 60 {
            self.throughput_buckets.pop_front();
        }
        self.throughput_buckets.push_back(count);
    }

    /// Build sparkline string from throughput buckets.
    pub fn sparkline(&self) -> String {
        const CHARS: &[char] = &[
            ' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}',
            '\u{2587}', '\u{2588}',
        ];
        let max = self
            .throughput_buckets
            .iter()
            .copied()
            .max()
            .unwrap_or(1)
            .max(1);
        self.throughput_buckets
            .iter()
            .map(|&v| {
                let idx = ((v as f64 / max as f64) * 8.0) as usize;
                CHARS[idx.min(8)]
            })
            .collect()
    }
}
