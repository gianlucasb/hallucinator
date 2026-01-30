# Hallucinated Reference Detector

**Detect fake citations in academic papers.** This tool extracts references from PDFs and validates them against academic databases. If a reference doesn't exist anywhere, it's probably hallucinated by an LLM.

Created by Gianluca Stringhini with Claude Code and ChatGPT.

> **Why this exists:** Academia is under attack from AI-generated slop—fake citations, fabricated papers, LLM-written reviews. The [November 2025 OpenReview incident](https://blog.iclr.cc/2025/12/03/iclr-2026-response-to-security-incident/) exposed how deep the rot goes: 21% of ICLR reviews were AI-generated, 199 papers were pure slop. This tool is one line of defense. It's not perfect—that's the point. We use AI to fight misuse of AI, openly and honestly. **[Read the full manifesto.](MANIFESTO.md)**
>
> (See those em dashes? They're a known tell of AI-generated text. This README was written with Claude. We're not hiding it—we're proving a point. **[Read why this matters, even if you're an AI absolutist.](MANIFESTO.md#why-ai-should-care)**)

---

## Quick Start

```bash
# 1. Clone and setup
git clone https://github.com/idramalab/hallucinator.git
cd hallucinator
python -m venv venv
venv\Scripts\activate      # Windows
source venv/bin/activate   # Linux/Mac
pip install -r requirements.txt

# 2. Run it
python check_hallucinated_references.py your_paper.pdf
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
| **OpenAlex** | 250M+ works (optional, needs free API key) |

~~**OpenReview**~~ - Disabled. API unreachable after the Nov 2025 incident.

---

## Command Line Usage

```bash
# Basic - just check a PDF
python check_hallucinated_references.py paper.pdf

# With API keys (recommended - better coverage, fewer rate limits)
python check_hallucinated_references.py --openalex-key=YOUR_KEY --s2-api-key=YOUR_KEY paper.pdf

# Save output to file
python check_hallucinated_references.py --output results.txt paper.pdf

# No colors (for logs/piping)
python check_hallucinated_references.py --no-color paper.pdf
```

### Command Line Options

| Option | What it does |
|--------|--------------|
| `--openalex-key=KEY` | OpenAlex API key. Get one free: https://openalex.org/settings/api |
| `--s2-api-key=KEY` | Semantic Scholar API key. Request here: https://www.semanticscholar.org/product/api |
| `--output=FILE` | Write output to a file instead of terminal |
| `--no-color` | Disable colored output |

---

## Web Interface

```bash
python app.py
# Open http://localhost:5001
```

Upload a PDF (or ZIP/tar.gz of multiple PDFs), optionally enter API keys, click Analyze. Watch results stream in real-time.

### Docker

```bash
docker build -t hallucinator .
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

## Understanding Results

### Verified
The reference was found in at least one database with matching authors. It exists.

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
