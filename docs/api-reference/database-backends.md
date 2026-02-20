# Database Backends

This document covers the `DatabaseBackend` trait, the existing database implementations, and how to add a new backend.

## The `DatabaseBackend` Trait

Defined in `hallucinator-core/src/db/mod.rs`:

```rust
pub trait DatabaseBackend: Send + Sync {
    /// Human-readable name (e.g., "CrossRef", "arXiv")
    fn name(&self) -> &str;

    /// Whether this is a local (offline) database.
    /// Local backends are queried inline by the coordinator (not via drainer tasks).
    fn is_local(&self) -> bool { false }

    /// Whether this backend requires a DOI to query (e.g., DOI resolver).
    /// References without a DOI are skipped for these backends.
    fn requires_doi(&self) -> bool { false }

    /// Query by title. Returns found title, authors, paper URL, and optional retraction info.
    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send + 'a>>;

    /// Query by DOI. Default implementation returns empty/not-found.
    fn query_doi<'a>(
        &'a self,
        doi: &'a str,
        title: &'a str,
        authors: &'a [String],
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> DoiQueryResult<'a> { ... }
}
```

### Return Types

```rust
pub struct DbQueryResult {
    pub found_title: Option<String>,   // Title as found in the database
    pub authors: Vec<String>,          // Author names
    pub paper_url: Option<String>,     // Direct link to the paper
    pub retraction: Option<RetractionResult>,  // Only CrossRef populates this
}

pub enum DbQueryError {
    RateLimited { retry_after: Option<Duration> },
    Other(String),
}
```

A `DbQueryResult` with `found_title = Some(...)` indicates the title was found. The validation engine then compares authors (if provided) to determine Verified vs. AuthorMismatch.

## Existing Backends

### Remote (HTTP-based)

| Backend | Name | Rate Limit | Auth | Notes |
|---------|------|-----------|------|-------|
| `CrossRef` | `"CrossRef"` | 1/s (3/s with mailto) | Optional mailto | Extracts retraction info inline |
| `Arxiv` | `"arXiv"` | 3/s | None | Searches arXiv API |
| `DblpOnline` | `"DBLP"` | 1/s | None | DBLP search API |
| `SemanticScholar` | `"Semantic Scholar"` | 1/s (100/s with key) | Optional API key | Searches papers by title |
| `EuropePmc` | `"Europe PMC"` | 3/s | None | Biomedical/life science literature |
| `PubMed` | `"PubMed"` | 3/s | None | Biomedical literature via NCBI |
| `OpenAlex` | `"OpenAlex"` | 10/s | Required API key | Inserted first in DB list when enabled |
| `DoiResolver` | `"DOI"` | 5/s | None | Resolves DOI via doi.org (`requires_doi = true`) |
| `AclAnthology` | `"ACL Anthology"` | 2/s | None | ACL Anthology online scraping |
| `NeurIPS` | `"NeurIPS"` | — | None | Currently disabled |
| `Ssrn` | `"SSRN"` | — | None | Currently disabled |
| `Searxng` | `"Web Search"` | 1/s | None | Meta-search fallback (requires self-hosted SearxNG) |

### Local (Offline)

| Backend | Name | Storage | Notes |
|---------|------|---------|-------|
| `DblpOffline` | `"DBLP"` | SQLite FTS5 | ~2–3GB, built from DBLP XML dump |
| `AclOffline` | `"ACL Anthology"` | SQLite FTS5 | ~50–100MB, built from ACL Anthology XML |

Note: offline and online backends for the same database share the same `name()`. The system avoids running both simultaneously — if an offline DB is available, the online API is skipped.

Local backends return `is_local() = true` and are queried inline by the coordinator task before dispatching to remote drainers. If a local backend verifies a reference, all remote queries are skipped.

## How Backends Are Selected

The `build_database_list()` function in `hallucinator-core/src/orchestrator.rs` assembles the list of enabled backends at startup:

1. **OpenAlex** — Added first if API key is provided
2. **CrossRef** — Always enabled (with optional mailto for higher rate)
3. **arXiv** — Always enabled
4. **DBLP Online** — Always enabled
5. **Semantic Scholar** — Always enabled (rate depends on API key)
6. **Europe PMC** — Always enabled
7. **PubMed** — Always enabled
8. **ACL Anthology** (online) — Always enabled
9. **DOI Resolver** — Always enabled (only queries refs with DOIs)
10. **DBLP Offline** — Added if `dblp_offline_path` is configured
11. **ACL Offline** — Added if `acl_offline_path` is configured
12. **SearxNG** — Used as last-resort fallback for NotFound refs (not in the main drainer pool)

Backends listed in `Config.disabled_dbs` are excluded. Database names are matched case-sensitively.

## Adding a New Backend

### Step 1: Create the Module

Create `hallucinator-core/src/db/my_backend.rs`:

```rust
use std::time::Duration;

use crate::db::{DatabaseBackend, DbQueryError, DbQueryResult};

pub struct MyBackend {
    // Configuration fields
}

impl MyBackend {
    pub fn new() -> Self {
        Self { }
    }
}

impl DatabaseBackend for MyBackend {
    fn name(&self) -> &str {
        "My Backend"
    }

    fn query<'a>(
        &'a self,
        title: &'a str,
        client: &'a reqwest::Client,
        timeout: Duration,
    ) -> std::pin::Pin<Box<dyn std::future::Future<
        Output = Result<DbQueryResult, DbQueryError>
    > + Send + 'a>> {
        Box::pin(async move {
            // 1. Build your API request
            let url = format!("https://api.example.com/search?q={}",
                              urlencoding::encode(title));

            // 2. Execute with timeout
            let response = client
                .get(&url)
                .timeout(timeout)
                .send()
                .await
                .map_err(|e| {
                    if e.is_timeout() {
                        DbQueryError::Other("timeout".into())
                    } else if e.status().map_or(false, |s| s == 429) {
                        DbQueryError::RateLimited { retry_after: None }
                    } else {
                        DbQueryError::Other(e.to_string())
                    }
                })?;

            // 3. Parse response
            let body: serde_json::Value = response
                .json()
                .await
                .map_err(|e| DbQueryError::Other(e.to_string()))?;

            // 4. Extract result
            if let Some(found_title) = body.get("title").and_then(|t| t.as_str()) {
                Ok(DbQueryResult {
                    found_title: Some(found_title.to_string()),
                    authors: vec![],  // Extract authors if available
                    paper_url: body.get("url")
                        .and_then(|u| u.as_str())
                        .map(|s| s.to_string()),
                    retraction: None,
                })
            } else {
                Ok(DbQueryResult {
                    found_title: None,
                    authors: vec![],
                    paper_url: None,
                    retraction: None,
                })
            }
        })
    }
}
```

### Step 2: Register the Module

In `hallucinator-core/src/db/mod.rs`, add:

```rust
pub mod my_backend;
```

### Step 3: Add to Database List

In `hallucinator-core/src/orchestrator.rs`, add the backend to `build_database_list()`:

```rust
dbs.push(Box::new(my_backend::MyBackend::new()));
```

### Step 4: Configure Rate Limiting

In `hallucinator-core/src/rate_limit.rs`, add a rate limiter for your backend in `RateLimiters::new()`:

```rust
// Example: 5 requests per second
let my_backend = AdaptiveDbLimiter::new(
    governor::Quota::per_second(std::num::NonZeroU32::new(5).unwrap()),
);
```

### Key Implementation Notes

- **Title matching:** You don't need to do fuzzy matching yourself. Return the title as found in your database; the validation engine handles comparison via `normalize_title()` and `rapidfuzz`.
- **Authors:** Return author names as provided by your API. The validation engine normalizes them before comparison.
- **Rate limiting:** Return `DbQueryError::RateLimited` on HTTP 429 responses. The adaptive rate limiter will back off automatically.
- **Caching:** Results are cached automatically by the validation engine. You don't need to implement caching in your backend.
- **Timeout:** Always use the provided `timeout` parameter with your HTTP requests.
