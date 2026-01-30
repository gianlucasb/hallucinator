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
    """Normalize title for comparison - keep only alphanumeric characters."""
    import html
    title = html.unescape(str(title))  # Decode HTML entities like &quot;
    title = unicodedata.normalize("NFKD", title)
    title = title.encode("ascii", "ignore").decode("ascii")
    # Keep only letters and numbers, remove everything else including spaces
    title = re.sub(r'[^a-zA-Z0-9]', '', title)
    return title.lower()

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
            # Find end markers (Appendix, Acknowledgments, etc.)
            end_markers = [
                r'\n\s*Appendix',
                r'\n\s*APPENDIX',
                r'\n\s*Acknowledgments',
                r'\n\s*ACKNOWLEDGMENTS',
                r'\n\s*Acknowledgements',
                r'\n\s*Supplementary',
                r'\n\s*SUPPLEMENTARY',
            ]
            ref_end = len(text)
            for end_pattern in end_markers:
                end_match = re.search(end_pattern, text[ref_start:], re.IGNORECASE)
                if end_match:
                    ref_end = min(ref_end, ref_start + end_match.start())

            return text[ref_start:ref_end]

    # Fallback: try last 30% of document
    cutoff = int(len(text) * 0.7)
    return text[cutoff:]


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

    # Try numbered list style: 1., 2., etc.
    # Validate that numbers are sequential starting from 1 (not years like 2019. or page numbers)
    numbered_pattern = r'\n\s*(\d+)\.\s+'
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
    # Use [a-z0-9)] before period to exclude author initials (which are uppercase like "A.")
    # This ensures we only match after page numbers (digits), venue names (lowercase), or volume/issue like 118(9).
    aaai_pattern = r'[a-z0-9)]\.\n([A-Z][a-zA-Z]+(?:[ -][A-Za-z]+)?,\s+[A-Z]\.)'
    aaai_matches = list(re.finditer(aaai_pattern, ref_text))

    if len(aaai_matches) >= 3:
        refs = []
        # Handle first reference (before first match) - starts at beginning of ref_text
        first_ref = ref_text[:aaai_matches[0].start()].strip()
        if first_ref and len(first_ref) > 20:
            refs.append(first_ref)
        # Handle remaining references
        for i, match in enumerate(aaai_matches):
            start = match.start() + 3  # +3 to skip the "[a-z0-9].\n"
            end = aaai_matches[i + 1].start() if i + 1 < len(aaai_matches) else len(ref_text)
            ref_content = ref_text[start:end].strip()
            if ref_content:
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

    # If title came from quotes, return it as-is (quotes already delimit the title)
    if from_quotes:
        return title.strip()

    # For non-quoted titles, truncate at first sentence-ending period
    # Skip periods that are part of abbreviations (e.g., "U.S." has short segments)
    for match in re.finditer(r'\.', title):
        pos = match.start()
        # Find start of segment (after last period or space, whichever is later)
        last_period = title.rfind('.', 0, pos)
        last_space = title.rfind(' ', 0, pos)
        segment_start = max(last_period + 1, last_space + 1, 0)
        segment = title[segment_start:pos]
        # If segment > 2 chars, it's likely a real sentence end, not an abbreviation
        if len(segment) > 2:
            # But skip if period is immediately followed by a letter (no space) - product names like "big.LITTLE", "Node.js"
            if pos + 1 < len(title) and title[pos + 1].isalpha():
                continue
            title = title[:pos]
            break

    # Also handle "? In" pattern for question-ending titles
    in_venue_match = re.search(r'\?\s*[Ii]n\s+(?:[A-Z]|[12]\d{3}\s)', title)
    if in_venue_match:
        title = title[:in_venue_match.start() + 1]  # Keep the question mark

    # Remove trailing journal/venue info that might have been included
    cutoff_patterns = [
        r'[.?!]\s*(?:Proceedings|Conference|Workshop|Symposium|IEEE|ACM|USENIX|AAAI|EMNLP|NAACL|arXiv|Available|CoRR).*$',
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
            # If char before is a single capital (and char before that is space/start), it's an initial
            if char_before.isupper() and (pos == 1 or not text[pos-2].isalpha()):
                continue  # Skip this period - it's an initial

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
    ]

    for pattern in quote_patterns:
        match = re.search(pattern, ref_text)
        if match:
            quoted_part = match.group(1).strip()
            after_quote = ref_text[match.end():].strip()

            # Check if there's a subtitle after the quote (starts with : or -)
            if after_quote and after_quote[0] in ':-':
                subtitle_text = after_quote[1:].strip()
                # Find where subtitle ends at venue/year markers
                end_patterns = [
                    r'\.\s*[Ii]n\s+',           # ". In "
                    r'\.\s*(?:Proc|IEEE|ACM|USENIX|NDSS|CCS|AAAI|WWW|CHI|arXiv)',
                    r',\s*[Ii]n\s+',            # ", in "
                    r'\.\s*\((?:19|20)\d{2}\)', # ". (2022)" style venue year
                    r'[,\.]\s*(?:19|20)\d{2}',  # year
                    r'\s+(?:19|20)\d{2}\.',     # year at end
                    r'[.,]\s+[A-Z][a-z]+\s+\d+[,\s]',  # ". Word Number" journal format (". Science 344,")
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

    # === Format 2: ACM - "Authors. Year. Title. In Venue" ===
    # Pattern: ". YYYY. Title-text. In "
    # Use \s+ after year to avoid matching DOIs like "10.1109/CVPR.2022.001234"
    acm_match = re.search(r'\.\s*((?:19|20)\d{2})\.\s+', ref_text)
    if acm_match:
        after_year = ref_text[acm_match.end():]
        # Find where title ends - at ". In " or at venue indicators
        title_end_patterns = [
            r'\.\s*[Ii]n\s+[A-Z]',  # ". In Proceedings"
            r'\.\s*(?:Proceedings|IEEE|ACM|USENIX|arXiv)',
            r'\s+doi:',
        ]
        title_end = len(after_year)
        for pattern in title_end_patterns:
            m = re.search(pattern, after_year)
            if m:
                title_end = min(title_end, m.start())

        title = after_year[:title_end].strip()
        title = re.sub(r'\.\s*$', '', title)
        if len(title.split()) >= 3:
            return title, False  # from_quotes=False

    # === Format 3: USENIX - "Authors. Title. In/Journal Venue, Year" ===
    # Find venue markers and extract title before them
    venue_patterns = [
        r'\.\s*[Ii]n\s+(?:Proceedings|Workshop|Conference|Symposium|AAAI|IEEE|ACM|USENIX)',
        r'\.\s*[Ii]n\s+[A-Z][a-z]+\s+(?:Conference|Workshop|Symposium)',
        r',\s*(?:19|20)\d{2}\.\s*$',  # Journal format ending with year
    ]

    for vp in venue_patterns:
        venue_match = re.search(vp, ref_text)
        if venue_match:
            before_venue = ref_text[:venue_match.start()].strip()

            # Split into sentences, skipping author initials like "M." "J."
            # For USENIX: "Authors. Title" - title is after first period
            parts = split_sentences_skip_initials(before_venue)
            if len(parts) >= 2:
                title = parts[1].strip()
                title = re.sub(r'\.\s*$', '', title)
                if len(title.split()) >= 3:
                    # Verify it doesn't look like authors
                    if not re.match(r'^[A-Z][a-z]+\s+[A-Z][a-z]+,', title):
                        return title, False  # from_quotes=False

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
        if not title or len(title.split()) < 5:
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

        references.append((title, authors))

    return (references, stats) if return_stats else references

# Common words to skip when building search queries
STOP_WORDS = {'a', 'an', 'the', 'of', 'and', 'or', 'for', 'to', 'in', 'on', 'with', 'by'}

def get_query_words(title, n=6):
    """Extract n significant words from title for query, skipping stop words and short words."""
    all_words = re.findall(r'[a-zA-Z0-9]+', title)
    # Skip stop words and words shorter than 3 characters (e.g., "s" from "Twitter's")
    significant = [w for w in all_words if w.lower() not in STOP_WORDS and len(w) >= 3]
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

def query_all_databases_concurrent(title, ref_authors, openalex_key=None, s2_api_key=None, longer_timeout=False, only_dbs=None):
    """Query all databases concurrently for a single reference.

    Args:
        title: Paper title to search for
        ref_authors: Authors from the reference
        openalex_key: Optional OpenAlex API key
        s2_api_key: Optional Semantic Scholar API key for higher rate limits
        longer_timeout: If True, use longer timeouts (for retries)
        only_dbs: If provided, only query these specific databases (for targeted retry)

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

    # Define the databases to query
    # Each entry is (name, query_func)
    # NOTE: OpenReview disabled due to API being unreachable after Nov 2025 security incident
    all_databases = [
        ('CrossRef', lambda: query_crossref(title)),
        ('arXiv', lambda: query_arxiv(title)),
        ('DBLP', lambda: query_dblp(title)),
        # ('OpenReview', lambda: query_openreview(title)),  # Disabled - API unreachable
        ('Semantic Scholar', lambda: query_semantic_scholar(title, s2_api_key)),
        ('ACL Anthology', lambda: query_acl(title)),
        ('NeurIPS', lambda: query_neurips(title)),
    ]

    # Add OpenAlex if API key is provided
    if openalex_key:
        all_databases.insert(0, ('OpenAlex', lambda: query_openalex(title, openalex_key)))

    # Filter to only requested DBs if specified
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
                        if first_mismatch is None:
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
    def normalize_author(name):
        # Handle AAAI "Surname, Initials" format (e.g., "Bail, C. A.")
        if ',' in name:
            parts = name.split(',')
            surname = parts[0].strip()
            initials = parts[1].strip() if len(parts) > 1 else ""
            # Get first initial letter
            first_initial = initials[0] if initials else ""
            return f"{first_initial} {surname.lower()}"
        # Standard format: "FirstName LastName"
        parts = name.split()
        if not parts:
            return ""
        return f"{parts[0][0]} {parts[-1].lower()}"

    def get_last_name(name):
        # Handle AAAI "Surname, Initials" format (e.g., "Bail, C. A.")
        if ',' in name:
            surname = name.split(',')[0].strip()
            return surname.lower()
        # Standard format: surname is last word
        parts = name.split()
        if not parts:
            return ""
        return parts[-1].lower()

    # Check if PDF-extracted authors are last-name-only (single words)
    ref_authors_are_last_name_only = all(len(a.split()) == 1 for a in ref_authors if a.strip())

    if ref_authors_are_last_name_only:
        # Only compare last names
        ref_set = set(get_last_name(a) for a in ref_authors)
        found_set = set(get_last_name(a) for a in found_authors)
    else:
        ref_set = set(normalize_author(a) for a in ref_authors)
        found_set = set(normalize_author(a) for a in found_authors)
    return bool(ref_set & found_set)

def check_references(refs, sleep_time=1.0, openalex_key=None, s2_api_key=None, on_progress=None, max_concurrent_refs=4):
    """Check references against databases with concurrent queries.

    Args:
        refs: List of (title, authors) tuples
        sleep_time: (unused, kept for API compatibility)
        openalex_key: Optional OpenAlex API key
        s2_api_key: Optional Semantic Scholar API key for higher rate limits
        on_progress: Optional callback function(event_type, data)
            event_type can be: 'checking', 'result', 'warning', 'retry_pass'
            data varies by event type
        max_concurrent_refs: Max number of references to check in parallel (default 4)

    Returns:
        Tuple of (results, check_stats) where:
        - results: List of result dicts with title, ref_authors, status, source, found_authors, error_type
        - check_stats: Dict with 'total_timeouts', 'retried_count', 'retry_successes'
    """
    import threading

    results = [None] * len(refs)  # Pre-allocate to maintain order
    # Track indices of "not found" results that had failed DBs for retry
    retry_candidates = []
    # Track total timeout/failure count
    total_timeouts = 0
    timeouts_lock = threading.Lock()
    retry_lock = threading.Lock()

    def check_single_ref(i, title, ref_authors):
        """Check a single reference and return result."""
        nonlocal total_timeouts

        # Notify progress: starting to check this reference
        if on_progress:
            on_progress('checking', {
                'index': i,
                'total': len(refs),
                'title': title,
            })

        # Query all databases concurrently
        result = query_all_databases_concurrent(
            title, ref_authors,
            openalex_key=openalex_key,
            s2_api_key=s2_api_key
        )

        # Build full result record
        full_result = {
            'title': title,
            'ref_authors': ref_authors,
            'status': result['status'],
            'source': result['source'],
            'found_authors': result['found_authors'],
            'paper_url': result.get('paper_url'),
            'error_type': result['error_type'],
            'failed_dbs': result.get('failed_dbs', []),
        }
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

        # Notify progress: result for this reference
        if on_progress:
            on_progress('result', {
                'index': i,
                'total': len(refs),
                'title': title,
                'status': result['status'],
                'source': result['source'],
            })

    # Process references in parallel with bounded concurrency
    with ThreadPoolExecutor(max_workers=max_concurrent_refs) as executor:
        futures = []
        for i, (title, ref_authors) in enumerate(refs):
            future = executor.submit(check_single_ref, i, title, ref_authors)
            futures.append(future)

        # Wait for all to complete
        for future in futures:
            future.result()  # This will raise any exceptions

    # Retry pass for "not found" references that had DB failures
    retry_successes = 0
    if retry_candidates:
        logger.info(f"=== RETRY PASS: {len(retry_candidates)} references had DB failures ===")
        if on_progress:
            on_progress('retry_pass', {
                'count': len(retry_candidates),
            })

        for retry_num, (idx, failed_dbs_for_ref) in enumerate(retry_candidates, 1):
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
                only_dbs=failed_dbs_for_ref
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
                    on_progress('result', {
                        'index': idx,
                        'total': len(refs),
                        'title': f"[RETRY] {title}",
                        'status': result['status'],
                        'source': result['source'],
                    })
            else:
                logger.info(f"  -> Still not found")

    if retry_candidates:
        logger.info(f"=== RETRY COMPLETE: {retry_successes}/{len(retry_candidates)} recovered ===")

    check_stats = {
        'total_timeouts': total_timeouts,
        'retried_count': len(retry_candidates),
        'retry_successes': retry_successes,
    }
    return results, check_stats


def main(pdf_path, sleep_time=1.0, openalex_key=None, s2_api_key=None):
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
    results, check_stats = check_references(refs, sleep_time=sleep_time, openalex_key=openalex_key, s2_api_key=s2_api_key, on_progress=cli_progress)

    # Count results
    found = sum(1 for r in results if r['status'] == 'verified')
    failed = sum(1 for r in results if r['status'] == 'not_found')
    mismatched = sum(1 for r in results if r['status'] == 'author_mismatch')

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
    print()
    print(f"  {Colors.GREEN}Verified:{Colors.RESET} {found}")
    if mismatched > 0:
        print(f"  {Colors.YELLOW}Author mismatches:{Colors.RESET} {mismatched}")
    if failed > 0:
        print(f"  {Colors.RED}Not found (potential hallucinations):{Colors.RESET} {failed}")
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

    if len(sys.argv) < 2:
        print("Usage: check_hallucinated_references.py [--no-color] [--sleep=SECONDS] [--openalex-key=KEY] [--s2-api-key=KEY] [--output=FILE|-o FILE] <path_to_pdf>")
        sys.exit(1)

    pdf_path = sys.argv[1]
    if not os.path.exists(pdf_path):
        print(f"Error: File '{pdf_path}' not found")
        sys.exit(1)

    if output_path:
        Colors.disable()
        with open(output_path, "w", encoding="utf-8") as f, \
             contextlib.redirect_stdout(f), \
             contextlib.redirect_stderr(f):
            main(pdf_path, sleep_time=sleep_time, openalex_key=openalex_key, s2_api_key=s2_api_key)
    else:
        main(pdf_path, sleep_time=sleep_time, openalex_key=openalex_key, s2_api_key=s2_api_key)
