# hallucinator-rs

Rust implementation of the Hallucinated Reference Detector. Includes a CLI and an interactive terminal UI (TUI) for batch-processing PDFs and archives.

Same validation engine as the Python version — queries 10 academic databases in parallel, fuzzy-matches titles, checks for retractions — but with a native async runtime and a full-screen TUI for working through large batches interactively.

---

## Building

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
hallucinator-cli check --dblp-offline=dblp.db --acl-offline=acl.db paper.pdf

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
| `--dblp-offline=PATH` | Path to offline DBLP database |
| `--acl-offline=PATH` | Path to offline ACL Anthology database |
| `--output=PATH` | Write output to file |
| `--no-color` | Disable colored output |
| `--disable-dbs=CSV` | Comma-separated database names to skip |
| `--check-openalex-authors` | Flag author mismatches from OpenAlex (off by default) |

### Building Offline Databases

```bash
# DBLP (~4.6GB download, builds SQLite with FTS5 index)
hallucinator-cli update-dblp dblp.db

# ACL Anthology
hallucinator-cli update-acl acl.db
```

---

## TUI

The TUI is designed for processing multiple papers at once — pick files, queue them up, and watch results stream in.

```bash
# Launch with file picker
hallucinator-tui

# Pre-load PDFs or archives
hallucinator-tui paper1.pdf paper2.pdf proceedings.zip

# With options
hallucinator-tui --dblp-offline=dblp.db --acl-offline=acl.db --theme=modern
```

### TUI Options

All CLI options above, plus:

| Option | Description |
|--------|-------------|
| `--theme hacker\|modern` | Color theme (default: hacker) |
| `--mouse` | Enable mouse support |
| `--fps N` | Target framerate, 1-120 (default: 30) |

The TUI also has `update-dblp` and `update-acl` subcommands, same as the CLI.

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
2. Environment variables (`OPENALEX_KEY`, `S2_API_KEY`, `DBLP_OFFLINE_PATH`, `ACL_OFFLINE_PATH`, `DB_TIMEOUT`, `DB_TIMEOUT_SHORT`)
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

[databases]
dblp_offline_path = "/path/to/dblp.db"
acl_offline_path = "/path/to/acl.db"
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
1. `dblp.db` / `acl.db` in the current directory
2. `~/.local/share/hallucinator/dblp.db` (or platform equivalent)

---

## Databases

Same 10 databases as the Python version:

| Database | Coverage |
|----------|----------|
| CrossRef | DOIs, journal articles, conference papers |
| arXiv | Preprints (CS, physics, math, etc.) |
| DBLP | Computer science bibliography (online + offline) |
| Semantic Scholar | Aggregates Academia.edu, SSRN, PubMed, and more |
| ACL Anthology | Computational linguistics (online + offline) |
| NeurIPS | NeurIPS proceedings |
| SSRN | Social science research |
| Europe PMC | Life science literature (42M+ abstracts) |
| PubMed | Biomedical literature via NCBI |
| OpenAlex | 250M+ works (optional, needs API key) |

Each reference is checked against all enabled databases concurrently. First verified match wins (early exit).

---

## Architecture

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `hallucinator-pdf` | PDF text extraction (MuPDF), reference parsing, archive handling |
| `hallucinator-core` | Validation engine, database backends, fuzzy matching, retraction checks |
| `hallucinator-dblp` | Offline DBLP database builder and querier (SQLite + FTS5) |
| `hallucinator-acl` | Offline ACL Anthology database builder and querier |
| `hallucinator-cli` | CLI binary |
| `hallucinator-tui` | Terminal UI (Ratatui) |
| `hallucinator-web` | Web interface |

### Concurrency Model

- Configurable number of papers processed in parallel (TUI)
- 4 references checked in parallel per paper (configurable)
- All enabled databases queried concurrently per reference
- Early exit on first verified match
- Retry pass for timed-out queries at the end
- Per-batch cancellation token for graceful stopping

### Result Persistence

The TUI automatically saves results to `~/.cache/hallucinator/runs/<timestamp>/` as JSON, so completed work is not lost if you quit mid-batch.
