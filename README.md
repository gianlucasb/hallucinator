# Hallucinated Reference Detector

A tool to detect potentially hallucinated or fabricated references in academic PDF papers. It extracts references from PDFs and validates them against academic databases (CrossRef, arXiv, DBLP, OpenReview, Semantic Scholar, and optionally OpenAlex). Available as both a command-line tool and a web interface.

Created by Gianluca Stringhini with the help of Claude Code and ChatGPT

## Features

- Pure Python PDF reference extraction using PyMuPDF (no external services required)
- Supports multiple citation formats:
  - IEEE (quoted titles)
  - ACM (year before title)
  - USENIX (author-title-venue format)
- Validates references against multiple academic databases (in order):
  - OpenAlex (optional, with API key)
  - CrossRef
  - arXiv
  - DBLP
  - OpenReview
  - Semantic Scholar (aggregates Academia.edu, SSRN, PubMed, and more)
- Author matching to detect title matches with wrong authors
- Colored terminal output for easy identification of issues
- Handles em-dash citations (same authors as previous reference)
- Web interface for easy PDF upload and analysis
- Google Scholar links for manual verification of flagged references

## Installation

### pip

```bash
# Create a virtual environment (recommended)
python3 -m venv venv
source venv/bin/activate

# Install dependencies
pip install -r requirements.txt
```

### Docker

Run the web interface in a container:

```bash
# Build the image
docker build -t hallucinator .

# Run the container
docker run -p 5001:5001 hallucinator
```

Then open `http://localhost:5001` in your browser.

To enable debug mode:

```bash
docker run -p 5001:5001 -e FLASK_DEBUG=1 hallucinator
```



## Usage

```bash
# Basic usage
python check_hallucinated_references.py <path_to_pdf>

# Without colored output (for piping or non-color terminals)
python check_hallucinated_references.py --no-color <path_to_pdf>

# Save output to file
python check_hallucinated_references.py --output log.txt <path_to_pdf>


# Adjust delay before DBLP requests (default: 1 second, to avoid rate limiting)
python check_hallucinated_references.py --sleep=0.5 <path_to_pdf>

# Use OpenAlex API (queries OpenAlex first, then CrossRef, arXiv, DBLP on failure)
python check_hallucinated_references.py --openalex-key=YOUR_API_KEY <path_to_pdf>

# Combine options
python check_hallucinated_references.py --no-color --sleep=0.1 <path_to_pdf>
```

### Options

| Option | Description |
|--------|-------------|
| `--no-color` | Disable colored output (useful for piping or logging) |
| `--sleep=SECONDS` | Set delay before DBLP requests to avoid rate limiting (default: 1.0 second). Only applies when a reference isn't found in earlier databases. |
| `--openalex-key=KEY` | OpenAlex API key. If provided, queries OpenAlex first before other databases. Get a free key at https://openalex.org/settings/api |

## Web Interface

The tool also includes a web interface for easier use.

### Starting the Web Server

```bash
# Activate virtual environment
source venv/bin/activate

# Start the Flask server
python app.py
```

The server will start at `http://localhost:5001`.

### Using the Web Interface

1. Open `http://localhost:5001` in your browser
2. Upload a PDF file using the file picker
3. (Optional) Enter your OpenAlex API key for improved coverage
4. Click "Analyze References"
5. View results showing:
   - Summary statistics (verified, author mismatches, not found)
   - List of potentially hallucinated references
   - Google Scholar links for manual verification

The web interface displays the same information as the command-line tool but provides clickable Google Scholar links to quickly verify flagged references.



## Example Output

```
Analyzing paper example.pdf

============================================================
POTENTIAL HALLUCINATION DETECTED
============================================================

Title:
  Some Fabricated Paper Title That Does Not Exist

Status: Reference not found in any database
Searched: CrossRef, arXiv, DBLP, OpenReview, Semantic Scholar

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
5. **Database Validation**: Queries databases in order of rate-limit generosity:
   - OpenAlex (if API key provided) - most generous rate limits
   - CrossRef - good coverage, generous limits
   - arXiv - moderate limits
   - DBLP - more restrictive, queried with configurable delay
   - OpenReview - conference papers (ICLR, NeurIPS, etc.)
   - Semantic Scholar - aggregates Academia.edu, SSRN, PubMed, and institutional repositories
6. **Author Matching**: Confirms that found titles have matching authors

## Skipped References

Some references are skipped during analysis and not checked against databases. The tool reports how many references were skipped and why:

- **Non-academic URLs**: References pointing to websites, GitHub repositories, documentation pages, or other non-academic sources are skipped. These cannot be verified in academic databases. URLs to academic publishers (ACM, IEEE, USENIX, arXiv, DOI) are still processed.

- **Short titles**: References with titles shorter than 5 words are skipped. Very short titles are often not academic papers (e.g., software names, dataset titles) and are prone to false matches.

## Title-Only Verification

References where author names could not be extracted are still checked against databases, but only by title. These are reported as "title-only" in the summary. If the title is found in a database, it counts as verified (without author confirmation).

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
- `flask` - Web interface

## License

This project is licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).

See the [LICENSE](LICENSE) file for details.
