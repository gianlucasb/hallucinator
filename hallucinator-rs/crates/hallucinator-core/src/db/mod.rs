//! Database backend trait and implementations for querying academic databases.

pub mod acl;
pub mod arxiv;
pub mod crossref;
pub mod dblp;
pub mod europe_pmc;
pub mod neurips;
pub mod openalex;
pub mod pubmed;
pub mod searxng;
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

/// A database backend that can search for papers by title.
pub trait DatabaseBackend: Send + Sync {
    /// The canonical name of this database (e.g., "CrossRef", "arXiv").
    fn name(&self) -> &str;

    /// Whether this backend is local (offline SQLite, etc.) and needs no rate limiting.
    fn is_local(&self) -> bool {
        false
    }

    /// Query the database for a paper matching the given title.
    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: std::time::Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>>;
}
