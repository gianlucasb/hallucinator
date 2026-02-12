# Python Bindings

Python bindings for the Rust hallucinator engine, powered by [PyO3](https://pyo3.rs/) and [Maturin](https://www.maturin.rs/). Extract references from academic PDFs and validate them against 10 academic databases — all from Python, with Rust-native performance.

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

# Extract references from a PDF
ext = PdfExtractor()
result = ext.extract("paper.pdf")
print(f"Found {len(result)} references")

# Validate them against academic databases
config = ValidatorConfig()
validator = Validator(config)
results = validator.check(result.references)

for r in results:
    print(f"[{r.status}] {r.title}")
```

---

## Installation

Requires Python 3.9+ and a Rust toolchain.

```bash
cd hallucinator-rs

# Using uv (recommended)
uv venv && source .venv/bin/activate   # or .venv\Scripts\activate on Windows
uv pip install maturin
maturin develop

# Or with pip
pip install maturin
maturin develop

# Release build (slower to compile, faster at runtime)
maturin develop --release
```

After `maturin develop`, the `hallucinator` package is importable:

```python
>>> import hallucinator
>>> hallucinator.PdfExtractor()
PdfExtractor(...)
```

---

## PDF Extraction

### Quick start

```python
from hallucinator import PdfExtractor

ext = PdfExtractor()
result = ext.extract("paper.pdf")

for ref in result.references:
    print(ref.title)
    print(f"  Authors: {', '.join(ref.authors)}")
    if ref.doi:
        print(f"  DOI: {ref.doi}")
```

### PdfExtractor

The main entry point for extraction. Wraps the Rust engine and adds support for custom Python segmentation strategies.

```python
ext = PdfExtractor()

# Full pipeline: PDF file → ExtractionResult
result = ext.extract("paper.pdf")

# Full pipeline on already-extracted text
result = ext.extract_from_text(text)

# Individual pipeline stages
text = ext.extract_text("paper.pdf")       # Step 1: PDF → raw text
section = ext.find_section(text)            # Step 2: locate references section
segments = ext.segment(section)             # Step 3: split into individual refs
ref = ext.parse_reference(segments[0])      # Step 4: parse a single reference
```

### Configuration

Override regex patterns and thresholds to handle non-standard paper formats.

| Property | Default | Description |
|----------|---------|-------------|
| `section_header_regex` | Matches "References", "Bibliography", etc. | Regex to find the start of the references section |
| `section_end_regex` | Matches "Appendix", "Acknowledgments", etc. | Regex to find the end of the references section |
| `fallback_fraction` | `0.25` | If no header found, use the last N% of the document |
| `ieee_segment_regex` | Matches `[1]`, `[2]`, etc. | Regex for IEEE-style reference numbering |
| `numbered_segment_regex` | Matches `1.`, `2.`, etc. | Regex for numbered-list references |
| `fallback_segment_regex` | Double newline | Fallback segmentation when no numbering detected |
| `min_title_words` | `4` | Minimum words in a title (shorter → skipped) |
| `max_authors` | `20` | Cap on extracted author count per reference |

```python
ext = PdfExtractor()

# Handle Spanish papers
ext.section_header_regex = r"(?i)\n\s*(?:Bibliografía|Referencias)\s*\n"

# Accept shorter titles
ext.min_title_words = 3

# Custom venue cutoff (don't include journal name in title)
ext.add_venue_cutoff_pattern(r"(?i)\.\s*Nature\b.*$")

# Preserve compound words across line breaks
ext.add_compound_suffix("powered")   # "AI- powered" → "AI-powered"
```

### Custom segmentation strategies

For reference formats that no regex can handle, register a Python callable:

```python
import re

def paren_segmenter(text: str) -> list[str] | None:
    """Split references numbered as (1), (2), (3)..."""
    parts = re.split(r'\n\s*\(\d+\)\s+', text)
    parts = [p.strip() for p in parts if p.strip()]
    return parts if len(parts) >= 3 else None

ext = PdfExtractor()
ext.add_segmentation_strategy(paren_segmenter)
result = ext.extract("unusual_paper.pdf")
```

Strategies are tried in registration order. Return `None` (or fewer than 3 items) to fall through to the next strategy, then to Rust built-ins.

```python
ext.add_segmentation_strategy(try_format_a)
ext.add_segmentation_strategy(try_format_b)
# Falls through: try_format_a → try_format_b → Rust built-ins

ext.clear_segmentation_strategies()  # Remove all custom strategies
```

### ExtractionResult

Returned by `extract()` and `extract_from_text()`.

```python
result = ext.extract("paper.pdf")

result.references   # list[Reference]
len(result)         # number of parsed references

# Skip statistics
result.skip_stats.total_raw     # total raw segments before filtering
result.skip_stats.url_only      # skipped: non-academic URLs only
result.skip_stats.short_title   # skipped: title too short
result.skip_stats.no_title      # references with no parseable title
result.skip_stats.no_authors    # references with no parseable authors
```

### Reference

A parsed reference with structured fields.

```python
ref.raw_citation  # str — the cleaned-up citation text
ref.title         # str | None — extracted title
ref.authors       # list[str] — author names
ref.doi           # str | None — DOI if found
ref.arxiv_id      # str | None — arXiv ID if found
```

---

## Reference Validation

After extracting references, validate them against academic databases. The validator queries up to 10 databases concurrently per reference, with early exit on first match.

### Quick start

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

ext = PdfExtractor()
result = ext.extract("paper.pdf")

config = ValidatorConfig()
validator = Validator(config)
results = validator.check(result.references)

for r in results:
    if r.status == "verified":
        print(f"  OK: {r.title} (via {r.source})")
    elif r.status == "not_found":
        print(f"  ?? {r.title}")
    elif r.status == "author_mismatch":
        print(f"  ~~ {r.title} (authors don't match)")
```

### ValidatorConfig

All configuration for database queries. Create one, tweak what you need, pass it to `Validator()`.

```python
config = ValidatorConfig()
```

#### API keys

```python
config.s2_api_key = "your-semantic-scholar-key"
config.openalex_key = "your-openalex-key"
config.crossref_mailto = "you@university.edu"  # CrossRef polite pool
```

#### Concurrency and timeouts

```python
config.max_concurrent_refs = 4       # references checked in parallel (default: 4)
config.db_timeout_secs = 10          # per-database timeout (default: 10)
config.db_timeout_short_secs = 5     # short timeout for fast DBs (default: 5)
```

#### Disable databases

```python
config.disabled_dbs = ["openalex", "pubmed"]
```

Database names: `crossref`, `arxiv`, `dblp`, `semantic_scholar`, `acl`, `neurips`, `ssrn`, `europe_pmc`, `pubmed`, `openalex`.

#### Offline databases

Point to local SQLite databases for DBLP and ACL Anthology (built with the CLI's `update-dblp` / `update-acl` commands). Dramatically faster than online queries.

```python
config.dblp_offline_path = "/path/to/dblp.db"
config.acl_offline_path = "/path/to/acl.db"
```

If the path doesn't exist or the file isn't a valid database, `Validator(config)` raises `RuntimeError`.

#### Author checking

```python
config.check_openalex_authors = True  # verify authors for OpenAlex matches (default: False)
```

### Validator

The main validation engine. Create it once, call `check()` as many times as needed.

```python
validator = Validator(config)
```

#### check()

Validates a list of `Reference` objects against all enabled databases. Blocks until complete but releases the Python GIL, so other threads can run.

```python
results = validator.check(references)
# or with a progress callback:
results = validator.check(references, progress=on_progress)
```

Returns `list[ValidationResult]`.

#### Progress callbacks

Pass a callable to `check()` to receive real-time progress events:

```python
def on_progress(event):
    if event.event_type == "checking":
        print(f"[{event.index + 1}/{event.total}] Checking: {event.title}")
    elif event.event_type == "result":
        r = event.result
        print(f"[{event.index + 1}/{event.total}] {r.status}: {r.title}")
    elif event.event_type == "warning":
        print(f"Warning: {event.title} — {event.message}")
    elif event.event_type == "retry_pass":
        print(f"Retrying {event.count} unresolved references...")
    elif event.event_type == "db_query_complete":
        print(f"  {event.db_name}: {event.db_status} ({event.elapsed_ms:.0f}ms)")

results = validator.check(refs, progress=on_progress)
```

#### ProgressEvent properties

All properties return `None` when not applicable to the event type.

| Property | Type | Event types |
|----------|------|-------------|
| `event_type` | `str` | all |
| `index` | `int` | checking, result, warning |
| `total` | `int` | checking, result, warning |
| `title` | `str` | checking, warning |
| `result` | `ValidationResult` | result |
| `failed_dbs` | `list[str]` | warning |
| `message` | `str` | warning |
| `count` | `int` | retry_pass |
| `paper_index` | `int` | db_query_complete |
| `ref_index` | `int` | db_query_complete |
| `db_name` | `str` | db_query_complete |
| `db_status` | `str` | db_query_complete |
| `elapsed_ms` | `float` | db_query_complete |

#### Cancellation

Cancel a running check from another thread:

```python
import threading

validator = Validator(config)

def run_check():
    results = validator.check(refs)

t = threading.Thread(target=run_check)
t.start()

# Cancel after 30 seconds
import time
time.sleep(30)
validator.cancel()
t.join()
```

#### Stats

Compute summary statistics from results:

```python
stats = Validator.stats(results)
print(f"Total:           {stats.total}")
print(f"Verified:        {stats.verified}")
print(f"Not found:       {stats.not_found}")
print(f"Author mismatch: {stats.author_mismatch}")
print(f"Retracted:       {stats.retracted}")
print(f"Skipped:         {stats.skipped}")
```

### ValidationResult

The result of checking a single reference.

```python
r = results[0]

r.title            # str — reference title
r.raw_citation     # str — original citation text
r.status           # "verified" | "not_found" | "author_mismatch"
r.source           # str | None — database that verified it (e.g. "crossref")
r.ref_authors      # list[str] — authors from the parsed reference
r.found_authors    # list[str] — authors from the matching DB record
r.paper_url        # str | None — URL in the matching database
r.failed_dbs       # list[str] — databases that timed out or errored
```

#### Per-database results

Every database query is recorded, even if it didn't match:

```python
for db in r.db_results:
    print(f"  {db.db_name}: {db.status}", end="")
    if db.elapsed_ms is not None:
        print(f" ({db.elapsed_ms:.0f}ms)", end="")
    if db.paper_url:
        print(f" → {db.paper_url}", end="")
    print()
```

`DbResult.status` values: `"match"`, `"no_match"`, `"author_mismatch"`, `"timeout"`, `"error"`, `"skipped"`.

#### DOI and arXiv info

```python
if r.doi_info:
    print(f"DOI: {r.doi_info.doi} (valid={r.doi_info.valid})")
    if r.doi_info.title:
        print(f"  Resolved title: {r.doi_info.title}")

if r.arxiv_info:
    print(f"arXiv: {r.arxiv_info.arxiv_id} (valid={r.arxiv_info.valid})")
```

#### Retraction info

```python
if r.retraction_info and r.retraction_info.is_retracted:
    print(f"RETRACTED!")
    if r.retraction_info.retraction_doi:
        print(f"  Retraction DOI: {r.retraction_info.retraction_doi}")
    if r.retraction_info.retraction_source:
        print(f"  Source: {r.retraction_info.retraction_source}")
```

---

## Complete example

Extract, validate, and report — the full pipeline:

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

# Extract
ext = PdfExtractor()
result = ext.extract("paper.pdf")
refs = result.references
print(f"Extracted {len(refs)} references")

# Configure
config = ValidatorConfig()
config.s2_api_key = "your-key"              # optional but improves results
config.dblp_offline_path = "dblp.db"        # optional, faster than online
config.disabled_dbs = ["openalex"]           # skip DBs you don't need

# Validate with progress
def on_progress(event):
    if event.event_type == "checking":
        print(f"  [{event.index + 1}/{event.total}] {event.title}")
    elif event.event_type == "result":
        r = event.result
        icon = {"verified": "+", "not_found": "?", "author_mismatch": "~"}[r.status]
        src = f" ({r.source})" if r.source else ""
        print(f"  [{icon}] {r.title}{src}")

validator = Validator(config)
results = validator.check(refs, progress=on_progress)

# Summary
stats = Validator.stats(results)
print(f"\nVerified: {stats.verified}/{stats.total}")
if stats.not_found:
    print(f"Potentially hallucinated: {stats.not_found}")
if stats.retracted:
    print(f"Retracted: {stats.retracted}")

# Flag suspicious references
for r in results:
    if r.status == "not_found":
        print(f"\n  NOT FOUND: {r.title}")
        print(f"  Citation: {r.raw_citation[:120]}...")
    if r.retraction_info and r.retraction_info.is_retracted:
        print(f"\n  RETRACTED: {r.title}")
```

---

## API Reference

### Extraction types

| Class | Description |
|-------|-------------|
| `PdfExtractor` | Configurable PDF extraction pipeline with custom strategy support |
| `ExtractionResult` | Container for parsed references and skip statistics |
| `Reference` | A parsed reference (title, authors, DOI, arXiv ID) |
| `SkipStats` | Counts of skipped references by reason |

### Validation types

| Class | Description |
|-------|-------------|
| `ValidatorConfig` | Configuration: API keys, timeouts, concurrency, disabled DBs, offline DB paths |
| `Validator` | Validation engine — call `.check(refs)` to validate |
| `ValidationResult` | Per-reference result: status, source, authors, per-DB details |
| `DbResult` | Single database query result: status, elapsed time, found authors |
| `DoiInfo` | DOI resolution result |
| `ArxivInfo` | arXiv resolution result |
| `RetractionInfo` | Retraction check result |
| `ProgressEvent` | Real-time progress callback event |
| `CheckStats` | Summary statistics (verified, not_found, author_mismatch, retracted) |

### Status values

**`ValidationResult.status`**: `"verified"` | `"not_found"` | `"author_mismatch"`

**`DbResult.status`**: `"match"` | `"no_match"` | `"author_mismatch"` | `"timeout"` | `"error"` | `"skipped"`

**`ProgressEvent.event_type`**: `"checking"` | `"result"` | `"warning"` | `"retry_pass"` | `"db_query_complete"`

---

## Examples

See [`python/examples/`](python/examples/) for runnable scripts:

| Example | Description |
|---------|-------------|
| [`basic_usage.py`](python/examples/basic_usage.py) | Extract references from a PDF |
| [`step_by_step.py`](python/examples/step_by_step.py) | Run each pipeline stage individually |
| [`custom_regexes.py`](python/examples/custom_regexes.py) | Override patterns for non-standard formats |
| [`validate_references.py`](python/examples/validate_references.py) | Full pipeline: extract + validate + report |

---

## Threading and performance

- **GIL release**: `Validator.check()` releases the Python GIL during the Rust async runtime call. Other Python threads can execute freely while validation runs.
- **Concurrency**: References are checked in parallel (default 4 at a time). All 10 databases are queried concurrently per reference. First match triggers early exit.
- **Progress callbacks**: The GIL is briefly re-acquired to call Python progress callbacks. Since events fire once per reference (not per HTTP request), overhead is negligible.
- **Tokio runtime**: Each `Validator` instance owns a tokio multi-threaded runtime. Creating many validators is wasteful — reuse a single instance for multiple `check()` calls.
