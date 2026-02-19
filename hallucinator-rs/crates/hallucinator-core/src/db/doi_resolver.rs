//! DOI resolver backend — validates references by looking up their DOI at doi.org.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use crate::doi::{DoiMatchResult, check_doi_match, validate_doi};
use crate::rate_limit::DbQueryError;

use super::{DbQueryResult, DoiQueryResult};

/// A database backend that resolves DOIs via doi.org metadata.
pub struct DoiResolver;

impl super::DatabaseBackend for DoiResolver {
    fn name(&self) -> &str {
        "DOI"
    }

    fn requires_doi(&self) -> bool {
        true
    }

    /// Title-based search is not supported — always returns not-found.
    fn query<'a>(
        &'a self,
        _title: &'a str,
        _client: &'a reqwest::Client,
        _timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>> {
        Box::pin(async { Ok(DbQueryResult::not_found()) })
    }

    fn query_doi<'a>(
        &'a self,
        doi: &'a str,
        title: &'a str,
        authors: &'a [String],
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> DoiQueryResult<'a> {
        Box::pin(async move {
            let doi_result = validate_doi(doi, client, timeout).await;
            let match_result = check_doi_match(&doi_result, title, authors);

            match match_result {
                DoiMatchResult::Verified { doi_authors, .. } => {
                    let url = format!("https://doi.org/{}", doi);
                    Some(Ok(DbQueryResult::found(
                        doi_result.title.unwrap_or_else(|| title.to_string()),
                        doi_authors,
                        Some(url),
                    )))
                }
                DoiMatchResult::AuthorMismatch { doi_authors, .. } => {
                    // Return found title + authors — report_result will handle
                    // the author mismatch classification.
                    let url = format!("https://doi.org/{}", doi);
                    Some(Ok(DbQueryResult::found(
                        doi_result.title.unwrap_or_else(|| title.to_string()),
                        doi_authors,
                        Some(url),
                    )))
                }
                DoiMatchResult::TitleMismatch { .. } | DoiMatchResult::Invalid { .. } => {
                    // DOI didn't match or was invalid — return not-found
                    Some(Ok(DbQueryResult::not_found()))
                }
            }
        })
    }
}
