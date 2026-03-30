# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Development Policy

**All new development should be in the Rust codebase (`hallucinator-rs/`).** The Python code in `legacy/` is frozen and should not be modified except for critical bug fixes. When porting features or fixes, implement them in the Rust crates.

## Project Overview

**Hallucinated Reference Detector** - Detects potentially fabricated references in academic PDF papers by validating against multiple databases (CrossRef, arXiv, DBLP, Semantic Scholar, ACL Anthology, Europe PMC, PubMed, DOI Resolver, OpenAlex, Open Library, GovInfo, plus optional SearXNG web search and URL liveness fallbacks). Also checks for retracted papers via CrossRef.

**Read [MANIFESTO.md](MANIFESTO.md)** for the mission statement and context on why this tool exists, including documentation of the November 2025 OpenReview incident and a note on human-AI collaboration written by Claude during development.

## Commands

### Legacy Python (in `legacy/` subdirectory)

#### Installation
```bash
python -m venv venv
venv\Scripts\activate  # Windows
pip install -r legacy/requirements.txt
```

#### CLI Usage
```bash
python legacy/check_hallucinated_references.py <path_to_pdf>
python legacy/check_hallucinated_references.py --no-color --openalex-key=KEY --s2-api-key=KEY <pdf>
python legacy/check_hallucinated_references.py --output log.txt <pdf>
python legacy/check_hallucinated_references.py --dblp-offline=dblp.db <pdf>  # Use offline DBLP
python legacy/check_hallucinated_references.py --update-dblp=dblp.db         # Download & build offline DB
```

#### Web Server
```bash
python legacy/app.py  # Starts on http://localhost:5001
DBLP_OFFLINE_PATH=dblp.db python legacy/app.py  # With offline DBLP
```

#### Docker
```bash
docker build -t hallucinator legacy/
docker run -p 5001:5001 hallucinator
```

## Architecture

### Processing Pipeline
1. **PDF Extraction** - PyMuPDF extracts text with ligature expansion
2. **Reference Detection** - Locates References/Bibliography section via regex
3. **Reference Segmentation** - Splits by IEEE `[1]` or numbered `1.` patterns
4. **Title/Author Extraction** - Parses from IEEE, ACM, USENIX, AAAI formats
5. **Validation** - Concurrent database queries with parallel reference checking
6. **Retry Pass** - Failed/timed out queries are retried at the end
7. **Reporting** - CLI colored output or web JSON response with SSE streaming

### Concurrency Model
- **4 references checked in parallel** (configurable via `max_concurrent_refs`)
- **All enabled databases queried concurrently** per reference
- **Early exit** - Returns immediately when verified match found
- **Request timeouts** - 10s default (`DB_TIMEOUT`), 5s short timeout (`DB_TIMEOUT_SHORT`)
- **Configurable timeouts** - Set `DB_TIMEOUT` and `DB_TIMEOUT_SHORT` env vars for testing

### Database Sources
- CrossRef (DOIs, journal articles, conference papers)
- arXiv (preprints)
- DBLP (CS bibliography; online API or offline SQLite + FTS5)
- Semantic Scholar (aggregates multiple sources; optional API key)
- ACL Anthology (computational linguistics; online or offline SQLite + FTS5)
- Europe PMC (life science/biomedical literature)
- PubMed (biomedical literature via NCBI)
- DOI Resolver (validates references by resolving DOIs via doi.org)
- OpenAlex (250M+ works; online with API key, or offline SQLite)
- Open Library (books, technical reports, non-academic publications)
- GovInfo (US federal laws, regulations, court opinions; optional, needs free API key)
- ~~OpenReview~~ (disabled - API unreachable after Nov 2025 incident; see [MANIFESTO.md](MANIFESTO.md) for details)
- URL Checker (liveness check for non-academic URLs; fallback only)
- Web Search / SearXNG (self-hosted metasearch fallback; optional, no author verification)

### Offline DBLP Database
- Downloads from https://dblp.org/rdf/dblp.nt.gz (~4.6GB compressed)
- Parses RDF N-Triples format to extract publications, authors, URLs
- Builds SQLite database with FTS5 full-text search index
- Staleness warning after 30 days (configurable via `STALENESS_THRESHOLD_DAYS`)
- CLI: `--dblp-offline=PATH` to use, `--update-dblp=PATH` to build/refresh
- Web: `DBLP_OFFLINE_PATH` environment variable

### Key Files

**Rust codebase (primary - all new development here):**
- `hallucinator-rs/` - Primary Rust implementation (see workspace structure in README)
- `hallucinator-rs/crates/hallucinator-parsing/` - Reference parsing pipeline (section detection, segmentation, title/author extraction)

**Legacy Python (frozen - reference only):**
- `legacy/check_hallucinated_references.py` - Core validation logic, CLI interface
- `legacy/app.py` - Flask web application
- `legacy/templates/index.html` - Web UI with embedded JS/CSS
- `legacy/dblp_offline.py` - Offline DBLP database builder and query module

### Validation Result Types
- **Verified** - Found in database with matching authors
- **Author Mismatch** - Title found but different authors
- **Not Found** - Potential hallucination (not in any database)
- **Retracted** - Paper found but has been retracted (checked via CrossRef)

### Skipped References
- Non-academic URLs (GitHub, documentation sites)
- Titles with <5 words (prone to false matches)

## Code Patterns

- **Fuzzy matching**: Uses rapidfuzz with 95% similarity threshold for title comparison
- **Ligature handling**: `expand_ligatures()` converts PDF typographic characters (ﬁ→fi)
- **Hyphenation fixing**: `fix_hyphenation()` distinguishes syllable breaks from compound words
- **Em-dash handling**: Recognizes em-dashes meaning "same authors as previous reference"
- **Dual interface**: CLI and web share the same validation functions
- **Concurrent queries**: `ThreadPoolExecutor` for parallel DB queries and reference checking
- **SSE streaming**: Real-time progress via Server-Sent Events (`/analyze/stream` endpoint)
- **Progress callbacks**: `on_progress(event_type, data)` pattern for both CLI and web
  - Events: `checking`, `result`, `warning`, `retry_pass`
- **Retry mechanism**: Tracks failed DBs and retries "not found" references at the end
- **Timeout tracking**: Per-reference tracking of which DBs timed out, displayed in final results
- **Retraction checking**: `check_retraction(doi)` and `check_retraction_by_title(title)` query CrossRef for retraction notices

## Testing with Ground Truth

When testing extraction against BibTeX/BBL ground truth files in `~/Software/arxiv-crawler/ground-truth`:
- **Not all references in a .bib file are cited in the PDF** - BibTeX files often contain more entries than what's actually referenced in the paper
- Compare extracted titles against ground truth using fuzzy matching, not exact counts
- A paper with 40 extracted references but 100 entries in its .bib file is normal - focus on whether extracted titles match entries in ground truth, not on count parity
