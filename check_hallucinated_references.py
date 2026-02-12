import re
import sys
import os
import requests
import urllib.parse
import unicodedata
import logging
from bs4 import BeautifulSoup
from rapidfuzz import fuzz
import feedparser
import time
import json
import contextlib
from concurrent.futures import ThreadPoolExecutor, as_completed

logger = logging.getLogger(__name__)

# Request timeout in seconds - can override with DB_TIMEOUT env var for testing
# Set to a low value (e.g., 0.001) to force timeouts for testing warnings
DB_TIMEOUT = float(os.environ.get('DB_TIMEOUT', '10'))  # 10s default for fast DBs
DB_TIMEOUT_RETRY = float(os.environ.get('DB_TIMEOUT_RETRY', '45'))  # 45s for retry pass (OpenReview is slow)

# Canonical list of all supported database sources.
#
# This serves as the single source of truth for database identifiers used across
# the CLI, web GUI, and core validation logic. Entry points (CLI arg parsing,
# web form handling) are responsible for determining which databases are enabled
# based on user configuration, then passing the filtered set downstream via the
# `enabled_dbs` parameter. The core query function (`query_all_databases_concurrent`)
# only constructs and executes queries for databases present in `enabled_dbs` —
# it does not make policy decisions about what to run.
#
# This design intentionally separates configuration from execution, mirroring
# how this would be modeled in a typed language (e.g., an enum Database with a
# HashSet<Database> passed to the query layer). Python lacks native enum support
# for this pattern, so we use a list of string constants. The goal is to eventually
# port infrastructure like this to Rust for static compilation, eliminating the
# need for a Python environment or Docker for end users.
ALL_DATABASES = [
    'CrossRef', 'arXiv', 'DBLP', 'Semantic Scholar',
    'SSRN', 'ACL Anthology', 'NeurIPS', 'Europe PMC', 'PubMed', 'OpenAlex',
]

# Thread-local storage for current timeout (allows retry pass to use longer timeout)
import threading
_timeout_local = threading.local()

def get_timeout():
    """Get current timeout, respecting retry pass longer timeout."""
    return getattr(_timeout_local, 'timeout', DB_TIMEOUT)

# ANSI color codes for terminal output
class Colors:
    RED = '\033[91m'
    GREEN = '\033[92m'
    YELLOW = '\033[93m'
    BLUE = '\033[94m'
    MAGENTA = '\033[95m'
    CYAN = '\033[96m'
    WHITE = '\033[97m'
    BOLD = '\033[1m'
    DIM = '\033[2m'
    RESET = '\033[0m'

    @classmethod
    def disable(cls):
        """Disable all colors by setting them to empty strings."""
        cls.RED = ''
        cls.GREEN = ''
        cls.YELLOW = ''
        cls.BLUE = ''
        cls.MAGENTA = ''
        cls.CYAN = ''
        cls.WHITE = ''
        cls.BOLD = ''
        cls.DIM = ''
        cls.RESET = ''


def print_hallucinated_reference(title, error_type, source=None, ref_authors=None, found_authors=None, searched_openalex=False):
    """Print formatted output for a hallucinated or mismatched reference."""
    print()
    print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
    print(f"{Colors.RED}{Colors.BOLD}POTENTIAL HALLUCINATION DETECTED{Colors.RESET}")
    print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
    print()
    print(f"{Colors.BOLD}Title:{Colors.RESET}")
    print(f"  {Colors.CYAN}{title}{Colors.RESET}")
    print()

    if error_type == "not_found":
        print(f"{Colors.RED}Status:{Colors.RESET} Reference not found in any database")
        if searched_openalex:
            print(f"{Colors.DIM}Searched: OpenAlex, CrossRef, arXiv, DBLP, Semantic Scholar, ACL, NeurIPS{Colors.RESET}")
        else:
            print(f"{Colors.DIM}Searched: CrossRef, arXiv, DBLP, Semantic Scholar, ACL, NeurIPS{Colors.RESET}")
    elif error_type == "author_mismatch":
        print(f"{Colors.YELLOW}Status:{Colors.RESET} Title found on {source} but authors don't match")
        print()
        print(f"{Colors.BOLD}Authors in paper:{Colors.RESET}")
        for author in ref_authors:
            print(f"  {Colors.GREEN}• {author}{Colors.RESET}")
        print()
        print(f"{Colors.BOLD}Authors in {source}:{Colors.RESET}")
        for author in found_authors:
            print(f"  {Colors.MAGENTA}• {author}{Colors.RESET}")

    print()
    print(f"{Colors.RED}{Colors.BOLD}{'-'*60}{Colors.RESET}")
    print()

def normalize_title(title):
    """Normalize title for comparison - keep only alphanumeric characters (Unicode-aware)."""
    import html
    title = html.unescape(str(title))  # Decode HTML entities like &quot;
    title = unicodedata.normalize("NFKD", title)
    # Keep only Unicode letters and numbers, remove everything else including spaces
    title = ''.join(c for c in title if c.isalnum())
    return title.lower()


def extract_doi(text):
    """Extract DOI from reference text.

    Handles formats like:
    - 10.1234/example
    - doi:10.1234/example
    - https://doi.org/10.1234/example
    - http://dx.doi.org/10.1234/example
    - DOI: 10.1234/example

    Also handles DOIs split across lines (common in PDFs).

    Returns the DOI string (e.g., "10.1234/example") or None if not found.
    """
    # First, fix DOIs that are split across lines (apply to all text before pattern matching)
    # Pattern 1: DOI ending with a period followed by newline and 3+ digits
    # e.g., "10.1145/3442381.\n3450048" -> "10.1145/3442381.3450048"
    # e.g., "10.48550/arXiv.2404.\n06011" -> "10.48550/arXiv.2404.06011"
    # Requires 3+ digits to avoid joining sentence periods with short page numbers (e.g., ".\n18")
    text_fixed = re.sub(r'(10\.\d{4,}/[^\s\]>)}]+\.)\s*\n\s*(\d{3,})', r'\1\2', text)

    # Pattern 1b: DOI ending with digits followed by newline and DOI continuation
    # e.g., "10.1109/SP40000.20\n20.00038" -> "10.1109/SP40000.2020.00038"
    # e.g., "10.1145/2884781.2884\n807" -> "10.1145/2884781.2884807"
    # e.g., "10.1109/TSE.20\n18.2884955" -> "10.1109/TSE.2018.2884955"
    # Continuation must look like DOI content: digits optionally followed by .digits
    text_fixed = re.sub(r'(10\.\d{4,}/[^\s\]>)}]+\d)\s*\n\s*(\d+(?:\.\d+)*)', r'\1\2', text_fixed)

    # Pattern 2: DOI ending with a dash followed by newline and continuation
    # e.g., "10.2478/popets-\n2019-0037" -> "10.2478/popets-2019-0037"
    text_fixed = re.sub(r'(10\.\d{4,}/[^\s\]>)}]+-)\s*\n\s*(\S+)', r'\1\2', text_fixed)

    # Pattern 3: URL split across lines - doi.org URL followed by newline and DOI continuation
    # e.g., "https://doi.org/10.48550/arXiv.2404.\n06011"
    text_fixed = re.sub(r'(https?://(?:dx\.)?doi\.org/10\.\d{4,}/[^\s\]>)}]+\.)\s*\n\s*(\d+)', r'\1\2', text_fixed, flags=re.IGNORECASE)

    # Pattern 3b: URL split mid-number
    text_fixed = re.sub(r'(https?://(?:dx\.)?doi\.org/10\.\d{4,}/[^\s\]>)}]+\d)\s*\n\s*(\d[^\s\]>)}]*)', r'\1\2', text_fixed, flags=re.IGNORECASE)

    # Priority 1: Extract from URL format (most reliable - clear boundaries)
    # Matches https://doi.org/... or http://dx.doi.org/... or http://doi.org/...
    url_pattern = r'https?://(?:dx\.)?doi\.org/(10\.\d{4,}/[^\s\]>)},]+)'
    url_match = re.search(url_pattern, text_fixed, re.IGNORECASE)
    if url_match:
        doi = url_match.group(1)
        # Clean trailing punctuation
        doi = doi.rstrip('.,;:')
        return doi

    # Priority 2: DOI pattern without URL prefix
    # 10.XXXX/suffix where suffix can contain various characters
    # The suffix ends at whitespace, or common punctuation at end of reference
    doi_pattern = r'10\.\d{4,}/[^\s\]>)}]+'

    match = re.search(doi_pattern, text_fixed)
    if match:
        doi = match.group(0)
        # Clean trailing punctuation that might have been captured
        doi = doi.rstrip('.,;:')
        return doi
    return None


def validate_doi(doi):
    """Validate a DOI by querying doi.org and return metadata.

    Returns a dict with:
        - valid: True if DOI resolves
        - title: Paper title from DOI metadata (if valid)
        - authors: List of author names (if valid)
        - error: Error message (if invalid)
    """
    if not doi:
        return {'valid': False, 'error': 'No DOI provided'}

    url = f"https://doi.org/{doi}"
    headers = {
        "Accept": "application/vnd.citationstyles.csl+json",
        "User-Agent": "HallucinatedReferenceChecker/1.0"
    }

    try:
        response = requests.get(url, headers=headers, timeout=get_timeout(), allow_redirects=True)

        if response.status_code == 200:
            try:
                data = response.json()
                title = data.get("title", "")
                # Handle title that might be a list
                if isinstance(title, list):
                    title = title[0] if title else ""

                authors = []
                for author in data.get("author", []):
                    # Build author name from family/given or literal
                    if "family" in author:
                        name = author.get("given", "") + " " + author["family"]
                        authors.append(name.strip())
                    elif "literal" in author:
                        authors.append(author["literal"])

                return {
                    'valid': True,
                    'title': title,
                    'authors': authors,
                    'error': None
                }
            except (json.JSONDecodeError, KeyError) as e:
                return {'valid': False, 'error': f'Failed to parse DOI metadata: {e}'}
        elif response.status_code == 404:
            return {'valid': False, 'error': 'DOI not found'}
        else:
            return {'valid': False, 'error': f'DOI lookup failed: HTTP {response.status_code}'}

    except requests.exceptions.Timeout:
        return {'valid': False, 'error': 'DOI lookup timed out'}
    except requests.exceptions.RequestException as e:
        return {'valid': False, 'error': f'DOI lookup failed: {e}'}


def check_retraction(doi):
    """Check if a paper with given DOI has been retracted using CrossRef API.

    CrossRef includes Retraction Watch database data since 2023.

    Returns a dict with:
        - retracted: True if paper has been retracted
        - retraction_doi: DOI of the retraction notice (if available)
        - retraction_date: Date of retraction (if available)
        - retraction_type: Type of notice (Retraction, Expression of Concern, etc.)
        - error: Error message (if lookup failed)
    """
    if not doi:
        return {'retracted': False, 'error': None}

    url = f"https://api.crossref.org/works/{doi}"
    headers = {
        "User-Agent": "HallucinatedReferenceChecker/1.0 (mailto:hallucination-checker@example.com)"
    }

    try:
        response = requests.get(url, headers=headers, timeout=get_timeout())

        if response.status_code == 200:
            data = response.json()
            work = data.get('message', {})

            # Check for update-to relations indicating retraction
            update_to = work.get('update-to', [])
            for update in update_to:
                update_type = update.get('type', '').lower()
                if update_type in ['retraction', 'removal']:
                    return {
                        'retracted': True,
                        'retraction_doi': update.get('DOI'),
                        'retraction_date': update.get('updated', {}).get('date-time'),
                        'retraction_type': update.get('type', 'Retraction').title(),
                        'error': None
                    }

            # Also check the relation field for retractions
            relation = work.get('relation', {})
            is_retracted_by = relation.get('is-retracted-by', [])
            if is_retracted_by:
                retraction = is_retracted_by[0]
                return {
                    'retracted': True,
                    'retraction_doi': retraction.get('id'),
                    'retraction_date': None,
                    'retraction_type': 'Retraction',
                    'error': None
                }

            # Check for expression of concern
            has_expression_of_concern = relation.get('has-expression-of-concern', [])
            if has_expression_of_concern:
                concern = has_expression_of_concern[0]
                return {
                    'retracted': True,
                    'retraction_doi': concern.get('id'),
                    'retraction_date': None,
                    'retraction_type': 'Expression of Concern',
                    'error': None
                }

            return {'retracted': False, 'error': None}

        elif response.status_code == 404:
            # DOI not found in CrossRef - can't check retraction status
            return {'retracted': False, 'error': None}
        else:
            return {'retracted': False, 'error': f'CrossRef lookup failed: HTTP {response.status_code}'}

    except requests.exceptions.Timeout:
        return {'retracted': False, 'error': 'Retraction check timed out'}
    except requests.exceptions.RequestException as e:
        return {'retracted': False, 'error': f'Retraction check failed: {e}'}


def check_retraction_by_title(title):
    """Check if a paper has been retracted by searching CrossRef by title.

    Searches CrossRef's retraction database (includes Retraction Watch) by title.
    Uses fuzzy matching to verify the found paper matches the reference.

    Returns a dict with:
        - retracted: True if paper has been retracted
        - retraction_doi: DOI of the retraction notice (if available)
        - original_doi: DOI of the original retracted paper
        - retraction_date: Date of retraction (if available)
        - retraction_type: Type of notice (Retraction, Expression of Concern, etc.)
        - error: Error message (if lookup failed)
    """
    if not title or len(title) < 10:
        return {'retracted': False, 'error': None}

    # Search CrossRef for retracted papers matching this title
    # We search papers that have update-type:retraction (papers that HAVE retractions)
    encoded_title = urllib.parse.quote(title)
    url = f"https://api.crossref.org/works?query.title={encoded_title}&filter=has-update:true&rows=5"
    headers = {
        "User-Agent": "HallucinatedReferenceChecker/1.0 (mailto:hallucination-checker@example.com)"
    }

    try:
        response = requests.get(url, headers=headers, timeout=get_timeout())

        if response.status_code == 200:
            data = response.json()
            items = data.get('message', {}).get('items', [])

            ref_norm = normalize_title(title)

            for item in items:
                item_title = item.get('title', [''])[0] if isinstance(item.get('title'), list) else item.get('title', '')
                if not item_title:
                    continue

                item_norm = normalize_title(item_title)

                # Check for fuzzy title match (95% threshold)
                similarity = fuzz.ratio(ref_norm, item_norm)
                if similarity >= 95:
                    # Found a matching paper - check if it's retracted
                    update_to = item.get('update-to', [])
                    for update in update_to:
                        update_type = update.get('type', '').lower()
                        if update_type in ['retraction', 'removal']:
                            return {
                                'retracted': True,
                                'original_doi': item.get('DOI'),
                                'retraction_doi': update.get('DOI'),
                                'retraction_date': update.get('updated', {}).get('date-time') if isinstance(update.get('updated'), dict) else None,
                                'retraction_type': update.get('type', 'Retraction').title(),
                                'error': None
                            }

                    # Check relation field
                    relation = item.get('relation', {})
                    is_retracted_by = relation.get('is-retracted-by', [])
                    if is_retracted_by:
                        retraction = is_retracted_by[0]
                        return {
                            'retracted': True,
                            'original_doi': item.get('DOI'),
                            'retraction_doi': retraction.get('id'),
                            'retraction_date': None,
                            'retraction_type': 'Retraction',
                            'error': None
                        }

                    # Check for expression of concern
                    has_expression_of_concern = relation.get('has-expression-of-concern', [])
                    if has_expression_of_concern:
                        concern = has_expression_of_concern[0]
                        return {
                            'retracted': True,
                            'original_doi': item.get('DOI'),
                            'retraction_doi': concern.get('id'),
                            'retraction_date': None,
                            'retraction_type': 'Expression of Concern',
                            'error': None
                        }

            return {'retracted': False, 'error': None}

        elif response.status_code == 404:
            return {'retracted': False, 'error': None}
        else:
            return {'retracted': False, 'error': f'CrossRef search failed: HTTP {response.status_code}'}

    except requests.exceptions.Timeout:
        return {'retracted': False, 'error': 'Retraction search timed out'}
    except requests.exceptions.RequestException as e:
        return {'retracted': False, 'error': f'Retraction search failed: {e}'}


def check_doi_match(doi_result, ref_title, ref_authors):
    """Check if DOI metadata matches the reference.

    Returns a dict with:
        - status: 'verified' | 'title_mismatch' | 'author_mismatch' | 'invalid'
        - message: Human-readable description
        - doi_title: Title from DOI (if valid)
        - doi_authors: Authors from DOI (if valid)
    """
    if not doi_result['valid']:
        return {
            'status': 'invalid',
            'message': doi_result['error'],
            'doi_title': None,
            'doi_authors': []
        }

    doi_title = doi_result['title']
    doi_authors = doi_result['authors']

    # Check title match using fuzzy matching
    ref_norm = normalize_title(ref_title)
    doi_norm = normalize_title(doi_title)

    # Multiple matching strategies:
    # 1. Full fuzzy match (for identical or nearly identical titles)
    title_ratio = fuzz.ratio(ref_norm, doi_norm)

    # 2. Check if DOI title is a prefix of reference title
    #    (DOI metadata often has just main title without subtitle)
    #    Require at least 8 chars to avoid false positives on very short titles
    is_prefix = ref_norm.startswith(doi_norm) and len(doi_norm) >= 8

    # 3. Partial ratio - good for when one string contains the other
    partial_ratio = fuzz.partial_ratio(ref_norm, doi_norm)

    # 4. Check if reference starts with DOI title (handles "FlowDroid: subtitle" vs "FlowDroid")
    #    100% partial match means DOI title is fully contained in reference
    is_contained_prefix = (
        partial_ratio == 100 and
        len(doi_norm) >= 8 and
        ref_norm.startswith(doi_norm)
    )

    # 5. Handle short tool/project names like "ReCon: Subtitle" vs "ReCon"
    #    If DOI title exactly matches the part before a colon in ref title, it's the tool name
    is_tool_name_match = False
    if len(doi_norm) >= 4 and partial_ratio == 100 and ':' in ref_title:
        # Extract part before colon in reference title and normalize
        ref_before_colon = ref_title.split(':')[0].strip()
        ref_before_colon_norm = normalize_title(ref_before_colon)
        # Check if DOI title matches the tool name part exactly
        if ref_before_colon_norm == doi_norm:
            is_tool_name_match = True

    # Consider it a match if:
    # - Full ratio >= 95% (nearly identical), OR
    # - DOI title is a prefix of ref title (at least 8 chars), OR
    # - DOI title is fully contained at the start of ref title, OR
    # - Partial ratio >= 95% AND DOI title is reasonably long (>= 20 chars normalized), OR
    # - DOI title matches the tool/project name before a colon (short names like "ReCon")
    title_match = (
        title_ratio >= 95 or
        is_prefix or
        is_contained_prefix or
        (partial_ratio >= 95 and len(doi_norm) >= 20) or
        is_tool_name_match
    )

    if not title_match:
        return {
            'status': 'title_mismatch',
            'message': f'DOI points to different paper: "{doi_title[:60]}..."' if len(doi_title) > 60 else f'DOI points to different paper: "{doi_title}"',
            'doi_title': doi_title,
            'doi_authors': doi_authors
        }

    # Check author match
    if ref_authors and doi_authors:
        if validate_authors(ref_authors, doi_authors):
            return {
                'status': 'verified',
                'message': 'DOI verified',
                'doi_title': doi_title,
                'doi_authors': doi_authors
            }
        else:
            return {
                'status': 'author_mismatch',
                'message': 'DOI title matches but authors differ',
                'doi_title': doi_title,
                'doi_authors': doi_authors
            }

    # No authors to compare, but title matches
    return {
        'status': 'verified',
        'message': 'DOI verified (title match)',
        'doi_title': doi_title,
        'doi_authors': doi_authors
    }


def extract_arxiv_id(text):
    """Extract arXiv ID from reference text.

    Handles formats like:
    - arXiv:2301.12345
    - arXiv:2301.12345v1
    - arxiv.org/abs/2301.12345
    - arXiv:hep-th/9901001 (old format)
    - arXiv preprint arXiv:2301.12345

    Also handles IDs split across lines (common in PDFs).

    Returns the arXiv ID string (e.g., "2301.12345") or None if not found.
    """
    # Fix IDs split across lines
    # e.g., "arXiv:2301.\n12345" -> "arXiv:2301.12345"
    text_fixed = re.sub(r'(arXiv:\d{4}\.)\s*\n\s*(\d+)', r'\1\2', text, flags=re.IGNORECASE)
    # e.g., "arxiv.org/abs/2301.\n12345" -> "arxiv.org/abs/2301.12345"
    text_fixed = re.sub(r'(arxiv\.org/abs/\d{4}\.)\s*\n\s*(\d+)', r'\1\2', text_fixed, flags=re.IGNORECASE)

    # New format: YYMM.NNNNN (with optional version)
    # e.g., arXiv:2301.12345, arXiv:2301.12345v2
    new_format = re.search(r'arXiv[:\s]+(\d{4}\.\d{4,5}(?:v\d+)?)', text_fixed, re.IGNORECASE)
    if new_format:
        return new_format.group(1)

    # URL format: arxiv.org/abs/YYMM.NNNNN
    url_format = re.search(r'arxiv\.org/abs/(\d{4}\.\d{4,5}(?:v\d+)?)', text_fixed, re.IGNORECASE)
    if url_format:
        return url_format.group(1)

    # Old format: category/YYMMNNN (e.g., hep-th/9901001)
    old_format = re.search(r'arXiv[:\s]+([a-z-]+/\d{7}(?:v\d+)?)', text_fixed, re.IGNORECASE)
    if old_format:
        return old_format.group(1)

    # URL old format
    url_old_format = re.search(r'arxiv\.org/abs/([a-z-]+/\d{7}(?:v\d+)?)', text_fixed, re.IGNORECASE)
    if url_old_format:
        return url_old_format.group(1)

    return None


def validate_arxiv(arxiv_id):
    """Validate an arXiv ID by querying the arXiv API and return metadata.

    Returns a dict with:
        - valid: True if arXiv ID resolves
        - title: Paper title from arXiv metadata (if valid)
        - authors: List of author names (if valid)
        - error: Error message (if invalid)
    """
    if not arxiv_id:
        return {'valid': False, 'error': 'No arXiv ID provided'}

    url = f"https://export.arxiv.org/api/query?id_list={arxiv_id}"
    headers = {
        "User-Agent": "HallucinatedReferenceChecker/1.0"
    }

    try:
        response = requests.get(url, headers=headers, timeout=get_timeout())

        if response.status_code == 200:
            try:
                # Parse XML response
                import xml.etree.ElementTree as ET
                root = ET.fromstring(response.content)

                # Define namespace
                ns = {
                    'atom': 'http://www.w3.org/2005/Atom',
                    'arxiv': 'http://arxiv.org/schemas/atom'
                }

                # Find entry
                entry = root.find('atom:entry', ns)
                if entry is None:
                    return {'valid': False, 'error': 'arXiv ID not found'}

                # Check if it's an error response (no title or "Error" in id)
                entry_id = entry.find('atom:id', ns)
                if entry_id is not None and 'Error' in entry_id.text:
                    return {'valid': False, 'error': 'arXiv ID not found'}

                title_elem = entry.find('atom:title', ns)
                if title_elem is None or not title_elem.text:
                    return {'valid': False, 'error': 'arXiv ID not found'}

                title = title_elem.text.strip()
                # Clean up title (remove newlines, extra spaces)
                title = ' '.join(title.split())

                authors = []
                for author in entry.findall('atom:author', ns):
                    name_elem = author.find('atom:name', ns)
                    if name_elem is not None and name_elem.text:
                        authors.append(name_elem.text.strip())

                return {
                    'valid': True,
                    'title': title,
                    'authors': authors,
                    'error': None
                }
            except ET.ParseError as e:
                return {'valid': False, 'error': f'Failed to parse arXiv response: {e}'}
        elif response.status_code == 429:
            return {'valid': False, 'error': 'Rate limited (429)'}
        else:
            return {'valid': False, 'error': f'arXiv lookup failed: HTTP {response.status_code}'}

    except requests.exceptions.Timeout:
        return {'valid': False, 'error': 'arXiv lookup timed out'}
    except requests.exceptions.RequestException as e:
        return {'valid': False, 'error': f'arXiv lookup failed: {e}'}


def check_arxiv_match(arxiv_result, ref_title, ref_authors):
    """Check if arXiv metadata matches the reference.

    Returns a dict with:
        - status: 'verified' | 'title_mismatch' | 'author_mismatch' | 'invalid'
        - message: Human-readable description
        - arxiv_title: Title from arXiv (if valid)
        - arxiv_authors: Authors from arXiv (if valid)
    """
    if not arxiv_result['valid']:
        return {
            'status': 'invalid',
            'message': arxiv_result['error'],
            'arxiv_title': None,
            'arxiv_authors': []
        }

    arxiv_title = arxiv_result['title']
    arxiv_authors = arxiv_result['authors']

    # Check title match using fuzzy matching (same logic as DOI)
    ref_norm = normalize_title(ref_title)
    arxiv_norm = normalize_title(arxiv_title)

    title_ratio = fuzz.ratio(ref_norm, arxiv_norm)
    is_prefix = ref_norm.startswith(arxiv_norm) and len(arxiv_norm) >= 8
    partial_ratio = fuzz.partial_ratio(ref_norm, arxiv_norm)
    is_contained_prefix = (
        partial_ratio == 100 and
        len(arxiv_norm) >= 8 and
        ref_norm.startswith(arxiv_norm)
    )

    # Handle short tool/project names
    is_tool_name_match = False
    if len(arxiv_norm) >= 4 and partial_ratio == 100 and ':' in ref_title:
        ref_before_colon = ref_title.split(':')[0].strip()
        ref_before_colon_norm = normalize_title(ref_before_colon)
        if ref_before_colon_norm == arxiv_norm:
            is_tool_name_match = True

    title_match = (
        title_ratio >= 95 or
        is_prefix or
        is_contained_prefix or
        (partial_ratio >= 95 and len(arxiv_norm) >= 20) or
        is_tool_name_match
    )

    if not title_match:
        return {
            'status': 'title_mismatch',
            'message': f'arXiv ID points to different paper: "{arxiv_title[:60]}..."' if len(arxiv_title) > 60 else f'arXiv ID points to different paper: "{arxiv_title}"',
            'arxiv_title': arxiv_title,
            'arxiv_authors': arxiv_authors
        }

    # Check author match
    if ref_authors and arxiv_authors:
        if validate_authors(ref_authors, arxiv_authors):
            return {
                'status': 'verified',
                'message': 'arXiv ID verified',
                'arxiv_title': arxiv_title,
                'arxiv_authors': arxiv_authors
            }
        else:
            return {
                'status': 'author_mismatch',
                'message': 'arXiv ID title matches but authors differ',
                'arxiv_title': arxiv_title,
                'arxiv_authors': arxiv_authors
            }

    # No authors to compare, but title matches
    return {
        'status': 'verified',
        'message': 'arXiv ID verified (title match)',
        'arxiv_title': arxiv_title,
        'arxiv_authors': arxiv_authors
    }


# Common compound-word suffixes that should keep the hyphen
COMPOUND_SUFFIXES = {
    'centered', 'based', 'driven', 'aware', 'oriented', 'specific', 'related',
    'dependent', 'independent', 'like', 'free', 'friendly', 'rich', 'poor',
    'scale', 'level', 'order', 'class', 'type', 'style', 'wise', 'fold',
    'shot', 'step', 'time', 'world', 'source', 'domain', 'task', 'modal',
    'intensive', 'efficient', 'agnostic', 'invariant', 'sensitive', 'grained',
    'agent', 'site',
}


def fix_hyphenation(text):
    """Fix hyphenation from PDF line breaks while preserving compound words.

    - 'detec- tion' or 'detec-\\ntion' → 'detection' (syllable break)
    - 'human- centered' or 'human-\\ncentered' → 'human-centered' (compound word)
    """
    def replace_hyphen(match):
        before = match.group(1)  # character before hyphen
        after_char = match.group(2)  # first character after hyphen
        after_rest = match.group(3)  # rest of word after hyphen

        after_word = after_char + after_rest
        # If the word after hyphen is a common compound suffix, keep the hyphen
        after_lower = after_word.lower()
        for suffix in COMPOUND_SUFFIXES:
            if after_lower == suffix or after_lower.startswith(suffix + ' ') or after_lower.startswith(suffix + ','):
                return f'{before}-{after_word}'
        # Check if the full word matches a suffix
        if after_lower.rstrip('.,;:') in COMPOUND_SUFFIXES:
            return f'{before}-{after_word}'
        # Otherwise, it's likely a syllable break - remove hyphen
        return f'{before}{after_word}'

    # Fix hyphen followed by space or newline, capturing the full word after
    text = re.sub(r'(\w)-\s+(\w)(\w*)', replace_hyphen, text)
    text = re.sub(r'(\w)- (\w)(\w*)', replace_hyphen, text)
    return text


def expand_ligatures(text):
    """Expand common typographic ligatures found in PDFs."""
    ligatures = {
        '\ufb00': 'ff',   # ﬀ
        '\ufb01': 'fi',   # ﬁ
        '\ufb02': 'fl',   # ﬂ
        '\ufb03': 'ffi',  # ﬃ
        '\ufb04': 'ffl',  # ﬄ
        '\ufb05': 'st',   # ﬅ (long s + t)
        '\ufb06': 'st',   # ﬆ
    }
    for lig, expanded in ligatures.items():
        text = text.replace(lig, expanded)
    return text


def extract_text_from_pdf(pdf_path):
    """Extract text from PDF using PyMuPDF."""
    import fitz
    doc = fitz.open(pdf_path)
    text = "\n".join(page.get_text() for page in doc)
    doc.close()
    # Expand typographic ligatures (ﬁ → fi, ﬂ → fl, etc.)
    text = expand_ligatures(text)
    return text


def find_references_section(text):
    """Locate the references section in the document text."""
    # Common reference section headers
    headers = [
        r'\n\s*References\s*\n',
        r'\n\s*REFERENCES\s*\n',
        r'\n\s*Bibliography\s*\n',
        r'\n\s*BIBLIOGRAPHY\s*\n',
        r'\n\s*Works Cited\s*\n',
    ]

    for pattern in headers:
        match = re.search(pattern, text, re.IGNORECASE)
        if match:
            ref_start = match.end()
            # Find end markers (Appendix, Acknowledgments, Ethics, etc.)
            # Also detect single-letter appendix markers (A Proofs, B Methods)
            # and TOC dot leaders (. . . . . page numbers)
            # End markers with appropriate flags
            # Case-insensitive for keywords, case-sensitive for appendix letter pattern
            end_markers_icase = [
                r'\n\s*(?:Appendix|Acknowledge?ments?|Supplementary|Ethics\s+Statement|Ethical\s+Considerations|Broader\s+Impact|Paper\s+Checklist|Checklist|Contents)\b',
                r'(?:\.\s*){5,}',  # TOC dot leaders (5+ dots with optional spaces)
            ]
            # Case-sensitive: single letter + capitalized word (A Proofs, B Methods)
            # Must NOT use IGNORECASE or it will match "A dataset..." etc.
            end_markers_case = [
                r'\n\s*[A-Z]\s+[A-Z][a-zA-Z-]+(?:\s+[a-zA-Z-]+)*\s*\n',
            ]
            ref_end = len(text)
            for end_marker in end_markers_icase:
                end_match = re.search(end_marker, text[ref_start:], re.IGNORECASE)
                if end_match:
                    ref_end = min(ref_end, ref_start + end_match.start())
            for end_marker in end_markers_case:
                end_match = re.search(end_marker, text[ref_start:])
                if end_match:
                    ref_end = min(ref_end, ref_start + end_match.start())

            ref_text = text[ref_start:ref_end]
            # Strip running headers (common in ACM papers)
            ref_text = strip_running_headers(ref_text)
            return ref_text

    # Fallback: try last 30% of document
    cutoff = int(len(text) * 0.7)
    return strip_running_headers(text[cutoff:])


def strip_running_headers(text):
    """Remove running headers that appear at page boundaries in references.

    ACM papers have headers like:
    - "CONFERENCE 'YY, Month DD–DD, YYYY, City, Country"
    - "Paper Title Here"
    - "A. Author, B. Author, and C. Author"

    These get mixed into references when they span page boundaries.
    """
    # Pattern for ACM-style venue headers
    # e.g., "ASIA CCS '26, June 01–05, 2026, Bangalore, India"
    # Matches: CONF_NAME 'YY, Month DD–DD, YYYY, Location
    # Note: Uses Unicode right single quote (U+2019) and en-dash (U+2013)
    venue_pattern = r"^[A-Z][A-Z\s&]+\s*['\u2019]\d{2},\s+[A-Z][a-z]+\s+\d{1,2}[\u2013\-]+\d{1,2},\s+\d{4},\s+[A-Z][A-Za-z\s,]+$"

    # Pattern for abbreviated author headers
    # e.g., "O.A Akanji, M. Egele, and G. Stringhini"
    author_header_pattern = r'^[A-Z]\.?[A-Z]?\s+[A-Z][a-z]+(?:,\s+[A-Z]\.?\s*[A-Z]?\.?\s*[A-Z][a-z]+)*(?:,?\s+and\s+[A-Z]\.?\s*[A-Z]?\.?\s*[A-Z][a-z]+)?$'

    lines = text.split('\n')
    filtered_lines = []
    i = 0

    while i < len(lines):
        line = lines[i].strip()

        # Check if this line matches venue pattern
        if re.match(venue_pattern, line):
            # Skip this line and check adjacent lines for paper title/authors
            # Check previous line (might be paper title)
            if filtered_lines and len(filtered_lines[-1].strip()) > 20:
                prev_line = filtered_lines[-1].strip()
                # If previous line looks like a title (not a reference continuation), remove it
                if not re.match(r'^\[\d+\]', prev_line) and not re.match(r'^\d+\.', prev_line):
                    # Check if it's not a normal sentence (titles usually don't end with period followed by venue)
                    if not prev_line.endswith('.') or 'doi:' not in lines[i-1] if i > 0 else True:
                        filtered_lines.pop()
            # Skip the venue line
            i += 1
            continue

        # Check if this line matches author header pattern
        if re.match(author_header_pattern, line) and len(line) < 100:
            # This is likely a running header with authors, skip it
            i += 1
            continue

        filtered_lines.append(lines[i])
        i += 1

    return '\n'.join(filtered_lines)


def segment_references(ref_text):
    """Split references section into individual references."""
    # Try IEEE style: [1], [2], etc.
    ieee_pattern = r'\n\s*\[(\d+)\]\s*'
    ieee_matches = list(re.finditer(ieee_pattern, ref_text))

    if len(ieee_matches) >= 3:
        refs = []
        for i, match in enumerate(ieee_matches):
            start = match.end()
            end = ieee_matches[i + 1].start() if i + 1 < len(ieee_matches) else len(ref_text)
            ref_content = ref_text[start:end].strip()
            if ref_content:
                refs.append(ref_content)
        return refs

    # Try alphabetic citation keys: [ACGH20], [CCY20], etc. (common in crypto/theory papers)
    # Pattern: uppercase letters (author initials) followed by 2-4 digits (year)
    # Also handles lowercase variants like [ABC+20] or [ABCea20]
    alpha_cite_pattern = r'\n\s*\[([A-Za-z+]+\d{2,4}[a-z]?)\]\s*'
    alpha_matches = list(re.finditer(alpha_cite_pattern, ref_text))

    if len(alpha_matches) >= 3:
        refs = []
        for i, match in enumerate(alpha_matches):
            start = match.end()
            end = alpha_matches[i + 1].start() if i + 1 < len(alpha_matches) else len(ref_text)
            ref_content = ref_text[start:end].strip()
            if ref_content:
                refs.append(ref_content)
        return refs

    # Try numbered list style: 1., 2., etc.
    # Validate that numbers are sequential starting from 1 (not years like 2019. or page numbers)
    # Use (?:^|\n) to also match at start of string (reference 1 has no preceding newline)
    numbered_pattern = r'(?:^|\n)\s*(\d+)\.\s+'
    numbered_matches = list(re.finditer(numbered_pattern, ref_text))

    if len(numbered_matches) >= 3:
        # Check if first few numbers look like sequential reference numbers (1, 2, 3...)
        first_nums = [int(m.group(1)) for m in numbered_matches[:5]]
        is_sequential = first_nums[0] == 1 and all(
            first_nums[i] == first_nums[i-1] + 1 for i in range(1, len(first_nums))
        )
        if is_sequential:
            refs = []
            for i, match in enumerate(numbered_matches):
                start = match.end()
                end = numbered_matches[i + 1].start() if i + 1 < len(numbered_matches) else len(ref_text)
                ref_content = ref_text[start:end].strip()
                if ref_content:
                    refs.append(ref_content)
            return refs

    # Try AAAI/ACM author-year style: "Surname, I.; ... Year. Title..."
    # Each reference starts with a surname (capitalized word, possibly hyphenated or two-part)
    # followed by comma and author initial(s)
    # Pattern matches: "Avalle, M.", "Camacho-collados, J.", "Del Vicario, M.", "Van Bavel, J."
    # Must be preceded by period+newline (end of previous reference) to avoid matching
    # author names that wrap to new lines mid-reference
    # Match after: lowercase letter, digit, closing paren, or 2+ uppercase letters (venue abbrevs like CSCW, CHI)
    # Single uppercase letter excluded to avoid matching author initials like "A."
    # (?!In\s) negative lookahead excludes "In Surname, I." which indicates editors, not new reference
    # Group 1 captures the prefix char(s) so we can include them in the previous reference
    # (?:\d{1,4}\n)? handles page/reference numbers on their own line between references
    # \s* after optional page number handles extra whitespace/newlines (e.g., column breaks)
    # Primary pattern: personal authors (unicode-aware for diacritics)
    aaai_pattern = r'([a-z0-9)]|[A-Z]{2})\.\n(?:\d{1,4}\n)?\s*(?!In\s)([A-Z][a-zA-Z\u00C0-\u024F]+(?:[ -][A-Za-z\u00C0-\u024F]+)?,\s+[A-Z]\.)'
    # Secondary pattern: organization authors (e.g., "European Union. 2022a.")
    aaai_org_pattern = r'([a-z0-9)]|[A-Z]{2})\.\n(?:\d{1,4}\n)?\s*(?!In\s)([A-Z][a-zA-Z\u00C0-\u024F]+(?:\s+[A-Z][a-zA-Z\u00C0-\u024F]+)+\.\s+\d{4}[a-z]?\.)'
    aaai_matches = list(re.finditer(aaai_pattern, ref_text))
    aaai_org_matches = list(re.finditer(aaai_org_pattern, ref_text))

    # Merge boundaries from both patterns, sort, deduplicate within 10 chars
    all_aaai = aaai_matches + aaai_org_matches
    if all_aaai:
        all_aaai.sort(key=lambda m: m.start())
        merged = [all_aaai[0]]
        for m in all_aaai[1:]:
            if m.start() - merged[-1].start() > 10:
                merged.append(m)
        all_aaai = merged

    if len(all_aaai) >= 3:
        refs = []
        # Handle first reference (before first match) - starts at beginning of ref_text
        # end(1) includes the consumed prefix char(s) in the previous reference
        first_ref = ref_text[:all_aaai[0].end(1)].strip()
        if first_ref and len(first_ref) > 20:
            refs.append(first_ref)
        # Handle remaining references
        for i, match in enumerate(all_aaai):
            start = match.start(2)  # Start at the author name (group 2)
            end = all_aaai[i + 1].end(1) if i + 1 < len(all_aaai) else len(ref_text)
            ref_content = ref_text[start:end].strip()
            if ref_content:
                refs.append(ref_content)
        return refs

    # Try Springer/Nature style: "Surname I, Surname I, ... (Year) Title"
    # Authors use format: Surname Initial (no comma/period between surname and initial)
    # e.g., "Abrahao S, Grundy J, Pezze M, et al (2025) Software Engineering..."
    # Each reference starts on a new line with author name and has (year) within first ~100 chars
    # Split by finding lines that look like reference starts
    lines = ref_text.split('\n')
    ref_starts = []
    current_pos = 0

    for i, line in enumerate(lines):
        # Check if line looks like a reference start:
        # - Starts with capital letter (author surname or organization)
        # - Contains (YYYY) or (YYYYa) pattern within reasonable distance
        # - Not just a page number
        if (line and
            re.match(r'^[A-Z]', line) and
            not re.match(r'^\d+$', line.strip()) and
            re.search(r'\(\d{4}[a-z]?\)', line)):
            ref_starts.append(current_pos)
        current_pos += len(line) + 1  # +1 for newline

    if len(ref_starts) >= 5:
        refs = []
        for i, start in enumerate(ref_starts):
            end = ref_starts[i + 1] if i + 1 < len(ref_starts) else len(ref_text)
            ref_content = ref_text[start:end].strip()
            # Remove trailing page number if present (standalone number at end)
            ref_content = re.sub(r'\n+\d+\s*$', '', ref_content).strip()
            if ref_content and len(ref_content) > 20:
                refs.append(ref_content)
        return refs

    # Try economics/math style: ", YYYY.\nAuthorName" (year at end, no parentheses)
    # e.g., "...pages 619–636, 2015.\nDaron Acemoglu, Ali Makhdoumi..."
    # Pattern: ends with ", YYYY." or "), YYYY." then new line starts with author name
    # Author pattern: FirstName LastName, FirstName LastName, ... or single capitalized name
    econ_pattern = r'[,)]\s*\d{4}[a-z]?\.\n+([A-Z][a-zA-Z\u00C0-\u024F]+(?:[ -][A-Za-z\u00C0-\u024F]+)*[,\s]+(?:[A-Z]\.?\s*)?[A-Z][a-zA-Z\u00C0-\u024F-]+)'
    econ_matches = list(re.finditer(econ_pattern, ref_text))

    if len(econ_matches) >= 5:
        refs = []
        # First reference: from start to first match
        first_ref = ref_text[:econ_matches[0].start() + econ_matches[0].group().index('\n')].strip()
        # Include up to the period after year
        period_pos = first_ref.rfind('.')
        if period_pos > 0:
            first_ref = first_ref[:period_pos + 1].strip()
        if first_ref and len(first_ref) > 20:
            refs.append(first_ref)
        # Remaining references: from author name to next match
        for i, match in enumerate(econ_matches):
            start = match.start(1)  # Start at the author name (group 1)
            if i + 1 < len(econ_matches):
                end_match = econ_matches[i + 1]
                end = end_match.start() + end_match.group().index('\n') + 1
            else:
                end = len(ref_text)
            ref_content = ref_text[start:end].strip()
            # Remove trailing page numbers
            ref_content = re.sub(r'\n+\d+\s*$', '', ref_content).strip()
            if ref_content and len(ref_content) > 20:
                refs.append(ref_content)
        return refs

    # Fallback: split by double newlines
    paragraphs = re.split(r'\n\s*\n', ref_text)
    return [p.strip() for p in paragraphs if p.strip() and len(p.strip()) > 20]


def extract_authors_from_reference(ref_text):
    """Extract author names from a reference string.

    Handles three main formats:
    - IEEE: "J. Smith, A. Jones, and C. Williams, "Title...""
    - ACM: "FirstName LastName, FirstName LastName, and FirstName LastName. Year."
    - USENIX: "FirstName LastName and FirstName LastName. Title..."

    Returns a list of author names, or the special value ['__SAME_AS_PREVIOUS__']
    if the reference uses em-dashes to indicate same authors as previous entry.
    """
    authors = []

    # Clean up the text - normalize whitespace
    ref_text = re.sub(r'\s+', ' ', ref_text).strip()

    # Check for em-dash pattern meaning "same authors as previous"
    if re.match(r'^[\u2014\u2013\-]{2,}\s*,', ref_text):
        return ['__SAME_AS_PREVIOUS__']

    # Determine where authors section ends based on format

    # IEEE format: authors end at quoted title
    quote_match = re.search(r'["\u201c\u201d]', ref_text)

    # Springer/Nature format: authors end before "(Year)" pattern
    # e.g., "Al Madi N (2023) How Readable..."
    springer_year_match = re.search(r'\s+\((\d{4}[a-z]?)\)\s+', ref_text)

    # ACM format: authors end before ". Year." pattern
    acm_year_match = re.search(r'\.\s*((?:19|20)\d{2})\.\s*', ref_text)

    # USENIX/default: authors end at first "real" period (not after initials like "M." or "J.")
    # Find period followed by space and a word that's not a single capital (another initial)
    first_period = -1
    for match in re.finditer(r'\. ', ref_text):
        pos = match.start()
        # Check what comes before the period - if it's a single capital letter, it's an initial
        if pos > 0:
            char_before = ref_text[pos-1]
            # Check if char before is a single capital (and the char before that is space or start)
            if char_before.isupper() and (pos == 1 or not ref_text[pos-2].isalpha()):
                # This is likely an initial like "M." or "J." - skip it
                continue
        first_period = pos
        break

    # Determine author section based on format detection
    author_end = len(ref_text)

    if quote_match:
        # IEEE format - quoted title
        author_end = quote_match.start()
    elif springer_year_match:
        # Springer/Nature format - "(Year)" after authors
        author_end = springer_year_match.start()
    elif acm_year_match:
        # ACM format - period before year
        author_end = acm_year_match.start() + 1
    elif first_period > 0:
        # USENIX format - first sentence is authors
        author_end = first_period

    author_section = ref_text[:author_end].strip()

    # Remove trailing punctuation
    author_section = re.sub(r'[\.,;:]+$', '', author_section).strip()

    if not author_section:
        return []

    # Check if this is AAAI format (semicolon-separated: "Surname, I.; Surname, I.; and Surname, I.")
    if '; ' in author_section and re.search(r'[A-Z][a-z]+,\s+[A-Z]\.', author_section):
        # AAAI format - split by semicolon
        author_section = re.sub(r';\s+and\s+', '; ', author_section, flags=re.IGNORECASE)
        parts = [p.strip() for p in author_section.split(';') if p.strip()]
        for part in parts:
            # Each part is "Surname, Initials" like "Bail, C. A."
            part = part.strip()
            if part and len(part) > 2 and re.search(r'[A-Z]', part):
                # Convert "Surname, I. M." to a cleaner form for matching
                # Keep as-is since validate_authors normalizes anyway
                authors.append(part)
        return authors[:15]

    # Normalize "and" and "&"
    author_section = re.sub(r',?\s+and\s+', ', ', author_section, flags=re.IGNORECASE)
    author_section = re.sub(r'\s*&\s*', ', ', author_section)

    # Remove "et al."
    author_section = re.sub(r',?\s*et\s+al\.?', '', author_section, flags=re.IGNORECASE)

    # Parse names - split by comma
    parts = [p.strip() for p in author_section.split(',') if p.strip()]

    for part in parts:
        if len(part) < 2:
            continue
        # Skip if it contains numbers (probably not an author)
        if re.search(r'\d', part):
            continue

        # Skip if it has too many words (names are typically 2-4 words)
        words = part.split()
        if len(words) > 5:
            continue

        # Skip if it looks like a sentence/title (has lowercase words that aren't prepositions)
        lowercase_words = [w for w in words if w[0].islower() and w not in ('and', 'de', 'van', 'von', 'la', 'del', 'di')]
        if len(lowercase_words) > 1:
            continue

        # Check if it looks like a name
        if re.search(r'[A-Z]', part) and re.search(r'[a-z]', part):
            name = part.strip()
            if name and len(name) > 2:
                authors.append(name)

    return authors[:15]


def clean_title(title, from_quotes=False):
    """Clean extracted title by removing trailing venue/metadata."""
    if not title:
        return ""

    # Fix hyphenation from PDF line breaks (preserves compound words like "human-centered")
    title = fix_hyphenation(title)

    # If title came from quotes, still apply venue cutoff patterns (quotes may include venue info)
    # but skip the sentence-truncation logic (which doesn't apply to quoted titles)

    # For non-quoted titles, truncate at first sentence-ending period
    # Skip periods that are part of abbreviations (e.g., "U.S." has short segments)
    if not from_quotes:
        for match in re.finditer(r'\.', title):
            pos = match.start()
            # Find start of segment (after last period or space, whichever is later)
            last_period = title.rfind('.', 0, pos)
            last_space = title.rfind(' ', 0, pos)
            segment_start = max(last_period + 1, last_space + 1, 0)
            segment = title[segment_start:pos]
            # If segment > 2 chars, it's likely a real sentence end, not an abbreviation
            # Also treat 2-char ALL-CAPS segments as sentence ends (acronyms like "AI.", "ML.")
            # but not mixed-case abbreviations like "vs.", "al.", "Jr."
            if len(segment) > 2 or (len(segment) == 2 and segment.isupper()):
                # But skip if period is immediately followed by a letter (no space) - product names like "big.LITTLE", "Node.js"
                if pos + 1 < len(title) and title[pos + 1].isalpha():
                    continue
                title = title[:pos]
                break

    # Also handle "? In" and "? In:" patterns for question-ending titles (Elsevier uses "In:")
    in_venue_match = re.search(r'\?\s*[Ii]n:?\s+(?:[A-Z]|[12]\d{3}\s)', title)
    if in_venue_match:
        title = title[:in_venue_match.start() + 1]  # Keep the question mark

    # Handle "? Journal Name, vol(" pattern (question-ending title leaking into journal)
    q_journal_match = re.search(r'[?!]\s+[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014\-]+,\s*\d+\s*[(:]', title)
    if q_journal_match:
        title = title[:q_journal_match.start() + 1]  # Keep the ?/!

    # Remove trailing journal/venue info that might have been included
    cutoff_patterns = [
        r'\.\s*[Ii]n:\s+[A-Z].*$',  # Elsevier ". In: Proceedings" or ". In: IFIP"
        r'\.\s*[Ii]n\s+[A-Z].*$',  # Standard ". In Proceedings"
        r'[.?!]\s*(?:Proceedings|Conference|Workshop|Symposium|IEEE|ACM|USENIX|AAAI|EMNLP|NAACL|arXiv|Available|CoRR|PACM[- ]\w+).*$',
        r'[.?!]\s*(?:Advances\s+in|Journal\s+of|Transactions\s+of|Transactions\s+on|Communications\s+of).*$',
        r'[.?!]\s+International\s+Journal\b.*$',  # "? International Journal" or ". International Journal"
        r'\.\s*[A-Z][a-z]+\s+(?:Journal|Review|Transactions|Letters|advances|Processing|medica|Intelligenz)\b.*$',
        r'\.\s*(?:Patterns|Data\s+&\s+Knowledge).*$',
        r'[.,]\s+[A-Z][a-z]+\s+\d+[,\s].*$',  # ". Word Number" or ", Word Number" (journal format like ". Science 344,")
        r',\s*volume\s+\d+.*$',  # ", volume 15"
        r',\s*\d+\s*\(\d+\).*$',  # Volume(issue) pattern
        r',\s*\d+\s*$',  # Trailing volume number
        r'\.\s*\d+\s*$',  # Trailing number after period
        r'\.\s*https?://.*$',  # URLs
        r'\.\s*ht\s*tps?://.*$',  # Broken URLs
        r',\s*(?:vol\.|pp\.|pages).*$',
        r'\.\s*Data\s+in\s+brief.*$',
        r'\.\s*Biochemia\s+medica.*$',
        r'\.\s*KI-Künstliche.*$',
        r'\s+arXiv\s+preprint.*$',  # "arXiv preprint arXiv:..."
        r'\s+arXiv:\d+.*$',  # "arXiv:2503..."
        r'\s+CoRR\s+abs/.*$',  # "CoRR abs/1234.5678"
        r',?\s*(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+(?:19|20)\d{2}.*$',  # "June 2024"
        r'[.,]\s*[Aa]ccessed\s+.*$',  # ", Accessed July 23, 2020" (URL access date)
        r'\s*\(\d+[–\-]\d*\)\s*$',  # Trailing page numbers in parens: "(280–28)" or "(280-289)"
        r'\s*\(pp\.?\s*\d+[–\-]\d*\)\s*$',  # "(pp. 280-289)" or "(pp 280–289)"
        r',?\s+\d+[–\-]\d+\s*$',  # Trailing page range: ", 280-289" or " 280–289"
        r'\.\s*[A-Z][a-zA-Z]+(?:\s+(?:in|of|on|and|for|the|a|an|&|[A-Z]?[a-zA-Z]+))+,\s*\d+\s*[,:]\s*\d+[–\-]?\d*.*$',  # ". Journal Name, vol: pages" like ". Computers in Human Behavior, 61: 280–28"
        r'\.\s*[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014-]+\d+\s*[(,:]\s*\d+[–\-]?\d*.*$',  # ". Journal Name vol(pages" with extended chars
        r'\.\s*[A-Z][a-zA-Z\s]+[&+]\s*[A-Z].*$',  # ". Words & More" or ". Words + More" (standalone journal names ending with &/+)
    ]

    for pattern in cutoff_patterns:
        title = re.sub(pattern, '', title, flags=re.IGNORECASE)

    title = title.strip()
    title = re.sub(r'[.,;:]+$', '', title)

    return title.strip()


# Abbreviations that should NEVER be sentence boundaries (mid-title abbreviations)
MID_SENTENCE_ABBREVIATIONS = {'vs', 'eg', 'ie', 'cf', 'fig', 'figs', 'eq', 'eqs', 'sec', 'ch', 'pt', 'no'}

# Abbreviations that ARE sentence boundaries when followed by a capitalized content word
# (e.g., "et al." followed by a title)
END_OF_AUTHOR_ABBREVIATIONS = {'al'}

def split_sentences_skip_initials(text):
    """Split text into sentences, but skip periods that are author initials (e.g., 'M.' 'J.') or mid-sentence abbreviations (e.g., 'vs.')."""
    sentences = []
    current_start = 0

    for match in re.finditer(r'\.\s+', text):
        pos = match.start()
        # Check if this period follows a single capital letter (author initial)
        if pos > 0:
            char_before = text[pos-1]
            # If char before is a single capital (and char before that is space/start), it might be an initial
            if char_before.isupper() and (pos == 1 or not text[pos-2].isalpha()):
                # Check what comes AFTER this period to determine if it's really an initial
                # If followed by "Capitalized lowercase" (title pattern), it's a sentence boundary
                # If followed by "Capitalized," or "Capitalized Capitalized," (author pattern), it's an initial
                after_period = text[match.end():]
                # Look at the pattern after the period
                # Author pattern: Capitalized word followed by comma or another capitalized word then comma
                # Surnames can be hyphenated (Aldana-Iuit), have accents (Sánchez), or apostrophes (O'Brien)
                # Also match Elsevier author pattern: "Surname Initial," like "Smith J," or "Smith JK,"
                # Also match "and Surname" pattern for author lists like "J. and Jones, M."
                # Also match another initial "X." or "X.-Y." for IEEE format like "H. W. Chung"
                surname_char = r"[a-zA-Z\u00A0-\u017F''`´\u2018\u2019\-]"  # Letters, accents, apostrophes (including curly quotes U+2018/U+2019), backticks, hyphens
                # Lowercase surname prefixes common in German, Dutch, Spanish, Portuguese, French, Italian names
                surname_prefix = r'(?:von|van|de|del|della|la|le|da|das|dos|der|den|ter|di|du|el|af|ten|op|zum|zur)'
                author_pattern = re.match(rf'^([A-Z]{surname_char}+)\s*,', after_period) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\s+([A-Z][A-Z]?)\s*,', after_period) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\s+[A-Z]{{1,2}},', after_period) or \
                                 re.match(r'^and\s+[A-Z]', after_period, re.IGNORECASE) or \
                                 re.match(r'^[A-Z]\.', after_period) or \
                                 re.match(r'^[A-Z]\.-[A-Z]\.', after_period) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\.\s+[A-Z]', after_period) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\s+and\s+[A-Z]', after_period, re.IGNORECASE) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\s+([A-Z]{surname_char}+)\s*,', after_period) or \
                                 re.match(rf'^{surname_prefix}\s+[A-Z]', after_period, re.IGNORECASE) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\s+([A-Z]{surname_char}+)\.', after_period) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\.\s+\d', after_period) or \
                                 re.match(rf'^([A-Z]{surname_char}+)\.\s+[A-Z][a-z]+\s+[a-z]', after_period)

                if author_pattern:
                    # This clearly looks like another author - skip this period
                    continue
                # Otherwise (title-like or uncertain pattern), treat as sentence boundary
                # This handles titles starting with proper nouns like "Facebook FAIR's..."

            # Check if this period follows a common abbreviation
            # Find the word before the period
            word_start = pos - 1
            while word_start > 0 and text[word_start-1].isalpha():
                word_start -= 1
            word_before = text[word_start:pos].lower()

            # Mid-sentence abbreviations are never sentence boundaries
            if word_before in MID_SENTENCE_ABBREVIATIONS:
                continue  # Skip this period - it's a mid-sentence abbreviation

            # "et al." is a sentence boundary (ends author list)
            # Don't skip it - let it be treated as a sentence boundary

        # This is a real sentence boundary
        sentences.append(text[current_start:pos].strip())
        current_start = match.end()

    # Add the remaining text as the last sentence
    if current_start < len(text):
        sentences.append(text[current_start:].strip())

    return sentences


def extract_title_from_reference(ref_text):
    """Extract title from a reference string.

    Handles three main formats:
    - IEEE: Authors, "Title," in Venue, Year.
    - ACM: Authors. Year. Title. In Venue.
    - USENIX: Authors. Title. In/Journal Venue, Year.

    Returns: (title, from_quotes) tuple where from_quotes indicates if title was in quotes.
    """
    # Fix hyphenation from PDF line breaks (preserves compound words like "human-centered")
    ref_text = fix_hyphenation(ref_text)
    ref_text = re.sub(r'\s+', ' ', ref_text).strip()

    # === Format 1: IEEE/USENIX - Quoted titles or titles with quoted portions ===
    # Handles: "Full Title" or "Quoted part": Subtitle
    quote_patterns = [
        r'["\u201c\u201d]([^"\u201c\u201d]+)["\u201c\u201d]',  # Smart quotes (any combo)
        r'"([^"]+)"',  # Regular quotes
        r'[\u2018]([^\u2018\u2019]{10,})[\u2019]',  # Smart single quotes (Harvard/APA)
        r"(?:^|[\s(])'([^']{10,})'(?:\s*[,.]|\s*$)",  # Plain single quotes with delimiters
    ]

    for pattern in quote_patterns:
        match = re.search(pattern, ref_text)
        if match:
            quoted_part = match.group(1).strip()
            after_quote = ref_text[match.end():].strip()

            # IEEE format: comma inside quotes ("Title,") means title is complete
            # What follows is venue/journal, not a subtitle - skip subtitle detection
            if quoted_part.endswith(','):
                if len(quoted_part.split()) >= 3:
                    return quoted_part, True
                continue  # Try next quote pattern

            # Check if there's a subtitle after the quote
            # Can start with : or - or directly with a capital letter
            if after_quote:
                # Determine if there's a subtitle and extract it
                subtitle_text = None
                if after_quote[0] in ':-':
                    subtitle_text = after_quote[1:].strip()
                elif after_quote[0].isupper():
                    # Subtitle starts directly with capital letter (no delimiter)
                    subtitle_text = after_quote

                if subtitle_text:
                    # Find where subtitle ends at venue/year markers
                    end_patterns = [
                        r'\.\s*[Ii]n\s+',           # ". In "
                        r'\.\s*(?:Proc|IEEE|ACM|USENIX|NDSS|CCS|AAAI|WWW|CHI|arXiv)',
                        r',\s*[Ii]n\s+',            # ", in "
                        r'\.\s*\((?:19|20)\d{2}\)', # ". (2022)" style venue year
                        r'[,\.]\s*(?:19|20)\d{2}',  # year
                        r'\s+(?:19|20)\d{2}\.',     # year at end
                        r'[.,]\s+[A-Z][a-z]+\s+\d+[,\s]',  # ". Word Number" journal format (". Science 344,")
                        r'\.\s*[A-Z][a-zA-Z]+(?:\s+(?:in|of|on|and|for|the|a|an|&|[A-Za-z]+))+,\s*\d+\s*[,:]',  # ". Journal Name, vol:" like ". Computers in Human Behavior, 61:"
                    ]
                    subtitle_end = len(subtitle_text)
                    for ep in end_patterns:
                        m = re.search(ep, subtitle_text)
                        if m:
                            subtitle_end = min(subtitle_end, m.start())

                    subtitle = subtitle_text[:subtitle_end].strip()
                    subtitle = re.sub(r'[.,;:]+$', '', subtitle)
                    if subtitle and len(subtitle.split()) >= 2:
                        title = f'{quoted_part}: {subtitle}'
                        return title, True

            # No subtitle - just use quoted part if long enough
            if len(quoted_part.split()) >= 3:
                return quoted_part, True

    # === Format 1b: LNCS/Springer - "Authors, I.: Title. In: Venue" ===
    # Pattern: Authors end with initial + colon, then title
    # Example: "Allix, K., Bissyandé, T.F.: Androzoo: Collecting millions. In: Proceedings"
    # Example: "Paulson, L.C.: Extending sledgehammer. Journal of..."
    # Example: "Klein, G., et al.: sel4: Formal verification. In: Proceedings"
    # The colon after author initials marks the start of the title
    # Match: comma/space + Initial(s) + colon (not just any word + colon)
    # Handles: X.: or X.Y.: or X.-Y.: or X.Y.Z.: (multiple consecutive initials)
    # Also handles: "et al.:" pattern
    lncs_match = re.search(r'(?:[,\s][A-Z]\.(?:[-–]?[A-Z]\.)*|et\s+al\.)\s*:\s*(.+)', ref_text)
    if lncs_match:
        after_colon = lncs_match.group(1).strip()
        # Find where title ends - at ". In:" or ". In " or journal patterns or (Year)
        title_end_patterns = [
            r'\.\s*[Ii]n:\s+',           # ". In: " (LNCS uses colon)
            r'\.\s*[Ii]n\s+[A-Z]',       # ". In Proceedings"
            r'\.\s*(?:Proceedings|IEEE|ACM|USENIX|NDSS|arXiv)',
            r'\.\s*(?:Journal|Transactions|Review|Advances)\s+(?:of|on|in)\s+',  # ". Journal of X"
            r'\.\s*[A-Z][a-zA-Z\s]+(?:Access|Journal|Review|Transactions)',  # "X Journal" ending
            r'\.\s*[A-Z][a-z]+\s+\d+\s*\(',  # ". Nature 123(" - journal with volume
            r'\.\s*https?://',           # URL follows title
            r'\.\s*pp?\.\s*\d+',         # Page numbers
            r'\s+\((?:19|20)\d{2}\)\s*[,.]?\s*(?:https?://|$)',  # (Year) followed by URL or end
            r'\s+\((?:19|20)\d{2}\)\s*,',  # (Year) followed by comma
        ]
        title_end = len(after_colon)
        for pattern in title_end_patterns:
            m = re.search(pattern, after_colon)
            if m:
                title_end = min(title_end, m.start())

        title = after_colon[:title_end].strip()
        title = re.sub(r'\.\s*$', '', title)
        if len(title.split()) >= 3:
            return title, False

    # === Format 1c: Organization/Documentation - "Organization: Title (Year), URL" ===
    # Pattern: Organization name at START followed by colon, then title
    # Example: "Android Developer: Define custom permissions (2024), https://..."
    # Only match at start of reference to avoid matching mid-title colons
    org_match = re.match(r'^([A-Z][a-zA-Z\s]+):\s*(.+)', ref_text)
    if org_match:
        after_colon = org_match.group(2).strip()
        # Find where title ends - at (Year) followed by URL or comma
        title_end_patterns = [
            r'\s+\((?:19|20)\d{2}\)\s*[,.]?\s*(?:https?://|$)',  # (Year) followed by URL or end
            r'\s+\((?:19|20)\d{2}\)\s*,',  # (Year) followed by comma
            r'\.\s*https?://',           # URL follows title
        ]
        title_end = len(after_colon)
        for pattern in title_end_patterns:
            m = re.search(pattern, after_colon)
            if m:
                title_end = min(title_end, m.start())

        title = after_colon[:title_end].strip()
        title = re.sub(r'\.\s*$', '', title)
        # Allow 2-word titles for this format (documentation titles can be short)
        if len(title.split()) >= 2:
            return title, False

    # === Format 2a: Springer/Nature/Harvard - "Authors (Year) Title" or "Authors (Year). Title" ===
    # Pattern: "Surname I, ... (YYYY) Title text. Journal Name Vol(Issue):Pages"
    # Also handles Harvard/APA: "Surname, I. (YYYY). Title. Venue."
    # Year is in parentheses, optionally followed by period, then title
    springer_year_match = re.search(r'\((\d{4}[a-z]?)\)\.?\s+', ref_text)
    if springer_year_match:
        after_year = ref_text[springer_year_match.end():]
        # Find where title ends - at journal/venue patterns
        title_end_patterns = [
            r'\.\s*[Ii]n:\s+',  # ". In: " (Springer uses colon)
            r'\.\s*[Ii]n\s+[A-Z]',  # ". In Proceedings"
            r'\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)',
            r'\.\s*[A-Z][a-zA-Z\s]+\d+\s*\(\d+\)',  # ". Journal Name 34(5)" - journal with volume
            r'\.\s*[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014]+\d+:\d+',  # ". Journal Name 34:123" - journal with pages
            r'\.\s*[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014-]+,\s*\d+',  # ". Journal Name, 11" or ". PACM-HCI, 4"
            r'[?!]\s+[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014-]+,\s*\d+\s*[(:]',  # "? Journal Name, vol(" - cut after ?/!
            r'\.\s*https?://',  # URL follows title
            r'\.\s*URL\s+',  # URL follows title
            r'\.\s*Tech\.\s*rep\.',  # Technical report
            r'\.\s*pp?\.\s*\d+',  # Page numbers
        ]
        title_end = len(after_year)
        for pattern in title_end_patterns:
            m = re.search(pattern, after_year)
            if m:
                # For ?/! patterns, keep the punctuation in the title (cut after it)
                if m.group(0)[0] in '?!':
                    title_end = min(title_end, m.start() + 1)
                else:
                    title_end = min(title_end, m.start())

        title = after_year[:title_end].strip()
        title = re.sub(r'\.\s*$', '', title)
        if len(title.split()) >= 3:
            return title, False  # from_quotes=False

    # === Format 2b: ACM - "Authors. Year. Title. In Venue" ===
    # Pattern: ". YYYY. Title-text. In "
    # Use \s+ after year to avoid matching DOIs like "10.1109/CVPR.2022.001234"
    acm_match = re.search(r'\.\s*((?:19|20)\d{2})\.\s+', ref_text)
    if acm_match:
        after_year = ref_text[acm_match.end():]
        # Find where title ends - at ". In " or at venue indicators
        title_end_patterns = [
            r'\.\s*[Ii]n\s+[A-Z]',  # ". In Proceedings"
            r'\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)',
            r'\.\s*[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014-]{10,},\s*\d+',  # ". Long Journal Name, vol" - long journal names
            r'[?!]\s+[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014-]+,\s*\d+\s*[(:]',  # "? Journal Name, vol(" - cut after ?/!
            r'\s+doi:',
        ]
        title_end = len(after_year)
        for pattern in title_end_patterns:
            m = re.search(pattern, after_year)
            if m:
                # For ?/! patterns, keep the punctuation in the title (cut after it)
                if m.group(0)[0] in '?!':
                    title_end = min(title_end, m.start() + 1)
                else:
                    title_end = min(title_end, m.start())

        title = after_year[:title_end].strip()
        title = re.sub(r'\.\s*$', '', title)
        if len(title.split()) >= 3:
            return title, False  # from_quotes=False

    # === Format 3: USENIX/ICML/NeurIPS/Elsevier - "Authors. Title. In Venue" or "Authors. Title. In: Venue" ===
    # Find venue markers and extract title before them
    # Order matters: more specific patterns first, generic patterns last
    venue_patterns = [
        r'\.\s*[Ii]n:\s+(?:Proceedings|Workshop|Conference|Symposium|IFIP|IEEE|ACM)',  # Elsevier "In:" format
        r'\.\s*[Ii]n:\s+[A-Z]',  # Elsevier generic "In:" format
        r'\.\s*[Ii]n\s+(?:Proceedings|Workshop|Conference|Symposium|AAAI|IEEE|ACM|USENIX)',
        r'\.\s*[Ii]n\s+[A-Z][a-z]+\s+(?:Conference|Workshop|Symposium)',
        r'\.\s*[Ii]n\s+(?:The\s+)?(?:\w+\s+)+(?:International\s+)?(?:Conference|Workshop|Symposium)',  # ICML/NeurIPS style
        r'\.\s*(?:NeurIPS|ICML|ICLR|CVPR|ICCV|ECCV|AAAI|IJCAI|CoRR|JMLR),',  # Common ML venue abbreviations
        r'\.\s*arXiv\s+preprint',  # arXiv preprint format
        r'\.\s*[Ii]n\s+[A-Z]',  # Generic ". In X" fallback
        r',\s*(?:19|20)\d{2}\.\s*(?:URL|$)',  # Year followed by URL or end - arXiv style (last resort)
        r',\s*(?:19|20)\d{2}\.\s*$',  # Journal format ending with year (last resort)
    ]

    for vp in venue_patterns:
        venue_match = re.search(vp, ref_text)
        if venue_match:
            before_venue = ref_text[:venue_match.start()].strip()

            # First try: Split into sentences using period boundaries
            # This works well for IEEE and many other formats: "Authors. Title. Venue"
            parts = split_sentences_skip_initials(before_venue)
            if len(parts) >= 2:
                title = parts[1].strip()
                title = re.sub(r'\.\s*$', '', title)
                if len(title.split()) >= 3:
                    # Verify it doesn't look like authors (Name Name, pattern)
                    if not re.match(r'^[A-Z][a-z]+\s+[A-Z][a-z]+,', title):
                        return title, False

            # Second try: For ICML/NeurIPS style where authors and title are in same "sentence"
            # Look for author initial pattern followed by title: "and LastName, I. TitleWords"
            author_end_pattern = r'(?:,\s+[A-Z]\.(?:[-\s]+[A-Z]\.)*|(?:Jr|Sr|III|II|IV)\.)\s+(.)'
            all_matches = list(re.finditer(author_end_pattern, before_venue))

            for match in reversed(all_matches):
                title_start = match.start(1)
                remaining = before_venue[title_start:]

                # Skip if this looks like start of another author: "X.," or "Lastname,"
                if re.match(r'^[A-Z]\.,', remaining) or re.match(r'^[A-Z][a-z]+,', remaining):
                    continue

                title = remaining.strip()
                title = re.sub(r'\.\s*$', '', title)
                if len(title.split()) >= 3:
                    # Verify it doesn't look like authors
                    if not re.match(r'^[A-Z][a-z]+,\s+[A-Z]\.', title):
                        return title, False
                break

            break

    # === Format 4: Journal - "Authors. Title. Journal Name, Vol(Issue), Year" ===
    # Look for journal patterns
    journal_match = re.search(r'\.\s*([A-Z][^.]+(?:Journal|Review|Transactions|Letters|Magazine|Science|Nature|Processing|Advances)[^.]*),\s*(?:vol\.|Volume|\d+\(|\d+,)', ref_text, re.IGNORECASE)
    if journal_match:
        before_journal = ref_text[:journal_match.start()].strip()
        parts = split_sentences_skip_initials(before_journal)
        if len(parts) >= 2:
            title = parts[1].strip()
            if len(title.split()) >= 3:
                return title, False  # from_quotes=False

    # === Format 4b: Elsevier journal - "Authors. Title. Journal Year;Vol(Issue):Pages" ===
    # Example: "Narouei M, Takabi H. Title here. IEEE Trans Dependable Secure Comput 2018;17(3):506–17"
    # Also handles: "Yang L, Chen X. Title here. Secur Commun Netw 2021;2021." (year-only volume)
    # Pattern: Journal name followed by Year;Volume (with optional Issue and Pages)
    elsevier_journal_match = re.search(r'\.\s*([A-Z][A-Za-z\s]+)\s+(?:19|20)\d{2};\d+(?:\(\d+\))?', ref_text)
    if elsevier_journal_match:
        before_journal = ref_text[:elsevier_journal_match.start()].strip()
        parts = split_sentences_skip_initials(before_journal)
        if len(parts) >= 2:
            title = parts[-1].strip()  # Last sentence before journal is likely title
            title = re.sub(r'\.\s*$', '', title)
            if len(title.split()) >= 3:
                return title, False

    # === Format 5: ALL CAPS authors (e.g., "SURNAME, F., AND SURNAME, G. Title here.") ===
    # Only triggers if text starts with a multi-char ALL CAPS surname (not just initials like "H. W.")
    # Look for pattern: "SURNAME... [initial]. Title" where Title starts with capital
    if re.match(r'^[A-Z]{2,}', ref_text):
        # Find title start: period-space-Capital followed by lowercase word
        # Handles both "A title..." and "Title..." patterns
        title_start_match = re.search(r'\.\s+([A-Z][a-z]*\s+[a-z])', ref_text)
        if title_start_match:
            title_text = ref_text[title_start_match.start(1):]
            # Find title end at venue markers
            title_end_patterns = [
                r'\.\s*[Ii]n\s+[A-Z]',  # ". In Proceedings"
                r'\.\s*(?:Proceedings|IEEE|ACM|USENIX|NDSS|arXiv|Technical\s+report)',
                r'\.\s*[A-Z][a-z]+\s+\d+,\s*\d+\s*\(',  # ". Journal 55, 3 (2012)"
                r'\.\s*(?:Ph\.?D\.?\s+thesis|Master.s\s+thesis)',
            ]
            title_end = len(title_text)
            for pattern in title_end_patterns:
                m = re.search(pattern, title_text)
                if m:
                    title_end = min(title_end, m.start())

            if title_end > 0:
                title = title_text[:title_end].strip()
                title = re.sub(r'\.\s*$', '', title)
                if len(title.split()) >= 3:
                    return title, False

    # === Fallback: second sentence if it looks like a title ===
    # Use smart splitting that skips author initials like "M." "J."
    sentences = split_sentences_skip_initials(ref_text)
    if len(sentences) >= 2:
        # First sentence is likely authors, second might be title
        potential_title = sentences[1].strip()

        # Skip if it looks like authors
        words = potential_title.split()
        if words:
            # Count name-like patterns (Capitalized words)
            cap_words = sum(1 for w in words if re.match(r'^[A-Z][a-z]+$', w))
            # Count "and" conjunctions
            and_count = sum(1 for w in words if w.lower() == 'and')

            # If high ratio of cap words and "and", probably authors
            if len(words) > 0 and (cap_words / len(words) > 0.7) and and_count > 0:
                # Try third sentence
                if len(sentences) >= 3:
                    potential_title = sentences[2].strip()

        # Skip if starts with "In " (venue)
        if not re.match(r'^[Ii]n\s+', potential_title):
            if len(potential_title.split()) >= 3:
                return potential_title, False  # from_quotes=False

    return "", False


def extract_references_with_titles_and_authors(pdf_path, return_stats=False):
    """Extract references from PDF using pure Python (PyMuPDF).

    If return_stats=True, returns (references, stats_dict) where stats_dict contains:
        - total_raw: total raw references found
        - skipped_url: count skipped due to non-academic URLs
        - skipped_short_title: count skipped due to short/missing title
        - skipped_no_authors: count skipped due to missing authors
    """
    stats = {
        'total_raw': 0,
        'skipped_url': 0,
        'skipped_short_title': 0,
        'skipped_no_authors': 0,
    }

    try:
        text = extract_text_from_pdf(pdf_path)
    except Exception as e:
        print(f"[Error] Failed to extract text from PDF: {e}")
        return ([], stats) if return_stats else []

    ref_section = find_references_section(text)
    if not ref_section:
        print("[Error] Could not locate references section")
        return ([], stats) if return_stats else []

    raw_refs = segment_references(ref_section)
    stats['total_raw'] = len(raw_refs)

    references = []
    previous_authors = []

    for ref_text in raw_refs:
        # Extract DOI and arXiv ID BEFORE fixing hyphenation (they can contain hyphens/periods split across lines)
        doi = extract_doi(ref_text)
        arxiv_id = extract_arxiv_id(ref_text)

        # Remove standalone page/column numbers on their own lines (PDF layout artifacts)
        ref_text = re.sub(r'\n\d{1,4}\n', '\n', ref_text)

        # Fix hyphenation from PDF line breaks (preserves compound words like "human-centered")
        ref_text = fix_hyphenation(ref_text)

        # Skip entries with non-academic URLs (keep acm, ieee, usenix, arxiv, doi)
        # Also catch broken URLs with spaces like "https: //" or "ht tps://"
        if re.search(r'https?\s*:\s*//', ref_text) or re.search(r'ht\s*tps?\s*:\s*//', ref_text):
            if not re.search(r'(acm\.org|ieee\.org|usenix\.org|arxiv\.org|doi\.org)', ref_text, re.IGNORECASE):
                stats['skipped_url'] += 1
                continue

        title, from_quotes = extract_title_from_reference(ref_text)
        title = clean_title(title, from_quotes=from_quotes)
        if not title or len(title.split()) < 4:
            stats['skipped_short_title'] += 1
            continue

        authors = extract_authors_from_reference(ref_text)

        # Handle em-dash meaning "same authors as previous"
        if authors == ['__SAME_AS_PREVIOUS__']:
            if previous_authors:
                authors = previous_authors
            else:
                authors = []  # No previous authors to use

        if not authors:
            stats['skipped_no_authors'] += 1  # Track refs with no authors (but still check them)

        # Update previous_authors for potential next em-dash reference
        if authors:
            previous_authors = authors

        # Clean up ref_text for display: collapse whitespace, strip numbering prefix
        raw_citation = re.sub(r'\s+', ' ', ref_text).strip()
        raw_citation = re.sub(r'^\[\d+\]\s*', '', raw_citation)
        raw_citation = re.sub(r'^\d+\.\s*', '', raw_citation)

        references.append((title, authors, doi, arxiv_id, raw_citation))

    return (references, stats) if return_stats else references

# Common words to skip when building search queries
STOP_WORDS = {'a', 'an', 'the', 'of', 'and', 'or', 'for', 'to', 'in', 'on', 'with', 'by'}

def get_query_words(title, n=6):
    """Extract n significant words from title for query, skipping stop words and short words."""
    all_words = re.findall(r'[a-zA-Z0-9]+', title)
    # Skip stop words and words shorter than 3 characters (e.g., "s" from "Twitter's")
    def is_significant(w):
        if w.lower() in STOP_WORDS:
            return False
        # Keep words with 3+ chars, OR short alphanumeric terms like "L2", "3D", "AI", "5G"
        if len(w) >= 3:
            return True
        # Keep short words that mix letters and digits (technical terms)
        has_letter = any(c.isalpha() for c in w)
        has_digit = any(c.isdigit() for c in w)
        return has_letter and has_digit

    significant = [w for w in all_words if is_significant(w)]
    return significant[:n] if len(significant) >= 3 else all_words[:n]

def query_dblp(title):
    # Use first 6 significant words for query (skip stop words, special chars fail)
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://dblp.org/search/publ/api?q={urllib.parse.quote(query)}&format=json"
    try:
        response = requests.get(url, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception(f"Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")
        result = response.json()
        hits = result.get("result", {}).get("hits", {}).get("hit", [])
        for hit in hits:
            info = hit.get("info", {})
            found_title = info.get("title", "")
            if fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                authors = info.get("authors", {}).get("author", [])
                if isinstance(authors, dict):
                    authors = [authors.get("text", "")]
                else:
                    authors = [a.get("text", "") if isinstance(a, dict) else a for a in authors]
                paper_url = info.get("url")  # DBLP provides URL
                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] DBLP search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_arxiv(title):
    # Use first 6 significant words for query (skip stop words)
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"http://export.arxiv.org/api/query?search_query=all:{urllib.parse.quote(query)}&start=0&max_results=5"
    try:
        # feedparser doesn't support timeout directly, so we fetch with requests first
        response = requests.get(url, timeout=get_timeout())
        feed = feedparser.parse(response.content)
        for entry in feed.entries:
            entry_title = entry.title
            if fuzz.ratio(normalize_title(title), normalize_title(entry_title)) >= 95:
                authors = [author.name for author in entry.authors]
                paper_url = entry.link  # arXiv provides direct link
                return entry_title, authors, paper_url
    except Exception as e:
        print(f"[Error] arXiv search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_crossref(title):
    # Use first 6 significant words for query (skip stop words)
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://api.crossref.org/works?query.title={urllib.parse.quote(query)}&rows=5"
    try:
        response = requests.get(url, headers={"User-Agent": "Academic Reference Parser"}, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception(f"Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")
        results = response.json().get("message", {}).get("items", [])
        for item in results:
            found_title = item.get("title", [""])[0]
            if fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                authors = [f"{a.get('given', '')} {a.get('family', '')}".strip() for a in item.get("author", [])]
                doi = item.get("DOI")
                paper_url = f"https://doi.org/{doi}" if doi else None
                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] CrossRef search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_openalex(title, api_key):
    """Query OpenAlex API for paper information."""
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://api.openalex.org/works?filter=title.search:{urllib.parse.quote(query)}&api_key={api_key}"
    try:
        response = requests.get(url, headers={"User-Agent": "Academic Reference Parser"}, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception(f"Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")
        results = response.json().get("results", [])
        for item in results[:5]:  # Check top 5 results
            found_title = item.get("title", "")
            if found_title and fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                # Extract author names from authorships
                authorships = item.get("authorships", [])
                authors = []
                for authorship in authorships:
                    author_info = authorship.get("author", {})
                    display_name = author_info.get("display_name", "")
                    if display_name:
                        authors.append(display_name)
                # Get DOI URL or OpenAlex landing page
                doi = item.get("doi")
                paper_url = doi if doi else item.get("id")
                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] OpenAlex search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_neurips(title):
    """Query NeurIPS papers archive."""
    try:
        years = [2023, 2022, 2021, 2020, 2019, 2018]
        for year in years:
            search_url = f"https://papers.nips.cc/paper_files/paper/{year}/hash/index.html"
            response = requests.get(search_url, timeout=get_timeout())
            if response.status_code != 200:
                continue

            soup = BeautifulSoup(response.content, "html.parser")
            for a in soup.find_all("a"):
                if fuzz.ratio(normalize_title(title), normalize_title(a.text)) >= 95:
                    paper_url = "https://papers.nips.cc" + a['href']
                    paper_response = requests.get(paper_url, timeout=get_timeout())
                    if paper_response.status_code != 200:
                        return a.text.strip(), [], paper_url
                    author_soup = BeautifulSoup(paper_response.content, "html.parser")
                    authors = [tag.text.strip() for tag in author_soup.find_all("li", class_="author")]
                    return a.text.strip(), authors, paper_url
    except Exception as e:
        print(f"[Error] NeurIPS search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_acl(title):
    """Query ACL Anthology for paper information."""
    try:
        query = urllib.parse.quote(title)
        url = f"https://aclanthology.org/search/?q={query}"
        response = requests.get(url, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception(f"Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")
        soup = BeautifulSoup(response.text, 'html.parser')
        for entry in soup.select(".d-sm-flex.align-items-stretch.p-2"):
            entry_title_tag = entry.select_one("h5")
            if entry_title_tag and fuzz.ratio(normalize_title(title), normalize_title(entry_title_tag.text)) >= 95:
                author_tags = entry.select("span.badge.badge-light")
                authors = [a.text.strip() for a in author_tags]
                # Try to get paper URL from the entry
                link_tag = entry.select_one("a[href*='/papers/']")
                paper_url = f"https://aclanthology.org{link_tag['href']}" if link_tag else None
                return entry_title_tag.text.strip(), authors, paper_url
    except Exception as e:
        print(f"[Error] ACL Anthology search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_openreview(title):
    """Query OpenReview API for paper information."""
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://api2.openreview.net/notes/search?query={urllib.parse.quote(query)}&limit=20"
    try:
        response = requests.get(url, headers={"User-Agent": "Academic Reference Parser"}, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception(f"Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")
        results = response.json().get("notes", [])
        for item in results:
            content = item.get("content", {})
            # Handle both old and new OpenReview API formats
            found_title = content.get("title", {})
            if isinstance(found_title, dict):
                found_title = found_title.get("value", "")
            if found_title and fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                # Extract authors
                authors_field = content.get("authors", {})
                if isinstance(authors_field, dict):
                    authors = authors_field.get("value", [])
                else:
                    authors = authors_field if isinstance(authors_field, list) else []
                # Construct OpenReview URL from forum ID
                forum_id = item.get("forum") or item.get("id")
                paper_url = f"https://openreview.net/forum?id={forum_id}" if forum_id else None
                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] OpenReview search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_semantic_scholar(title, api_key=None):
    """Query Semantic Scholar API for paper information.

    Semantic Scholar aggregates papers from many sources including
    Academia.edu, SSRN, PubMed, and institutional repositories.

    Args:
        title: Paper title to search for
        api_key: Optional Semantic Scholar API key for higher rate limits
    """
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://api.semanticscholar.org/graph/v1/paper/search?query={urllib.parse.quote(query)}&limit=10&fields=title,authors,url"
    headers = {"User-Agent": "Academic Reference Parser"}
    if api_key:
        headers["x-api-key"] = api_key
    try:
        response = requests.get(url, headers=headers, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception(f"Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")
        results = response.json().get("data", [])
        for item in results:
            found_title = item.get("title", "")
            if found_title and fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                authors = [a.get("name", "") for a in item.get("authors", []) if a.get("name")]
                paper_url = item.get("url")  # Semantic Scholar provides URL
                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] Semantic Scholar search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None

def query_ssrn(title):
    """Query SSRN (Social Science Research Network) for paper information.

    SSRN hosts working papers and preprints in social sciences, economics,
    law, and humanities.
    """
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = "https://papers.ssrn.com/sol3/results.cfm"
    params = {'txtKey_Words': query}
    headers = {
        'User-Agent': 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36',
        'Accept': 'text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8',
        'Accept-Language': 'en-US,en;q=0.5',
    }
    try:
        response = requests.get(url, params=params, headers=headers, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception("Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")

        soup = BeautifulSoup(response.text, 'html.parser')
        # Find paper titles - they're in <a class="title"> tags
        title_links = soup.select('a.title')

        for link in title_links[:10]:  # Check first 10 results
            found_title = link.get_text().strip()
            if found_title and fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                # Extract paper URL from the link
                href = link.get('href', '')
                paper_url = href if href.startswith('http') else f"https://papers.ssrn.com{href}" if href else None

                # Try to find authors - they're typically in nearby elements
                authors = []
                parent = link.find_parent('div')
                if parent:
                    author_elem = parent.find('span', class_='authors')
                    if author_elem:
                        authors = [a.strip() for a in author_elem.get_text().split(',') if a.strip()]

                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] SSRN search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None


def titles_match(ref_title, found_title, threshold=95):
    """Check if two titles match, handling subtitles and truncation.

    Returns True if:
    - Fuzzy match score >= threshold, OR
    - One title is a prefix of the other (handles subtitles/truncation)
    """
    ref_norm = normalize_title(ref_title)
    found_norm = normalize_title(found_title)

    # Standard fuzzy match
    if fuzz.ratio(ref_norm, found_norm) >= threshold:
        return True

    # Check if one is a prefix of the other (handles subtitles)
    # Reference might be truncated, or database might have full title with subtitle
    min_len = min(len(ref_norm), len(found_norm))
    if min_len >= 30:  # Require reasonable length to avoid false positives
        # Check if shorter is prefix of longer
        shorter, longer = (ref_norm, found_norm) if len(ref_norm) <= len(found_norm) else (found_norm, ref_norm)
        if longer.startswith(shorter):
            return True

    return False


def query_europe_pmc(title):
    """Query Europe PMC for paper information.

    Europe PMC is a free database of life science literature with 42M+ abstracts.
    It mirrors PubMed/PMC and includes preprints, theses, and agricultural publications.
    Covers journals from SAGE, MDPI, Elsevier, Springer, and many others.
    API docs: https://europepmc.org/RestfulWebService
    """
    url = "https://www.ebi.ac.uk/europepmc/webservices/rest/search"

    # Clean title for search - remove special characters that might break query
    clean_title = re.sub(r'["\'\[\](){}:;]', ' ', title)
    clean_title = ' '.join(clean_title.split())  # Normalize whitespace

    # Use free-text search with the title - Europe PMC's ranking will prioritize
    # papers with matching titles, and we use fuzzy matching to verify
    params = {
        'query': clean_title[:100],  # Limit query length
        'format': 'json',
        'pageSize': 15,  # Get more results since free-text search is broader
    }

    try:
        response = requests.get(url, params=params, headers={"User-Agent": "Academic Reference Parser"}, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception("Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")

        data = response.json()
        results = data.get("resultList", {}).get("result", [])

        for item in results:
            found_title = item.get("title", "")
            if found_title and titles_match(title, found_title):
                # Extract authors from authorString (format: "Smith J, Jones A, ...")
                author_string = item.get("authorString", "")
                authors = [a.strip() for a in author_string.split(",") if a.strip()] if author_string else []

                # Get URL - prefer DOI, then PMCID, then PMID
                doi = item.get("doi")
                pmcid = item.get("pmcid")
                pmid = item.get("pmid")
                if doi:
                    paper_url = f"https://doi.org/{doi}"
                elif pmcid:
                    paper_url = f"https://europepmc.org/article/PMC/{pmcid}"
                elif pmid:
                    paper_url = f"https://europepmc.org/article/MED/{pmid}"
                else:
                    paper_url = None

                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] Europe PMC search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None


def query_pubmed(title):
    """Query PubMed via NCBI E-utilities for paper information.

    PubMed is the primary database for biomedical literature.
    API docs: https://www.ncbi.nlm.nih.gov/books/NBK25500/
    """
    # Clean title for search
    clean_title = re.sub(r'["\'\[\](){}:;]', ' ', title)
    clean_title = ' '.join(clean_title.split())

    # Step 1: Search for matching articles using title field search
    search_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esearch.fcgi"

    # Use significant words in title field search
    words = get_query_words(title, 6)
    query = ' '.join(words)
    search_params = {
        'db': 'pubmed',
        'term': f'{query}[Title]',
        'retmode': 'json',
        'retmax': 10,
    }
    try:
        response = requests.get(search_url, params=search_params, headers={"User-Agent": "Academic Reference Parser"}, timeout=get_timeout())
        if response.status_code == 429:
            raise Exception("Rate limited (429)")
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code}")

        data = response.json()
        id_list = data.get("esearchresult", {}).get("idlist", [])

        if not id_list:
            return None, [], None

        # Step 2: Fetch details for found articles
        fetch_url = "https://eutils.ncbi.nlm.nih.gov/entrez/eutils/esummary.fcgi"
        fetch_params = {
            'db': 'pubmed',
            'id': ','.join(id_list),
            'retmode': 'json',
        }
        response = requests.get(fetch_url, params=fetch_params, headers={"User-Agent": "Academic Reference Parser"}, timeout=get_timeout())
        if response.status_code != 200:
            raise Exception(f"HTTP {response.status_code} on fetch")

        data = response.json()
        results = data.get("result", {})

        for pmid in id_list:
            item = results.get(pmid, {})
            found_title = item.get("title", "")
            if found_title and titles_match(title, found_title):
                # Extract authors
                authors = []
                for author in item.get("authors", []):
                    name = author.get("name", "")
                    if name:
                        authors.append(name)

                # Build URL
                paper_url = f"https://pubmed.ncbi.nlm.nih.gov/{pmid}/"

                return found_title, authors, paper_url
    except Exception as e:
        print(f"[Error] PubMed search failed: {e}")
        raise  # Re-raise so failed_dbs gets tracked
    return None, [], None


def query_all_databases_concurrent(title, ref_authors, openalex_key=None, s2_api_key=None, longer_timeout=False, only_dbs=None, dblp_offline_path=None, check_openalex_authors=False, enabled_dbs=None):
    """Query all databases concurrently for a single reference.

    Args:
        title: Paper title to search for
        ref_authors: Authors from the reference
        openalex_key: Optional OpenAlex API key
        s2_api_key: Optional Semantic Scholar API key for higher rate limits
        longer_timeout: If True, use longer timeouts (for retries)
        only_dbs: If provided, only query these specific databases (for targeted retry)
        dblp_offline_path: Optional path to offline DBLP SQLite database
        enabled_dbs: If provided, only include these databases (set of canonical names).
                     None means all databases are enabled (backward compat).

    Returns a dict with:
        - status: 'verified' | 'not_found' | 'author_mismatch'
        - source: database name where found (if any)
        - found_authors: authors from the database (if found)
        - paper_url: URL to the paper (if found)
        - error_type: None | 'not_found' | 'author_mismatch'
        - failed_dbs: list of database names that failed/timed out
    """
    # Set timeout for this call (longer for retries)
    _timeout_local.timeout = DB_TIMEOUT_RETRY if longer_timeout else DB_TIMEOUT

    # Build the database list, only including databases in enabled_dbs.
    # enabled_dbs=None means all enabled (backward compat).
    # NOTE: OpenReview disabled due to API being unreachable after Nov 2025 security incident
    all_databases = []

    if enabled_dbs is None or 'CrossRef' in enabled_dbs:
        all_databases.append(('CrossRef', lambda: query_crossref(title)))
    if enabled_dbs is None or 'arXiv' in enabled_dbs:
        all_databases.append(('arXiv', lambda: query_arxiv(title)))
    if enabled_dbs is None or 'DBLP' in enabled_dbs:
        if dblp_offline_path:
            from dblp_offline import query_offline as query_dblp_offline
            all_databases.append(('DBLP (offline)', lambda: query_dblp_offline(title, dblp_offline_path)))
        else:
            all_databases.append(('DBLP', lambda: query_dblp(title)))
    if enabled_dbs is None or 'Semantic Scholar' in enabled_dbs:
        all_databases.append(('Semantic Scholar', lambda: query_semantic_scholar(title, s2_api_key)))
    if enabled_dbs is None or 'SSRN' in enabled_dbs:
        all_databases.append(('SSRN', lambda: query_ssrn(title)))
    if enabled_dbs is None or 'ACL Anthology' in enabled_dbs:
        all_databases.append(('ACL Anthology', lambda: query_acl(title)))
    if enabled_dbs is None or 'NeurIPS' in enabled_dbs:
        all_databases.append(('NeurIPS', lambda: query_neurips(title)))
    if enabled_dbs is None or 'Europe PMC' in enabled_dbs:
        all_databases.append(('Europe PMC', lambda: query_europe_pmc(title)))
    if enabled_dbs is None or 'PubMed' in enabled_dbs:
        all_databases.append(('PubMed', lambda: query_pubmed(title)))
    if (enabled_dbs is None or 'OpenAlex' in enabled_dbs) and openalex_key:
        all_databases.insert(0, ('OpenAlex', lambda: query_openalex(title, openalex_key)))

    # Filter to only requested DBs if specified (for targeted retry)
    if only_dbs:
        databases = [(name, func) for name, func in all_databases if name in only_dbs]
    else:
        databases = all_databases

    result = {
        'status': 'not_found',
        'source': None,
        'found_authors': [],
        'paper_url': None,
        'error_type': 'not_found',
        'failed_dbs': [],
    }

    # Track author mismatches to report if nothing better found
    first_mismatch = None
    failed_dbs = []

    def query_single_db(db_info):
        """Execute a single database query. Returns (name, found_title, found_authors, paper_url, error)."""
        name, query_func = db_info
        try:
            found_title, found_authors, paper_url = query_func()
            if found_title:
                logger.debug(f"    {name}: FOUND")
            else:
                logger.debug(f"    {name}: not found")
            return (name, found_title, found_authors, paper_url, None)
        except requests.exceptions.Timeout:
            logger.warning(f"    {name}: TIMEOUT")
            return (name, None, [], None, "timeout")
        except Exception as e:
            logger.warning(f"    {name}: ERROR - {str(e)[:50]}")
            return (name, None, [], None, str(e))

    # Use ThreadPoolExecutor to query databases concurrently
    with ThreadPoolExecutor(max_workers=8) as executor:
        # Submit all queries
        future_to_db = {executor.submit(query_single_db, db): db[0] for db in databases}

        for future in as_completed(future_to_db):
            db_name = future_to_db[future]
            try:
                name, found_title, found_authors, paper_url, error = future.result()

                if error:
                    failed_dbs.append(name)
                    continue

                if found_title:
                    # Check author match
                    if not ref_authors or validate_authors(ref_authors, found_authors):
                        # Found and verified - cancel remaining futures and return
                        for f in future_to_db:
                            f.cancel()
                        return {
                            'status': 'verified',
                            'source': name,
                            'found_authors': found_authors,
                            'paper_url': paper_url,
                            'error_type': None,
                            'failed_dbs': [],
                        }
                    else:
                        # Author mismatch - save first one but keep looking
                        # Skip OpenAlex mismatches unless explicitly enabled (they often have false positives)
                        if first_mismatch is None and (name != 'OpenAlex' or check_openalex_authors):
                            first_mismatch = {
                                'status': 'author_mismatch',
                                'source': name,
                                'found_authors': found_authors,
                                'paper_url': paper_url,
                                'error_type': 'author_mismatch',
                                'failed_dbs': [],
                            }
            except Exception:
                failed_dbs.append(db_name)

    # If we found the title but authors didn't match, report that
    if first_mismatch:
        return first_mismatch

    result['failed_dbs'] = failed_dbs
    return result


def validate_authors(ref_authors, found_authors):
    # Common surname prefixes (case-insensitive)
    SURNAME_PREFIXES = {'van', 'von', 'de', 'del', 'della', 'di', 'da', 'al', 'el', 'la', 'le', 'ben', 'ibn', 'mac', 'mc', 'o'}
    NAME_SUFFIXES = {'jr', 'sr', 'ii', 'iii', 'iv', 'v'}

    def get_surname_from_parts(parts):
        """Extract surname from name parts, handling multi-word surnames and suffixes."""
        if not parts:
            return ""
        # Strip name suffixes (Jr, Sr, III, etc.)
        while len(parts) >= 2 and parts[-1].lower().rstrip('.') in NAME_SUFFIXES:
            parts = parts[:-1]
        # Check if second-to-last part is a surname prefix
        # e.g., ['Jay', 'J.', 'Van', 'Bavel'] -> surname is 'Van Bavel'
        if len(parts) >= 2 and parts[-2].lower().rstrip('.') in SURNAME_PREFIXES:
            return ' '.join(parts[-2:])
        # Check for three-part surnames like "De La Cruz"
        if len(parts) >= 3 and parts[-3].lower().rstrip('.') in SURNAME_PREFIXES:
            return ' '.join(parts[-3:])
        return parts[-1]

    def normalize_author(name):
        # Handle AAAI "Surname, Initials" format (e.g., "Bail, C. A.")
        if ',' in name:
            parts = name.split(',')
            surname = parts[0].strip()
            initials = parts[1].strip() if len(parts) > 1 else ""
            # Get first initial letter
            first_initial = initials[0] if initials else ""
            return f"{first_initial} {surname.lower()}"

        parts = name.split()
        if not parts:
            return ""

        # Handle Springer "Surname Initial" format (e.g., "Abrahao S", "Van Bavel JJ")
        # Last part is initials if it's 1-2 uppercase letters (no periods)
        if len(parts) >= 2 and len(parts[-1]) <= 2 and parts[-1].isupper():
            surname = ' '.join(parts[:-1])  # Everything except last part
            initial = parts[-1][0]  # First letter of initials
            return f"{initial} {surname.lower()}"

        # Standard format: "FirstName LastName" or "FirstName MiddleName LastName"
        # Handle multi-word surnames like "Van Bavel", "Al Madi"
        surname = get_surname_from_parts(parts)
        first_initial = parts[0][0]
        return f"{first_initial} {surname.lower()}"

    def get_last_name(name):
        # Handle AAAI "Surname, Initials" format (e.g., "Bail, C. A.")
        if ',' in name:
            surname = name.split(',')[0].strip()
            return surname.lower()
        # Standard format: extract surname (may be multi-word like "Van Bavel")
        parts = name.split()
        if not parts:
            return ""
        return get_surname_from_parts(parts).lower()

    def has_first_name_or_initial(name):
        """Check if a name contains a first name or initial (not just a surname)."""
        name = name.strip()
        if not name:
            return False
        # "Surname, Initial" format has a first name/initial
        if ',' in name:
            parts = name.split(',')
            return len(parts) > 1 and parts[1].strip()
        parts = name.split()
        # Strip name suffixes (Jr, Sr, etc.) before analysis
        core_parts = [p for p in parts if p.lower().rstrip('.') not in NAME_SUFFIXES]
        if len(core_parts) <= 1:
            # Single word (+ optional suffix) - just a surname
            return False
        # Check if any part looks like an initial (single letter, possibly with period)
        for part in core_parts[:-1]:  # Exclude last part (likely surname)
            if len(part.rstrip('.')) == 1:
                return True  # Found an initial
        # Check for Elsevier/Springer "Surname Initial" format where initial is at the END
        # e.g., "Narouei M", "Cranor LF" - last part is 1-2 uppercase letters
        last = core_parts[-1]
        if len(last) <= 2 and last.isupper():
            return True  # Found initial at end (Elsevier format)
        # Check if first part looks like a first name (capitalized, 2+ chars, not a surname prefix)
        first = core_parts[0].rstrip('.')
        if len(first) >= 2 and first[0].isupper() and first.lower() not in SURNAME_PREFIXES:
            # Could be a first name like "John" or a surname prefix like "Van"
            # If second part is also capitalized and not a prefix, first is likely a first name
            if len(core_parts) >= 2:
                second = core_parts[1].rstrip('.')
                if len(second) >= 2 and second[0].isupper():
                    return True  # Likely "FirstName LastName"
        return False

    # Check if PDF-extracted authors are last-name-only (no first names or initials)
    # Use majority threshold: if most authors are single surnames, treat as last-name-only
    # (handles malformed fragments like "F. d" from broken author extraction)
    ref_authors_clean = [a for a in ref_authors if a.strip()]
    last_name_only_count = sum(1 for a in ref_authors_clean if not has_first_name_or_initial(a))
    ref_authors_are_last_name_only = last_name_only_count > len(ref_authors_clean) / 2

    if ref_authors_are_last_name_only:
        # Only compare last names
        ref_surnames = [get_last_name(a) for a in ref_authors if get_last_name(a)]
        found_surnames = [get_last_name(a) for a in found_authors if get_last_name(a)]

        # Check for matches, including partial surname matches
        # e.g., "Bavel" should match "Van Bavel"
        for ref_name in ref_surnames:
            for found_name in found_surnames:
                if ref_name == found_name:
                    return True
                # Check if one surname ends with the other (handles "Bavel" vs "Van Bavel")
                if found_name.endswith(ref_name) or ref_name.endswith(found_name):
                    return True
        return False
    else:
        ref_set = set(normalize_author(a) for a in ref_authors)
        found_set = set(normalize_author(a) for a in found_authors)
    return bool(ref_set & found_set)

def check_references(refs, sleep_time=1.0, openalex_key=None, s2_api_key=None, on_progress=None, max_concurrent_refs=4, dblp_offline_path=None, check_openalex_authors=False, enabled_dbs=None, cancel_event=None):
    """Check references against databases with concurrent queries.

    Args:
        refs: List of (title, authors, doi) tuples (doi may be None)
              Also accepts legacy (title, authors) tuples for backwards compatibility
        sleep_time: (unused, kept for API compatibility)
        openalex_key: Optional OpenAlex API key
        s2_api_key: Optional Semantic Scholar API key for higher rate limits
        on_progress: Optional callback function(event_type, data)
            event_type can be: 'checking', 'result', 'warning', 'retry_pass'
            data varies by event type
        max_concurrent_refs: Max number of references to check in parallel (default 4)
        dblp_offline_path: Optional path to offline DBLP SQLite database
        enabled_dbs: If provided, only query these databases (set of canonical names).
                     None means all databases are enabled.

    Returns:
        Tuple of (results, check_stats) where:
        - results: List of result dicts with title, ref_authors, status, source, found_authors, error_type, doi_info
        - check_stats: Dict with 'total_timeouts', 'retried_count', 'retry_successes'
    """
    import threading

    results = [None] * len(refs)  # Pre-allocate to maintain order
    # Track indices of "not found" results that had failed DBs for retry
    retry_candidates = []
    # Track DOIs that got 429 errors for retry
    doi_retry_candidates = []
    doi_retry_lock = threading.Lock()
    # Track arXiv IDs that got 429 errors for retry
    arxiv_retry_candidates = []
    arxiv_retry_lock = threading.Lock()
    # Track total timeout/failure count
    total_timeouts = 0
    timeouts_lock = threading.Lock()
    retry_lock = threading.Lock()

    def check_single_ref(i, title, ref_authors, doi=None, arxiv_id=None, raw_citation=None):
        """Check a single reference and return result."""
        nonlocal total_timeouts

        # Check for cancellation before starting
        if cancel_event and cancel_event.is_set():
            return

        # Notify progress: starting to check this reference
        if on_progress:
            on_progress('checking', {
                'index': i,
                'total': len(refs),
                'title': title,
            })

        # Validate DOI if present
        doi_info = None
        if doi:
            logger.debug(f"  Validating DOI: {doi}")
            doi_result = validate_doi(doi)

            # Check if DOI got rate limited - track for retry
            if not doi_result['valid'] and '429' in str(doi_result.get('error', '')):
                with doi_retry_lock:
                    doi_retry_candidates.append((i, doi, title, ref_authors))
                logger.info(f"  DOI rate limited, will retry: {doi}")
                # Mark as needing retry in doi_info
                doi_info = {
                    'doi': doi,
                    'status': 'invalid',
                    'message': 'DOI lookup rate limited (will retry)',
                    'doi_title': None,
                    'doi_authors': [],
                    'needs_retry': True,
                }
            else:
                doi_match = check_doi_match(doi_result, title, ref_authors)
                doi_info = {
                    'doi': doi,
                    'status': doi_match['status'],
                    'message': doi_match['message'],
                    'doi_title': doi_match.get('doi_title'),
                    'doi_authors': doi_match.get('doi_authors', []),
                }
                logger.debug(f"  DOI validation: {doi_match['status']} - {doi_match['message']}")

        # Check if paper has been retracted
        retraction_info = None
        retraction_result = None

        # First try DOI-based lookup (more reliable)
        if doi:
            logger.debug(f"  Checking retraction status for DOI: {doi}")
            retraction_result = check_retraction(doi)
            if retraction_result.get('retracted'):
                retraction_info = {
                    'retracted': True,
                    'doi': doi,
                    'retraction_doi': retraction_result.get('retraction_doi'),
                    'retraction_date': retraction_result.get('retraction_date'),
                    'retraction_type': retraction_result.get('retraction_type', 'Retraction'),
                }
                logger.info(f"  ⚠️  RETRACTED: {title[:50]}... ({retraction_info['retraction_type']})")
            elif retraction_result.get('error'):
                logger.debug(f"  Retraction check error: {retraction_result['error']}")

        # If no DOI or DOI check didn't find retraction, try title-based search
        if not retraction_info:
            logger.debug(f"  Checking retraction status by title: {title[:50]}...")
            retraction_result = check_retraction_by_title(title)
            if retraction_result.get('retracted'):
                retraction_info = {
                    'retracted': True,
                    'doi': retraction_result.get('original_doi') or doi,
                    'retraction_doi': retraction_result.get('retraction_doi'),
                    'retraction_date': retraction_result.get('retraction_date'),
                    'retraction_type': retraction_result.get('retraction_type', 'Retraction'),
                }
                logger.info(f"  ⚠️  RETRACTED (by title): {title[:50]}... ({retraction_info['retraction_type']})")
            elif retraction_result.get('error'):
                logger.debug(f"  Retraction title search error: {retraction_result['error']}")

        # Validate arXiv ID if present
        arxiv_info = None
        if arxiv_id:
            logger.debug(f"  Validating arXiv ID: {arxiv_id}")
            arxiv_result = validate_arxiv(arxiv_id)

            # Check if arXiv got rate limited - track for retry
            if not arxiv_result['valid'] and '429' in str(arxiv_result.get('error', '')):
                with arxiv_retry_lock:
                    arxiv_retry_candidates.append((i, arxiv_id, title, ref_authors))
                logger.info(f"  arXiv rate limited, will retry: {arxiv_id}")
                arxiv_info = {
                    'arxiv_id': arxiv_id,
                    'status': 'invalid',
                    'message': 'arXiv lookup rate limited (will retry)',
                    'arxiv_title': None,
                    'arxiv_authors': [],
                    'needs_retry': True,
                }
            else:
                arxiv_match = check_arxiv_match(arxiv_result, title, ref_authors)
                arxiv_info = {
                    'arxiv_id': arxiv_id,
                    'status': arxiv_match['status'],
                    'message': arxiv_match['message'],
                    'arxiv_title': arxiv_match.get('arxiv_title'),
                    'arxiv_authors': arxiv_match.get('arxiv_authors', []),
                }
                logger.debug(f"  arXiv validation: {arxiv_match['status']} - {arxiv_match['message']}")

        # Query all databases concurrently
        result = query_all_databases_concurrent(
            title, ref_authors,
            openalex_key=openalex_key,
            s2_api_key=s2_api_key,
            dblp_offline_path=dblp_offline_path,
            check_openalex_authors=check_openalex_authors,
            enabled_dbs=enabled_dbs
        )

        # Build full result record
        full_result = {
            'title': title,
            'raw_citation': raw_citation,
            'ref_authors': ref_authors,
            'status': result['status'],
            'source': result['source'],
            'found_authors': result['found_authors'],
            'paper_url': result.get('paper_url'),
            'error_type': result['error_type'],
            'failed_dbs': result.get('failed_dbs', []),
            'doi_info': doi_info,
            'arxiv_info': arxiv_info,
            'retraction_info': retraction_info,
        }

        # If DOI verified, use that as verification even if DB search failed
        if doi_info and doi_info['status'] == 'verified' and result['status'] == 'not_found':
            full_result['status'] = 'verified'
            full_result['source'] = 'DOI'
            full_result['error_type'] = None
            logger.info(f"  -> VERIFIED via DOI (DB search found nothing)")

        # If arXiv ID verified, use that as verification even if DB search failed
        if arxiv_info and arxiv_info['status'] == 'verified' and full_result['status'] == 'not_found':
            full_result['status'] = 'verified'
            full_result['source'] = 'arXiv ID'
            full_result['error_type'] = None
            logger.info(f"  -> VERIFIED via arXiv ID (DB search found nothing)")

        results[i] = full_result

        # Track for retry if not found and had failures
        failed_dbs = result.get('failed_dbs', [])
        if failed_dbs:
            with timeouts_lock:
                total_timeouts += len(failed_dbs)
            logger.debug(f"  Failed DBs: {', '.join(failed_dbs)}")
            # Notify progress: warning about failed DBs
            if on_progress:
                status = result['status']
                if status == 'not_found':
                    context = "not found in other DBs"
                    will_retry = " (will retry)"
                elif status == 'verified':
                    context = f"verified via {result['source']}"
                    will_retry = ""
                else:
                    context = f"{status} via {result['source']}"
                    will_retry = ""
                on_progress('warning', {
                    'index': i,
                    'total': len(refs),
                    'title': title,
                    'failed_dbs': failed_dbs,
                    'status': status,
                    'message': f"{', '.join(failed_dbs)} timed out; {context}{will_retry}",
                })
        if result['status'] == 'not_found' and failed_dbs:
            with retry_lock:
                retry_candidates.append((i, failed_dbs))
            logger.info(f"  -> Will retry ({len(failed_dbs)} DBs failed: {', '.join(failed_dbs)})")

        # Notify progress: result for this reference (include full result data)
        if on_progress:
            progress_data = dict(full_result)
            progress_data['index'] = i
            progress_data['total'] = len(refs)
            on_progress('result', progress_data)

    # Process references in parallel with bounded concurrency
    with ThreadPoolExecutor(max_workers=max_concurrent_refs) as executor:
        futures = []
        for i, ref in enumerate(refs):
            # Check for cancellation before submitting more work
            if cancel_event and cancel_event.is_set():
                break
            # Handle (title, authors, doi, arxiv_id, raw_citation), and legacy tuples
            if len(ref) >= 5:
                title, ref_authors, doi, arxiv_id, raw_citation = ref[0], ref[1], ref[2], ref[3], ref[4]
            elif len(ref) >= 4:
                title, ref_authors, doi, arxiv_id = ref[0], ref[1], ref[2], ref[3]
                raw_citation = None
            elif len(ref) >= 3:
                title, ref_authors, doi = ref[0], ref[1], ref[2]
                arxiv_id = None
                raw_citation = None
            else:
                title, ref_authors = ref[0], ref[1]
                doi = None
                arxiv_id = None
                raw_citation = None
            future = executor.submit(check_single_ref, i, title, ref_authors, doi, arxiv_id, raw_citation)
            futures.append(future)

        # Wait for all to complete (or cancel if requested)
        for future in futures:
            if cancel_event and cancel_event.is_set():
                future.cancel()
            else:
                future.result()  # This will raise any exceptions

    # Skip retries if cancelled
    if cancel_event and cancel_event.is_set():
        results = [r for r in results if r is not None]
        return results, {'total_timeouts': total_timeouts, 'retried_count': 0, 'retry_successes': 0}

    # Retry pass for "not found" references that had DB failures
    retry_successes = 0
    if retry_candidates:
        logger.info(f"=== RETRY PASS: {len(retry_candidates)} references had DB failures ===")
        if on_progress:
            on_progress('retry_pass', {
                'count': len(retry_candidates),
            })

        for retry_num, (idx, failed_dbs_for_ref) in enumerate(retry_candidates, 1):
            if cancel_event and cancel_event.is_set():
                break
            title = results[idx]['title']
            ref_authors = results[idx]['ref_authors']
            short_title = title[:50] + '...' if len(title) > 50 else title
            logger.info(f"[RETRY {retry_num}/{len(retry_candidates)}] {short_title} (retrying: {', '.join(failed_dbs_for_ref)})")

            if on_progress:
                on_progress('checking', {
                    'index': idx,
                    'total': len(refs),
                    'title': f"[RETRY: {', '.join(failed_dbs_for_ref)}] {title}",
                })

            # Retry only the DBs that failed, with longer timeout
            result = query_all_databases_concurrent(
                title, ref_authors,
                openalex_key=openalex_key,
                s2_api_key=s2_api_key,
                longer_timeout=True,
                only_dbs=failed_dbs_for_ref,
                dblp_offline_path=dblp_offline_path,
                check_openalex_authors=check_openalex_authors,
                enabled_dbs=enabled_dbs
            )

            # Only update if we found something better
            if result['status'] != 'not_found':
                results[idx]['status'] = result['status']
                results[idx]['source'] = result['source']
                results[idx]['found_authors'] = result['found_authors']
                results[idx]['error_type'] = result['error_type']
                retry_successes += 1
                logger.info(f"  -> RECOVERED: {result['status'].upper()} ({result['source']})")

                if on_progress:
                    progress_data = dict(results[idx])
                    progress_data['index'] = idx
                    progress_data['total'] = len(refs)
                    progress_data['title'] = f"[RETRY] {title}"
                    on_progress('result', progress_data)
            else:
                logger.info(f"  -> Still not found")

    if retry_candidates:
        logger.info(f"=== RETRY COMPLETE: {retry_successes}/{len(retry_candidates)} recovered ===")

    # DOI retry pass for rate-limited DOIs
    doi_retry_successes = 0
    if doi_retry_candidates and not (cancel_event and cancel_event.is_set()):
        logger.info(f"=== DOI RETRY PASS: {len(doi_retry_candidates)} DOIs were rate limited ===")
        if on_progress:
            on_progress('doi_retry_pass', {
                'count': len(doi_retry_candidates),
            })

        import time
        for retry_num, (idx, doi, title, ref_authors) in enumerate(doi_retry_candidates, 1):
            short_title = title[:50] + '...' if len(title) > 50 else title
            logger.info(f"[DOI RETRY {retry_num}/{len(doi_retry_candidates)}] {short_title}")

            if on_progress:
                on_progress('checking', {
                    'index': idx,
                    'total': len(refs),
                    'title': f"[RETRY DOI] {short_title}",
                })

            # Brief delay before retry
            time.sleep(0.5)

            # Retry DOI validation
            doi_result = validate_doi(doi)
            if doi_result['valid'] or '429' not in str(doi_result.get('error', '')):
                # Either succeeded or got a different error - update the result
                doi_match = check_doi_match(doi_result, title, ref_authors)
                new_doi_info = {
                    'doi': doi,
                    'status': doi_match['status'],
                    'message': doi_match['message'],
                    'doi_title': doi_match.get('doi_title'),
                    'doi_authors': doi_match.get('doi_authors', []),
                }
                results[idx]['doi_info'] = new_doi_info

                # If DOI now verified and DB search failed, update status
                if doi_match['status'] == 'verified' and results[idx]['status'] == 'not_found':
                    results[idx]['status'] = 'verified'
                    results[idx]['source'] = 'DOI'
                    results[idx]['error_type'] = None

                if doi_result['valid']:
                    doi_retry_successes += 1
                    logger.info(f"  -> DOI RECOVERED: {doi_match['status']}")
                else:
                    logger.info(f"  -> DOI still invalid: {doi_result.get('error', 'unknown error')}")
            else:
                logger.info(f"  -> DOI still rate limited")

    if doi_retry_candidates:
        logger.info(f"=== DOI RETRY COMPLETE: {doi_retry_successes}/{len(doi_retry_candidates)} recovered ===")

    # arXiv retry pass for rate-limited arXiv IDs
    arxiv_retry_successes = 0
    if arxiv_retry_candidates and not (cancel_event and cancel_event.is_set()):
        logger.info(f"=== arXiv RETRY PASS: {len(arxiv_retry_candidates)} arXiv IDs were rate limited ===")
        if on_progress:
            on_progress('arxiv_retry_pass', {
                'count': len(arxiv_retry_candidates),
            })

        import time
        for retry_num, (idx, arxiv_id, title, ref_authors) in enumerate(arxiv_retry_candidates, 1):
            short_title = title[:50] + '...' if len(title) > 50 else title
            logger.info(f"[arXiv RETRY {retry_num}/{len(arxiv_retry_candidates)}] {short_title}")

            if on_progress:
                on_progress('checking', {
                    'index': idx,
                    'total': len(refs),
                    'title': f"[RETRY arXiv] {short_title}",
                })

            # Brief delay before retry
            time.sleep(0.5)

            # Retry arXiv validation
            arxiv_result = validate_arxiv(arxiv_id)
            if arxiv_result['valid'] or '429' not in str(arxiv_result.get('error', '')):
                # Either succeeded or got a different error - update the result
                arxiv_match = check_arxiv_match(arxiv_result, title, ref_authors)
                new_arxiv_info = {
                    'arxiv_id': arxiv_id,
                    'status': arxiv_match['status'],
                    'message': arxiv_match['message'],
                    'arxiv_title': arxiv_match.get('arxiv_title'),
                    'arxiv_authors': arxiv_match.get('arxiv_authors', []),
                }
                results[idx]['arxiv_info'] = new_arxiv_info

                # If arXiv now verified and DB search failed, update status
                if arxiv_match['status'] == 'verified' and results[idx]['status'] == 'not_found':
                    results[idx]['status'] = 'verified'
                    results[idx]['source'] = 'arXiv ID'
                    results[idx]['error_type'] = None

                if arxiv_result['valid']:
                    arxiv_retry_successes += 1
                    logger.info(f"  -> arXiv RECOVERED: {arxiv_match['status']}")
                else:
                    logger.info(f"  -> arXiv still invalid: {arxiv_result.get('error', 'unknown error')}")
            else:
                logger.info(f"  -> arXiv still rate limited")

    if arxiv_retry_candidates:
        logger.info(f"=== arXiv RETRY COMPLETE: {arxiv_retry_successes}/{len(arxiv_retry_candidates)} recovered ===")

    check_stats = {
        'total_timeouts': total_timeouts,
        'retried_count': len(retry_candidates),
        'retry_successes': retry_successes,
        'doi_retried_count': len(doi_retry_candidates),
        'doi_retry_successes': doi_retry_successes,
        'arxiv_retried_count': len(arxiv_retry_candidates),
        'arxiv_retry_successes': arxiv_retry_successes,
    }
    return results, check_stats


def main(pdf_path, sleep_time=1.0, openalex_key=None, s2_api_key=None, dblp_offline_path=None, check_openalex_authors=False, enabled_dbs=None):
    # Print DBLP offline status / staleness warning
    if dblp_offline_path:
        from dblp_offline import check_staleness, get_db_metadata
        import os
        if not os.path.exists(dblp_offline_path):
            print(f"{Colors.RED}Error: DBLP offline database not found: {dblp_offline_path}{Colors.RESET}")
            print(f"Run with --update-dblp={dblp_offline_path} to download and build the database.")
            sys.exit(1)
        meta = get_db_metadata(dblp_offline_path)
        if meta:
            pub_count = meta.get('publication_count', 'unknown')
            print(f"{Colors.CYAN}Using offline DBLP database ({pub_count} publications){Colors.RESET}")
        staleness_warning = check_staleness(dblp_offline_path)
        if staleness_warning:
            print(f"{Colors.YELLOW}Warning: {staleness_warning}{Colors.RESET}")
        print()

    # Print OpenReview warning
    print(f"{Colors.YELLOW}OpenReview Disabled: On Nov 27, 2025, an OpenReview API vulnerability was exploited")
    print(f"to deanonymize ~10k ICLR 2026 papers, leaking reviewer/author/AC identities.")
    print(f"The leaked data was used for harassment, bribery, and author-reviewer collusion.")
    print(f"Analysis found 21% of reviews were fully AI-generated; 199 papers were pure AI slop.")
    print(f"This is why tools like this need to exist. The API remains unreachable.")
    print(f"")
    print(f"Sources:")
    print(f"  - ICLR Official: https://blog.iclr.cc/2025/12/03/iclr-2026-response-to-security-incident/")
    print(f"  - Science/AAAS: https://www.science.org/content/article/hack-reveals-reviewer-identities-huge-ai-conference{Colors.RESET}")
    print()

    # Extract references
    print(f"Extracting references from {pdf_path.split('/')[-1]}...")
    refs, skip_stats = extract_references_with_titles_and_authors(pdf_path, return_stats=True)

    total = len(refs)
    print(f"Found {total} references to check")
    if skip_stats['skipped_url'] + skip_stats['skipped_short_title'] > 0:
        print(f"{Colors.DIM}(Skipped {skip_stats['skipped_url']} URLs, {skip_stats['skipped_short_title']} short titles){Colors.RESET}")
    print()

    # Progress callback for CLI
    def cli_progress(event_type, data):
        if event_type == 'checking':
            idx = data['index'] + 1
            total = data['total']
            short_title = data['title'][:50] + '...' if len(data['title']) > 50 else data['title']
            print(f"[{idx}/{total}] Checking: \"{short_title}\"")
        elif event_type == 'result':
            idx = data['index'] + 1
            total = data['total']
            status = data['status']
            source = data['source']
            if status == 'verified':
                print(f"[{idx}/{total}] -> {Colors.GREEN}VERIFIED{Colors.RESET} ({source})")
            elif status == 'author_mismatch':
                print(f"[{idx}/{total}] -> {Colors.YELLOW}AUTHOR MISMATCH{Colors.RESET} ({source})")
            else:
                print(f"[{idx}/{total}] -> {Colors.RED}NOT FOUND{Colors.RESET}")
        elif event_type == 'warning':
            idx = data['index'] + 1
            total = data['total']
            message = data['message']
            print(f"[{idx}/{total}] {Colors.YELLOW}WARNING:{Colors.RESET} {message}")

    # Check all references with progress
    results, check_stats = check_references(refs, sleep_time=sleep_time, openalex_key=openalex_key, s2_api_key=s2_api_key, on_progress=cli_progress, dblp_offline_path=dblp_offline_path, check_openalex_authors=check_openalex_authors, enabled_dbs=enabled_dbs)

    # Count results
    found = sum(1 for r in results if r['status'] == 'verified')
    failed = sum(1 for r in results if r['status'] == 'not_found')
    mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')

    # Count DOI stats
    dois_found = sum(1 for r in results if r.get('doi_info'))
    dois_valid = sum(1 for r in results if r.get('doi_info') and r['doi_info']['status'] == 'verified')
    dois_invalid = sum(1 for r in results if r.get('doi_info') and r['doi_info']['status'] == 'invalid')
    dois_mismatch = sum(1 for r in results if r.get('doi_info') and r['doi_info']['status'] in ('title_mismatch', 'author_mismatch'))

    # Count arXiv stats
    arxivs_found = sum(1 for r in results if r.get('arxiv_info'))
    arxivs_valid = sum(1 for r in results if r.get('arxiv_info') and r['arxiv_info']['status'] == 'verified')
    arxivs_invalid = sum(1 for r in results if r.get('arxiv_info') and r['arxiv_info']['status'] == 'invalid')
    arxivs_mismatch = sum(1 for r in results if r.get('arxiv_info') and r['arxiv_info']['status'] in ('title_mismatch', 'author_mismatch'))

    # Print detailed hallucination info
    for result in results:
        if result['status'] == 'not_found':
            print_hallucinated_reference(result['title'], "not_found", searched_openalex=bool(openalex_key))
        elif result['status'] == 'author_mismatch':
            print_hallucinated_reference(
                result['title'], "author_mismatch",
                source=result['source'],
                ref_authors=result['ref_authors'],
                found_authors=result['found_authors']
            )

    # Print DOI issues as potential hallucinations
    doi_issues = [r for r in results if r.get('doi_info') and r['doi_info']['status'] in ('invalid', 'title_mismatch', 'author_mismatch')]
    if doi_issues:
        print()
        print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
        print(f"{Colors.RED}{Colors.BOLD}DOI ISSUES - POTENTIAL HALLUCINATIONS{Colors.RESET}")
        print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
        for result in doi_issues:
            doi_info = result['doi_info']
            print()
            print(f"{Colors.BOLD}Reference:{Colors.RESET} {result['title'][:70]}{'...' if len(result['title']) > 70 else ''}")
            print(f"{Colors.BOLD}DOI:{Colors.RESET} {doi_info['doi']}")
            if doi_info['status'] == 'invalid':
                print(f"{Colors.RED}Issue:{Colors.RESET} DOI does not resolve - {doi_info['message']}")
            elif doi_info['status'] == 'title_mismatch':
                print(f"{Colors.RED}Issue:{Colors.RESET} DOI points to a different paper")
                print(f"{Colors.BOLD}DOI resolves to:{Colors.RESET} {doi_info['doi_title'][:70]}{'...' if doi_info.get('doi_title') and len(doi_info['doi_title']) > 70 else ''}")
            elif doi_info['status'] == 'author_mismatch':
                print(f"{Colors.RED}Issue:{Colors.RESET} DOI title matches but authors differ")
            # Show database verification status
            if result['status'] == 'verified':
                print(f"{Colors.DIM}Note: Paper found in {result['source']} but DOI is problematic{Colors.RESET}")
            elif result['status'] == 'not_found':
                print(f"{Colors.RED}Note: Paper also not found in any database{Colors.RESET}")
        print()

    # Print arXiv issues as potential hallucinations
    arxiv_issues = [r for r in results if r.get('arxiv_info') and r['arxiv_info']['status'] in ('invalid', 'title_mismatch', 'author_mismatch')]
    if arxiv_issues:
        print()
        print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
        print(f"{Colors.RED}{Colors.BOLD}arXiv ISSUES - POTENTIAL HALLUCINATIONS{Colors.RESET}")
        print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
        for result in arxiv_issues:
            arxiv_info = result['arxiv_info']
            print()
            print(f"{Colors.BOLD}Reference:{Colors.RESET} {result['title'][:70]}{'...' if len(result['title']) > 70 else ''}")
            print(f"{Colors.BOLD}arXiv ID:{Colors.RESET} {arxiv_info['arxiv_id']}")
            if arxiv_info['status'] == 'invalid':
                print(f"{Colors.RED}Issue:{Colors.RESET} arXiv ID does not resolve - {arxiv_info['message']}")
            elif arxiv_info['status'] == 'title_mismatch':
                print(f"{Colors.RED}Issue:{Colors.RESET} arXiv ID points to a different paper")
                print(f"{Colors.BOLD}arXiv resolves to:{Colors.RESET} {arxiv_info['arxiv_title'][:70]}{'...' if arxiv_info.get('arxiv_title') and len(arxiv_info['arxiv_title']) > 70 else ''}")
            elif arxiv_info['status'] == 'author_mismatch':
                print(f"{Colors.RED}Issue:{Colors.RESET} arXiv ID title matches but authors differ")
            # Show database verification status
            if result['status'] == 'verified':
                print(f"{Colors.DIM}Note: Paper found in {result['source']} but arXiv ID is problematic{Colors.RESET}")
            elif result['status'] == 'not_found':
                print(f"{Colors.RED}Note: Paper also not found in any database{Colors.RESET}")
        print()

    # Print retracted papers warning
    retracted_papers = [r for r in results if r.get('retraction_info') and r['retraction_info'].get('retracted')]
    if retracted_papers:
        print()
        print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
        print(f"{Colors.RED}{Colors.BOLD}⚠️  RETRACTED PAPERS{Colors.RESET}")
        print(f"{Colors.RED}{Colors.BOLD}{'='*60}{Colors.RESET}")
        for result in retracted_papers:
            retraction_info = result['retraction_info']
            print()
            print(f"{Colors.BOLD}Reference:{Colors.RESET} {result['title'][:70]}{'...' if len(result['title']) > 70 else ''}")
            print(f"{Colors.RED}{Colors.BOLD}Status:{Colors.RESET} {retraction_info.get('retraction_type', 'Retraction')}")
            if retraction_info.get('retraction_doi'):
                print(f"{Colors.BOLD}Retraction notice:{Colors.RESET} https://doi.org/{retraction_info['retraction_doi']}")
            if retraction_info.get('retraction_date'):
                print(f"{Colors.BOLD}Date:{Colors.RESET} {retraction_info['retraction_date']}")
            if result.get('doi_info') and result['doi_info'].get('doi'):
                print(f"{Colors.BOLD}Original DOI:{Colors.RESET} https://doi.org/{result['doi_info']['doi']}")
        print()

    # Print summary
    print()
    print(f"{Colors.BOLD}{'='*60}{Colors.RESET}")
    print(f"{Colors.BOLD}SUMMARY{Colors.RESET}")
    print(f"{Colors.BOLD}{'='*60}{Colors.RESET}")
    total_skipped = skip_stats['skipped_url'] + skip_stats['skipped_short_title']
    print(f"  Total references found: {skip_stats['total_raw']}")
    print(f"  References analyzed: {len(refs)}")
    if total_skipped > 0:
        print(f"  {Colors.DIM}Skipped: {total_skipped} (URLs: {skip_stats['skipped_url']}, short titles: {skip_stats['skipped_short_title']}){Colors.RESET}")
    if skip_stats['skipped_no_authors'] > 0:
        print(f"  {Colors.DIM}Title-only (no authors extracted): {skip_stats['skipped_no_authors']}{Colors.RESET}")
    if check_stats['total_timeouts'] > 0:
        print(f"  {Colors.DIM}DB timeouts/errors: {check_stats['total_timeouts']} (retried {check_stats['retried_count']}, {check_stats['retry_successes']} recovered){Colors.RESET}")
    if check_stats.get('doi_retried_count', 0) > 0:
        print(f"  {Colors.DIM}DOI rate limits: {check_stats['doi_retried_count']} (retried, {check_stats['doi_retry_successes']} recovered){Colors.RESET}")
    if check_stats.get('arxiv_retried_count', 0) > 0:
        print(f"  {Colors.DIM}arXiv rate limits: {check_stats['arxiv_retried_count']} (retried, {check_stats['arxiv_retry_successes']} recovered){Colors.RESET}")
    print()
    print(f"  {Colors.GREEN}Verified:{Colors.RESET} {found}")
    if mismatched > 0:
        print(f"  {Colors.YELLOW}Author mismatches:{Colors.RESET} {mismatched}")
    if failed > 0:
        print(f"  {Colors.RED}Not found (potential hallucinations):{Colors.RESET} {failed}")

    # DOI issues count as potential hallucinations
    doi_issues_count = dois_invalid + dois_mismatch
    if doi_issues_count > 0:
        print(f"  {Colors.RED}DOI issues (potential hallucinations):{Colors.RESET} {doi_issues_count}")
        if dois_invalid > 0:
            print(f"    {Colors.DIM}- Invalid/unresolved DOIs: {dois_invalid}{Colors.RESET}")
        if dois_mismatch > 0:
            print(f"    {Colors.DIM}- DOI mismatches: {dois_mismatch}{Colors.RESET}")

    # arXiv issues count as potential hallucinations
    arxiv_issues_count = arxivs_invalid + arxivs_mismatch
    if arxiv_issues_count > 0:
        print(f"  {Colors.RED}arXiv issues (potential hallucinations):{Colors.RESET} {arxiv_issues_count}")
        if arxivs_invalid > 0:
            print(f"    {Colors.DIM}- Invalid/unresolved arXiv IDs: {arxivs_invalid}{Colors.RESET}")
        if arxivs_mismatch > 0:
            print(f"    {Colors.DIM}- arXiv mismatches: {arxivs_mismatch}{Colors.RESET}")

    # Retracted papers warning
    if retracted_papers:
        print(f"  {Colors.RED}⚠️  Retracted papers:{Colors.RESET} {len(retracted_papers)}")

    # Show DOI/arXiv stats if any were found
    id_stats = []
    if dois_found > 0 and dois_valid > 0:
        id_stats.append(f"DOIs: {dois_valid}/{dois_found}")
    if arxivs_found > 0 and arxivs_valid > 0:
        id_stats.append(f"arXiv IDs: {arxivs_valid}/{arxivs_found}")
    if id_stats:
        print()
        print(f"  {Colors.DIM}IDs validated: {', '.join(id_stats)}{Colors.RESET}")
    print()

if __name__ == "__main__":
    import os

    # Check for --no-color flag
    if '--no-color' in sys.argv:
        Colors.disable()
        sys.argv.remove('--no-color')

    # Check for --output / -o flag
    output_path = None
    for i, arg in enumerate(sys.argv[:]):
        if arg.startswith('--output='):
            output_path = arg.split('=', 1)[1]
            sys.argv.remove(arg)
            break
        elif arg in ('--output', '-o') and i + 1 < len(sys.argv):
            output_path = sys.argv[i + 1]
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break


    # Check for --sleep flag
    sleep_time = 1.0
    for i, arg in enumerate(sys.argv):
        if arg.startswith('--sleep='):
            sleep_time = float(arg.split('=')[1])
            sys.argv.remove(arg)
            break
        elif arg == '--sleep' and i + 1 < len(sys.argv):
            sleep_time = float(sys.argv[i + 1])
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break

    # Check for --openalex-key flag
    openalex_key = None
    for i, arg in enumerate(sys.argv[:]):  # Use copy to safely modify
        if arg.startswith('--openalex-key='):
            openalex_key = arg.split('=', 1)[1]
            sys.argv.remove(arg)
            break
        elif arg == '--openalex-key' and i + 1 < len(sys.argv):
            openalex_key = sys.argv[i + 1]
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break

    # Check for --s2-api-key flag (Semantic Scholar)
    s2_api_key = None
    for i, arg in enumerate(sys.argv[:]):  # Use copy to safely modify
        if arg.startswith('--s2-api-key='):
            s2_api_key = arg.split('=', 1)[1]
            sys.argv.remove(arg)
            break
        elif arg == '--s2-api-key' and i + 1 < len(sys.argv):
            s2_api_key = sys.argv[i + 1]
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break

    # Check for --dblp-offline flag (offline DBLP database)
    dblp_offline_path = None
    for i, arg in enumerate(sys.argv[:]):
        if arg.startswith('--dblp-offline='):
            dblp_offline_path = arg.split('=', 1)[1]
            sys.argv.remove(arg)
            break
        elif arg == '--dblp-offline' and i + 1 < len(sys.argv):
            dblp_offline_path = sys.argv[i + 1]
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break

    # Check for --update-dblp flag (download and build offline DBLP database)
    update_dblp_path = None
    for i, arg in enumerate(sys.argv[:]):
        if arg.startswith('--update-dblp='):
            update_dblp_path = arg.split('=', 1)[1]
            sys.argv.remove(arg)
            break
        elif arg == '--update-dblp' and i + 1 < len(sys.argv):
            update_dblp_path = sys.argv[i + 1]
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break

    # Check for --check-openalex-authors flag
    check_openalex_authors = False
    for i, arg in enumerate(sys.argv[:]):
        if arg == '--check-openalex-authors':
            check_openalex_authors = True
            sys.argv.remove(arg)
            break

    # Check for --disable-dbs flag (comma-separated list of databases to skip)
    disabled_dbs_raw = None
    for i, arg in enumerate(sys.argv[:]):
        if arg.startswith('--disable-dbs='):
            disabled_dbs_raw = arg.split('=', 1)[1]
            sys.argv.remove(arg)
            break
        elif arg == '--disable-dbs' and i + 1 < len(sys.argv):
            disabled_dbs_raw = sys.argv[i + 1]
            sys.argv.remove(sys.argv[i + 1])
            sys.argv.remove(arg)
            break

    if disabled_dbs_raw:
        disabled_list = [db.strip() for db in disabled_dbs_raw.split(',')]
        invalid_dbs = [db for db in disabled_list if db not in ALL_DATABASES]
        if invalid_dbs:
            print(f"Error: Unknown database(s): {', '.join(invalid_dbs)}")
            print(f"Valid databases: {', '.join(ALL_DATABASES)}")
            sys.exit(1)
        enabled_dbs = set(ALL_DATABASES) - set(disabled_list)
    else:
        enabled_dbs = None

    # Handle --update-dblp: download and build database, then exit
    if update_dblp_path:
        from dblp_offline import update_dblp_db
        print(f"Downloading and building DBLP offline database at: {update_dblp_path}")
        print("This will download ~4.6GB and may take 20-30 minutes total.")
        print()
        try:
            update_dblp_db(update_dblp_path)
            print()
            print(f"Done! Use --dblp-offline={update_dblp_path} to use the offline database.")
            sys.exit(0)
        except Exception as e:
            print(f"Error: {e}")
            sys.exit(1)

    if len(sys.argv) < 2:
        print("Usage: check_hallucinated_references.py [OPTIONS] <path_to_pdf>")
        print()
        print("Options:")
        print("  --no-color              Disable colored output")
        print("  --output=FILE, -o FILE  Write output to file")
        print("  --sleep=SECONDS         Delay between checks (default: 1.0)")
        print("  --openalex-key=KEY      OpenAlex API key")
        print("  --s2-api-key=KEY        Semantic Scholar API key")
        print("  --dblp-offline=PATH     Use offline DBLP database (SQLite)")
        print("  --update-dblp=PATH      Download DBLP dump and build offline database")
        print("  --check-openalex-authors  Flag author mismatches from OpenAlex (off by default)")
        print("  --disable-dbs=DB1,DB2   Disable specific databases (comma-separated)")
        print(f"    Available: {', '.join(ALL_DATABASES)}")
        sys.exit(1)

    pdf_path = sys.argv[1]
    if not os.path.exists(pdf_path):
        print(f"Error: File '{pdf_path}' not found")
        sys.exit(1)

    if enabled_dbs is not None:
        disabled = set(ALL_DATABASES) - enabled_dbs
        print(f"{Colors.YELLOW}Disabled databases: {', '.join(sorted(disabled))}{Colors.RESET}")
        print()

    if output_path:
        Colors.disable()
        with open(output_path, "w", encoding="utf-8") as f, \
             contextlib.redirect_stdout(f), \
             contextlib.redirect_stderr(f):
            main(pdf_path, sleep_time=sleep_time, openalex_key=openalex_key, s2_api_key=s2_api_key, dblp_offline_path=dblp_offline_path, check_openalex_authors=check_openalex_authors, enabled_dbs=enabled_dbs)
    else:
        main(pdf_path, sleep_time=sleep_time, openalex_key=openalex_key, s2_api_key=s2_api_key, dblp_offline_path=dblp_offline_path, check_openalex_authors=check_openalex_authors, enabled_dbs=enabled_dbs)
