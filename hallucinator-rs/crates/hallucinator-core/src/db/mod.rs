//! Database backend trait and implementations for querying academic databases.

pub mod acl;
pub mod arxiv;
pub mod crossref;
pub mod dblp;
pub mod doi_resolver;
pub mod europe_pmc;
pub mod neurips;
pub mod openalex;
pub mod pubmed;
pub mod semantic_scholar;
pub mod ssrn;

#[cfg(test)]
pub(crate) mod mock;

use std::future::Future;
use std::pin::Pin;

pub use crate::rate_limit::DbQueryError;

/// Result of a database query: (found_title, authors, paper_url).
/// `None` title means not found.
pub type DbQueryResult = (Option<String>, Vec<String>, Option<String>);

/// Result type for `query_doi`: `None` means the backend doesn't handle DOI queries.
pub type DoiQueryResult<'a> =
    Pin<Box<dyn Future<Output = Option<Result<DbQueryResult, DbQueryError>>> + Send + 'a>>;

/// A database backend that can search for papers by title.
pub trait DatabaseBackend: Send + Sync {
    /// The canonical name of this database (e.g., "CrossRef", "arXiv").
    fn name(&self) -> &str;

    /// Whether this backend is local (offline SQLite, etc.) and needs no rate limiting.
    fn is_local(&self) -> bool {
        false
    }

    /// Whether this backend requires a DOI instead of a title search.
    /// When true, the drainer skips refs without a DOI and uses `query_doi` instead of `query`.
    fn requires_doi(&self) -> bool {
        false
    }

    /// Query the database for a paper matching the given title.
    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>>;

    /// Query the database using a DOI.
    ///
    /// Returns `None` if this backend doesn't support DOI queries (default).
    /// Returns `Some(result)` when the backend handled the query via DOI.
    fn query_doi<'a>(
        &'a self,
        _doi: &'a str,
        _title: &'a str,
        _authors: &'a [String],
        _client: &'a reqwest::Client,
        _timeout: std::time::Duration,
    ) -> DoiQueryResult<'a> {
        Box::pin(async { None })
    }
}
