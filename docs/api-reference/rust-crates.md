# Using Hallucinator as a Rust Library

This guide covers how to use hallucinator crates as dependencies in your own Rust project.

## Which Crate to Depend On

| Use case | Crate | What you get |
|----------|-------|-------------|
| Validate references programmatically | `hallucinator-core` | `check_references()`, all DB backends, caching, rate limiting |
| Extract references from PDFs | `hallucinator-pdf` + `hallucinator-pdf-mupdf` | `PdfExtractor`, section detection, title/author extraction |
| Parse BBL/BIB files | `hallucinator-bbl` | `extract_references_from_bbl()`, `extract_references_from_bib()` |
| Unified file dispatch | `hallucinator-ingest` | Auto-detection (PDF/BBL/BIB/archive), streaming archive extraction |
| Export results | `hallucinator-reporting` | JSON, CSV, Markdown, Text, HTML export |
| Build offline DBLP | `hallucinator-dblp` | `build_database()`, `DblpDatabase::search()` |
| Build offline ACL | `hallucinator-acl` | `build_database()`, `AclDatabase::search()` |

Most users will want `hallucinator-core` for validation and `hallucinator-ingest` for file handling.

## Minimal Example: Validate References

```rust
use hallucinator_core::{Config, ProgressEvent, RateLimiters, check_references};
use hallucinator_ingest::extract_references;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::path::Path::new("paper.pdf");

    // Extract references
    let extraction = extract_references(path)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    println!("Found {} references", extraction.references.len());

    // Build config with defaults
    let config = Config {
        rate_limiters: Arc::new(RateLimiters::new(false, false)),
        ..Default::default()
    };

    // Validate
    let cancel = CancellationToken::new();
    let results = check_references(
        extraction.references,
        config,
        |event| {
            if let ProgressEvent::Result { result, .. } = &event {
                println!("[{:?}] {}", result.status, result.title);
            }
        },
        cancel,
    ).await;

    println!("{} total, {} verified, {} not found",
        results.len(),
        results.iter().filter(|r| r.status == hallucinator_core::Status::Verified).count(),
        results.iter().filter(|r| r.status == hallucinator_core::Status::NotFound).count(),
    );

    Ok(())
}
```

## Config Construction

The `Config` struct controls all runtime behavior:

```rust
use hallucinator_core::{Config, RateLimiters, QueryCache, build_query_cache};
use std::sync::Arc;

let rate_limiters = Arc::new(RateLimiters::new(
    true,  // has_crossref_mailto (enables 3/s instead of 1/s)
    true,  // has_s2_api_key (enables higher S2 rate)
));

let cache = build_query_cache(
    Some(std::path::Path::new("/tmp/cache.db")),
    604800,  // positive TTL: 7 days in seconds
    86400,   // negative TTL: 24 hours in seconds
);

let config = Config {
    openalex_key: Some("your-key".to_string()),
    s2_api_key: Some("your-key".to_string()),
    num_workers: 4,
    db_timeout_secs: 10,
    db_timeout_short_secs: 5,
    max_rate_limit_retries: 3,
    rate_limiters,
    query_cache: Some(cache),
    ..Default::default()
};
```

## ProgressEvent Variants

The progress callback receives these events during validation:

| Event | When | Key fields |
|-------|------|------------|
| `Checking` | Starting a reference | `index`, `total`, `title` |
| `DatabaseQueryComplete` | A single DB query finished | `db_name`, `status`, `elapsed` |
| `RateLimitWait` | Waiting for rate limiter | `db_name`, `wait_time` |
| `RateLimitRetry` | Retrying after 429 | `db_name`, `attempt` |
| `Warning` | DB timeouts for a reference | `title`, `failed_dbs`, `message` |
| `Result` | Reference validation complete | `index`, `total`, `result: Box<ValidationResult>` |
| `RetryPass` | Starting retry pass | — |
| `Retrying` | Retrying a reference | `index`, `title` |

## PDF Extraction

Extract and parse references without validating:

```rust
use hallucinator_pdf::{PdfBackend, PdfExtractor};
use hallucinator_pdf_mupdf::MupdfBackend;

let text = MupdfBackend.extract_text(std::path::Path::new("paper.pdf"))?;

// Use PdfExtractor for the full pipeline
let extractor = PdfExtractor::new(MupdfBackend);
let result = extractor.extract(std::path::Path::new("paper.pdf"))?;

for reference in &result.references {
    println!("Title: {:?}", reference.title);
    println!("Authors: {:?}", reference.authors);
    println!("DOI: {:?}", reference.doi);
}
```

## Adding a Custom PDF Backend

Implement `PdfBackend` to use a different PDF library:

```rust
use hallucinator_pdf::PdfBackend;

struct MyPdfBackend;

impl PdfBackend for MyPdfBackend {
    fn extract_text(&self, path: &std::path::Path) -> Result<String, String> {
        // Your PDF text extraction logic here
        let text = my_pdf_library::extract(path)
            .map_err(|e| format!("extraction failed: {}", e))?;
        Ok(text)
    }
}
```

## Adding a Custom Database Backend

See [Database Backends](database-backends.md) for the `DatabaseBackend` trait reference and a step-by-step guide.
