import re
import sys
import requests
import urllib.parse
import unicodedata
from bs4 import BeautifulSoup
from rapidfuzz import fuzz
import feedparser
import time
import json

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
            print(f"{Colors.DIM}Searched: OpenAlex, DBLP, arXiv, CrossRef{Colors.RESET}")
        else:
            print(f"{Colors.DIM}Searched: DBLP, arXiv, CrossRef{Colors.RESET}")
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

def extract_text_from_pdf(pdf_path):
    """Extract text from PDF using PyMuPDF."""
    import fitz
    doc = fitz.open(pdf_path)
    text = "\n".join(page.get_text() for page in doc)
    doc.close()
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
    numbered_pattern = r'\n\s*(\d+)\.\s+'
    numbered_matches = list(re.finditer(numbered_pattern, ref_text))

    if len(numbered_matches) >= 3:
        refs = []
        for i, match in enumerate(numbered_matches):
            start = match.end()
            end = numbered_matches[i + 1].start() if i + 1 < len(numbered_matches) else len(ref_text)
            ref_content = ref_text[start:end].strip()
            if ref_content:
                refs.append(ref_content)
        return refs

    # Fallback: split by double newlines or lines starting with author patterns
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

    # USENIX/default: authors end at first period
    first_period = ref_text.find('. ')

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

    # Fix hyphenation from PDF line breaks
    title = re.sub(r'(\w)- (\w)', r'\1\2', title)
    title = re.sub(r'(\w)-\s+(\w)', r'\1\2', title)

    # If title came from quotes, return it as-is (quotes already delimit the title)
    if from_quotes:
        return title.strip()

    # For non-quoted titles, truncate at first period
    period_pos = title.find('.')
    if period_pos > 0:
        title = title[:period_pos]

    # Also handle "? In" pattern for question-ending titles
    in_venue_match = re.search(r'\?\s*[Ii]n\s+(?:[A-Z]|[12]\d{3}\s)', title)
    if in_venue_match:
        title = title[:in_venue_match.start() + 1]  # Keep the question mark

    # Remove trailing journal/venue info that might have been included
    cutoff_patterns = [
        r'\.\s*(?:Proceedings|Conference|Workshop|Symposium|IEEE|ACM|USENIX|AAAI|EMNLP|NAACL|arXiv|Available).*$',
        r'\.\s*(?:Advances\s+in|Journal\s+of|Transactions\s+of|Transactions\s+on|Communications\s+of).*$',
        r'\.\s*[A-Z][a-z]+\s+(?:Journal|Review|Transactions|Letters|advances|Processing|medica|Intelligenz)\b.*$',
        r'\.\s*(?:Patterns|Data\s+&\s+Knowledge).*$',
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
    ]

    for pattern in cutoff_patterns:
        title = re.sub(pattern, '', title, flags=re.IGNORECASE)

    title = title.strip()
    title = re.sub(r'[.,;:]+$', '', title)

    return title.strip()


def extract_title_from_reference(ref_text):
    """Extract title from a reference string.

    Handles three main formats:
    - IEEE: Authors, "Title," in Venue, Year.
    - ACM: Authors. Year. Title. In Venue.
    - USENIX: Authors. Title. In/Journal Venue, Year.

    Returns: (title, from_quotes) tuple where from_quotes indicates if title was in quotes.
    """
    # Fix hyphenation from PDF line breaks (e.g., "detec- tion" -> "detection")
    ref_text = re.sub(r'(\w)- (\w)', r'\1\2', ref_text)
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
                    r'[,\.]\s*(?:19|20)\d{2}',  # year
                    r'\s+(?:19|20)\d{2}\.',     # year at end
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
    acm_match = re.search(r'\.\s*((?:19|20)\d{2})\.\s*', ref_text)
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

            # Split into sentences
            # For USENIX: "Authors. Title" - title is after first period
            parts = re.split(r'\.\s+', before_venue, maxsplit=1)
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
        parts = re.split(r'\.\s+', before_journal, maxsplit=1)
        if len(parts) >= 2:
            title = parts[1].strip()
            if len(title.split()) >= 3:
                return title, False  # from_quotes=False

    # === Fallback: second sentence if it looks like a title ===
    sentences = re.split(r'\.\s+', ref_text)
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


def extract_references_with_titles_and_authors(pdf_path):
    """Extract references from PDF using pure Python (PyMuPDF)."""
    try:
        text = extract_text_from_pdf(pdf_path)
    except Exception as e:
        print(f"[Error] Failed to extract text from PDF: {e}")
        return []

    ref_section = find_references_section(text)
    if not ref_section:
        print("[Error] Could not locate references section")
        return []

    raw_refs = segment_references(ref_section)

    references = []
    previous_authors = []

    for ref_text in raw_refs:
        # Fix hyphenation from PDF line breaks (e.g., "detec- tion" -> "detection")
        ref_text = re.sub(r'(\w)- (\w)', r'\1\2', ref_text)

        # Skip entries with non-academic URLs (keep acm, ieee, usenix, arxiv, doi)
        # Also catch broken URLs with spaces like "https: //" or "ht tps://"
        if re.search(r'https?\s*:\s*//', ref_text) or re.search(r'ht\s*tps?\s*:\s*//', ref_text):
            if not re.search(r'(acm\.org|ieee\.org|usenix\.org|arxiv\.org|doi\.org)', ref_text, re.IGNORECASE):
                continue

        title, from_quotes = extract_title_from_reference(ref_text)
        title = clean_title(title, from_quotes=from_quotes)
        if not title or len(title.split()) < 5:
            continue

        authors = extract_authors_from_reference(ref_text)

        # Handle em-dash meaning "same authors as previous"
        if authors == ['__SAME_AS_PREVIOUS__']:
            if previous_authors:
                authors = previous_authors
            else:
                continue  # No previous authors to use

        if not authors:
            continue

        # Update previous_authors for potential next em-dash reference
        previous_authors = authors

        references.append((title, authors))

    return references

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
        response = requests.get(url)
        if response.status_code != 200:
            return None, []
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
                return found_title, authors
    except Exception as e:
        print(f"[Error] DBLP search failed: {e}")
    return None, []

def query_arxiv(title):
    # Use first 6 significant words for query (skip stop words)
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"http://export.arxiv.org/api/query?search_query=all:{urllib.parse.quote(query)}&start=0&max_results=5"
    try:
        feed = feedparser.parse(url)
        for entry in feed.entries:
            entry_title = entry.title
            if fuzz.ratio(normalize_title(title), normalize_title(entry_title)) >= 95:
                authors = [author.name for author in entry.authors]
                return entry_title, authors
    except Exception as e:
        print(f"[Error] arXiv search failed: {e}")
    return None, []

def query_crossref(title):
    # Use first 6 significant words for query (skip stop words)
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://api.crossref.org/works?query.title={urllib.parse.quote(query)}&rows=5"
    try:
        response = requests.get(url, headers={"User-Agent": "Academic Reference Parser"})
        if response.status_code != 200:
            return None, []
        results = response.json().get("message", {}).get("items", [])
        for item in results:
            found_title = item.get("title", [""])[0]
            if fuzz.ratio(normalize_title(title), normalize_title(found_title)) >= 95:
                authors = [f"{a.get('given', '')} {a.get('family', '')}".strip() for a in item.get("author", [])]
                return found_title, authors
    except Exception as e:
        print(f"[Error] CrossRef search failed: {e}")
    return None, []

def query_openalex(title, api_key):
    """Query OpenAlex API for paper information."""
    words = get_query_words(title, 6)
    query = ' '.join(words)
    url = f"https://api.openalex.org/works?filter=title.search:{urllib.parse.quote(query)}&api_key={api_key}"
    try:
        response = requests.get(url, headers={"User-Agent": "Academic Reference Parser"})
        if response.status_code != 200:
            return None, []
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
                return found_title, authors
    except Exception as e:
        print(f"[Error] OpenAlex search failed: {e}")
    return None, []

def query_neurips(title):
    try:
        years = [2023, 2022, 2021, 2020, 2019, 2018]
        for year in years:
            search_url = f"https://papers.nips.cc/paper_files/paper/{year}/hash/index.html"
            response = requests.get(search_url)
            if response.status_code != 200:
                continue

            soup = BeautifulSoup(response.content, "html.parser")
            for a in soup.find_all("a"):
                if fuzz.ratio(normalize_title(title), normalize_title(a.text)) >= 95:
                    paper_url = "https://papers.nips.cc" + a['href']
                    paper_response = requests.get(paper_url)
                    if paper_response.status_code != 200:
                        return a.text.strip(), []
                    author_soup = BeautifulSoup(paper_response.content, "html.parser")
                    authors = [tag.text.strip() for tag in author_soup.find_all("li", class_="author")]
                    return a.text.strip(), authors
    except Exception as e:
        print(f"[Error] NeurIPS search failed: {e}")
    return None, []

def query_acl(title):
    try:
        query = urllib.parse.quote(title)
        url = f"https://aclanthology.org/search/?q={query}"
        response = requests.get(url)
        if response.status_code != 200:
            return None, []
        soup = BeautifulSoup(response.text, 'html.parser')
        for entry in soup.select(".d-sm-flex.align-items-stretch.p-2"):
            entry_title_tag = entry.select_one("h5")
            if entry_title_tag and fuzz.ratio(normalize_title(title), normalize_title(entry_title_tag.text)) >= 95:
                author_tags = entry.select("span.badge.badge-light")
                authors = [a.text.strip() for a in author_tags]
                return entry_title_tag.text.strip(), authors
    except Exception as e:
        print(f"[Error] ACL Anthology search failed: {e}")
    return None, []

def validate_authors(ref_authors, found_authors):
    def normalize_author(name):
        parts = name.split()
        if not parts:
            return ""
        return f"{parts[0][0]} {parts[-1].lower()}"

    ref_set = set(normalize_author(a) for a in ref_authors)
    found_set = set(normalize_author(a) for a in found_authors)
    return bool(ref_set & found_set)

def main(pdf_path, sleep_time=1.0, openalex_key=None):
    refs = extract_references_with_titles_and_authors(pdf_path)
#    print(f"Found {len(refs)} references.")
    print("Analyzing paper %s"%(pdf_path.split("/")[-1]))

    found = 0
    failed = 0
    mismatched = 0

    for i, (title, ref_authors) in enumerate(refs):
        # If OpenAlex API key provided, query OpenAlex first
        if openalex_key:
            found_title, found_authors = query_openalex(title, openalex_key)
            if found_title:
                if validate_authors(ref_authors, found_authors):
                    found += 1
                else:
                    print_hallucinated_reference(
                        title, "author_mismatch", source="OpenAlex",
                        ref_authors=ref_authors, found_authors=found_authors
                    )
                    mismatched += 1
                continue

        # Sleep before DBLP query to avoid rate limiting
        time.sleep(sleep_time)
        found_title, found_authors = query_dblp(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                found += 1
            else:
                print_hallucinated_reference(
                    title, "author_mismatch", source="DBLP",
                    ref_authors=ref_authors, found_authors=found_authors
                )
                mismatched += 1
            continue

        found_title, found_authors = query_arxiv(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                found += 1
            else:
                print_hallucinated_reference(
                    title, "author_mismatch", source="arXiv",
                    ref_authors=ref_authors, found_authors=found_authors
                )
                mismatched += 1
            continue

        found_title, found_authors = query_crossref(title)
        if found_title:
            if validate_authors(ref_authors, found_authors):
                found += 1
            else:
                print_hallucinated_reference(
                    title, "author_mismatch", source="CrossRef",
                    ref_authors=ref_authors, found_authors=found_authors
                )
                mismatched += 1
            continue

        print_hallucinated_reference(title, "not_found", searched_openalex=bool(openalex_key))
        failed += 1

    # Print summary
    print()
    print(f"{Colors.BOLD}{'='*60}{Colors.RESET}")
    print(f"{Colors.BOLD}SUMMARY{Colors.RESET}")
    print(f"{Colors.BOLD}{'='*60}{Colors.RESET}")
    print(f"  Total references analyzed: {len(refs)}")
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

    if len(sys.argv) < 2:
        print("Usage: check_hallucinated_references.py [--no-color] [--sleep=SECONDS] [--openalex-key=KEY] <path_to_pdf>")
        sys.exit(1)

    pdf_path = sys.argv[1]
    if not os.path.exists(pdf_path):
        print(f"Error: File '{pdf_path}' not found")
        sys.exit(1)

    main(pdf_path, sleep_time=sleep_time, openalex_key=openalex_key)

