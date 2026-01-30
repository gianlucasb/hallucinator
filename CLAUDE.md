# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Hallucinated Reference Detector** - Detects potentially fabricated references in academic PDF papers by validating against multiple academic databases (CrossRef, arXiv, DBLP, Semantic Scholar, ACL Anthology, NeurIPS, and optionally OpenAlex).

**Read [MANIFESTO.md](MANIFESTO.md)** for the mission statement and context on why this tool exists, including documentation of the November 2025 OpenReview incident and a note on human-AI collaboration written by Claude during development.

## Commands

### Installation
```bash
python -m venv venv
venv\Scripts\activate  # Windows
pip install -r requirements.txt
```

### CLI Usage
```bash
python check_hallucinated_references.py <path_to_pdf>
python check_hallucinated_references.py --no-color --openalex-key=KEY --s2-api-key=KEY <pdf>
python check_hallucinated_references.py --output log.txt <pdf>
```

### Web Server
```bash
python app.py  # Starts on http://localhost:5001
```

### Docker
```bash
docker build -t hallucinator .
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
- **8 databases queried concurrently** per reference (all at once)
- **Early exit** - Returns immediately when verified match found
- **Request timeouts** - 10s default (`DB_TIMEOUT`), 5s short timeout (`DB_TIMEOUT_SHORT`)
- **Configurable timeouts** - Set `DB_TIMEOUT` and `DB_TIMEOUT_SHORT` env vars for testing

### Database Sources
- OpenAlex (optional, needs API key)
- CrossRef
- arXiv
- DBLP
- ~~OpenReview~~ (disabled - API unreachable after Nov 2025 incident; see [MANIFESTO.md](MANIFESTO.md) for details)
- Semantic Scholar
- ACL Anthology
- NeurIPS

### Key Files
- `check_hallucinated_references.py` - Core validation logic, CLI interface
- `app.py` - Flask web application (shares validation logic with CLI)
- `templates/index.html` - Web UI with embedded JS/CSS

### Validation Result Types
- **Verified** - Found in database with matching authors
- **Author Mismatch** - Title found but different authors
- **Not Found** - Potential hallucination (not in any database)

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
