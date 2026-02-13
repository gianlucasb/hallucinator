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
# COMBINED: Custom Title Extractor with All Improvements
# =============================================================================


def extract_title_with_improvements(ref_text: str) -> Optional[str]:
    """Extract title using all improvements.

    This combines all the improvements into a single function that can be
    used to validate behavior before porting to Rust.
    """
    # Preprocessing
    ref_text = strip_reference_prefix(ref_text)

    # Try Chinese ALL CAPS format first (before Format 5)
    if should_skip_format5_for_chinese(ref_text):
        title = extract_title_chinese_allcaps(ref_text)
        if title:
            return clean_title_question_mark_fix(title)

    # Fall back to native extraction
    ext = PdfExtractor()
    ref = ext.parse_reference(ref_text)
    if ref and ref.title:
        return clean_title_question_mark_fix(ref.title)

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
    test_combined_extraction()
    print_patterns_to_port()

    print("=" * 60)
    print("All tests completed.")
    print()
    print("To port these improvements to Rust, update:")
    print("  - hallucinator-pdf/src/title.rs (format detection)")
    print("  - hallucinator-core/src/matching.rs (H-infinity normalization)")
    print("=" * 60)
