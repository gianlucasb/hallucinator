# System Architecture Overview

Hallucinator is a multi-crate Rust workspace that validates academic references extracted from PDFs against 10+ academic databases. This document covers the high-level architecture, key design decisions, and how the pieces fit together.

## Workspace Structure

The workspace lives in `hallucinator-rs/` and contains 10 member crates plus 2 excluded crates:

```
hallucinator-rs/
├── crates/
│   ├── hallucinator-core       # Validation engine, DB backends, caching, rate limiting
│   ├── hallucinator-pdf        # PDF text extraction (backend-agnostic)
│   ├── hallucinator-pdf-mupdf  # MuPDF backend (AGPL-licensed)
│   ├── hallucinator-bbl        # BibTeX .bbl/.bib file parsing
│   ├── hallucinator-ingest     # Unified file dispatch + archive handling
│   ├── hallucinator-dblp       # DBLP offline database (RDF → SQLite FTS5)
│   ├── hallucinator-acl        # ACL Anthology offline database
│   ├── hallucinator-reporting  # Export formats (JSON, CSV, Markdown, HTML, Text)
│   ├── hallucinator-cli        # CLI binary
│   ├── hallucinator-tui        # TUI binary (Ratatui)
│   ├── hallucinator-python     # PyO3 Python bindings (excluded from workspace)
│   └── hallucinator-web        # Axum web server (excluded from workspace)
```

Only `hallucinator-cli` and `hallucinator-tui` are distributed as release binaries. The Python and web crates are excluded from the workspace to avoid CI complications (pyo3 version conflicts, unnecessary axum compilation during `dist` builds).

## Crate Dependency Graph

```
                    ┌──────────────┐
                    │  CLI / TUI   │
                    └──────┬───────┘
                           │
                    ┌──────▼───────┐
                    │    ingest    │──────────────┐
                    └──────┬───────┘              │
                           │                      │
              ┌────────────▼────────────┐   ┌─────▼──────┐
              │          core           │   │    pdf      │
              │  (validation, DB, cache)│   │  (extract)  │
              └────┬───────┬───────┬────┘   └─────┬───────┘
                   │       │       │              │
              ┌────▼──┐ ┌──▼───┐ ┌▼────────┐ ┌───▼────────┐
              │ dblp  │ │ acl  │ │reporting│ │ pdf-mupdf  │
              └───────┘ └──────┘ └─────────┘ │  (AGPL)    │
                                             └────────────┘
```

## AGPL Isolation

The MuPDF library is licensed under AGPL. To keep the rest of the codebase under a permissive license:

- `hallucinator-pdf` defines the `PdfBackend` trait (permissive license)
- `hallucinator-pdf-mupdf` implements `PdfBackend` using MuPDF (AGPL)
- Only the final binaries (CLI/TUI) link to the AGPL crate
- Library consumers (`hallucinator-core`, `hallucinator-python`) never depend on `hallucinator-pdf-mupdf` directly

This means the core validation logic remains AGPL-free. Alternative PDF backends (e.g., `pdf-extract`, `pdfium`) can be implemented by providing the `PdfBackend` trait.

## Key Traits

### `DatabaseBackend`

Defined in `hallucinator-core/src/db/mod.rs`. Every academic database implements this trait:

```rust
pub trait DatabaseBackend: Send + Sync {
    fn name(&self) -> &str;
    fn is_local(&self) -> bool { false }
    fn requires_doi(&self) -> bool { false }
    fn query(&self, title: &str, client: &reqwest::Client,
             timeout: Duration) -> Pin<Box<dyn Future<Output = Result<DbQueryResult, DbQueryError>> + Send>>;
    fn query_doi(&self, doi: &str, title: &str, authors: &[String],
                 client: &reqwest::Client, timeout: Duration) -> DoiQueryResult;
}
```

Local backends (`is_local() = true`) are queried inline by the coordinator before fanning out to remote drainers. See [Concurrency Model](concurrency.md) for details.

### `PdfBackend`

Defined in `hallucinator-pdf/src/lib.rs`. Abstracts PDF text extraction:

```rust
pub trait PdfBackend {
    fn extract_text(&self, path: &Path) -> Result<String, String>;
}
```

## Configuration Layering

Configuration is resolved with the following precedence (highest wins):

1. **CLI flags** — `--num-workers 8`, `--dblp-offline /path`, etc.
2. **Environment variables** — `OPENALEX_KEY`, `DB_TIMEOUT`, `SEARXNG_URL`, etc.
3. **CWD config** — `.hallucinator.toml` in the current directory
4. **Platform config** — `~/.config/hallucinator/config.toml` (Linux/macOS) or `%APPDATA%\hallucinator\config.toml` (Windows)
5. **Defaults** — Hardcoded defaults (4 workers, 10s timeout, etc.)

CWD config overlays platform config field-by-field, so you can keep API keys in the global config and override concurrency settings per-project.

See [Configuration](../user-guide/configuration.md) for the full reference.

## Caching

A two-tier cache prevents redundant API calls:

- **L1 (in-memory):** `DashMap` — lock-free concurrent reads, sub-microsecond lookups
- **L2 (optional SQLite):** WAL-mode database for persistence across runs

Cache keys use aggressive title normalization (Unicode NFKD, Greek letter transliteration, math symbol replacement, ASCII-only lowercasing) to maximize hit rates across PDF extraction artifacts.

TTLs: 7 days for positive (found) entries, 24 hours for negative (not-found) entries. Both are configurable.

See [Concurrency Model](concurrency.md) for how the cache interacts with the drainer pool.

## Rate Limiting

Each remote database has its own `AdaptiveDbLimiter` using the `governor` crate for token-bucket rate limiting:

- **Per-DB drainer task** — Each drainer is the sole consumer of its DB's rate limiter, eliminating governor contention
- **Adaptive backoff** — On HTTP 429: doubles the slowdown factor (1x → 2x → 4x → ... → 16x max), atomically swaps the governor via `ArcSwap`
- **Recovery** — After 30 seconds without a 429, the original rate is restored
- **Default rates** — CrossRef 1/s (3/s with `crossref_mailto`), arXiv 3/s, DBLP 1/s, Semantic Scholar varies by API key presence

## Title Matching

References are matched using fuzzy string comparison with a 95% similarity threshold (via `rapidfuzz`). Before comparison, titles are normalized:

1. HTML entity unescaping
2. Separated diacritic fixing (e.g., `B ¨UNZ` → `BÜNZ`)
3. Greek letter transliteration (α → alpha, β → beta)
4. Math symbol replacement (√ → sqrt, ∞ → infinity)
5. Unicode NFKD decomposition
6. Strip to `[a-z0-9]` only

## Author Validation

Two modes based on the quality of extracted author names:

- **Full mode** — Normalizes each author to `FirstInitial Surname`, checks set intersection between PDF authors and DB authors
- **Last-name-only mode** — Used when >50% of reference authors lack first names/initials; compares surnames only with partial suffix matching for multi-word surnames

## Entry Points

All interfaces consume the same `hallucinator-core` library:

| Interface | Crate | Description |
|-----------|-------|-------------|
| CLI | `hallucinator-cli` | Single-file checking with colored terminal output |
| TUI | `hallucinator-tui` | Batch processing with Ratatui, result navigation, false-positive overrides |
| Web | `hallucinator-web` | Axum HTTP server with SSE streaming (excluded from workspace) |
| Python | `hallucinator-python` | PyO3 bindings with pre-compiled wheels (excluded from workspace) |
| Library | `hallucinator-core` | Direct Rust API via `check_references()` |

The core `check_references()` function signature:

```rust
pub async fn check_references(
    refs: Vec<Reference>,
    config: Config,
    progress: impl Fn(ProgressEvent) + Send + Sync + 'static,
    cancel: CancellationToken,
) -> Vec<ValidationResult>
```
