# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Hallucinated Reference Detector** - Detects potentially fabricated references in academic PDF papers by validating against multiple academic databases (CrossRef, arXiv, DBLP, OpenReview, Semantic Scholar, and optionally OpenAlex).

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
python check_hallucinated_references.py --no-color --sleep=0.5 --openalex-key=KEY <pdf>
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
4. **Title/Author Extraction** - Parses from IEEE, ACM, USENIX formats
5. **Validation** - Queries databases in rate-limit-friendly order
6. **Reporting** - CLI colored output or web JSON response

### Database Query Order (by rate-limit generosity)
1. OpenAlex (optional, needs API key)
2. CrossRef
3. arXiv
4. DBLP (configurable delay, default 1s)
5. OpenReview
6. Semantic Scholar

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
