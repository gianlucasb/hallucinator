//! Shared types, traits, and fuzzy matching for hallucinator crates.
//!
//! This crate provides the common abstractions that allow hallucinator's offline
//! database crates (DBLP, ACL, OpenAlex) to work with different storage backends
//! (SQLite, Tantivy, PostgreSQL, Meilisearch, etc.).

pub mod fuzzy;

/// A parsed academic record from any source (DBLP, ACL, OpenAlex, etc.).
///
/// This is the common currency between parsers and storage backends.
/// Parsers produce these; `RecordSink` implementations consume them.
#[derive(Debug, Clone)]
pub struct ParsedRecord {
    /// Source-specific identifier (DBLP key, ACL anthology ID, OpenAlex W-id).
    pub source_id: String,
    /// Paper title.
    pub title: String,
    /// Author names in order.
    pub authors: Vec<String>,
    /// Paper URL (e.g., DBLP `<ee>` URL, ACL Anthology URL).
    pub url: Option<String>,
    /// DOI if available.
    pub doi: Option<String>,
}

/// Result of a title search against a `TitleIndex`.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// The matched title as stored in the index.
    pub title: String,
    /// Author names.
    pub authors: Vec<String>,
    /// Paper URL.
    pub url: Option<String>,
    /// Fuzzy match score (0.0–1.0).
    pub score: f64,
}

/// Trait for storing parsed records (write path).
///
/// Implementations handle deduplication, indexing, and backend-specific
/// optimizations (e.g., bulk load mode, transaction batching).
pub trait RecordSink {
    type Error: std::error::Error;

    /// Insert a batch of records. Implementations handle deduplication.
    fn insert_batch(&mut self, records: &[ParsedRecord]) -> Result<(), Self::Error>;

    /// Finalize after all batches (rebuild indexes, commit, vacuum, etc.).
    fn finalize(&mut self) -> Result<(), Self::Error>;
}

/// Trait for querying records by title (read path).
///
/// Implementations retrieve candidates from their backing store (FTS5, Tantivy,
/// `pg_trgm`, etc.) and use fuzzy matching to find the best match above the
/// given threshold.
pub trait TitleIndex: Send {
    type Error: std::error::Error + Send + Sync;

    /// Search for a title, returning the best match above the threshold.
    ///
    /// `threshold` is a similarity score in 0.0–1.0 (e.g., 0.90).
    fn search(&self, title: &str, threshold: f64) -> Result<Option<QueryResult>, Self::Error>;
}
