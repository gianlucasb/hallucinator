use crate::authors::validate_authors;
use crate::db::DatabaseBackend;
use crate::{Config, Status};
use std::sync::Arc;
use std::time::Duration;

/// Result of querying all databases for a single reference.
#[derive(Debug, Clone)]
pub struct DbSearchResult {
    pub status: Status,
    pub source: Option<String>,
    pub found_authors: Vec<String>,
    pub paper_url: Option<String>,
    pub failed_dbs: Vec<String>,
}

/// Query all databases concurrently for a single reference, with early exit on match.
pub async fn query_all_databases(
    title: &str,
    ref_authors: &[String],
    config: &Config,
    client: &reqwest::Client,
    longer_timeout: bool,
    only_dbs: Option<&[String]>,
) -> DbSearchResult {
    let check_openalex_authors = config.check_openalex_authors;
    let timeout = if longer_timeout {
        Duration::from_secs(config.db_timeout_secs * 2)
    } else {
        Duration::from_secs(config.db_timeout_secs)
    };

    // Build the list of databases to query
    let databases: Vec<Arc<dyn DatabaseBackend>> = build_database_list(config, only_dbs)
        .into_iter()
        .map(|b| Arc::from(b))
        .collect();

    if databases.is_empty() {
        return DbSearchResult {
            status: Status::NotFound,
            source: None,
            found_authors: vec![],
            paper_url: None,
            failed_dbs: vec![],
        };
    }

    // Use tokio::select! pattern with JoinSet for early exit
    let mut join_set = tokio::task::JoinSet::new();

    for db in &databases {
        let db = Arc::clone(db);
        let title = title.to_string();
        let client = client.clone();
        let ref_authors = ref_authors.to_vec();

        join_set.spawn(async move {
            let name = db.name().to_string();
            let result = db.query(&title, &client, timeout).await;
            (name, result, ref_authors)
        });
    }

    let mut first_mismatch: Option<DbSearchResult> = None;
    let mut failed_dbs = Vec::new();

    while let Some(result) = join_set.join_next().await {
        let (name, query_result, ref_authors) = match result {
            Ok(r) => r,
            Err(_) => continue,
        };

        match query_result {
            Ok((Some(_found_title), found_authors, paper_url)) => {
                if ref_authors.is_empty() || validate_authors(&ref_authors, &found_authors) {
                    // Found and verified — abort remaining tasks
                    join_set.abort_all();
                    return DbSearchResult {
                        status: Status::Verified,
                        source: Some(name),
                        found_authors,
                        paper_url,
                        failed_dbs: vec![],
                    };
                } else if first_mismatch.is_none() && (name != "OpenAlex" || check_openalex_authors) {
                    first_mismatch = Some(DbSearchResult {
                        status: Status::AuthorMismatch,
                        source: Some(name),
                        found_authors,
                        paper_url,
                        failed_dbs: vec![],
                    });
                }
            }
            Ok((None, _, _)) => {
                // Not found in this DB
            }
            Err(_e) => {
                failed_dbs.push(name);
            }
        }
    }

    if let Some(mismatch) = first_mismatch {
        return mismatch;
    }

    DbSearchResult {
        status: Status::NotFound,
        source: None,
        found_authors: vec![],
        paper_url: None,
        failed_dbs: failed_dbs,
    }
}

/// Build the list of database backends based on config.
fn build_database_list(
    config: &Config,
    only_dbs: Option<&[String]>,
) -> Vec<Box<dyn DatabaseBackend>> {
    use crate::db::*;

    let mut databases: Vec<Box<dyn DatabaseBackend>> = Vec::new();

    let should_include = |name: &str| -> bool {
        if config.disabled_dbs.iter().any(|d| d.eq_ignore_ascii_case(name)) {
            return false;
        }
        match only_dbs {
            Some(dbs) => dbs.iter().any(|d| d == name),
            None => true,
        }
    };

    if should_include("CrossRef") {
        databases.push(Box::new(crossref::CrossRef));
    }
    if should_include("arXiv") {
        databases.push(Box::new(arxiv::Arxiv));
    }
    if should_include("DBLP") {
        // Use offline DBLP if available, otherwise online
        if let Some(ref db) = config.dblp_offline_db {
            databases.push(Box::new(dblp::DblpOffline {
                db: std::sync::Arc::clone(db),
            }));
        } else {
            databases.push(Box::new(dblp::DblpOnline));
        }
    }
    if should_include("Semantic Scholar") {
        databases.push(Box::new(semantic_scholar::SemanticScholar {
            api_key: config.s2_api_key.clone(),
        }));
    }
    if should_include("SSRN") {
        databases.push(Box::new(ssrn::Ssrn));
    }
    if should_include("ACL Anthology") {
        databases.push(Box::new(acl::AclAnthology));
    }
    if should_include("NeurIPS") {
        databases.push(Box::new(neurips::NeurIPS));
    }
    if should_include("Europe PMC") {
        databases.push(Box::new(europe_pmc::EuropePmc));
    }
    if should_include("PubMed") {
        databases.push(Box::new(pubmed::PubMed));
    }
    if let Some(ref key) = config.openalex_key {
        if should_include("OpenAlex") {
            databases.insert(
                0,
                Box::new(openalex::OpenAlex {
                    api_key: key.clone(),
                }),
            );
        }
    }

    databases
}
