# Hallucinated Reference Detector

A tool to detect potentially hallucinated or fabricated references in academic PDF papers. It extracts references from PDFs and validates them against academic databases (DBLP, arXiv, CrossRef).

Created by Gianluca Stringhini with the help of Claude Code and ChatGPT

## Features

- Pure Python PDF reference extraction using PyMuPDF (no external services required)
- Supports multiple citation formats:
  - IEEE (quoted titles)
  - ACM (year before title)
  - USENIX (author-title-venue format)
- Validates references against multiple academic databases:
  - DBLP
  - arXiv
  - CrossRef
- Author matching to detect title matches with wrong authors
- Colored terminal output for easy identification of issues
- Handles em-dash citations (same authors as previous reference)

## Installation

```bash
# Create a virtual environment (recommended)
python3 -m venv venv
source venv/bin/activate

# Install dependencies
pip install -r requirements.txt
```

## Usage

```bash
# Basic usage
python check_hallucinated_references.py <path_to_pdf>

# Without colored output (for piping or non-color terminals)
python check_hallucinated_references.py --no-color <path_to_pdf>

# Adjust delay between API requests (default: 1 second)
python check_hallucinated_references.py --sleep=0.5 <path_to_pdf>

# Use OpenAlex API (queries OpenAlex first, then DBLP, arXiv, CrossRef on failure)
python check_hallucinated_references.py --openalex-key=YOUR_API_KEY <path_to_pdf>

# Combine options
python check_hallucinated_references.py --no-color --sleep=0.1 <path_to_pdf>
```

### Options

| Option | Description |
|--------|-------------|
| `--no-color` | Disable colored output (useful for piping or logging) |
| `--sleep=SECONDS` | Set delay between API requests (default: 1.0 second) |
| `--openalex-key=KEY` | OpenAlex API key. If provided, queries OpenAlex first before other databases. Get a free key at https://openalex.org/settings/api |

## Example Output

```
Analyzing paper example.pdf

============================================================
POTENTIAL HALLUCINATION DETECTED
============================================================

Title:
  Some Fabricated Paper Title That Does Not Exist

Status: Reference not found in any database
Searched: DBLP, arXiv, CrossRef

------------------------------------------------------------

============================================================
SUMMARY
============================================================
  Total references analyzed: 35
  Verified: 34
  Not found (potential hallucinations): 1
```

## How It Works

1. **PDF Text Extraction**: Uses PyMuPDF to extract text from the PDF
2. **Reference Section Detection**: Locates the "References" or "Bibliography" section
3. **Reference Segmentation**: Splits references by numbered patterns ([1], [2], etc.)
4. **Title & Author Extraction**: Parses each reference to extract title and authors
5. **Database Validation**: Queries DBLP, arXiv, and CrossRef to verify the reference exists
6. **Author Matching**: Confirms that found titles have matching authors

## Limitations

- References to non-indexed sources (technical reports, websites, books) may be flagged as "not found"
- Very recent papers may not yet be indexed in databases
- Some legitimate papers in niche journals may not be found
- PDF extraction quality depends on the PDF structure

## Dependencies

- `requests` - HTTP requests for API queries
- `beautifulsoup4` - HTML parsing
- `rapidfuzz` - Fuzzy string matching for title comparison
- `feedparser` - arXiv API response parsing
- `PyMuPDF` - PDF text extraction

## License

MIT License
