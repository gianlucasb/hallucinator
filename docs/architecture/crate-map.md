# Crate Map

Quick reference for each crate in the workspace: its responsibility, key types, and dependencies.

## hallucinator-core

**Responsibility:** The validation engine — database backends, caching, rate limiting, author matching, title normalization, and the main `check_references()` entry point.

**Key types:**
- `Reference` — Parsed reference (title, authors, DOI, arXiv ID, raw citation)
- `ExtractionResult` — References extracted from a document plus skip statistics
- `ValidationResult` — Complete result for one reference (status, source, per-DB results, retraction info)
- `Config` — Runtime configuration (API keys, timeouts, offline DBs, disabled DBs, rate limiters, cache)
- `ProgressEvent` — Events emitted during validation (`Checking`, `Result`, `Warning`, `RetryPass`, `DatabaseQueryComplete`, `RateLimitWait`)
- `Status` — `Verified`, `NotFound`, `AuthorMismatch`
- `DbStatus` — `Match`, `NoMatch`, `AuthorMismatch`, `Timeout`, `RateLimited`, `Error`, `Skipped`
- `CheckStats` — Summary counts (total, verified, not_found, author_mismatch, retracted, skipped)
- `DatabaseBackend` trait — Interface for all database backends
- `ValidationPool` — Per-DB drainer pool for concurrent validation
- `QueryCache` — Two-tier (DashMap + optional SQLite) cache
- `RateLimiters` — Per-DB adaptive rate limiters

**Key files:**
- `src/lib.rs` — Public API and type exports
- `src/pool.rs` — ValidationPool, coordinators, drainers, RefCollector
- `src/orchestrator.rs` — Database query orchestration (local then remote)
- `src/checker.rs` — `check_references()` entry point
- `src/db/mod.rs` — `DatabaseBackend` trait and `DbQueryResult`
- `src/db/*.rs` — Individual database implementations
- `src/cache.rs` — Two-tier caching system
- `src/rate_limit.rs` — Adaptive per-DB rate limiting
- `src/matching.rs` — Title normalization and fuzzy matching
- `src/authors.rs` — Author name validation
- `src/retraction.rs` — Retraction checking
- `src/config_file.rs` — TOML configuration file loading and merging

**Dependencies:** reqwest, tokio, async-channel, governor, dashmap, arc-swap, rapidfuzz, serde, rusqlite

---

## hallucinator-pdf

**Responsibility:** PDF text extraction pipeline — backend-agnostic text extraction, reference section detection, segmentation into individual references, title/author/identifier extraction.

**Key types:**
- `PdfBackend` trait — Abstraction for PDF text extraction
- `PdfExtractor` — Configurable extraction pipeline
- `PdfParsingConfig` — Custom regex patterns, thresholds, segment strategies

**Key files:**
- `src/lib.rs` — `PdfBackend` trait definition
- `src/extractor.rs` — `PdfExtractor` pipeline orchestration
- `src/section.rs` — `find_references_section()`, `segment_references()`
- `src/title.rs` — `extract_title_from_reference()`, `clean_title()`
- `src/authors.rs` — `extract_authors_from_reference()`
- `src/identifiers.rs` — `extract_doi()`, `extract_arxiv_id()`
- `src/ligatures.rs` — Ligature expansion
- `src/hyphenation.rs` — Hyphenation fixing

**Dependencies:** regex

---

## hallucinator-pdf-mupdf

**Responsibility:** MuPDF implementation of the `PdfBackend` trait. **AGPL-licensed** — isolated to keep other crates permissive.

**Key types:**
- `MupdfBackend` — Implements `PdfBackend` using the `mupdf` crate

**Dependencies:** mupdf, hallucinator-pdf

---

## hallucinator-bbl

**Responsibility:** Parse BibTeX `.bbl` and `.bib` files into `Reference` structs.

**Key functions:**
- `extract_references_from_bbl(path)` — Parse `.bbl` files
- `extract_references_from_bib(path)` — Parse `.bib` files

**Dependencies:** hallucinator-core (for `Reference`, `ExtractionResult`)

---

## hallucinator-ingest

**Responsibility:** Unified file dispatch — detects file type (PDF, BBL, BIB, archive) and routes to the appropriate extractor. Handles archive streaming with size limits.

**Key functions:**
- `extract_references(path)` — Dispatch to PDF or BBL/BIB extractor
- `is_archive_path(path)` — Check if path is a `.tar.gz` or `.zip`

**Key types:**
- `ArchiveItem` — Streaming archive extraction results (`Pdf`, `Warning`, `Done`)

**Dependencies:** hallucinator-pdf, hallucinator-pdf-mupdf, hallucinator-bbl, hallucinator-core, zip, tar, flate2, tempfile

---

## hallucinator-dblp

**Responsibility:** Build and query an offline DBLP database. Downloads DBLP's XML dump (~4.6GB compressed), parses it, and creates a SQLite database with FTS5 full-text search.

**Key types:**
- `DblpDatabase` — SQLite database handle with FTS5 search
- `BuildProgress` — Progress events during database building (`Downloading`, `Parsing`, `RebuildingIndex`, `Compacting`, `Complete`)

**Key functions:**
- `DblpDatabase::open(path)` — Open existing database
- `DblpDatabase::search(title)` — FTS5 title search
- `build_database(path, callback)` — Download, parse, and build database

**Dependencies:** rusqlite, reqwest, quick-xml, flate2

---

## hallucinator-acl

**Responsibility:** Build and query an offline ACL Anthology database. Downloads ACL XML data from GitHub, parses it, and creates a SQLite FTS5 database.

**Key types:**
- `AclDatabase` — SQLite database handle
- `BuildProgress` — Progress events during building

**Key functions:**
- `AclDatabase::open(path)` — Open existing database
- `AclDatabase::search(title)` — FTS5 title search
- `build_database(path, callback)` — Download and build database

**Dependencies:** rusqlite, reqwest, quick-xml, tar, flate2

---

## hallucinator-reporting

**Responsibility:** Export validation results to various formats.

**Key types:**
- `ExportFormat` — `Json`, `Csv`, `Markdown`, `Text`, `Html`
- `ReportPaper` — Per-paper metadata for export (filename, stats, results, verdict)
- `ReportRef` — Per-reference state for export (index, title, skip info, false-positive reason)
- `FpReason` — False-positive override reasons (`BrokenParse`, `ExistsElsewhere`, `AllTimedOut`, `KnownGood`, `NonAcademic`)
- `PaperVerdict` — Overall paper judgment (`Safe`, `Questionable`)

**Key functions:**
- `export_results(papers, ref_states, format, path)` — Write results to file in specified format

**Dependencies:** hallucinator-core

---

## hallucinator-cli

**Responsibility:** Command-line binary for single-file reference checking.

**Commands:**
- `check <file>` — Check PDF/BBL/BIB file (or archive)
- `update-dblp <path>` — Build/update offline DBLP database
- `update-acl <path>` — Build/update offline ACL database

**Dependencies:** hallucinator-core, hallucinator-ingest, hallucinator-dblp, hallucinator-acl, clap, owo-colors, indicatif, tokio

---

## hallucinator-tui

**Responsibility:** Terminal UI for batch processing. Built with Ratatui. Supports multiple PDFs, result navigation, sorting/filtering, false-positive overrides, result persistence (JSON), and configurable themes.

**Screens:** Queue → Paper → Reference Detail → Config

**Dependencies:** hallucinator-core, hallucinator-ingest, hallucinator-reporting, ratatui, crossterm, tokio

See [TUI Design Document](../tui-design.md) for design details.

---

## hallucinator-python (excluded)

**Responsibility:** PyO3 Python bindings providing `PdfExtractor`, `Validator`, `ValidatorConfig`, and result types. Pre-compiled wheels available for major platforms.

**Excluded from workspace** to avoid pyo3/Python version conflicts in CI.

See the [Python Bindings](../api-reference/python-bindings.md) page for an overview, or the full [PYTHON_BINDINGS.md](https://github.com/gianlucasb/hallucinator/blob/main/hallucinator-rs/PYTHON_BINDINGS.md) on GitHub.

---

## hallucinator-web (excluded)

**Responsibility:** Axum web server with HTML UI and SSE streaming.

**Endpoints:**
- `GET /` — HTML interface
- `POST /analyze/stream` — SSE-streaming reference validation (multipart PDF upload)
- `POST /retry` — Recheck specific references

**Excluded from workspace** to avoid compiling axum/tower during dist builds (not distributed as a binary).
