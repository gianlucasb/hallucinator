//! Mock database backend for testing.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::{DatabaseBackend, DbQueryResult};
use crate::rate_limit::DbQueryError;

/// A configurable mock response for [`MockDb`].
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum MockResponse {
    /// Simulate a successful match.
    Found {
        title: String,
        authors: Vec<String>,
        url: Option<String>,
    },
    /// Simulate "not found in this database".
    NotFound,
    /// Simulate a 429 rate-limit response.
    RateLimited { retry_after: Option<Duration> },
    /// Simulate a generic error.
    Error(String),
}

/// A hand-rolled mock implementing [`DatabaseBackend`] for tests.
///
/// Supports:
/// - A fixed response (used for every call), **or**
/// - A sequence of responses (one per call, cycling the last if exhausted).
/// - Optional per-call latency.
/// - Call counting via [`call_count()`](MockDb::call_count).
pub struct MockDb {
    name: &'static str,
    /// If `Some`, each call pops the next response (last is repeated if exhausted).
    responses: Mutex<Vec<MockResponse>>,
    /// Fallback when the sequence is empty (or single-response mode).
    fallback: MockResponse,
    delay: Option<Duration>,
    call_count: AtomicUsize,
}

impl MockDb {
    /// Create a mock that always returns `response`.
    pub fn new(name: &'static str, response: MockResponse) -> Self {
        Self {
            name,
            responses: Mutex::new(Vec::new()),
            fallback: response,
            delay: None,
            call_count: AtomicUsize::new(0),
        }
    }

    /// Create a mock that returns responses in order, repeating the last one.
    #[allow(dead_code)]
    pub fn with_sequence(name: &'static str, mut responses: Vec<MockResponse>) -> Self {
        assert!(
            !responses.is_empty(),
            "sequence must have at least one response"
        );
        // Reverse so we can pop() from the front cheaply.
        responses.reverse();
        let fallback = responses.first().cloned().unwrap();
        Self {
            name,
            responses: Mutex::new(responses),
            fallback,
            delay: None,
            call_count: AtomicUsize::new(0),
        }
    }

    /// Set simulated network latency per call.
    #[allow(dead_code)]
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// How many times `query()` has been called.
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn next_response(&self) -> MockResponse {
        let mut seq = self.responses.lock().unwrap();
        if let Some(resp) = seq.pop() {
            resp
        } else {
            self.fallback.clone()
        }
    }
}

impl DatabaseBackend for MockDb {
    fn name(&self) -> &str {
        self.name
    }

    fn query<'a>(
        &'a self,
        _title: &'a str,
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        let response = self.next_response();
        let delay = self.delay;

        Box::pin(async move {
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }

            match response {
                MockResponse::Found {
                    title,
                    authors,
                    url,
                } => Ok((Some(title), authors, url)),
                MockResponse::NotFound => Ok((None, vec![], None)),
                MockResponse::RateLimited { retry_after } => {
                    Err(DbQueryError::RateLimited { retry_after })
                }
                MockResponse::Error(msg) => Err(DbQueryError::Other(msg)),
            }
        })
    }
}
