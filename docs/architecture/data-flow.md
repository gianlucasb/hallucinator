# Data Flow: PDF to Results

This document traces a reference's journey from PDF file to final validation result.

## Pipeline Overview

```
PDF file
  │
  ▼
┌─────────────────┐
│  File Dispatch   │  hallucinator-ingest
│  (PDF/BBL/BIB/  │  Detects file type, extracts from archives
│   archive)       │
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Text Extraction  │  hallucinator-pdf + hallucinator-pdf-mupdf
│  (PdfBackend)    │  MuPDF extracts raw text with ligature expansion
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Section Detection│  hallucinator-pdf/src/section.rs
│                  │  Locates "References" / "Bibliography" header
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  Segmentation   │  hallucinator-pdf/src/section.rs
│                  │  Splits section into individual references
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Title/Author    │  hallucinator-pdf/src/title.rs, authors.rs
│  Extraction     │  Parses title, authors, DOI, arXiv ID per ref
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Skip Filtering  │  hallucinator-pdf/src/extractor.rs
│                  │  Removes URL-only and short-title refs
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│   Validation    │  hallucinator-core (pool, orchestrator, db/*)
│   Pool          │  Concurrent DB queries with early exit
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│ Result Assembly │  hallucinator-core/src/pool.rs
│                  │  Merge local+remote results, retraction check
└────────┬────────┘
         │
         ▼
  Vec<ValidationResult>
```

## Stage 1: File Dispatch

**Crate:** `hallucinator-ingest`

The ingest crate handles file type detection and archive extraction:

- **PDF files** — Passed to the PDF extraction pipeline
- **BBL/BIB files** — Parsed by `hallucinator-bbl` (LaTeX bibliography entries)
- **Archives** (`.tar.gz`, `.zip`) — Extracted streaming via `ArchiveIterator`, each contained PDF processed independently
- **Size limits** — Configurable `max_archive_size_mb` to prevent resource exhaustion

## Stage 2: Text Extraction

**Crate:** `hallucinator-pdf` + `hallucinator-pdf-mupdf`

The `PdfBackend` trait abstracts text extraction. The MuPDF backend:

1. Opens the PDF and iterates page-by-page
2. Extracts raw text blocks
3. Expands ligatures (`ﬁ` → `fi`, `ﬂ` → `fl`, `ﬀ` → `ff`, etc.)
4. Fixes hyphenation — distinguishes syllable breaks from compound words using a suffix heuristic

## Stage 3: Section Detection

**File:** `hallucinator-pdf/src/section.rs`

Locates the references section by scanning for header patterns:

- Primary: `References`, `Bibliography`, `REFERENCES`, `BIBLIOGRAPHY`
- End markers: `Appendix`, `Acknowledgments`, `Supplementary`, `Author Contributions`

If no header is found, falls back to using the last 30% of the document text.

The section text between the header and the first end-marker (or EOF) is extracted.

## Stage 4: Reference Segmentation

**File:** `hallucinator-pdf/src/section.rs`

Individual references are split using priority-ordered strategies:

| Priority | Strategy | Pattern | Example |
|----------|----------|---------|---------|
| 1 | IEEE | `[1]`, `[2]`, ... | `[1] A. Author, "Title..."` |
| 2 | Numbered | `1.`, `2.`, ... | `1. Author, Title...` |
| 3 | ML author-based | Full names / initials | `Author, A. B. (2023). Title...` |
| 4 | Springer/Nature | Uppercase + `(YYYY)` | `AUTHOR, A. Title. J. (2023)` |
| 5 | Fallback | Double newline | Two blank lines between refs |

The system tries each strategy and picks the one that produces the most valid segments. For IEEE and numbered styles, a sequential check ensures numbering is contiguous.

## Stage 5: Title and Author Extraction

**Files:** `hallucinator-pdf/src/title.rs`, `authors.rs`, `identifiers.rs`

For each segmented reference:

1. **DOI extraction** — Regex: `/10\.\d+/[^\s]+/`
2. **arXiv ID extraction** — Regex for `arXiv:YYMM.NNNNN` patterns
3. **Title extraction** — Two strategies tried in order:
   - Quoted strings (e.g., `"Title Here"`)
   - Capitalized word sequences between author and venue patterns
4. **Author extraction** — Format-specific parsing for IEEE, ACM, USENIX, AAAI, NeurIPS styles
5. **Em-dash handling** — `———` means "same authors as previous reference"

## Stage 6: Skip Filtering

**File:** `hallucinator-pdf/src/extractor.rs`

References are skipped (not validated) if:

- **URL-only** — The reference is just a URL to a non-academic site (GitHub, docs, etc.)
- **Short title** — Title has fewer than 5 words (prone to false matches), unless a DOI or arXiv ID is present
- **No title** — No title could be extracted

Skip statistics are tracked and reported: `total_raw`, `url_only`, `short_title`, `no_title`.

## Stage 7: Validation

**Crate:** `hallucinator-core` (see [Concurrency Model](concurrency.md) for the full deep dive)

Each reference goes through:

1. **Coordinator picks up reference** from job queue
2. **Local DB query** (DBLP offline, ACL offline) — inline, < 1ms
3. **If verified locally** → skip all remote DBs, emit result immediately
4. **Cache pre-check** — synchronously check cache for all remote DBs
5. **If verified from cache** → skip all drainers
6. **Fan out cache-miss DBs** to per-DB drainer queues
7. **Drainer queries DB** — rate-limited HTTP call
8. **Author validation** — compare PDF authors against DB authors
9. **Early exit** — if any drainer verifies, others skip remaining work

### Database Query Flow (per reference, per DB)

```
Drainer receives job
  │
  ├─ Already verified? → skip
  ├─ Cancelled? → skip
  ├─ Requires DOI but ref has none? → skip
  │
  ▼
Rate limit acquire (governor token)
  │
  ▼
Cache check
  ├─ Cache hit → return cached result
  │
  ▼
HTTP request (with timeout)
  │
  ├─ Success + title found → author validation
  │     ├─ Authors match → set verified flag
  │     └─ Authors don't match → record mismatch
  ├─ Success + title not found → NoMatch
  ├─ 429 Rate Limited → adaptive backoff + retry
  └─ Error/Timeout → record failure
  │
  ▼
Cache insert (if successful)
  │
  ▼
Decrement remaining counter
  ├─ Not last → done
  └─ Last drainer → finalize result
```

## Stage 8: Result Assembly

**File:** `hallucinator-core/src/pool.rs` (`finalize_collector`)

When the last drainer for a reference completes:

1. **Merge** local and remote `DbResult` lists
2. **Determine status** — Verified (any DB matched) > AuthorMismatch (title found, wrong authors) > NotFound
3. **SearxNG fallback** — If still NotFound and SearxNG is configured, try web search as last resort
4. **DOI info** — Mark DOI as valid/invalid based on DOI backend result
5. **Retraction info** — Use inline retraction data extracted from CrossRef response (no extra API call)
6. **Emit events** — `ProgressEvent::Warning` (if DBs timed out) + `ProgressEvent::Result`
7. **Send result** via oneshot channel back to the caller

## Output Types

The final `Vec<ValidationResult>` can be:

- Displayed in the CLI with colored output
- Navigated in the TUI with sorting/filtering
- Streamed via SSE in the web interface
- Exported to JSON/CSV/Markdown/Text/HTML via `hallucinator-reporting`
- Returned as Python objects via `hallucinator-python`

See [Export Formats](../user-guide/export-formats.md) for output schema details.
