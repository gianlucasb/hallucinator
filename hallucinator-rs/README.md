# hallucinator-rs

Rust implementation of the Hallucinated Reference Detector. Includes a CLI and an interactive terminal UI (TUI) for batch-processing PDFs and archives.

Same validation engine as the Python version — queries 13 databases in parallel (academic APIs, DOI resolution, book catalogs, government documents, and web search fallback), fuzzy-matches titles, checks for retractions — but with a native async runtime and a full-screen TUI for working through large batches interactively.

---

## Python Bindings

Pre-compiled wheels are available — no Rust toolchain needed:

```bash
pip install hallucinator
```

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

ext = PdfExtractor()
result = ext.extract("paper.pdf")

config = ValidatorConfig()
validator = Validator(config)
results = validator.check(result.references)

for r in results:
    print(f"[{r.status}] {r.title}")
```

Available for Python 3.12 on Linux (x86_64), macOS (x86_64 + Apple Silicon), and Windows (x86_64). See **[PYTHON_BINDINGS.md](PYTHON_BINDINGS.md)** for full API docs.

---

## Building from Source

Requires a Rust toolchain. Install from [rustup.rs](https://rustup.rs/) or [rust-lang.org/tools/install](https://rust-lang.org/tools/install/).

```bash
cd hallucinator-rs
cargo build --release
```

Binaries are placed in `target/release/`:
- `hallucinator-cli` — command-line interface
- `hallucinator-tui` — terminal UI

---

## CLI

```bash
# Check a PDF
hallucinator-cli check paper.pdf

# With offline databases (recommended)
hallucinator-cli check --dblp-offline=dblp.db --acl-offline=acl.db --arxiv-offline=arxiv.db paper.pdf

# With API keys
hallucinator-cli check --openalex-key=KEY --s2-api-key=KEY paper.pdf

# Save output to file
hallucinator-cli check --output=report.log paper.pdf

# Disable specific databases
hallucinator-cli check --disable-dbs=OpenAlex,PubMed paper.pdf

# No color
hallucinator-cli check --no-color paper.pdf
```

### CLI Options

| Option | Description |
|--------|-------------|
| `--openalex-key=KEY` | OpenAlex API key |
| `--s2-api-key=KEY` | Semantic Scholar API key |
| `--govinfo-key=KEY` | GovInfo API key (for US federal laws) |
| `--dblp-offline=PATH` | Path to offline DBLP database |
| `--acl-offline=PATH` | Path to offline ACL Anthology database |
| `--arxiv-offline=PATH` | Path to offline arXiv database (Kaggle snapshot) |
| `--output=PATH` | Write output to file |
| `--no-color` | Disable colored output |
| `--disable-dbs=CSV` | Comma-separated database names to skip |
| `--check-openalex-authors` | Flag author mismatches from OpenAlex (off by default) |
| `--searxng` | Enable SearxNG web search fallback (see below) |
| `--cache-path=PATH` | Path to query cache database |

### Building Offline Databases

```bash
# DBLP (~4.6GB download, builds SQLite with FTS5 index)
hallucinator-cli update-dblp dblp.db

# ACL Anthology
hallucinator-cli update-acl acl.db

# arXiv (~4GB download from Kaggle — needs ~/.kaggle/kaggle.json or
# KAGGLE_USERNAME+KAGGLE_KEY env vars; accept the dataset license once
# at https://www.kaggle.com/datasets/Cornell-University/arxiv)
hallucinator-cli update-arxiv arxiv.db

# Alternative: skip the download and point at an already-downloaded
# Kaggle zip / JSON dump (useful for retries)
hallucinator-cli update-arxiv arxiv.db --dump /path/to/arxiv-metadata-oai-snapshot.json
```

When `--arxiv-offline` is configured, the online arXiv backend is replaced entirely (same pattern as DBLP / ACL / OpenAlex).

---

## TUI

The TUI is designed for processing multiple papers at once — pick files, queue them up, and watch results stream in.

```bash
# Launch with file picker
hallucinator-tui

# Pre-load PDFs or archives
hallucinator-tui paper1.pdf paper2.pdf proceedings.zip

# With options
hallucinator-tui --dblp-offline=dblp.db --acl-offline=acl.db --arxiv-offline=arxiv.db --theme=modern
```

### TUI Options

All CLI options above, plus:

| Option | Description |
|--------|-------------|
| `--theme hacker\|modern` | Color theme (default: hacker) |
| `--mouse` | Enable mouse support |
| `--fps N` | Target framerate, 1-120 (default: 30) |

The TUI also has `update-dblp`, `update-acl`, and `update-arxiv` subcommands, same as the CLI.

### Screens

**File Picker** — Browse directories, select PDFs or archives (ZIP, tar.gz). Archives are streamed: PDFs are extracted and queued as they're found, so processing starts immediately.

**Queue** — Shows all papers with real-time progress bars. Sort by order, problem count, problem %, or filename. Filter by status (all, has problems, done, running, queued). Search by filename with `/`.

**Paper Detail** — All references for a single paper. Filter to show problems only. Sort by reference number, verdict, or source database.

**Reference Detail** — Full info for a single reference: title, authors, raw citation, matched authors, source database, DOI/arXiv info, retraction warnings, per-database timeout status. Mark false positives as safe with Space.

**Config** — Edit all settings inline: API keys (masked display), database paths, disabled databases, concurrency limits, timeouts, archive size limit, theme, FPS.

**Export** — Save results as JSON, CSV, Markdown, plain text, or HTML. Export a single paper or all papers at once.

### Key Bindings

| Key | Action |
|-----|--------|
| `j`/`k` or arrows | Navigate |
| `Enter` | Select / confirm |
| `Esc` | Back / cancel |
| `o` | Add more PDFs to queue |
| `e` | Export results |
| `,` | Open config |
| `s` | Cycle sort order |
| `f` | Cycle filter |
| `Space` | Mark reference as safe |
| `Tab` | Toggle activity pane |
| `?` | Help screen |

---

## Configuration

Settings are loaded from (highest to lowest priority):

1. CLI arguments
2. Environment variables (`OPENALEX_KEY`, `S2_API_KEY`, `GOVINFO_KEY`, `DBLP_OFFLINE_PATH`, `ACL_OFFLINE_PATH`, `SEARXNG_URL`, `DB_TIMEOUT`, `DB_TIMEOUT_SHORT`)
3. Config file
4. Defaults

### Config File

The TUI looks for config files at:

1. `./hallucinator.toml` (current directory)
2. `~/.config/hallucinator/config.toml` (or platform equivalent via `$XDG_CONFIG_HOME`)

Settings changed in the TUI config screen are persisted automatically.

```toml
[api_keys]
openalex_key = "..."
s2_api_key = "..."
govinfo_key = "..."  # Free from api.data.gov

[databases]
dblp_offline_path = "/path/to/dblp.db"
acl_offline_path = "/path/to/acl.db"
arxiv_offline_path = "/path/to/arxiv.db"
disabled = ["OpenAlex", "PubMed"]

[concurrency]
max_concurrent_papers = 2
max_concurrent_refs = 4
db_timeout_secs = 10
db_timeout_short_secs = 5
max_archive_size_mb = 500  # 0 = unlimited

[display]
theme = "modern"
fps = 30
```

### Offline Database Auto-Detection

If no path is specified, the tool checks:
1. `dblp.db` / `acl.db` / `arxiv.db` in the current directory
2. `~/.local/share/hallucinator/dblp.db` (or platform equivalent)

---

## Databases

| Database | Coverage | Notes |
|----------|----------|-------|
| CrossRef | DOIs, journal articles, conference papers | |
| arXiv | Preprints (CS, physics, math, etc.) | Online API or offline SQLite + FTS5 (Kaggle snapshot) |
| DBLP | Computer science bibliography | Online API or offline SQLite + FTS5 |
| Semantic Scholar | Aggregates Academia.edu, SSRN, PubMed, and more | Optional API key for higher rate limits |
| ACL Anthology | Computational linguistics | Online API or offline SQLite + FTS5 |
| Europe PMC | Life science literature (42M+ abstracts) | |
| PubMed | Biomedical literature via NCBI | |
| DOI Resolver | Validates references by resolving DOIs via doi.org | Only used when a DOI is present |
| OpenAlex | 250M+ works | Online (needs API key) or offline SQLite |
| Open Library | Books, technical reports, and non-academic publications | |
| GovInfo | US federal laws, regulations, court opinions | Optional, needs free API key from api.data.gov |
| URL Checker | Liveness check for non-academic URLs (GitHub, blogs, etc.) | Weaker verification — confirms URL is reachable |
| Web Search | SearxNG metasearch fallback (Google, Bing, Google Scholar) | Optional, self-hosted, no author verification |

Each reference is checked against all enabled databases concurrently. First verified match wins (early exit). SearxNG and URL checks are used as fallbacks only when no academic database match is found.

---

## Web Search Fallback (SearxNG)

For papers not found in any academic database, you can enable a web search fallback using [SearxNG](https://docs.searxng.org/), a self-hosted metasearch engine.

> **Important:** Web search matches are **weaker** than database matches. They only confirm that a paper with a matching title exists somewhere on the web—they **cannot verify authors**. The tool displays these as "Web Search" matches to distinguish them from verified database matches. Treat web matches as hints for manual verification.

### Setup

```bash
cd docker/searxng
docker compose up -d
```

This starts SearxNG on `http://localhost:8080` with Google, Bing, DuckDuckGo, and Google Scholar enabled.

### Usage

```bash
# CLI
hallucinator-cli check --searxng paper.pdf

# Custom URL
SEARXNG_URL=http://your-server:8080 hallucinator-cli check --searxng paper.pdf
```

In the TUI, configure via the Config screen (press `,`): set SearxNG URL to `http://localhost:8080`.

### Config file

```toml
[databases]
searxng_url = "http://localhost:8080"
```

### How it works

1. References are checked against all academic databases first
2. If not found anywhere, SearxNG is queried as a fallback
3. Results are filtered for academic domains and exact title matches
4. Matches are marked as "Web Search" (no author verification)

---

## GovInfo (US Federal Laws)

[GovInfo](https://www.govinfo.gov/) provides access to US federal government publications including laws, regulations, congressional records, and court opinions. This is useful for verifying legal citations in academic papers.

### Getting an API Key

1. Go to [api.data.gov/signup](https://api.data.gov/signup/)
2. Enter your name and email
3. You'll receive an API key instantly (no approval process)
4. The key works for all api.data.gov services including GovInfo

### Usage

```bash
# CLI
hallucinator-cli check --govinfo-key=YOUR_KEY paper.pdf

# Environment variable
export GOVINFO_KEY=YOUR_KEY
hallucinator-cli check paper.pdf
```

### Config file

```toml
[api_keys]
govinfo_key = "your-key-here"
```

### Coverage

GovInfo indexes:
- **Public and Private Laws** (e.g., "Clean Air Act Amendments of 1990")
- **Congressional Bills and Reports**
- **Code of Federal Regulations (CFR)**
- **Federal Register**
- **US Court Opinions**
- **Congressional Record**

> **Note:** Academic papers often cite laws by informal names (e.g., "CIPA" instead of "Children's Internet Protection Act"). GovInfo searches the full text, so exact title matching may not always succeed for informal citations.

---

## Architecture

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `hallucinator-parsing` | Reference section parsing and extraction (section detection, segmentation, title/author extraction) |
| `hallucinator-pdf-mupdf` | MuPDF backend for PDF text extraction (AGPL-3.0 isolation layer) |
| `hallucinator-core` | Validation engine, database backends, fuzzy matching, retraction checks |
| `hallucinator-dblp` | Offline DBLP database builder and querier (SQLite + FTS5) |
| `hallucinator-acl` | Offline ACL Anthology database builder and querier |
| `hallucinator-arxiv-offline` | Offline arXiv database builder (Kaggle snapshot ingester) and querier |
| `hallucinator-cli` | CLI binary |
| `hallucinator-tui` | Terminal UI (Ratatui) |
| `hallucinator-web` | Web interface |
| `hallucinator-python` | Python bindings (PyO3) — the `pip install hallucinator` package |

### Concurrency Model

- Configurable number of papers processed in parallel (TUI)
- 4 references checked in parallel per paper (configurable)
- All enabled databases queried concurrently per reference
- Early exit on first verified match
- Retry pass for timed-out queries at the end
- Per-batch cancellation token for graceful stopping

### Result Persistence

The TUI automatically saves results to `~/.cache/hallucinator/runs/<timestamp>/` as JSON, so completed work is not lost if you quit mid-batch.
