"""Recent parsing improvements to port into the Rust engine.

This script demonstrates and tests the parsing improvements made to the Python
version of check_hallucinated_references.py. Each improvement is documented
with test cases that can be used to verify the Rust implementation.

Improvements covered:
1. H-infinity unicode symbol normalization (for fuzzy matching)
2. Chinese ALL CAPS author format (SURNAME I, SURNAME I, et al.)
3. Chinese citation markers [J], [C], [M], [D] as title terminators
4. Venue leak fixes after question marks
5. 2-word quoted titles support
6. Reference number prefix stripping in title extraction
7. Format 5 skip for Chinese ALL CAPS
8. Author names with particles and special characters (von, van der, etc.)

Run with:
    pip install .          # from hallucinator-rs/
    python examples/regexp_improvements.py
"""

import re
import unicodedata
from typing import Optional

from hallucinator import PdfExtractor


# =============================================================================
# IMPROVEMENT 1: H-infinity Unicode Symbol Normalization
# =============================================================================
# Control theory papers often use H∞ (H-infinity) in titles. The infinity
# symbol should be normalized for fuzzy matching to work correctly.
#
# Location in Python: normalize_title() function
# Rust location: hallucinator-core/src/matching.rs (normalize_for_comparison)


def normalize_title_improved(title: str) -> str:
    """Normalize title for comparison with H-infinity handling.

    This is the improved version that should be ported to Rust.
    """
    import html
    title = html.unescape(str(title))
    title = unicodedata.normalize("NFKD", title)
    # Handle mathematical symbols that would otherwise be stripped
    # H∞ (H-infinity) is common in control theory papers
    title = title.replace('∞', 'infinity')
    title = title.replace('∞', 'infinity')  # Alternative infinity symbol
    # Keep only Unicode letters and numbers
    title = ''.join(c for c in title if c.isalnum())
    return title.lower()


def test_h_infinity_normalization():
    """Test H∞ symbol normalization for fuzzy matching."""
    print("=" * 60)
    print("IMPROVEMENT 1: H-infinity Unicode Normalization")
    print("=" * 60)

    test_cases = [
        ("H∞ almost state synchronization", "hinfinity almost state synchronization"),
        ("H∞ control for nonlinear systems", "hinfinity control for nonlinear systems"),
        ("Robust H∞ filtering", "robust hinfinity filtering"),
    ]

    for original, expected_normalized in test_cases:
        normalized = normalize_title_improved(original)
        # Remove spaces for comparison (normalize_title strips them)
        expected = expected_normalized.replace(' ', '')
        assert normalized == expected, f"Failed: {original} -> {normalized} (expected {expected})"
        print(f"  OK: '{original}' -> '{normalized}'")

    print()


# =============================================================================
# IMPROVEMENT 2: Chinese ALL CAPS Author Format
# =============================================================================
# Chinese biomedical papers use format: "SURNAME I, SURNAME I, et al. Title"
# Example: "CAO X, YANG B, WANG K, et al. AI-empowered multiple access for 6G"
#
# Location in Python: extract_title_from_reference() - Format 8
# Rust location: hallucinator-pdf/src/title.rs


def extract_title_chinese_allcaps(ref_text: str) -> Optional[str]:
    """Extract title from Chinese ALL CAPS author format.

    Pattern: SURNAME I, SURNAME I, et al. Title[J]. Venue
    """
    # Strip reference number prefixes
    ref_text = re.sub(r'^\[\d+\]\s*', '', ref_text)
    ref_text = re.sub(r'^\d+\.\s*', '', ref_text)
    ref_text = ref_text.lstrip('. ')

    # Check for ALL CAPS pattern at start: "CAO X," or "LIU Z,"
    all_caps_match = re.search(r'^([A-Z]{2,})\s+[A-Z](?:,|\s|$)', ref_text)
    if not all_caps_match:
        return None

    # Find end of author list at "et al." or sentence boundary
    et_al_match = re.search(r',?\s+et\s+al\.?\s*[,.]?\s*', ref_text, re.IGNORECASE)
    if et_al_match:
        after_authors = ref_text[et_al_match.end():].strip()
    else:
        # Find where ALL CAPS author pattern ends
        parts = ref_text.split(', ')
        title_start_idx = None
        for i, part in enumerate(parts):
            part = part.strip()
            # Check if this looks like an ALL CAPS author (SURNAME X or just SURNAME)
            if re.match(r'^[A-Z]{2,}(?:\s+[A-Z])?$', part):
                continue  # Still in author list
            # Found non-author part - this is the title start
            title_start_idx = i
            break

        if title_start_idx is not None:
            after_authors = ', '.join(parts[title_start_idx:]).strip()
        else:
            return None

    if not after_authors:
        return None

    # Find where title ends - at journal/year markers
    # Key addition: Chinese citation markers [J], [C], [M], [D]
    title_end_patterns = [
        r'\[J\]',  # Chinese citation marker for journal
        r'\[C\]',  # Chinese citation marker for conference
        r'\[M\]',  # Chinese citation marker for book
        r'\[D\]',  # Chinese citation marker for dissertation
        r'\.\s*[A-Z][a-zA-Z\s]+\d+\s*\(\d+\)',  # ". Journal Name 34(5)"
        r'\.\s*[A-Z][a-zA-Z\s&+]+\d+:\d+',  # ". Journal 34:123"
        r'\.\s*[A-Z][a-zA-Z\s&+]+,\s*\d+',  # ". Journal Name, vol"
        r'\.\s*(?:19|20)\d{2}',  # ". 2024"
        r'\.\s*https?://',
        r'\.\s*doi:',
    ]
    title_end = len(after_authors)
    for pattern in title_end_patterns:
        m = re.search(pattern, after_authors)
        if m:
            title_end = min(title_end, m.start())

    title = after_authors[:title_end].strip()
    title = re.sub(r'\.\s*$', '', title)

    if len(title.split()) >= 3:
        return title
    return None


def test_chinese_allcaps_format():
    """Test Chinese ALL CAPS author format extraction."""
    print("=" * 60)
    print("IMPROVEMENT 2: Chinese ALL CAPS Author Format")
    print("=" * 60)

    test_cases = [
        # (input, expected_title)
        (
            'CAO X, YANG B, WANG K, et al. AI-empowered multiple access for 6G: '
            'A survey of spectrum sensing, protocol designs, and optimizations[J]. '
            'Proceedings of the IEEE, 2024, 112(9): 1264-1302.',
            'AI-empowered multiple access for 6G: A survey of spectrum sensing, '
            'protocol designs, and optimizations'
        ),
        (
            'LIU Z, SABERI A, et al. H∞ almost state synchronization for '
            'homogeneous networks[J]. IEEE Trans. Aut. Contr. 53 (2008), no. 4.',
            'H∞ almost state synchronization for homogeneous networks'
        ),
        (
            'WANG X, QIAN L P, et al. Multi-agent reinforcement learning assisted '
            'trust-aware cooperative spectrum sensing[J]. Journal of Communications, 2023.',
            'Multi-agent reinforcement learning assisted trust-aware cooperative spectrum sensing'
        ),
    ]

    for ref_text, expected in test_cases:
        result = extract_title_chinese_allcaps(ref_text)
        if result is None:
            print(f"  FAIL: No title extracted from: {ref_text[:60]}...")
            continue
        # Normalize for comparison
        result_norm = ' '.join(result.split())
        expected_norm = ' '.join(expected.split())
        if result_norm == expected_norm:
            print(f"  OK: '{result[:50]}...'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# =============================================================================
# IMPROVEMENT 3: Chinese Citation Markers
# =============================================================================
# Chinese papers use [J], [C], [M], [D] to indicate document type:
#   [J] = Journal article
#   [C] = Conference paper
#   [M] = Book (monograph)
#   [D] = Dissertation
#
# These markers should terminate the title, not be included in it.
# Already covered in test_chinese_allcaps_format above.


# =============================================================================
# IMPROVEMENT 4: Venue Leak After Question Marks
# =============================================================================
# Titles ending with "?" should not leak venue names that follow.
# Example: "Is this a question? IEEE Trans..." -> title should end at "?"
#
# Location in Python: clean_title() function
# Rust location: hallucinator-pdf/src/title.rs (clean_title or similar)


def clean_title_question_mark_fix(title: str) -> str:
    """Clean title with improved venue leak detection after question marks.

    This is the improved version that should be ported to Rust.
    """
    # Handle "? In" and "? In:" patterns
    in_venue_match = re.search(r'\?\s*[Ii]n:?\s+(?:[A-Z]|[12]\d{3}\s)', title)
    if in_venue_match:
        title = title[:in_venue_match.start() + 1]  # Keep the question mark

    # Handle "? Journal Name, vol" pattern (journal with comma before volume)
    q_journal_comma_match = re.search(
        r'[?!]\s+[A-Z][a-zA-Z\s&+\u00AE\u2013\u2014\-]+,\s*(?:vol\.?\s*)?\d+', title
    )
    if q_journal_comma_match:
        title = title[:q_journal_comma_match.start() + 1]

    # Handle "? Automatica 34(" or "? IEEE Trans... 53(" patterns
    # Journal + volume without comma (with parens or brackets)
    q_journal_vol_match = re.search(
        r'[?!]\s+(?:IEEE\s+Trans[a-z.]*|ACM\s+Trans[a-z.]*|Automatica|'
        r'J\.\s*[A-Z][a-z]+|[A-Z][a-z]+\.?\s+[A-Z][a-z]+\.?)\s+\d+\s*[(\[]',
        title
    )
    if q_journal_vol_match:
        title = title[:q_journal_vol_match.start() + 1]

    # Handle "? IEEE Trans. Aut. Contr. 53" (abbreviated journal + volume, no parens)
    # This catches patterns like "IEEE Trans. Xxx. Yyy. NN" or "IEEE Trans. Xxx. NN"
    q_abbrev_journal_match = re.search(
        r'[?!]\s+(?:IEEE|ACM|SIAM)\s+Trans[a-z.]*'
        r'(?:\s+[A-Z][a-z]+\.?)+\s+\d+',
        title
    )
    if q_abbrev_journal_match:
        title = title[:q_abbrev_journal_match.start() + 1]

    return title


def test_venue_leak_after_question():
    """Test venue leak prevention after question marks."""
    print("=" * 60)
    print("IMPROVEMENT 4: Venue Leak After Question Marks")
    print("=" * 60)

    test_cases = [
        (
            "Is information the key? Nature Physics, vol. 1, no. 1, pp. 2-4",
            "Is information the key?"
        ),
        (
            "Can machines think? IEEE Trans. Aut. Contr. 53 (2008), no. 4",
            "Can machines think?"
        ),
        (
            "What is consciousness? Automatica 34(5): 123-456",
            "What is consciousness?"
        ),
        (
            "Are toll lanes elitist? In Proceedings of AAAI 2024",
            "Are toll lanes elitist?"
        ),
    ]

    for dirty_title, expected_clean in test_cases:
        cleaned = clean_title_question_mark_fix(dirty_title)
        if cleaned == expected_clean:
            print(f"  OK: '{cleaned}'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {cleaned}")
            print(f"    Expected: {expected_clean}")

    print()


# =============================================================================
# IMPROVEMENT 5: 2-Word Quoted Titles
# =============================================================================
# IEEE-style quoted titles like "Cyclo-dissipativity revisited," should be
# accepted even if they're only 2 words. Quotes are a strong indicator.
#
# Location in Python: extract_title_from_reference() - Format 0 (quoted titles)
# Rust location: hallucinator-pdf/src/title.rs


def test_two_word_quoted_titles():
    """Test 2-word quoted title extraction."""
    print("=" * 60)
    print("IMPROVEMENT 5: 2-Word Quoted Titles")
    print("=" * 60)

    ext = PdfExtractor()
    # Current Rust requires 3+ words; this should be reduced to 2 for quoted

    test_cases = [
        (
            'A. van der Schaft, "Cyclo-dissipativity revisited," IEEE Transactions '
            'on Automatic Control, vol. 66, no. 6, pp. 2925-2931, 2021.',
            "Cyclo-dissipativity revisited,"
        ),
        (
            'Smith, J. "Neural networks," Proc. IEEE, 2023.',
            "Neural networks,"
        ),
        (
            'Jones, A. "Deep learning," Nature 2024.',
            "Deep learning,"
        ),
    ]

    print("  NOTE: Rust currently requires 3+ words for quoted titles.")
    print("  These tests may fail until the improvement is ported.\n")

    for ref_text, expected_title in test_cases:
        ref = ext.parse_reference(ref_text)
        if ref and ref.title:
            # The title may have trailing comma stripped
            got = ref.title.rstrip(',') + (',' if expected_title.endswith(',') else '')
            if got == expected_title or ref.title == expected_title.rstrip(','):
                print(f"  OK: '{ref.title}'")
            else:
                print(f"  MISMATCH: got '{ref.title}', expected '{expected_title}'")
        else:
            print(f"  SKIPPED (no title extracted): {ref_text[:50]}...")

    print()


# =============================================================================
# IMPROVEMENT 6: Reference Number Prefix Stripping
# =============================================================================
# Reference text may start with [1], [23], 1., 23. etc. These should be
# stripped before format detection to allow proper pattern matching.
#
# Location in Python: extract_title_from_reference() - preprocessing
# Rust location: hallucinator-pdf/src/title.rs (preprocessing)


def strip_reference_prefix(ref_text: str) -> str:
    """Strip reference number prefixes from reference text.

    This should be part of preprocessing in title extraction.
    """
    # Strip [N] prefix
    ref_text = re.sub(r'^\[\d+\]\s*', '', ref_text)
    # Strip N. prefix
    ref_text = re.sub(r'^\d+\.\s*', '', ref_text)
    # Strip leading punctuation artifacts
    ref_text = ref_text.lstrip('. ')
    return ref_text


def test_reference_prefix_stripping():
    """Test reference number prefix stripping."""
    print("=" * 60)
    print("IMPROVEMENT 6: Reference Number Prefix Stripping")
    print("=" * 60)

    test_cases = [
        ("[1] Smith, J. Title here.", "Smith, J. Title here."),
        ("[23] Jones, A. Another title.", "Jones, A. Another title."),
        ("1. Brown, C. Third title.", "Brown, C. Third title."),
        ("42. Williams, D. Fourth title.", "Williams, D. Fourth title."),
        (". Leading period artifact.", "Leading period artifact."),
    ]

    for original, expected in test_cases:
        result = strip_reference_prefix(original)
        if result == expected:
            print(f"  OK: '{original[:30]}...' -> '{result[:30]}...'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# =============================================================================
# IMPROVEMENT 7: Format 5 Skip for Chinese ALL CAPS
# =============================================================================
# Format 5 (Western ALL CAPS: "SURNAME, F., AND SURNAME, G. Title") should
# skip Chinese ALL CAPS pattern ("SURNAME I, SURNAME I,") to let Format 8
# handle it correctly.
#
# Location in Python: extract_title_from_reference() - Format 5 condition
# Rust location: hallucinator-pdf/src/title.rs


def should_skip_format5_for_chinese(ref_text: str) -> bool:
    """Check if Format 5 should skip this reference (Chinese ALL CAPS pattern).

    Format 5 handles: SURNAME, F., AND SURNAME, G. Title
    Format 8 handles: SURNAME I, SURNAME I, et al. Title

    The key difference is the spacing around the initial.
    """
    # Chinese pattern: SURNAME followed by space and single initial
    # e.g., "CAO X," or "LIU Z,"
    return bool(re.match(r'^[A-Z]{2,}', ref_text) and
                re.search(r'^[A-Z]{2,}\s+[A-Z](?:,|\s)', ref_text))


def test_format5_skip_detection():
    """Test Format 5 skip detection for Chinese ALL CAPS."""
    print("=" * 60)
    print("IMPROVEMENT 7: Format 5 Skip for Chinese ALL CAPS")
    print("=" * 60)

    test_cases = [
        # (ref_text, should_skip_format5)
        ("CAO X, YANG B, et al. Title here.", True),   # Chinese - skip Format 5
        ("LIU Z, WANG Q. Title here.", True),          # Chinese - skip Format 5
        ("SMITH, J., AND JONES, A. Title.", False),    # Western - use Format 5
        ("BROWN, C. Title here.", False),              # Western - use Format 5
    ]

    for ref_text, expected_skip in test_cases:
        result = should_skip_format5_for_chinese(ref_text)
        status = "SKIP Format 5" if result else "USE Format 5"
        expected_status = "SKIP Format 5" if expected_skip else "USE Format 5"
        if result == expected_skip:
            print(f"  OK: '{ref_text[:40]}...' -> {status}")
        else:
            print(f"  MISMATCH: expected {expected_status}, got {status}")

    print()


# =============================================================================
# IMPROVEMENT 8: Author Names with Particles and Special Characters
# =============================================================================
# Academic names often include:
#   - Lowercase particles: von, van, van der, de, del, della, Le, La, etc.
#   - Accented characters: Bissyand´e, Müller, García
#   - Apostrophes: Dell'Amico, O'Brien
#   - Hyphenated names: Styp-Rekowsky, Vallina-Rodriguez
#
# The current title extraction fails because these patterns break the
# "Initial. LastName, Initial. LastName, and Initial. LastName. Title."
# boundary detection.
#
# Example failures:
#   "K. Allix, T. F. Bissyand´e, J. Klein, and Y. Le Traon. Androzoo: ..."
#   -> Title extracted as "Bissyand´e, J. Klein, and Y. Le Traon" (WRONG)
#   -> Should be "Androzoo: Collecting millions of android apps..."
#
# Location in Python: extract_title_from_reference()
# Rust location: hallucinator-pdf/src/title.rs

# Common name particles (lowercase) that appear between first/middle names and surnames
NAME_PARTICLES = {
    'von', 'van', 'de', 'del', 'della', 'di', 'da', 'dos', 'das', 'du',
    'le', 'la', 'les', 'den', 'der', 'ten', 'ter', 'op', 'het',
    'bin', 'ibn', 'al', 'el', 'ben',  # Arabic/Hebrew
    'mac', 'mc', "o'",  # Celtic (note: O' with apostrophe)
}


def extract_title_author_particle_aware(ref_text: str) -> Optional[str]:
    """Extract title from references with complex author names.

    Handles:
    - Lowercase particles: von, van der, de, Le, etc.
    - Accented characters: Bissyand´e, Müller
    - Apostrophes: Dell'Amico, O'Brien
    - Hyphenated surnames: Styp-Rekowsky, Vallina-Rodriguez

    The key insight is that the author list follows a strict pattern:
    "I. Name, I. Name, and I. Name. Title"

    Where:
    - I = Initial(s) like "J." or "T. F." or "A. E. B."
    - Name = Surname possibly with particles/accents/hyphens
    - The author list ends with ". " followed by the title

    The title typically starts with a capital letter or number.
    """
    # Strip reference number prefixes first
    ref_text = re.sub(r'^\[\d+\]\s*', '', ref_text)
    ref_text = re.sub(r'^\d+\.\s*', '', ref_text)
    ref_text = ref_text.lstrip('. ')

    # Pattern for author initials: "J." or "T. F." or "A. E. B." or "P.-A."
    # Must handle:
    # - Accented initials like "´A." (seen in Spanish names)
    # - Hyphenated initials like "P.-A." (seen in European names)
    # Using Unicode ranges instead of literal characters to avoid escaping issues
    initial_pattern = r"(?:[\u0041-\u005A\u00C0-\u00D6\u00D8-\u00DE\u0027\u0060\u00B4]\.(?:[\s\-]*[A-Z]\.)*)"

    # Pattern for surname: letters, accents, hyphens, apostrophes, particles
    # Examples: "Smith", "von Styp-Rekowsky", "Bissyand´e", "Dell'Amico", "van der Sloot"
    # Using Unicode ranges: \u00C0-\u024F covers Latin Extended-A and Extended-B
    surname_chars = r"[A-Za-z\u00C0-\u024F\u0027\u0060\u00B4\u2019\-]"
    # Structure: optional particles, then base name, then optional additional parts
    surname_pattern = (
        r"(?:(?:von|van|de|del|della|di|da|dos|das|du|le|la|les|den|der|ten|ter|op|het)\s+)?"  # optional particle
        + surname_chars + r"+"  # base name with accents, apostrophes, hyphens
        + r"(?:\s+" + surname_chars + r"+)*"  # additional name parts (e.g., "van der Sloot")
    )

    # Full author pattern: "I. Surname" or "I. I. Surname"
    author_pattern = initial_pattern + r"\s*" + surname_pattern

    # Pattern for author list: "Author, Author, and Author."
    # The key is finding where the author list ends (after "and LastName.")
    # and the title begins (capital letter or number after ". ")

    # Strategy: Find the pattern "and Initial. Surname. Title"
    # where Title starts with a capital letter (not an initial pattern)

    # Look for "and I. Name. " followed by title start
    # Title start patterns:
    # - Capital + lowercase letter (most titles): "Androzoo:", "Artist:", "The european"
    # - Digit (numbered titles): "50 ways"
    # - Quote (quoted titles): '"A title"'
    # We need to avoid matching another author initial like "A." or "J."
    # So we look for capital + lowercase, or capital + space + lowercase (single-word start like "A ")
    and_author_title_pattern = (
        r",?\s+and\s+"  # "and" or ", and"
        + author_pattern +  # Final author
        r"\.\s+"  # Period and space after author list
        # Title start: avoid matching "X." (initial) by requiring lowercase after capital,
        # or a digit, or a quote. Also handle "A simple" (capital + space + lowercase)
        r"([A-Z\u00C0-\u00D6][a-z]|[A-Z]\s+[a-z]|[0-9]|[\"\u0022])"
    )

    match = re.search(and_author_title_pattern, ref_text)
    if match:
        # Title starts at the captured group
        title_start = match.start(1)
        title_text = ref_text[title_start:]

        # Find where title ends (venue/year markers)
        title_end_patterns = [
            r'\.\s+In\s+',  # ". In Proceedings"
            r'\s+In\s+Proceedings',  # " In Proceedings" (no period)
            r'\.\s+(?:Proc\.|Proceedings\s+of)',  # ". Proc." or ". Proceedings of"
            r'\.\s+(?:IEEE|ACM|USENIX|NDSS|CCS|AAAI|ICML|NeurIPS|EuroS&P)\b',  # ". IEEE" venue
            r'\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\s+\d{4}',  # ". Journal Name 2021"
            r'\.\s+[A-Z][a-z]+(?:\s*&\s*[A-Z][a-z]+)+',  # ". Information & Communications"
            r'\.\s+arXiv\s+preprint',  # ". arXiv preprint"
            r',\s+(?:vol\.|pp\.|pages)\s',  # ", vol." or ", pp."
            r',\s+\d{4}\.$',  # ", 2021." at end
            r',\s+\d+\(\d+\)',  # ", 28(1)" - volume(issue)
            # Publisher/journal names
            r'\.\s+(?:Springer|Elsevier|Wiley|Nature|Science|PLOS|Oxford|Cambridge)\b',
            # "The X of Y" and "X of Y" journal patterns
            r'\.\s+The\s+(?:Annals|Journal|Proceedings)\s+of\b',
            r'\.\s+Journal\s+of\s+[A-Z]',  # ". Journal of X"
            # Generic journal pattern: ". Word Word, vol" or ". Word Word 123"
            r'\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+,\s*\d',
            r'\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)+\s+\d+[:(]',
        ]

        title_end = len(title_text)
        for pattern in title_end_patterns:
            m = re.search(pattern, title_text)
            if m:
                title_end = min(title_end, m.start())

        title = title_text[:title_end].strip()
        # Clean up trailing period if present
        title = re.sub(r'\.\s*$', '', title)

        if len(title.split()) >= 3:
            return title

    return None


def test_author_particle_names():
    """Test title extraction from references with complex author names."""
    print("=" * 60)
    print("IMPROVEMENT 8: Author Names with Particles/Special Chars")
    print("=" * 60)

    test_cases = [
        # (raw_citation, expected_title)
        (
            "K. Allix, T. F. Bissyand´e, J. Klein, and Y. Le Traon. "
            "Androzoo: Collecting millions of android apps for the research community. In MSR, 2016.",
            "Androzoo: Collecting millions of android apps for the research community"
        ),
        (
            "M. Backes, S. Bugiel, O. Schranz, P. von Styp-Rekowsky, and S. Weisgerber. "
            "Artist: The android runtime instrumentation and security toolkit. In EuroS&P, 2017.",
            "Artist: The android runtime instrumentation and security toolkit"
        ),
        (
            "C. J. Hoofnagle, B. van der Sloot, and F. Z. Borgesius. "
            "The european union general data protection regulation: what it is and what it means. "
            "Information & Communications Technology Law, 28(1), 2019.",
            "The european union general data protection regulation: what it is and what it means"
        ),
        (
            "J. Reardon, ´A. Feal, P. Wijesekera, A. E. B. On, N. Vallina-Rodriguez, and S. Egelman. "
            "50 ways to leak your data: An exploration of apps' circumvention of the android permissions system. "
            "In USENIX Security, 2019.",
            "50 ways to leak your data: An exploration of apps' circumvention of the android permissions system"
        ),
        (
            "A. Smith, B. Dell'Amico, D. Balzarotti, and P.-A. Vervier. "
            "Detecting malicious behavior in networks. In IEEE S&P, 2020.",
            "Detecting malicious behavior in networks"
        ),
        # Negative test: should still work for simple names
        (
            "A. Smith, B. Jones, and C. Brown. "
            "A simple title for testing. In Proceedings of Test, 2023.",
            "A simple title for testing"
        ),
    ]

    for raw, expected in test_cases:
        result = extract_title_author_particle_aware(raw)
        if result is None:
            print(f"  FAIL: No title extracted")
            print(f"    Input: {raw[:70]}...")
            print(f"    Expected: {expected[:50]}...")
        elif result.lower() == expected.lower():
            print(f"  OK: '{result[:55]}{'...' if len(result) > 55 else ''}'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# Regex patterns for Rust implementation
AUTHOR_PARTICLE_PATTERNS = {
    # Initial pattern: handles accented initials and hyphenated like "P.-A."
    # Unicode ranges: \u0041-\u005A = A-Z, \u00C0-\u00DE = accented capitals
    "initial": r"[\u0041-\u005A\u00C0-\u00D6\u00D8-\u00DE\u0027\u0060\u00B4]\.(?:[\s\-]*[A-Z]\.)*",

    # Common lowercase particles (should be case-insensitive in matching)
    "particles": [
        "von", "van", "de", "del", "della", "di", "da", "dos", "das", "du",
        "le", "la", "les", "den", "der", "ten", "ter", "op", "het",
    ],

    # Surname character class: letters (including accented), hyphens, apostrophes
    # Unicode ranges for apostrophes: \u0027=', \u0060=`, \u00B4=´, \u2019='
    "surname_chars": r"[A-Za-z\u00C0-\u024F\u0027\u0060\u00B4\u2019\-]",

    # Pattern to find author list end: ", and I. Surname. TitleStart"
    "author_list_end": (
        r",?\s+and\s+"  # ", and" or "and"
        r"[\u0041-\u005A\u00C0-\u00D6\u00D8-\u00DE\u0027\u0060\u00B4]\.(?:[\s\-]*[A-Z]\.)*"  # Initial(s)
        r"\s*"
        r"(?:(?:von|van|de|del|della|di|da|dos|das|du|le|la|les|den|der|ten|ter|op|het)\s+)?"  # Optional particle
        r"[A-Za-z\u00C0-\u024F\u0027\u0060\u00B4\u2019\-]+"  # Base surname
        r"(?:\s+[A-Za-z\u00C0-\u024F\u0027\u0060\u00B4\u2019\-]+)*"  # Additional name parts
        r"\.\s+"  # Period after surname
        r"([A-Z\u00C0-\u00D6][a-z]|[A-Z]\s+[a-z]|[0-9]|[\"\u0022])"  # Title start
    ),

    # Title end patterns (for trimming venue/journal info)
    "title_end_patterns": [
        r"\.\s+In\s+",  # ". In Proceedings"
        r"\.\s+(?:Proc\.|Proceedings\s+of)",  # ". Proc."
        r"\.\s+(?:IEEE|ACM|USENIX|NDSS|CCS|AAAI|ICML|NeurIPS|EuroS&P)\b",  # Venues
        r"\.\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\s+\d{4}",  # ". Journal Name 2021"
        r"\.\s+[A-Z][a-z]+(?:\s*&\s*[A-Z][a-z]+)+",  # ". Information & Communications"
        r"\.\s+arXiv\s+preprint",  # ". arXiv preprint"
        r",\s+(?:vol\.|pp\.|pages)\s",  # ", vol." or ", pp."
        r",\s+\d{4}\.$",  # ", 2021." at end
        r",\s+\d+\(\d+\)",  # ", 28(1)" - volume(issue)
    ],
}


# =============================================================================
# IMPROVEMENT 9: Springer/LNCS Format (colon after authors)
# =============================================================================
# Springer/LNCS references use: "LastName, F., LastName, F.: Title. In: Venue"
# The colon after the last author initial is the key marker.
#
# Example failures from evaluation:
#   "Micali, S., Ohta, K., Reyzin, L.: Accountable-subgroup multisignatures. In: ..."
#   "Schnorr, C.P.: Efficient signature generation by smart cards. Journal of ..."
#
# Location: hallucinator-pdf/src/title.rs


def extract_title_springer_format(ref_text: str) -> Optional[str]:
    """Extract title from Springer/LNCS format references.

    Pattern: LastName, F., LastName, F.: Title. In: Venue
    or: LastName, F.: Title. Journal Name vol(issue)
    """
    # Strip reference number prefixes
    ref_text = re.sub(r'^\[\d+\]\s*', '', ref_text)
    ref_text = re.sub(r'^\d+\.\s*', '', ref_text)

    # Look for the pattern: "Initial.: Title" where Initial is like "S." or "C.P."
    # The colon after the initial(s) marks the end of authors
    # Pattern: comma-space-Initial(s)-period-colon
    colon_match = re.search(
        r',\s*'  # comma before last author
        r'[A-Z](?:\.[A-Z])*\.'  # Initial(s) like "S." or "C.P."
        r':\s*'  # colon after authors
        r'([A-Z\u00C0-\u00D6][^:]{10,}?)'  # Title (at least 10 chars, no colons)
        r'(?:\.\s+(?:In[:\s]|Journal|[A-Z][a-z]+\s+\d))',  # End marker (In: or In space)
        ref_text
    )

    if colon_match:
        title = colon_match.group(1).strip()
        # Clean up trailing period
        title = re.sub(r'\.\s*$', '', title)
        # Accept 2+ words for this format (hyphenated words count as 1)
        if len(title.split()) >= 2:
            return title

    return None


def test_springer_format():
    """Test Springer/LNCS format extraction."""
    print("=" * 60)
    print("IMPROVEMENT 9: Springer/LNCS Format (colon after authors)")
    print("=" * 60)

    test_cases = [
        (
            "Micali, S., Ohta, K., Reyzin, L.: Accountable-subgroup multisignatures. "
            "In: Proceedings of the 8th ACM Conference on Computer and Communications Security.",
            "Accountable-subgroup multisignatures"
        ),
        (
            "Schnorr, C.P.: Efficient signature generation by smart cards. "
            "Journal of cryptology 4(3), 161–174 (1991)",
            "Efficient signature generation by smart cards"
        ),
        (
            "Hedabou, M., Abdulsalam, Y.S.: Efficient and secure implementation of BLS "
            "multisignature scheme on TPM. In: 2020 IEEE International Conference.",
            "Efficient and secure implementation of BLS multisignature scheme on TPM"
        ),
        (
            "Rezaeighaleh, H., Zou, C.C.: New secure approach to backup cryptocurrency wallets. "
            "In: 2019 IEEE Global Communications Conference (GLOBECOM)",
            "New secure approach to backup cryptocurrency wallets"
        ),
    ]

    for raw, expected in test_cases:
        result = extract_title_springer_format(raw)
        if result is None:
            print(f"  FAIL: No title extracted")
            print(f"    Raw: {raw[:70]}...")
        elif result.lower() == expected.lower():
            print(f"  OK: '{result[:55]}...' " if len(result) > 55 else f"  OK: '{result}'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# =============================================================================
# IMPROVEMENT 10: Bracket Citation Format [CODE]
# =============================================================================
# Some papers use bracket codes: "[ACGH20] Authors. Title. In Venue"
# The bracket code should be stripped and not confused with IEEE format.
#
# Example failures:
#   "[ACGH20] Gorjan Alagic, Andrew M. Childs, Alex B. Grilo, and Shih-Han Hung.
#    Noninteractive classical verification of quantum computation. In CRYPTO 2020"
#
# Location: hallucinator-pdf/src/title.rs


def extract_title_bracket_code_format(ref_text: str) -> Optional[str]:
    """Extract title from bracket-code format references.

    Pattern: [CODE] Authors. Title. In Venue
    where CODE is like ACGH20, CCY20, GR25, etc.
    """
    # Check for bracket code at start: [LettersNumbers]
    bracket_match = re.match(r'\[([A-Z]+\d+[a-z]?)\]\s*', ref_text)
    if not bracket_match:
        return None

    # Remove the bracket code
    ref_text = ref_text[bracket_match.end():]

    # Now look for "Authors. Title. In" pattern
    # Authors are like "First Last, First Last, and First Last."
    # Title follows and ends at ". In" or venue markers

    # Find the first sentence that looks like a title (after author names)
    # Authors typically end with a period after a name, then title starts with capital
    # Look for pattern: "LastName. Title" where Title is capitalized

    # Strategy: Find ". " followed by capital letter, then find title end
    sentences = re.split(r'\.\s+', ref_text)

    if len(sentences) >= 2:
        # First sentence is likely authors, second is likely title
        # But we need to handle cases where author list has multiple sentences
        for i, sent in enumerate(sentences[:-1]):  # Exclude last (likely venue)
            # Check if this sentence ends with what looks like an author name
            # and next sentence starts with a title-like pattern
            next_sent = sentences[i + 1] if i + 1 < len(sentences) else ""

            # If current ends with a name pattern and next starts with capital
            if re.search(r'(?:and\s+)?[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*$', sent):
                # Check if next sentence looks like a title (not a venue)
                if next_sent and re.match(r'[A-Z]', next_sent):
                    # Check it's not starting with "In" (venue marker)
                    if not re.match(r'In\s+', next_sent):
                        # This is likely the title
                        # Find where it ends (at "In" or next venue marker)
                        title = next_sent
                        in_match = re.search(r'\.\s*In\s+', '. ' + '. '.join(sentences[i+1:]))
                        if in_match:
                            # Extract just the title part
                            remaining = '. '.join(sentences[i+1:])
                            title_end = remaining.find('. In ')
                            if title_end > 0:
                                title = remaining[:title_end]

                        if len(title.split()) >= 3:
                            return title.strip()

    return None


def test_bracket_code_format():
    """Test bracket code format extraction."""
    print("=" * 60)
    print("IMPROVEMENT 10: Bracket Citation Format [CODE]")
    print("=" * 60)

    test_cases = [
        (
            "[ACGH20] Gorjan Alagic, Andrew M. Childs, Alex B. Grilo, and Shih-Han Hung. "
            "Noninteractive classical verification of quantum computation. In CRYPTO 2020.",
            "Noninteractive classical verification of quantum computation"
        ),
        (
            "[CCY20] Nai-Hui Chia, Kai-Min Chung, and Takashi Yamakawa. "
            "Classical verification of quantum computations with efficient verifier. "
            "In Theory of Cryptography Conference 2020.",
            "Classical verification of quantum computations with efficient verifier"
        ),
    ]

    for raw, expected in test_cases:
        result = extract_title_bracket_code_format(raw)
        if result is None:
            print(f"  FAIL: No title extracted")
            print(f"    Raw: {raw[:70]}...")
        elif result.lower() == expected.lower():
            print(f"  OK: '{result[:55]}...' " if len(result) > 55 else f"  OK: '{result}'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# =============================================================================
# IMPROVEMENT 11: Remove Editor Names from Venue
# =============================================================================
# Some references include editor names after "In":
#   "Title. In Naveen Garg, Klaus Jansen, ... editors, Venue"
# The editor list should not be extracted as part of the title.
#
# Example:
#   "Beating the random assignment on constraint satisfaction problems of bounded degree.
#    In Naveen Garg, Klaus Jansen, Anup Rao, and José D. P. Rolim, editors, Approximation..."
#
# Location: hallucinator-pdf/src/title.rs (title cleaning)


def clean_title_editor_list(title: str) -> str:
    """Remove editor lists that leaked into title.

    Editors pattern: "In FirstName LastName, ... editors, Venue"
    Handles names with initials like "José D. P. Rolim"
    """
    # Name pattern: handles accented characters and initials
    # e.g., "Naveen Garg", "José D. P. Rolim", "Klaus Jansen"
    name_pattern = r'[A-Za-z\u00C0-\u024F]+(?:\s+[A-Z]\.)*(?:\s+[A-Za-z\u00C0-\u024F]+)?'

    # Look for "In Name, Name, ... editors," pattern at end
    editor_match = re.search(
        r'\.\s*In\s+' + name_pattern +  # First name
        r'(?:,\s*' + name_pattern + r')*'  # More names
        r'(?:,?\s*and\s+' + name_pattern + r')?'  # "and Name"
        r',\s*editors?,',
        title,
        re.IGNORECASE
    )

    if editor_match:
        title = title[:editor_match.start()]

    # Also catch simpler "In Venue" patterns at end
    in_venue_match = re.search(r'\.\s*In\s+(?:Proceedings|Proc\.|[A-Z][a-z]+\s+\d{4})', title)
    if in_venue_match:
        title = title[:in_venue_match.start()]

    return title.strip()


def test_editor_list_cleaning():
    """Test editor list removal from titles."""
    print("=" * 60)
    print("IMPROVEMENT 11: Remove Editor Names from Venue")
    print("=" * 60)

    test_cases = [
        (
            "Beating the random assignment on constraint satisfaction problems. "
            "In Naveen Garg, Klaus Jansen, Anup Rao, and José D. P. Rolim, editors, Approximation",
            "Beating the random assignment on constraint satisfaction problems"
        ),
        (
            "A great paper title. In John Smith and Jane Doe, editors, Proceedings of Something",
            "A great paper title"
        ),
        (
            "Another title here. In Proceedings of ICML 2024",
            "Another title here"
        ),
    ]

    for dirty, expected in test_cases:
        result = clean_title_editor_list(dirty)
        if result == expected:
            print(f"  OK: '{result[:55]}...' " if len(result) > 55 else f"  OK: '{result}'")
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# =============================================================================
# IMPROVEMENT 12: Direct Title Before "In Venue" Pattern
# =============================================================================
# Some references have the title directly followed by "In Venue" without authors,
# or the authors were already stripped. Handle this as a fallback.
#
# Example: "Title of the paper. In Proceedings of Something."
#
# Location: hallucinator-pdf/src/title.rs


def extract_title_direct_in_venue(ref_text: str) -> Optional[str]:
    """Extract title from 'Title. In Venue' pattern.

    This is a fallback for references where the title appears directly
    without a recognizable author pattern.
    """
    # Strip reference number prefixes
    ref_text = re.sub(r'^\[\d+\]\s*', '', ref_text)
    ref_text = re.sub(r'^\d+\.\s*', '', ref_text)

    # Look for "Title. In Something" pattern
    # Title must start with capital letter and have multiple words
    in_match = re.search(
        r'^([A-Z][^.]{15,}?)\.\s+In\s+(?:[A-Z]|Proceedings|Proc\.)',
        ref_text
    )

    if in_match:
        title = in_match.group(1).strip()
        if len(title.split()) >= 4:  # Require at least 4 words for this pattern
            return title

    return None


def test_direct_in_venue():
    """Test direct 'Title. In Venue' extraction."""
    print("=" * 60)
    print("IMPROVEMENT 12: Direct Title Before 'In Venue'")
    print("=" * 60)

    test_cases = [
        (
            "Beating the random assignment on constraint satisfaction problems of bounded degree. "
            "In Naveen Garg, Klaus Jansen, Anup Rao, and José D. P. Rolim, editors, Approximation.",
            "Beating the random assignment on constraint satisfaction problems of bounded degree"
        ),
        (
            "A great paper about something interesting. In Proceedings of ICML 2024.",
            "A great paper about something interesting"
        ),
    ]

    for raw, expected in test_cases:
        result = extract_title_direct_in_venue(raw)
        if result and result.lower() == expected.lower():
            print(f"  OK: '{result[:55]}...' " if len(result) > 55 else f"  OK: '{result}'")
        else:
            print(f"  FAIL: expected '{expected[:40]}...', got '{result}'")

    print()


# Regex patterns for Rust implementation - Improvements 9-11
SPRINGER_LNCS_PATTERNS = {
    # Colon after author initials marks end of author list
    "author_end_colon": r",\s*[A-Z](?:\.[A-Z])*\.:\s*",

    # Title follows colon, ends at venue markers
    "title_after_colon": r"([A-Z\u00C0-\u00D6][^:]{10,}?)(?:\.\s+(?:In:|In\s|Journal|[A-Z][a-z]+\s+\d))",
}

BRACKET_CODE_PATTERNS = {
    # Bracket code at start: [ACGH20], [CCY20], etc.
    "bracket_code": r"^\[([A-Z]+\d+[a-z]?)\]\s*",

    # After removing bracket, find author-title boundary
    # Authors end with name, title starts with capital
    "author_title_boundary": r"(?:and\s+)?[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\.\s+([A-Z])",
}

EDITOR_LIST_PATTERNS = {
    # Editor list: "In Name, Name, and Name, editors,"
    "editor_list": (
        r"\.\s*In\s+[A-Z][a-z]+\s+[A-Z][a-z]+"
        r"(?:,\s*[A-Z][a-z]+\s+[A-Z][a-z]+)*"
        r"(?:,\s*and\s+[A-Z][a-z]+(?:\s+[A-Z]\.?)?\s+[A-Z][a-z]+)?"
        r",\s*editors?,"
    ),
}


# =============================================================================
# COMBINED: Custom Title Extractor with All Improvements
# =============================================================================


def extract_title_with_improvements(ref_text: str) -> Optional[str]:
    """Extract title using all improvements.

    This combines all the improvements into a single function that can be
    used to validate behavior before porting to Rust.
    """
    original_text = ref_text

    # Preprocessing
    ref_text = strip_reference_prefix(ref_text)

    # Try Chinese ALL CAPS format first (before Format 5)
    if should_skip_format5_for_chinese(ref_text):
        title = extract_title_chinese_allcaps(ref_text)
        if title:
            return clean_title_question_mark_fix(title)

    # Try bracket code format [ACGH20]
    title = extract_title_bracket_code_format(original_text)
    if title:
        title = clean_title_editor_list(title)
        return clean_title_question_mark_fix(title)

    # Try Springer/LNCS format (colon after authors)
    title = extract_title_springer_format(ref_text)
    if title:
        return clean_title_question_mark_fix(title)

    # Try author-particle-aware extraction for complex names
    # This handles von, van der, accented chars, etc.
    title = extract_title_author_particle_aware(ref_text)
    if title:
        title = clean_title_editor_list(title)
        return clean_title_question_mark_fix(title)

    # Try direct "Title. In Venue" pattern as fallback
    title = extract_title_direct_in_venue(ref_text)
    if title:
        title = clean_title_editor_list(title)
        return clean_title_question_mark_fix(title)

    # Fall back to native extraction
    ext = PdfExtractor()
    ref = ext.parse_reference(original_text)
    if ref and ref.title:
        title = clean_title_editor_list(ref.title)
        return clean_title_question_mark_fix(title)

    return None


def test_combined_extraction():
    """Test combined title extraction with all improvements."""
    print("=" * 60)
    print("COMBINED: Title Extraction with All Improvements")
    print("=" * 60)

    test_cases = [
        # Chinese ALL CAPS with [J] marker
        (
            'CAO X, YANG B, WANG K, et al. AI-empowered multiple access for 6G[J]. '
            'Proceedings of the IEEE, 2024.',
            'AI-empowered multiple access for 6G'
        ),
        # Question mark with venue leak (using Chinese format to test our extractor)
        (
            'SMITH J, JONES A, et al. Is machine learning the answer?[J] '
            'IEEE Trans. AI 2024.',
            'Is machine learning the answer?'
        ),
        # Standard format - test that we fall back correctly
        (
            '[42] Jones, A. "A comprehensive survey on neural networks," Proc. AAAI, 2023.',
            'A comprehensive survey on neural networks'  # Rust may include comma
        ),
    ]

    for ref_text, expected in test_cases:
        result = extract_title_with_improvements(ref_text)
        if result is None:
            print(f"  FAIL: No title from '{ref_text[:50]}...'")
        elif result.rstrip(',?') == expected.rstrip(',?'):
            print(f"  OK: '{result}'")
        else:
            print(f"  PARTIAL:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print()


# =============================================================================
# REGEX PATTERNS TO PORT TO RUST
# =============================================================================


def print_patterns_to_port():
    """Print all regex patterns that should be ported to Rust."""
    print("=" * 60)
    print("REGEX PATTERNS TO PORT TO RUST")
    print("=" * 60)
    print()

    print("1. Chinese ALL CAPS author detection:")
    print("   ^([A-Z]{2,})\\s+[A-Z](?:,|\\s|$)")
    print()

    print("2. Chinese et al. detection:")
    print("   ,?\\s+et\\s+al\\.?\\s*[,.]?\\s*")
    print()

    print("3. Chinese citation markers (title terminators):")
    print("   \\[J\\]  - Journal")
    print("   \\[C\\]  - Conference")
    print("   \\[M\\]  - Book (monograph)")
    print("   \\[D\\]  - Dissertation")
    print()

    print("4. Venue leak after question mark:")
    print("   [?!]\\s+(?:IEEE\\s+Trans[a-z.]*|ACM\\s+Trans[a-z.]*|Automatica|")
    print("          J\\.\\s*[A-Z][a-z]+|[A-Z][a-z]+\\.?\\s+[A-Z][a-z]+\\.?)\\s+\\d+\\s*\\(")
    print()

    print("5. Reference number prefixes to strip:")
    print("   ^\\[\\d+\\]\\s*")
    print("   ^\\d+\\.\\s*")
    print()

    print("6. Format 5 skip condition (Chinese pattern):")
    print("   ^[A-Z]{2,}\\s+[A-Z](?:,|\\s)")
    print()

    print("7. H-infinity normalization (in matching.rs):")
    print("   Replace '∞' with 'infinity' before stripping non-alnum")
    print()

    print("8. Author names with particles/special characters:")
    print("   Initial pattern (handles accented):")
    print("     [A-Z\\u00C0-\\u00D6\\u00D8-\\u00DE´`']\\.(?:\\s*[A-Z]\\.)*")
    print()
    print("   Name particles (case-insensitive):")
    print("     von, van, de, del, della, di, da, dos, das, du,")
    print("     le, la, les, den, der, ten, ter, op, het")
    print()
    print("   Surname characters (Unicode letters + hyphens + apostrophes):")
    print("     [A-Za-z\\u00C0-\\u024F'´`'\\-]+")
    print()
    print("   Author list end pattern (finds where title starts):")
    print("     ,?\\s+and\\s+")
    print("     [A-Z\\u00C0-\\u00DE´`']\\.(?:\\s*[A-Z]\\.)*\\s*")  # Initials
    print("     (?:(?:von|van|de|del|della|di|da|le|la|den|der|ter)\\s+)*")  # Particles
    print("     [A-Za-z\\u00C0-\\u024F'´`'\\-]+(?:\\s+[...]+)*")  # Surname
    print("     \\.\\s+")  # Period after surname
    print("     ([A-Z0-9\\u00C0-\\u024F\\\"\\'])  <- Capture title start")
    print()

    print("9. Springer/LNCS format (colon after authors):")
    print("   Author end marker: ,\\s*[A-Z](?:\\.[A-Z])*\\.:\\s*")
    print("   Example: 'Micali, S., Ohta, K., Reyzin, L.: Title. In: Venue'")
    print("   Title extraction: ([A-Z][^:]{10,}?)(?:\\.\\s+(?:In:|In\\s|Journal))")
    print()

    print("10. Bracket citation format [CODE]:")
    print("   Bracket code: ^\\[([A-Z]+\\d+[a-z]?)\\]\\s*")
    print("   Example: '[ACGH20] Authors. Title. In Venue'")
    print("   After stripping bracket, find 'Authors. Title' boundary")
    print()

    print("11. Editor list removal from title:")
    print("   Pattern: \\.\\s*In\\s+[A-Z][a-z]+\\s+[A-Z][a-z]+")
    print("            (?:,\\s*[A-Z][a-z]+\\s+[A-Z][a-z]+)*")
    print("            (?:,\\s*and\\s*[A-Z][a-z]+...)?")
    print("            ,\\s*editors?,")
    print("   Example: 'Title. In John Smith, Jane Doe, editors, Venue'")
    print()

    print("13. Journal metadata detection (validate extracted text is title):")
    print("   Reject if starts with: ^In[:\\s]")
    print("   Reject if matches journal vol/page pattern:")
    print("     ^[A-Z][A-Za-z\\s&\\-]+\\s+\\d+\\s*\\(\\d+\\)\\s*[,:]\\s*\\d+[–\\-]\\d+\\s*\\(\\d{4}\\)")
    print("   Example rejections:")
    print("     'Educational Researcher 13(6), 4–16 (1984)' -> REJECTED (journal info)")
    print("     'In: Proceedings of ACM' -> REJECTED (venue marker)")
    print()

    print("14. Enhanced Springer title end detection:")
    print("   After finding 'Author.: ' colon marker, find title end at:")
    print("     \\. Journal Vol(Issue),")
    print("     \\. In[:\\s] or \\. Proceedings")
    print("     \\. IEEE|ACM|Nature etc.")
    print("     doi: or https://")
    print("   Example: 'Bloom, B.S.: The 2 sigma problem... Educational Researcher 13(6)'")
    print("            -> Extract 'The 2 sigma problem...' (stop at journal)")
    print()


# =============================================================================
# IMPROVEMENT 13: Title Cleanup for Journal Volume/Page/Year Leak
# =============================================================================
# The Rust parser sometimes extracts journal metadata instead of the title.
# Example failures:
#   "Educational Researcher 13(6), 4–16 (1984), doi:10" (should be the journal)
#   "Trends in Cognitive Sciences 3(4), 128–135 (1999)" (journal info leaked)
#
# These patterns indicate the extracted "title" is actually journal metadata:
#   - "JournalName Vol(Issue), Pages (Year)"
#   - "JournalName Vol:Pages (Year)"
#   - Starts with "In:" or "In " followed by venue
#
# Solution: Detect these patterns and mark extraction as failed.
#
# Location: hallucinator-pdf/src/title.rs (validation/cleanup)


def is_journal_metadata_not_title(text: str) -> bool:
    """Check if extracted text looks like journal metadata, not a title.

    Returns True if the text appears to be journal volume/page/year info
    rather than an actual paper title.
    """
    text = text.strip()

    # Pattern 1: Starts with "In:" or "In " (venue marker, not title)
    if re.match(r'^In[:\s]', text, re.IGNORECASE):
        return True

    # Pattern 2: Journal Vol(Issue), Pages (Year) format
    # Example: "Educational Researcher 13(6), 4–16 (1984)"
    # Example: "Trends in Cognitive Sciences 3(4), 128–135 (1999)"
    journal_vol_pattern = re.match(
        r'^[A-Z][A-Za-z\s&\-]+\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]\d+\s*\(\d{4}\)',
        text
    )
    if journal_vol_pattern:
        return True

    # Pattern 3: Journal Vol:Pages (Year) format
    # Example: "Nature 123:456-789 (2020)"
    journal_colon_pattern = re.match(
        r'^[A-Z][A-Za-z\s&\-]+\s+\d+\s*:\s*\d+[–\-]\d+\s*\(\d{4}\)',
        text
    )
    if journal_colon_pattern:
        return True

    # Pattern 4: Just volume/page info
    # Example: "13(6), 4–16 (1984)"
    vol_page_only = re.match(
        r'^\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]\d+',
        text
    )
    if vol_page_only:
        return True

    # Pattern 5: Journal name with year;volume format
    # Example: "IEEE Trans Dependable Secure Comput 2018;17(3):506–17"
    elsevier_pattern = re.match(
        r'^[A-Z][A-Za-z\s]+\d{4};\d+',
        text
    )
    if elsevier_pattern:
        return True

    return False


def clean_title_journal_leak(title: str) -> str:
    """Remove journal volume/page/year info that leaked into title.

    This handles cases where the title extraction grabbed too much and
    included trailing journal metadata.
    """
    if not title:
        return title

    # Pattern 1: ", Vol(Issue), Pages (Year)" at end
    title = re.sub(
        r',?\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]\d+\s*\(\d{4}\).*$',
        '', title
    )

    # Pattern 2: ". JournalName Vol(Issue)" at end
    title = re.sub(
        r'\.\s+[A-Z][A-Za-z\s]+\d+\s*\(\d+\)\s*[,:]\s*\d+.*$',
        '', title
    )

    # Pattern 3: ", doi:10..." at end
    title = re.sub(r',?\s*doi:\s*10\.\d+.*$', '', title, flags=re.IGNORECASE)

    # Pattern 4: Trailing URL
    title = re.sub(r',?\s*https?://.*$', '', title)

    # Pattern 5: "(Year), doi" or "(Year)." at end after title
    title = re.sub(r'\s*\(\d{4}\)\s*[,.]?\s*(?:doi)?.*$', '', title)

    return title.strip()


def test_journal_metadata_detection():
    """Test detection of journal metadata vs actual titles."""
    print("=" * 60)
    print("IMPROVEMENT 13: Detect Journal Metadata (Not Titles)")
    print("=" * 60)

    # Should be detected as journal metadata (not titles)
    metadata_cases = [
        "Educational Researcher 13(6), 4–16 (1984), doi:10",
        "Trends in Cognitive Sciences 3(4), 128–135 (1999)",
        "User Modeling and User-Adapted Interaction 4(4), 253–278 (1995), doi:10",
        "In: Proceedings of the 46th IEEE/ACM International Conference",
        "In Ouyang, F., Jiao, P., McLaren, B.M., Alavi, A.H",
        "Nature 123:456-789 (2020)",
        "13(6), 4–16 (1984)",
    ]

    # Should NOT be detected as metadata (these are real titles)
    title_cases = [
        "The 2 sigma problem: The search for methods of group instruction",
        "Knowledge tracing: Modeling the acquisition of procedural knowledge",
        "Catastrophic forgetting in connectionist networks",
        "A survey on deep learning for cybersecurity",
    ]

    print("  Should be detected as journal metadata:")
    for text in metadata_cases:
        result = is_journal_metadata_not_title(text)
        status = "OK" if result else "FAIL"
        print(f"    {status}: '{text[:55]}...'")

    print("  Should NOT be detected as metadata (real titles):")
    for text in title_cases:
        result = is_journal_metadata_not_title(text)
        status = "OK" if not result else "FAIL"
        print(f"    {status}: '{text[:55]}...'")

    print()


# =============================================================================
# IMPROVEMENT 14: Springer/LNCS Enhanced Title Extraction
# =============================================================================
# The current Springer format (Improvement 9) needs enhancement to better
# extract titles from references like:
#   "Bloom, B.S.: The 2 sigma problem: The search for methods... Educational Researcher 13(6)..."
#
# The title should be "The 2 sigma problem: The search for methods of group instruction"
# But the Rust parser may be extracting the journal info instead.
#
# Enhanced pattern: After "Author.: " find title ending at journal patterns.


def extract_title_springer_enhanced(ref_text: str) -> Optional[str]:
    """Enhanced Springer/LNCS title extraction.

    Handles the common format: Author, I., Author, I.: Title. Journal Vol(Issue), Pages (Year)

    Key improvement: Better detection of where title ends and journal begins.
    """
    # Strip reference number prefixes
    ref_text = re.sub(r'^\[\d+\]\s*', '', ref_text)
    ref_text = re.sub(r'^\d+\.\s*', '', ref_text)

    # Look for "Initial.: " or "Initial.Initial.: " pattern marking author end
    # Examples: "B.S.:", "A.T.:", "L.:", "T.F.:", "C.P.:"
    colon_match = re.search(
        r'(?:,\s*)?'  # Optional comma before final author
        r'([A-Z](?:\.[A-Z])*\.)'  # Initials like "B.S." or "L."
        r':\s*'  # Colon after authors
        r'(.+)',  # Everything after colon
        ref_text
    )

    if not colon_match:
        return None

    after_colon = colon_match.group(2).strip()

    # Find where title ends - comprehensive journal/venue patterns
    title_end_patterns = [
        # Standard journal patterns
        r'\.\s+[A-Z][A-Za-z\s&\-]+\s+\d+\s*\(\d+\)\s*[,:]',  # ". Journal 13(6),"
        r'\.\s+[A-Z][A-Za-z\s&\-]+\s+\d+\s*[,:]\s*\d+',  # ". Journal 13, 456" or ": 456"
        # In/In: venue markers
        r'\.\s+In[:\s]\s*[A-Z]',  # ". In Proceedings" or ". In: Proceedings"
        r'\.\s+(?:Proceedings|Proc\.)\s+of',  # ". Proceedings of"
        # Common journals/venues
        r'\.\s+(?:IEEE|ACM|USENIX|arXiv|Nature|Science)\b',
        r'\.\s+(?:Journal|Trans\.|Transactions|Review)\s+(?:of|on)',
        # Volume patterns without journal name
        r'\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]',  # " 13(6), 456-"
        # DOI/URL
        r',?\s+doi:\s*10\.',
        r',?\s+https?://',
        # Publisher locations
        r'\.\s+(?:Springer|Elsevier|Wiley|Cambridge|Oxford)\b',
        # Year in parentheses at end of title-like segment
        r'\.\s+[A-Z][A-Za-z\s]+\s+\(\d{4}\)',  # ". Venue Name (2020)"
    ]

    title_end = len(after_colon)
    for pattern in title_end_patterns:
        m = re.search(pattern, after_colon)
        if m:
            title_end = min(title_end, m.start())

    title = after_colon[:title_end].strip()
    # Clean up trailing punctuation
    title = re.sub(r'[.,;:]+\s*$', '', title)

    # Validate: not too short, not journal metadata
    if len(title.split()) < 2:
        return None
    if is_journal_metadata_not_title(title):
        return None

    return title


def test_springer_enhanced():
    """Test enhanced Springer/LNCS format extraction."""
    print("=" * 60)
    print("IMPROVEMENT 14: Enhanced Springer/LNCS Title Extraction")
    print("=" * 60)

    test_cases = [
        (
            "Bloom, B.S.: The 2 sigma problem: The search for methods of group instruction "
            "as effective as one-to-one tutoring. Educational Researcher 13(6), 4–16 (1984)",
            "The 2 sigma problem: The search for methods of group instruction as effective as one-to-one tutoring"
        ),
        (
            "Corbett, A.T., Anderson, J.R.: Knowledge tracing: Modeling the acquisition of "
            "procedural knowledge. User Modeling and User-Adapted Interaction 4(4), 253–278 (1995)",
            "Knowledge tracing: Modeling the acquisition of procedural knowledge"
        ),
        (
            "French, R.M.: Catastrophic forgetting in connectionist networks. "
            "Trends in Cognitive Sciences 3(4), 128–135 (1999)",
            "Catastrophic forgetting in connectionist networks"
        ),
        (
            "Kulik, J.A., Fletcher, J.D.: Effectiveness of intelligent tutoring systems: "
            "A meta-analytic review. Review of Educational Research 86(1), 42–78 (2016)",
            "Effectiveness of intelligent tutoring systems: A meta-analytic review"
        ),
        (
            "Garofalo, J., Lester, F.K.: Metacognition, cognitive monitoring, and mathematical "
            "performance. Journal for Research in Mathematics Education 16(3), 163–176 (1985)",
            "Metacognition, cognitive monitoring, and mathematical performance"
        ),
        # Existing test case (should still work)
        (
            "Micali, S., Ohta, K., Reyzin, L.: Accountable-subgroup multisignatures. "
            "In: Proceedings of the 8th ACM Conference",
            "Accountable-subgroup multisignatures"
        ),
    ]

    passed = 0
    for raw, expected in test_cases:
        result = extract_title_springer_enhanced(raw)
        if result is None:
            print(f"  FAIL: No title extracted")
            print(f"    Raw: {raw[:70]}...")
        elif result.lower().rstrip('.,') == expected.lower().rstrip('.,'):
            print(f"  OK: '{result[:60]}{'...' if len(result) > 60 else ''}'")
            passed += 1
        else:
            print(f"  MISMATCH:")
            print(f"    Got:      {result}")
            print(f"    Expected: {expected}")

    print(f"\n  Passed: {passed}/{len(test_cases)}")
    print()


# Regex patterns for Rust implementation - Improvements 13-14
JOURNAL_METADATA_PATTERNS = {
    # Patterns that indicate extracted text is journal metadata, not title
    "in_venue_start": r"^In[:\s]",  # Starts with "In:" or "In "
    "journal_vol_issue": r"^[A-Z][A-Za-z\s&\-]+\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]\d+\s*\(\d{4}\)",
    "journal_colon_pages": r"^[A-Z][A-Za-z\s&\-]+\s+\d+\s*:\s*\d+[–\-]\d+\s*\(\d{4}\)",
    "vol_page_only": r"^\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]\d+",
}

TITLE_CLEANUP_PATTERNS = {
    # Patterns to remove from end of title
    "trailing_vol_page_year": r",?\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]\d+\s*\(\d{4}\).*$",
    "trailing_journal_vol": r"\.\s+[A-Z][A-Za-z\s]+\d+\s*\(\d+\)\s*[,:]\s*\d+.*$",
    "trailing_doi": r",?\s*doi:\s*10\.\d+.*$",
    "trailing_url": r",?\s*https?://.*$",
    "trailing_year_paren": r"\s*\(\d{4}\)\s*[,.]?\s*(?:doi)?.*$",
}

SPRINGER_TITLE_END_PATTERNS = [
    # Enhanced patterns for finding where Springer title ends
    r"\.\s+[A-Z][A-Za-z\s&\-]+\s+\d+\s*\(\d+\)\s*[,:]",  # ". Journal 13(6),"
    r"\.\s+[A-Z][A-Za-z\s&\-]+\s+\d+\s*[,:]\s*\d+",  # ". Journal 13, 456"
    r"\.\s+In[:\s]\s*[A-Z]",  # ". In Proceedings" or ". In: Proceedings"
    r"\.\s+(?:Proceedings|Proc\.)\s+of",
    r"\.\s+(?:IEEE|ACM|USENIX|arXiv|Nature|Science)\b",
    r"\.\s+(?:Journal|Trans\.|Transactions|Review)\s+(?:of|on)",
    r"\s+\d+\s*\(\d+\)\s*[,:]\s*\d+[–\-]",  # Volume without journal
    r",?\s+doi:\s*10\.",
    r",?\s+https?://",
]


# =============================================================================
# MAIN
# =============================================================================


if __name__ == "__main__":
    test_h_infinity_normalization()
    test_chinese_allcaps_format()
    test_venue_leak_after_question()
    test_two_word_quoted_titles()
    test_reference_prefix_stripping()
    test_format5_skip_detection()
    test_author_particle_names()
    test_springer_format()
    test_bracket_code_format()
    test_editor_list_cleaning()
    test_direct_in_venue()
    test_journal_metadata_detection()
    test_springer_enhanced()
    test_combined_extraction()
    print_patterns_to_port()

    print("=" * 60)
    print("All tests completed.")
    print()
    print("To port these improvements to Rust, update:")
    print("  - hallucinator-pdf/src/title.rs (format detection)")
    print("  - hallucinator-core/src/matching.rs (H-infinity normalization)")
    print("=" * 60)
