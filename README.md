# Hallucinated Reference Detector

**Detect fake citations in academic papers.** This tool extracts references from PDFs and validates them against academic databases. If a reference doesn't exist anywhere, it's probably hallucinated by an LLM.

Created by Gianluca Stringhini with Claude Code and ChatGPT.

> **Why this exists:** Academia is under attack from AI-generated slop—fake citations, fabricated papers, LLM-written reviews. We observed several papers with hallucinated citations submitted to ACSAC 2025, but the [November 2025 OpenReview incident](https://blog.iclr.cc/2025/12/03/iclr-2026-response-to-security-incident/) exposed how deep the rot goes: 21% of ICLR reviews were likely AI-generated, 199 papers were likely completely written by an AI. This tool is one line of defense. It's not perfect—that's the point. We use AI to fight misuse of AI, openly and honestly. **[Read the full manifesto.](MANIFESTO.md)**
>
> (See those em dashes? They're a known tell of AI-generated text. This README was written with Claude. We're not hiding it—we're proving a point. **[Read why this matters, even if you're an AI absolutist.](MANIFESTO.md#why-ai-should-care)**)

---



## Rust TUI (Recommended for Batch Processing)

If you're willing to install Rust, the **[hallucinator-rs](hallucinator-rs/)** version includes a full terminal UI for batch-processing PDFs and archives interactively, with real-time progress, sorting/filtering, result export, and persistent configuration.

[Install Rust](https://rust-lang.org/tools/install/), then:

```bash
cd hallucinator-rs

# Interactive TUI
cargo build -p hallucinator-tui --release
./target/release/hallucinator-tui

# CLI
cargo build -p hallucinator-cli --release
./target/release/hallucinator-cli check paper.pdf
```

See **[hallucinator-rs/README.md](hallucinator-rs/README.md)** for full documentation.

---

## Python Bindings (Early Release)

Pre-compiled wheels are available for Python 3.12 on Linux (x86_64), macOS (x86_64 + Apple Silicon), and Windows (x86_64). These provide Rust-native performance from Python — no Rust toolchain required.

```bash
pip install hallucinator
```

```python
from hallucinator import PdfExtractor, Validator, ValidatorConfig

# Extract references from a PDF
ext = PdfExtractor()
result = ext.extract("paper.pdf")

# Validate against academic databases
config = ValidatorConfig()
validator = Validator(config)
results = validator.check(result.references)

for r in results:
    print(f"[{r.status}] {r.title}")
```

See **[hallucinator-rs/PYTHON_BINDINGS.md](hallucinator-rs/PYTHON_BINDINGS.md)** for full API docs, configuration options, progress callbacks, and examples.

---

## Python Quick Start (Legacy)

The original pure-Python implementation lives in the `legacy/` directory.

```bash
# 1. Clone and setup
git clone https://github.com/idramalab/hallucinator.git
cd hallucinator
python -m venv venv
venv\Scripts\activate      # Windows
source venv/bin/activate   # Linux/Mac
pip install -r legacy/requirements.txt

# 2. Run it
python legacy/check_hallucinated_references.py your_paper.pdf
```

That's it. You'll see which references check out and which don't exist in any database.

---

## What It Checks

The tool queries these databases simultaneously:

| Database | What it covers |
|----------|----------------|
| **CrossRef** | DOIs, journal articles, conference papers |
| **arXiv** | Preprints (CS, physics, math, etc.) |
| **DBLP** | Computer science bibliography |
| **Semantic Scholar** | Aggregates Academia.edu, SSRN, PubMed, and more |
| **ACL Anthology** | Computational linguistics papers |
| **NeurIPS** | NeurIPS conference proceedings |
| **Europe PMC** | Life science literature (42M+ abstracts, mirrors PubMed/PMC) |
| **PubMed** | Biomedical literature via NCBI E-utilities |
| **OpenAlex** | 250M+ works (optional, needs free API key) |
| **Web Search** | SearxNG fallback for unindexed papers (optional, see below) |

~~**OpenReview**~~ - Disabled. API unreachable after the Nov 2025 incident.

We **strongly recommend** to download the latest **DBLP database** and query it locally, due to their aggressive rate limiting of online queries (see the "Offline DBLP database" section below).

---

## Command Line Usage (Legacy Python)

```bash
# Basic - just check a PDF
python legacy/check_hallucinated_references.py paper.pdf

# With API keys (recommended - better coverage, fewer rate limits)
python legacy/check_hallucinated_references.py --openalex-key=YOUR_KEY --s2-api-key=YOUR_KEY paper.pdf

# Save output to file
python legacy/check_hallucinated_references.py --output results.txt paper.pdf

# No colors (for logs/piping)
python legacy/check_hallucinated_references.py --no-color paper.pdf

# Use offline DBLP database (avoids rate limits)
python legacy/check_hallucinated_references.py --dblp-offline=dblp.db paper.pdf
```

### Command Line Options

| Option | What it does |
|--------|--------------|
| `--openalex-key=KEY` | OpenAlex API key. Get one free: https://openalex.org/settings/api |
| `--s2-api-key=KEY` | Semantic Scholar API key. Request here: https://www.semanticscholar.org/product/api |
| `--output=FILE` | Write output to a file instead of terminal |
| `--no-color` | Disable colored output |
| `--dblp-offline=PATH` | Use offline DBLP database instead of API |
| `--update-dblp=PATH` | Download DBLP dump and build offline database |
| `--check-openalex-authors` | Flag author mismatches from OpenAlex (off by default due to false positives) |

---

## Web Interface (Legacy Python)

```bash
python legacy/app.py
# Open http://localhost:5001
```

Upload a PDF (or ZIP/tar.gz of multiple PDFs), optionally enter API keys, click Analyze. Watch results stream in real-time.

### Web Interface Features

**Retry Failed Queries**
If a database times out during analysis, a "Retry" button appears next to the affected reference. Click it to retry those specific databases with a longer timeout.

**Mark as Safe**
False positive? Click "Mark as safe" on any flagged reference to move it to the verified list. This updates the summary counts and is useful for references that exist but aren't indexed (technical reports, books, etc.).

**Download Report**
After analysis, download a report of problematic references in HTML or plain text format. The report includes:
- The analyzed filename
- Summary statistics
- All problematic references with details
- Author comparisons for mismatches

### Docker

```bash
docker build -t hallucinator legacy/
docker run -p 5001:5001 hallucinator
# Open http://localhost:5001
```

---

## Getting API Keys

API keys are optional but recommended. They improve coverage and reduce rate limiting.

### OpenAlex (free, instant)
1. Go to https://openalex.org/settings/api
2. Sign in with your email
3. Copy your API key

### Semantic Scholar (free, requires approval)
1. Go to https://www.semanticscholar.org/product/api
2. Click "Request API Key"
3. Fill out the form (academic use)
4. Wait for email (usually same day)

---

## Offline DBLP Database

DBLP aggressively rate-limits API requests. For heavy usage, you can download their full database (~4.6GB) and query it locally.

### Setup (one-time, takes 20-30 minutes)

```bash
python legacy/check_hallucinated_references.py --update-dblp=dblp.db
```

This downloads the latest [DBLP N-Triples dump](https://dblp.org/rdf/) and builds a SQLite database with ~6M publications.

### Usage

```bash
# CLI
python legacy/check_hallucinated_references.py --dblp-offline=dblp.db paper.pdf

# Web app (set environment variable)
DBLP_OFFLINE_PATH=dblp.db python legacy/app.py
```

### Keeping it fresh

The database is a snapshot. If it's more than 30 days old, you'll see a warning:

```
Warning: Your DBLP database is 47 days old. Run with --update-dblp to refresh.
```

Re-run `--update-dblp` to download the latest dump. DBLP publishes daily updates but there's no incremental download—you'll need to re-download the full 4.6GB each time.

---

## Web Search Fallback (SearxNG)

For papers not found in any academic database, you can enable a web search fallback using [SearxNG](https://docs.searxng.org/), a self-hosted metasearch engine. This is useful for technical reports, workshop papers, or other publications not indexed in traditional academic databases.

> **Important:** Web search matches are **weaker** than database matches. They only confirm that a paper with a matching title exists somewhere on the web—they **cannot verify authors**. Use web matches as a hint for manual verification, not as definitive proof. The tool will display these as "Web Search" matches to distinguish them from verified database matches.

### Setup (Rust TUI/CLI only)

```bash
cd hallucinator-rs/docker/searxng
docker compose up -d
```

This starts a SearxNG instance on `http://localhost:8080`.

### Usage

```bash
# CLI
hallucinator-cli check --searxng paper.pdf

# TUI - configure in the Config screen (press ,)
# Set SearxNG URL to http://localhost:8080
```

### Custom URL

```bash
# Use a different SearxNG instance
SEARXNG_URL=http://your-searxng:8080 hallucinator-cli check --searxng paper.pdf
```

### How it works

1. References are checked against all academic databases first
2. If a reference is **not found** in any database, SearxNG is queried
3. SearxNG searches Google, Bing, DuckDuckGo, and Google Scholar
4. If a matching title is found, the reference is marked as "Verified (Web Search)"
5. No author verification is performed—only title matching

---

## Understanding Results

### Verified
The reference was found in at least one database with matching authors. It exists.

**Note on Web Search matches:** If a reference is marked as "Verified (Web Search)", this is a weaker match—the title was found on the web but authors could not be verified. These should be manually confirmed.

### Author Mismatch
The title was found but with different authors. Could be:
- A citation error in the paper
- Authors listed differently in the database
- A real problem worth investigating

### Not Found (Potential Hallucination)
The reference wasn't found in any database. This could mean:
- **Hallucinated** - LLM made it up
- **Too new** - Not indexed yet
- **Not indexed** - Technical reports, books, websites
- **Database timeout** - Check if timeouts were reported

The tool tells you which databases timed out so you can assess confidence.

### Retracted Papers
The tool automatically checks if verified papers have been retracted using CrossRef's retraction metadata (which includes the Retraction Watch database). Retracted papers are flagged with a warning and shown in a dedicated "Retracted Papers" section in the web interface.

Retraction checks work via:
- **DOI lookup** - If the reference has a DOI, checks CrossRef for retraction notices
- **Title search** - Falls back to title-based search if no DOI is available

This helps identify cases where a paper cites work that has since been withdrawn due to errors, fraud, or other issues.

---

## What Gets Skipped

Some references are intentionally not checked:

- **URLs** - Links to GitHub, docs, websites (not in academic DBs)
- **Short titles** - Less than 5 words (too generic, false matches)

The output tells you how many were skipped and why.

---

## Limitations

We're not perfect. Neither is anyone else. Here's what can go wrong:

1. **Database coverage** - Some legitimate papers aren't indexed anywhere
2. **Very recent papers** - Takes time to appear in databases
3. **Books and technical reports** - Often not in these databases
4. **PDF extraction** - Bad PDF formatting can mangle references
5. **Rate limits** - Heavy use may hit API limits (use API keys)

If something is flagged as "not found," verify manually with Google Scholar before accusing anyone of anything.

---

## How It Works

1. **Extract text** from PDF using PyMuPDF
2. **Find references section** (looks for "References" or "Bibliography")
3. **Parse each reference** - extracts title and authors
4. **Query all databases in parallel** - 4 references at a time, all DBs simultaneously
5. **Early exit** - stops querying once a match is found
6. **Retry failed queries** - timeouts get a second chance at the end
7. **Report results** - verified, mismatched, or not found

---

## License

GNU Affero General Public License v3.0 (AGPL-3.0). See [LICENSE](LICENSE).

If you use this to catch fake papers, we'd love to hear about it.
