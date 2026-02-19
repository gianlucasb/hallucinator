//! Offline OpenAlex database builder and querier.
//!
//! Provides a Tantivy-backed OpenAlex index with full-text search and fuzzy
//! title matching via rapidfuzz. Downloads data from the public OpenAlex S3
//! snapshot, filters by work type, and supports incremental updates via
//! date-partitioned data. Mirrors the `hallucinator-dblp` / `hallucinator-acl`
//! crate architecture.

mod builder;
mod metadata;
mod query;
mod s3;

use std::path::{Path, PathBuf};

use tantivy::Index;
use thiserror::Error;

pub use query::DEFAULT_THRESHOLD;

#[derive(Error, Debug)]
pub enum OpenAlexError {
    #[error("index error: {0}")]
    Index(String),
    #[error("download error: {0}")]
    Download(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl From<tantivy::TantivyError> for OpenAlexError {
    fn from(e: tantivy::TantivyError) -> Self {
        OpenAlexError::Index(e.to_string())
    }
}

/// A publication record from the offline OpenAlex database.
#[derive(Debug, Clone)]
pub struct OpenAlexRecord {
    pub title: String,
    pub authors: Vec<String>,
    pub url: Option<String>,
}

/// Query result with fuzzy match score.
#[derive(Debug, Clone)]
pub struct OpenAlexQueryResult {
    pub record: OpenAlexRecord,
    pub score: f64,
}

/// Database build/download statistics.
#[derive(Debug, Clone)]
pub struct DatabaseInfo {
    pub build_date: Option<String>,
    pub schema_version: Option<String>,
    pub publication_count: Option<u64>,
    pub last_sync_date: Option<String>,
}

/// Progress events emitted during database building.
#[derive(Debug, Clone)]
pub enum BuildProgress {
    ListingPartitions {
        message: String,
    },
    Downloading {
        partitions_done: u64,
        partitions_total: u64,
        bytes_downloaded: u64,
        records_indexed: u64,
    },
    Committing {
        records_indexed: u64,
    },
    Merging,
    Complete {
        publications: u64,
        skipped: bool,
    },
}

/// Result of a staleness check.
#[derive(Debug, Clone)]
pub struct StalenessCheck {
    pub is_stale: bool,
    pub age_days: Option<u64>,
    pub build_date: Option<String>,
}

/// Handle to an opened offline OpenAlex database.
pub struct OpenAlexDatabase {
    index: Index,
    reader: tantivy::IndexReader,
    path: PathBuf,
}

impl OpenAlexDatabase {
    /// Open an existing offline OpenAlex index directory.
    pub fn open(path: &Path) -> Result<Self, OpenAlexError> {
        let index = Index::open_in_dir(path)?;
        let reader = index
            .reader_builder()
            .reload_policy(tantivy::ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .map_err(|e: tantivy::TantivyError| OpenAlexError::Index(e.to_string()))?;

        Ok(Self {
            index,
            reader,
            path: path.to_path_buf(),
        })
    }

    /// Query for a title, returning the best fuzzy match above the default threshold.
    pub fn query(&self, title: &str) -> Result<Option<OpenAlexQueryResult>, OpenAlexError> {
        query::query_index(&self.index, &self.reader, title, DEFAULT_THRESHOLD)
    }

    /// Query with a custom similarity threshold.
    pub fn query_with_threshold(
        &self,
        title: &str,
        threshold: f64,
    ) -> Result<Option<OpenAlexQueryResult>, OpenAlexError> {
        query::query_index(&self.index, &self.reader, title, threshold)
    }

    /// Get database metadata/info.
    pub fn info(&self) -> Result<DatabaseInfo, OpenAlexError> {
        let meta = metadata::read_metadata(&self.path)?;
        Ok(DatabaseInfo {
            build_date: meta.build_date,
            schema_version: Some(meta.schema_version),
            publication_count: meta.publication_count,
            last_sync_date: meta.last_sync_date,
        })
    }

    /// Check if the database is stale (older than `threshold_days`).
    pub fn check_staleness(&self, threshold_days: u64) -> Result<StalenessCheck, OpenAlexError> {
        let meta = metadata::read_metadata(&self.path)?;

        let age_days = meta.build_date.as_ref().and_then(|ts| {
            let build_secs: u64 = ts.parse().ok()?;
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs();
            Some((now_secs.saturating_sub(build_secs)) / 86400)
        });

        let is_stale = age_days.is_none_or(|days| days >= threshold_days);

        Ok(StalenessCheck {
            is_stale,
            age_days,
            build_date: meta.build_date,
        })
    }

    /// Convenience: check staleness with the default 30-day threshold.
    pub fn is_stale(&self) -> Result<bool, OpenAlexError> {
        Ok(self.check_staleness(30)?.is_stale)
    }

    /// Get the path to the index directory.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Download and build (or incrementally update) the offline OpenAlex database.
///
/// Downloads work records from the public OpenAlex S3 snapshot, filters by
/// academic work types, and builds a Tantivy index with fuzzy search support.
/// Uses date partitioning for incremental updates.
/// Returns `false` if the index is already up to date.
pub async fn build_database(
    db_path: &Path,
    progress: impl FnMut(BuildProgress),
) -> Result<bool, OpenAlexError> {
    builder::build(db_path, None, None, progress).await
}

/// Build or incrementally update the OpenAlex index with filtering options.
///
/// - `since`: only download S3 partitions newer than this date (YYYY-MM-DD).
///   Useful for testing with a small slice of data.
/// - `min_year`: skip works published before this year during indexing.
///   e.g. `Some(2020)` keeps only 2020+ publications.
pub async fn build_database_filtered(
    db_path: &Path,
    since: Option<&str>,
    min_year: Option<u32>,
    progress: impl FnMut(BuildProgress),
) -> Result<bool, OpenAlexError> {
    builder::build(db_path, since.map(String::from), min_year, progress).await
}
