# Hallucinated Reference Detector

**Detect fake citations in academic papers.** This tool extracts references from PDFs and validates them against academic databases. If a reference doesn't exist anywhere, it's probably hallucinated by an LLM.

Created by Gianluca Stringhini with Claude Code and ChatGPT.

> **Why this exists:** Academia is under attack from AI-generated slop—fake citations, fabricated papers, LLM-written reviews. We observed several papers with hallucinated citations submitted to ACSAC 2025, but the [November 2025 OpenReview incident](https://blog.iclr.cc/2025/12/03/iclr-2026-response-to-security-incident/) exposed how deep the rot goes: 21% of ICLR reviews were likely AI-generated, 199 papers were likely completely written by an AI. This tool is one line of defense. It's not perfect—that's the point. We use AI to fight misuse of AI, openly and honestly. **[Read the full manifesto.](MANIFESTO.md)**
>
> (See those em dashes? They're a known tell of AI-generated text. This README was written with Claude. We're not hiding it—we're proving a point. **[Read why this matters, even if you're an AI absolutist.](MANIFESTO.md#why-ai-should-care)**)

---



## Rust TUI (Recommended for Most Scenarios)

The TUI includes a full terminal UI for batch-processing PDFs and archives interactively, with real-time progress, sorting/filtering, result export, and persistent configuration.

You can install it with:
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gianlucasb/hallucinator/releases/latest/download/hallucinator-tui-installer.sh | sh
```

![demo](https://github.com/user-attachments/assets/b879eb66-5b94-4a75-9e34-79a024c5646e)

See **[hallucinator-rs/README.md](hallucinator-rs/README.md)** for full documentation.

---

## Rust CLI

When the TUI is not suitable, there's a pure CLI available too:
```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/gianlucasb/hallucinator/releases/latest/download/hallucinator-cli-installer.sh | sh
```

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

We **strongly recommend** downloading the **DBLP** and **ACL Anthology** databases for local querying—DBLP in particular rate-limits online requests aggressively. See the "Offline Databases" section below.

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

## Offline Databases

Three databases can be downloaded for local querying. Offline databases are faster than online APIs and avoid rate limits.

```bash
# Build all three (run once, refresh periodically)
hallucinator-cli update-dblp dblp.db
hallucinator-cli update-acl acl.db
hallucinator-cli update-openalex openalex.idx

# Use them
hallucinator-cli check --dblp-offline=dblp.db --acl-offline=acl.db --openalex-offline=openalex.idx paper.pdf
```

If you place the databases in `~/.local/share/hallucinator/`, they're detected automatically—no flags needed.

### DBLP (strongly recommended)

DBLP aggressively rate-limits API requests. Download their full database (~4.6GB) and query it locally:

```bash
hallucinator-cli update-dblp dblp.db
```

This downloads the latest [DBLP N-Triples dump](https://dblp.org/rdf/) and builds a SQLite database with ~6M publications. Takes 20-30 minutes.

### ACL Anthology

Downloads the full ACL Anthology and builds a local SQLite database:

```bash
hallucinator-cli update-acl acl.db
```

### OpenAlex

> **Warning:** The full OpenAlex snapshot is very large. OpenAlex records are regularly updated with citation counts, metadata corrections, and new works—so a local snapshot goes stale quickly. **For most users, the online API with a free API key (see above) is the better choice.** Only build a local index if you have a specific need for fully offline operation or are hitting rate limits you can't solve with an API key.

```bash
# Full build (large download — expect many gigabytes and hours of indexing)
hallucinator-cli update-openalex openalex.idx

# Only works published 2020 or later (smaller, faster)
hallucinator-cli update-openalex openalex.idx --min-year 2020

# Incremental update — only download partitions newer than a date
hallucinator-cli update-openalex openalex.idx --since 2026-01-01
```

The local index uses Tantivy (full-text search). Use `--min-year` to limit scope and `--since` for incremental refreshes.

### Keeping databases fresh

All offline databases are snapshots. If any database is more than 30 days old, the tool shows a staleness warning with instructions to refresh. Re-run the corresponding `update-*` command to rebuild.

See **[hallucinator-rs/README.md](hallucinator-rs/README.md)** for config file options, auto-detection paths, and more details.

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

1. **Extract text** from PDF (MuPDF)
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
