# Getting Started

This guide covers installation and your first reference check across all available interfaces.

## Choose Your Interface

| Interface | Best for | Install method |
|-----------|----------|----------------|
| **TUI** | Batch processing, exploring results interactively | Pre-built binary or `cargo install` |
| **CLI** | Single-file checks, scripting, CI pipelines | Pre-built binary or `cargo install` |
| **Python** | Integration into existing Python workflows | `pip install hallucinator` |
| **From source** | Development, customization | `cargo build --release` |

## Install Pre-built Binaries

Download the latest release for your platform from [GitHub Releases](https://github.com/hallucinator-project/hallucinator/releases). Both `hallucinator-cli` and `hallucinator-tui` binaries are included.

### macOS / Linux

```bash
# Example: download and extract
tar xzf hallucinator-*-x86_64-unknown-linux-gnu.tar.gz
sudo mv hallucinator-cli hallucinator-tui /usr/local/bin/
```

### Build from Source

```bash
cd hallucinator-rs
cargo build --release
# Binaries are in target/release/hallucinator-cli and target/release/hallucinator-tui
```

## Install Python Bindings

Pre-compiled wheels are available for major platforms:

```bash
pip install hallucinator
```

Or build from source (requires Rust toolchain):

```bash
cd hallucinator-rs/crates/hallucinator-python
pip install maturin
maturin develop --release
```

See [Python Bindings](../api-reference/python-bindings.md) for the full API.

## First Run: CLI

Check a single PDF:

```bash
hallucinator-cli check paper.pdf
```

The CLI will extract references, query databases, and print results with colored output. Each reference gets a verdict: **Verified**, **Not Found**, or **Author Mismatch**.

### Useful Options

```bash
# Dry run — extract references without querying databases
hallucinator-cli check --dry-run paper.pdf

# Use offline DBLP for faster local lookups
hallucinator-cli check --dblp-offline dblp.db paper.pdf

# Save output to a file
hallucinator-cli check -o results.txt paper.pdf

# Check a .bbl or .bib file (LaTeX bibliography)
hallucinator-cli check references.bbl
```

## First Run: TUI

Process multiple PDFs interactively:

```bash
hallucinator-tui paper1.pdf paper2.pdf *.pdf
```

The TUI opens with a queue of papers. Navigate with arrow keys:
- **Enter** — Open paper results
- **Tab** — Switch between panels
- **q** — Quit
- **?** — Show help

See the [Rust README](https://github.com/gianlucasb/hallucinator/blob/main/hallucinator-rs/README.md) for full key bindings.

## First Run: Python

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

# Extract references from a PDF
extractor = PdfExtractor()
result = extractor.extract("paper.pdf")
print(f"Found {len(result.references)} references")

# Validate references
config = ValidatorConfig()
validator = Validator(config)
results = validator.check(result.references)

for r in results:
    print(f"  [{r.status}] {r.title}")
```

See [PYTHON_BINDINGS.md](https://github.com/gianlucasb/hallucinator/blob/main/hallucinator-rs/PYTHON_BINDINGS.md) for the complete API.

## Optional: API Keys

Some databases offer higher rate limits or additional features with API keys:

| Key | Environment Variable | Effect |
|-----|---------------------|--------|
| OpenAlex | `OPENALEX_KEY` | Enables OpenAlex database (disabled without key) |
| Semantic Scholar | `S2_API_KEY` | Higher rate limit (100/s vs 1/s) |
| CrossRef mailto | `CROSSREF_MAILTO` | Polite pool: 3/s instead of 1/s |

Set them as environment variables or in your [config file](configuration.md).

## Optional: Offline Databases

For faster local lookups and reduced API dependence, build offline databases:

```bash
# DBLP (~4.6GB download, 20–30 minutes)
hallucinator-cli update-dblp dblp.db

# ACL Anthology (smaller, a few minutes)
hallucinator-cli update-acl acl.db
```

Then use them:

```bash
hallucinator-cli check --dblp-offline dblp.db --acl-offline acl.db paper.pdf
```

Or set paths in your config file for automatic detection. See [Offline Databases](offline-databases.md) for details.

## Next Steps

- [Configuration](configuration.md) — All config options (CLI, env vars, TOML)
- [Understanding Results](understanding-results.md) — Interpreting what the output means
- [Offline Databases](offline-databases.md) — Setup and maintenance
- [Export Formats](export-formats.md) — Saving results as JSON, CSV, Markdown, etc.
