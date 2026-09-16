//! Mock database backend for testing.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::{ArxivIdQueryResult, DatabaseBackend, DbQueryResult};
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
    is_local: bool,
    /// Simulates a backend that implements `query_arxiv_id` (like the
    /// offline arXiv backend): `Some((id, response))` makes `query_arxiv_id`
    /// return `response` when called with exactly that id, and `None`
    /// (fall through to title search) for any other id. Regression coverage
    /// for the bug where `arxiv_id_context`/`doi_context` never reached
    /// local backends — see `rate_limit::tests`.
    arxiv_id_response: Option<(&'static str, MockResponse)>,
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
            is_local: false,
            arxiv_id_response: None,
        }
    }

    /// Make `is_local()` return `true` (routes through the local-DB path
    /// instead of the per-DB remote drainer).
    #[allow(dead_code)]
    pub fn with_is_local(mut self, is_local: bool) -> Self {
        self.is_local = is_local;
        self
    }

    /// Simulate a backend that answers `query_arxiv_id` for exactly `id`,
    /// independent of whatever `query`/`query_with_authors` would return.
    #[allow(dead_code)]
    pub fn with_arxiv_id_response(mut self, id: &'static str, response: MockResponse) -> Self {
        self.arxiv_id_response = Some((id, response));
        self
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
            is_local: false,
            arxiv_id_response: None,
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

    fn is_local(&self) -> bool {
        self.is_local
    }

    fn query_arxiv_id<'a>(
        &'a self,
        arxiv_id: &'a str,
        _title: &'a str,
        _authors: &'a [String],
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> ArxivIdQueryResult<'a> {
        let matched = self
            .arxiv_id_response
            .as_ref()
            .filter(|(id, _)| *id == arxiv_id)
            .map(|(_, resp)| resp.clone());
        Box::pin(async move {
            match matched? {
                MockResponse::Found {
                    title,
                    authors,
                    url,
                } => Some(Ok(DbQueryResult::found(title, authors, url))),
                MockResponse::NotFound => Some(Ok(DbQueryResult::not_found())),
                MockResponse::RateLimited { retry_after } => {
                    Some(Err(DbQueryError::RateLimited { retry_after }))
                }
                MockResponse::Error(msg) => Some(Err(DbQueryError::Other(msg))),
            }
        })
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
                } => Ok(DbQueryResult::found(title, authors, url)),
                MockResponse::NotFound => Ok(DbQueryResult::not_found()),
                MockResponse::RateLimited { retry_after } => {
                    Err(DbQueryError::RateLimited { retry_after })
                }
                MockResponse::Error(msg) => Err(DbQueryError::Other(msg)),
            }
        })
    }
}
