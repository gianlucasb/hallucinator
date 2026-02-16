#!/usr/bin/env python3
"""
FTS5 Query Normalization Patterns for Rust Port

This script documents the regex patterns and normalization logic needed to
robustly match PDF-extracted titles against FTS5 indexes.

Background:
-----------
PDF text extraction often produces titles with formatting artifacts:
- Hyphens removed: "C-FLAT" becomes "CFLAT"
- Compounds merged: "Cross-Privilege-Boundary" becomes "Crossprivilege-boundary"
- Special characters stripped or mangled

FTS5 uses implicit AND between terms, so if any term doesn't exist in the
index, the entire query fails. This module filters out problematic terms
and selects the most distinctive words for reliable matching.

Usage:
------
    python fts5_query_normalization.py

    # Or import as module:
    from fts5_query_normalization import build_fts5_query
    query = build_fts5_query("ZombieLoad: Crossprivilege-boundary data sampling")
"""

import re
from typing import List, Optional, Set

# =============================================================================
# Regex Patterns (for Rust port)
# =============================================================================

# Matches word characters (letters only, used for initial word extraction)
# Rust: r"[a-zA-Z]+"
WORD_RE = re.compile(r'[a-zA-Z]+')

# Matches hyphenated compound words (e.g., "control-flow", "Cross-Privilege-Boundary")
# Rust: r"[a-zA-Z]+-[a-zA-Z]+(?:-[a-zA-Z]+)*"
HYPHENATED_WORD_RE = re.compile(r'[a-zA-Z]+-[a-zA-Z]+(?:-[a-zA-Z]+)*')

# Detects camelCase or PascalCase patterns indicating merged compounds
# Matches words with uppercase letters after the first character
# Examples: "CrossPrivilege", "ZombieLoad", "SGXDump"
# Rust: r"^[a-zA-Z][a-z]*[A-Z]"
CAMEL_CASE_RE = re.compile(r'^[a-zA-Z][a-z]*[A-Z]')

# Detects all-uppercase words (potential acronyms)
# Used to identify merged acronyms like "CFLAT" (from "C-FLAT")
# Rust: r"^[A-Z]+$"
ALL_CAPS_RE = re.compile(r'^[A-Z]+$')

# Matches non-alphanumeric characters for title normalization
# Rust: r"[^a-zA-Z0-9]"
NON_ALNUM_RE = re.compile(r'[^a-zA-Z0-9]')

# =============================================================================
# Constants
# =============================================================================

# Maximum length for a word to be considered valid for FTS5 queries
# Words longer than this are likely merged compounds that won't match
# Example: "Crossprivilege" (14 chars) from "Cross-Privilege"
MAX_WORD_LENGTH = 12

# Minimum length for a word part when splitting hyphenated words
MIN_WORD_PART_LENGTH = 3

# Maximum length for an all-caps word to be included in queries
# Longer all-caps words are likely merged acronyms (e.g., "CFLAT" from "C-FLAT")
MAX_ACRONYM_LENGTH = 4

# Number of top-scoring words to include in FTS5 queries
MAX_QUERY_WORDS = 4

# Scoring weights
CAPITALIZED_BONUS = 10.0   # Bonus for capitalized words (proper nouns, acronyms)
ACRONYM_BONUS = 5.0        # Additional bonus for short all-caps words
POSITION_PENALTY = 0.5     # Penalty per position in title

# Common stop words to exclude from FTS5 queries
STOP_WORDS: Set[str] = {
    "the", "and", "for", "with", "from", "that", "this", "have", "are", "was", "were",
    "been", "being", "has", "had", "does", "did", "will", "would", "could", "should",
    "may", "might", "must", "shall", "can", "not", "but", "its", "our", "their", "your",
    "into", "over", "under", "about", "between", "through", "during", "before", "after",
    "above", "below", "each", "every", "both", "few", "more", "most", "other", "some",
    "such", "only", "than", "too", "very",
}

# =============================================================================
# Word Classification Functions
# =============================================================================

def is_merged_compound(word: str) -> bool:
    """
    Check if a word is likely a merged compound that won't match in FTS5.

    Examples:
        >>> is_merged_compound("Crossprivilege")  # > 12 chars
        True
        >>> is_merged_compound("CrossPrivilege")  # camelCase > 10 chars
        True
        >>> is_merged_compound("ZombieLoad")      # 10 chars, has camelCase but not > 10
        False
        >>> is_merged_compound("attestation")     # normal word
        False
    """
    # Very long words are likely merged compounds
    if len(word) > MAX_WORD_LENGTH:
        return True

    # CamelCase words > 10 chars are likely merged compounds
    if len(word) > 10 and CAMEL_CASE_RE.match(word):
        return True

    return False


def is_merged_acronym(word: str) -> bool:
    """
    Check if a word is likely a merged acronym that won't match in FTS5.

    Examples:
        >>> is_merged_acronym("CFLAT")    # 5 chars, all caps -> likely C-FLAT
        True
        >>> is_merged_acronym("SGXDUMP")  # 7 chars, all caps
        True
        >>> is_merged_acronym("SGX")      # 3 chars, valid acronym
        False
        >>> is_merged_acronym("TLS")      # 3 chars, valid acronym
        False
        >>> is_merged_acronym("BERT")     # 4 chars, valid acronym
        False
    """
    return bool(ALL_CAPS_RE.match(word)) and len(word) > MAX_ACRONYM_LENGTH


def should_exclude_word(word: str) -> bool:
    """Check if a word should be excluded from FTS5 queries."""
    # Too short
    if len(word) < MIN_WORD_PART_LENGTH:
        return True

    # Stop word
    if word.lower() in STOP_WORDS:
        return True

    # Merged compound
    if is_merged_compound(word):
        return True

    # Merged acronym
    if is_merged_acronym(word):
        return True

    return False

# =============================================================================
# Word Extraction and Normalization
# =============================================================================

def split_hyphenated(word: str) -> List[str]:
    """
    Split a hyphenated word into parts, filtering invalid parts.

    Examples:
        >>> split_hyphenated("control-flow")
        ['control', 'flow']
        >>> split_hyphenated("Cross-Privilege-Boundary")
        ['Cross', 'Privilege', 'Boundary']
        >>> split_hyphenated("Crossprivilege-boundary")  # Crossprivilege filtered (>12)
        ['boundary']
        >>> split_hyphenated("A-B")  # Parts too short
        []
    """
    return [
        part for part in word.split('-')
        if MIN_WORD_PART_LENGTH <= len(part) <= MAX_WORD_LENGTH
    ]


def extract_query_words(title: str) -> List[str]:
    """
    Extract and normalize words from a title for FTS5 queries.

    This function:
    1. Extracts words and hyphenated compounds
    2. Splits hyphenated words into parts
    3. Filters out merged compounds, acronyms, and stop words
    4. Deduplicates while preserving order

    Examples:
        >>> words = extract_query_words("ZombieLoad: Crossprivilege-boundary data sampling")
        >>> "ZombieLoad" in words
        True
        >>> "boundary" in words
        True
        >>> "Crossprivilege" in words  # filtered as merged compound
        False

        >>> words = extract_query_words("CFLAT: control-flow attestation")
        >>> "control" in words
        True
        >>> "flow" in words
        True
        >>> "CFLAT" in words  # filtered as merged acronym
        False
    """
    words = []
    seen = set()

    # First pass: extract hyphenated words and split them
    for match in HYPHENATED_WORD_RE.finditer(title):
        for part in split_hyphenated(match.group()):
            lower = part.lower()
            if lower not in seen and not should_exclude_word(part):
                seen.add(lower)
                words.append(part)

    # Second pass: extract non-hyphenated words
    for match in WORD_RE.finditer(title):
        word = match.group()
        lower = word.lower()

        # Skip if already seen (from hyphenated splitting)
        if lower in seen:
            continue

        # Skip if it should be excluded
        if should_exclude_word(word):
            continue

        seen.add(lower)
        words.append(word)

    return words

# =============================================================================
# Word Scoring
# =============================================================================

def word_score(word: str, position: int) -> float:
    """
    Calculate a distinctiveness score for a word.

    Higher scores indicate more distinctive words that are better for FTS5 queries.

    Scoring factors:
    - Base score: word length
    - +10 for capitalized words (proper nouns, acronyms)
    - +5 for short all-caps words (valid acronyms)
    - -0.5 per position in title (earlier is slightly better)

    Examples:
        >>> word_score("Return", 0) > word_score("return", 0)  # capitalized bonus
        True
        >>> word_score("SGX", 0) > word_score("sgx", 0)  # acronym bonus
        True
        >>> word_score("word", 0) > word_score("word", 5)  # position penalty
        True
    """
    score = float(len(word))

    # Boost capitalized words
    if word[0].isupper():
        score += CAPITALIZED_BONUS

    # Extra boost for short all-caps (valid acronyms like SGX, TLS)
    if ALL_CAPS_RE.match(word) and 3 <= len(word) <= MAX_ACRONYM_LENGTH:
        score += ACRONYM_BONUS

    # Slight penalty for later position
    score -= position * POSITION_PENALTY

    return score


def select_top_words(words: List[str], max_words: int = MAX_QUERY_WORDS) -> List[str]:
    """
    Select the top N most distinctive words for an FTS5 query.

    Examples:
        >>> words = ["Return", "oriented", "programming", "Systems", "languages", "applications"]
        >>> top = select_top_words(words, 4)
        >>> "Return" in top  # capitalized, should be prioritized
        True
        >>> "Systems" in top  # capitalized, should be prioritized
        True
    """
    scored = [(word_score(w, i), w) for i, w in enumerate(words)]
    scored.sort(key=lambda x: -x[0])  # Sort by score descending
    return [w for _, w in scored[:max_words]]

# =============================================================================
# FTS5 Query Building
# =============================================================================

def quote_fts5_word(word: str) -> str:
    """
    Escape a word for use in FTS5 MATCH queries.

    FTS5 requires double-quoting to prevent interpretation as operators.
    Internal double quotes are escaped by doubling them.

    Examples:
        >>> quote_fts5_word("word")
        '"word"'
        >>> quote_fts5_word("it's")
        '"it\\'s"'
        >>> quote_fts5_word('say "hello"')
        '"say ""hello""'
    """
    return '"' + word.replace('"', '""') + '"'


def build_fts5_query(title: str) -> Optional[str]:
    """
    Build an FTS5 MATCH query string from a title.

    This is the main entry point for FTS5 query generation. It:
    1. Extracts and normalizes words
    2. Selects the most distinctive words
    3. Quotes them for FTS5
    4. Joins with spaces (implicit AND)

    Examples:
        >>> query = build_fts5_query("ZombieLoad: Crossprivilege-boundary data sampling")
        >>> '"ZombieLoad"' in query
        True
        >>> 'Crossprivilege' in query  # should be filtered out
        False

        >>> query = build_fts5_query("Return-oriented programming: Systems, languages, and applications")
        >>> '"Return"' in query  # capitalized, should be included despite being shorter
        True
        >>> '"Systems"' in query
        True

        >>> build_fts5_query("")  # empty title
        >>> build_fts5_query("the and for")  # all stop words
    """
    words = extract_query_words(title)
    if not words:
        return None

    top_words = select_top_words(words, MAX_QUERY_WORDS)
    if not top_words:
        return None

    quoted = [quote_fts5_word(w) for w in top_words]
    return ' '.join(quoted)

# =============================================================================
# Test Cases (for Rust port verification)
# =============================================================================

TEST_CASES = [
    # (title, expected_found_words, expected_excluded_words)
    (
        "ZombieLoad: Crossprivilege-boundary data sampling",
        ["ZombieLoad", "boundary", "data", "sampling"],
        ["Crossprivilege"],  # merged compound > 12 chars
    ),
    (
        "CFLAT: control-flow attestation for embedded systems software",
        ["control", "flow", "attestation", "embedded", "systems", "software"],
        ["CFLAT"],  # merged acronym > 4 chars
    ),
    (
        "Return-oriented programming: Systems, languages, and applications",
        ["Return", "oriented", "programming", "Systems", "languages", "applications"],
        [],
    ),
    (
        "C-FLAT: Control-Flow Attestation for Embedded Systems Software",
        ["FLAT", "Control", "Flow", "Attestation", "Embedded", "Systems", "Software"],
        [],  # C is too short, but hyphen splitting works
    ),
    (
        "SGX: Secure enclaves for trusted execution",
        ["SGX", "Secure", "enclaves", "trusted", "execution"],
        [],  # SGX is a valid short acronym
    ),
    (
        "SGXDUMP: extracting enclave memory",
        ["extracting", "enclave", "memory"],
        ["SGXDUMP"],  # merged acronym > 4 chars
    ),
]


def run_tests():
    """Run test cases and print results."""
    print("=" * 70)
    print("FTS5 Query Normalization Test Cases")
    print("=" * 70)

    all_passed = True

    for title, expected_found, expected_excluded in TEST_CASES:
        print(f"\nTitle: {title[:60]}...")

        words = extract_query_words(title)
        query = build_fts5_query(title)

        print(f"  Extracted words: {words}")
        print(f"  FTS5 query: {query}")

        # Check expected found words
        for word in expected_found:
            if word.lower() not in [w.lower() for w in words]:
                print(f"  FAIL: Expected to find '{word}'")
                all_passed = False

        # Check expected excluded words
        for word in expected_excluded:
            if word.lower() in [w.lower() for w in words]:
                print(f"  FAIL: Expected to exclude '{word}'")
                all_passed = False

    print("\n" + "=" * 70)

    # Test word scoring
    print("\nWord Scoring Tests:")
    print(f"  word_score('Return', 0) = {word_score('Return', 0):.1f}")
    print(f"  word_score('return', 0) = {word_score('return', 0):.1f}")
    print(f"  word_score('SGX', 0) = {word_score('SGX', 0):.1f}")
    print(f"  word_score('programming', 0) = {word_score('programming', 0):.1f}")

    # Test top word selection
    print("\nTop Word Selection Test:")
    words = ["Return", "oriented", "programming", "Systems", "languages", "applications"]
    top = select_top_words(words, 4)
    print(f"  Input: {words}")
    print(f"  Top 4: {top}")

    if "Return" not in top or "Systems" not in top:
        print("  FAIL: Capitalized words should be prioritized")
        all_passed = False

    print("\n" + "=" * 70)
    if all_passed:
        print("All tests PASSED")
    else:
        print("Some tests FAILED")
    print("=" * 70)

    return all_passed


if __name__ == "__main__":
    run_tests()
