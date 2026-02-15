#!/usr/bin/env python3
"""
NeurIPS Format Fixes for Reference Parsing

This file documents patterns and fixes discovered from analyzing 51 NeurIPS papers
(2,582 references). These patterns should be ported to the Rust engine in:
  - hallucinator-core/src/matching.rs (title normalization/validation)
  - hallucinator-pdf/src/title.rs (title extraction validation)

Issues found (17 problematic refs, 0.7%):
  1. Title ending with ?/! followed by venue name (3 cases)
  2. Venue/journal name extracted as title (1 case)
  3. Author initials list as title - "AL, Name Name," format (1 case)
  4. Very long titles with author lists (4 cases)
  5. Non-reference content (checklists, acknowledgments) (8 cases)

Run this file to test all patterns:
    python neurips_fps_regexps.py
"""

import re
from typing import Optional, List, Tuple


# =============================================================================
# FIX 1: Title Ending with ?/! Followed by Venue
# =============================================================================
# Some reference formats don't properly delimit title from venue when the
# title ends with a question mark or exclamation point.
#
# Examples from NeurIPS papers:
#   "Can unconfident llm annotations be used for confident conclusions? Nations
#    of the Americas Chapter of the Association for Computational Linguistics"
#   "Can large language models be an alternative to human evaluations? The 2023
#    Conference on Empirical Methods in Natural Language Processing (EMNLP)"
#   "In-context learning for discrete optimal transport: Can transformers sort?
#    International Conference on Artificial Intelligence and Statistics"
#
# Location: hallucinator-pdf/src/title.rs (post-extraction validation)

VENUE_AFTER_PUNCTUATION_PATTERN = re.compile(
    r'[?!]\s+(?:International|Proceedings|Conference|Workshop|Symposium|Association|'
    r'The\s+\d{4}\s+Conference|Nations|Annual|IEEE|ACM|USENIX|AAAI|NeurIPS|ICML|ICLR|'
    r'CVPR|ICCV|ECCV|ACL|EMNLP|NAACL)'
)


def truncate_title_at_venue(title: str) -> str:
    """Truncate title if it contains venue name after ?/! punctuation.

    Returns the truncated title (keeping the ?/!) or original if no venue found.
    """
    match = VENUE_AFTER_PUNCTUATION_PATTERN.search(title)
    if match:
        # Keep everything up to and including the ?/!
        return title[:match.start() + 1].strip()
    return title


def test_venue_after_punctuation():
    """Test venue-after-punctuation truncation."""
    print("=" * 60)
    print("FIX 1: Title? + Venue Truncation")
    print("=" * 60)

    test_cases = [
        # Should be truncated
        ("Can unconfident llm annotations be used? Nations of the Americas Chapter",
         "Can unconfident llm annotations be used?"),
        ("Can transformers sort? International Conference on AI",
         "Can transformers sort?"),
        ("Is this the answer! The 2023 Conference on Methods",
         "Is this the answer!"),
        # Should NOT be truncated (no venue after ?)
        ("Can LLMs keep a secret? Testing privacy implications",
         "Can LLMs keep a secret? Testing privacy implications"),
        ("What does BERT learn? A study of representations",
         "What does BERT learn? A study of representations"),
    ]

    for original, expected in test_cases:
        result = truncate_title_at_venue(original)
        status = "OK" if result == expected else "FAIL"
        print(f"  {status}: '{original[:50]}...'")
        if result != expected:
            print(f"       Expected: '{expected}'")
            print(f"       Got:      '{result}'")

    print()


# Rust implementation pattern:
RUST_VENUE_AFTER_PUNCTUATION = '''
// In title.rs, after extracting title:

use once_cell::sync::Lazy;
use regex::Regex;

static VENUE_AFTER_PUNCTUATION_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"[?!]\\s+(?:International|Proceedings|Conference|Workshop|Symposium|Association|\
        The\\s+\\d{4}\\s+Conference|Nations|Annual|IEEE|ACM|USENIX|AAAI|NeurIPS|ICML|ICLR|\
        CVPR|ICCV|ECCV|ACL|EMNLP|NAACL)"
    ).unwrap()
});

fn truncate_title_at_venue(title: &str) -> String {
    if let Some(m) = VENUE_AFTER_PUNCTUATION_RE.find(title) {
        // Keep up to and including the ?/!
        title[..m.start() + 1].trim().to_string()
    } else {
        title.to_string()
    }
}
'''


# =============================================================================
# FIX 2: Venue/Journal Name as Title
# =============================================================================
# Sometimes the extraction grabs a venue or journal name instead of the title.
# These should be rejected.
#
# Examples from NeurIPS papers:
#   "SIAM Journal on Scientific Computing"
#   "Advances in Neural Information Processing Systems"
#   "Proceedings of the International Conference on..."
#
# Location: hallucinator-pdf/src/title.rs (post-extraction validation)

VENUE_ONLY_PATTERNS = [
    # SIAM/IEEE/ACM Journal/Transactions/Review
    re.compile(r'^(?:SIAM|IEEE|ACM|PNAS)\s+(?:Journal|Transactions|Review)', re.IGNORECASE),
    # Journal/Transactions/Proceedings of/on
    re.compile(r'^(?:Journal|Transactions|Proceedings)\s+(?:of|on)\s+', re.IGNORECASE),
    # Advances in Neural Information Processing Systems
    re.compile(r'^Advances\s+in\s+Neural', re.IGNORECASE),
]


def is_venue_only(text: str) -> bool:
    """Check if text is just a venue/journal name, not a paper title."""
    for pattern in VENUE_ONLY_PATTERNS:
        if pattern.match(text):
            return True
    return False


def test_venue_only():
    """Test venue-only detection."""
    print("=" * 60)
    print("FIX 2: Venue-Only Detection")
    print("=" * 60)

    # Should be detected as venue-only (rejected)
    venue_only = [
        "SIAM Journal on Scientific Computing",
        "IEEE Transactions on Pattern Analysis",
        "ACM Journal on Computing Surveys",
        "Journal of Machine Learning Research",
        "Proceedings of the International Conference",
        "Advances in Neural Information Processing Systems",
    ]

    # Should NOT be detected (valid titles)
    valid_titles = [
        "A Survey of Machine Learning Techniques",
        "Neural Networks for Image Recognition",
        "Deep Learning: A Comprehensive Overview",
        "Attention Is All You Need",
    ]

    print("  Should be detected as venue-only:")
    for text in venue_only:
        result = is_venue_only(text)
        status = "OK" if result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print("  Should NOT be detected as venue-only:")
    for text in valid_titles:
        result = is_venue_only(text)
        status = "OK" if not result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print()


# Rust implementation pattern:
RUST_VENUE_ONLY = '''
// In title.rs:

static VENUE_ONLY_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| vec![
    Regex::new(r"(?i)^(?:SIAM|IEEE|ACM|PNAS)\\s+(?:Journal|Transactions|Review)").unwrap(),
    Regex::new(r"(?i)^(?:Journal|Transactions|Proceedings)\\s+(?:of|on)\\s+").unwrap(),
    Regex::new(r"(?i)^Advances\\s+in\\s+Neural").unwrap(),
]);

fn is_venue_only(text: &str) -> bool {
    VENUE_ONLY_PATTERNS.iter().any(|re| re.is_match(text))
}
'''


# =============================================================================
# FIX 3: Author Initials List as Title
# =============================================================================
# Extended author-list detection for NeurIPS-style formats where organization
# papers have short initials followed by full names.
#
# Examples from NeurIPS papers:
#   "AL, Andrew Ahn, Nic Becker, Stephanie Carroll, Nico Christie..."
#   (This is from "Altera. AL, Andrew Ahn, ..." where Altera is the org)
#
# This pattern extends the existing AUTHOR_LIST_PATTERNS with a new case.
#
# Location: hallucinator-pdf/src/title.rs (title extraction validation)

# New pattern to add to existing AUTHOR_LIST_PATTERNS
# Must have: initials, FirstName LastName, followed by another FirstName (not "and")
AUTHOR_INITIALS_LIST_PATTERN = re.compile(
    r'^[A-Z]{1,3},\s+[A-Z][a-z]+\s+[A-Z][a-z]+,\s+[A-Z][a-z]+\s+[A-Z][a-z]+'
)


def is_author_initials_list(text: str) -> bool:
    """Check if text looks like 'AL, Name Name,' style author list."""
    return bool(AUTHOR_INITIALS_LIST_PATTERN.match(text))


def test_author_initials_list():
    """Test author initials list detection."""
    print("=" * 60)
    print("FIX 3: Author Initials List Detection")
    print("=" * 60)

    # Should be detected as author list (rejected)
    author_lists = [
        "AL, Andrew Ahn, Nic Becker, Stephanie Carroll,",
        "AB, John Smith, Jane Doe, Bob Wilson,",
        "XYZ, First Last, Another Name, Third Person",
    ]

    # Should NOT be detected (valid titles)
    valid_titles = [
        "AI, Machine Learning, and Deep Networks",  # AI is acronym, not initials
        "Attention Is All You Need",
        "BERT: Pre-training of Deep Bidirectional",
        "GPT-4 Technical Report",
    ]

    print("  Should be detected as author initials list:")
    for text in author_lists:
        result = is_author_initials_list(text)
        status = "OK" if result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print("  Should NOT be detected as author initials list:")
    for text in valid_titles:
        result = is_author_initials_list(text)
        status = "OK" if not result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print()


# Rust implementation pattern:
RUST_AUTHOR_INITIALS = '''
// Add to existing AUTHOR_LIST_PATTERNS in title.rs:

// Short initials followed by name list: "AL, Andrew Ahn, Nic Becker," (OpenAI-style)
Regex::new(r"^[A-Z]{1,3},\\s+[A-Z][a-z]+\\s+[A-Z][a-z]+,").unwrap(),
'''


# =============================================================================
# FIX 4: Non-Reference Content
# =============================================================================
# NeurIPS papers include checklists and acknowledgments that can be
# incorrectly extracted as references.
#
# Examples from NeurIPS papers:
#   "• The answer NA means that the paper has no limitation while..."
#   "We gratefully acknowledge the support of the OpenReview sponsors..."
#
# Location: hallucinator-pdf/src/references.rs (reference section detection)
#           hallucinator-pdf/src/title.rs (post-extraction validation)

NON_REFERENCE_PATTERNS = [
    # NeurIPS checklist bullet points
    re.compile(r'^[•\-]\s+(?:The answer|Released models|If you are using)', re.IGNORECASE),
    # Acknowledgments
    re.compile(r'^We gratefully acknowledge', re.IGNORECASE),
]


def is_non_reference_content(text: str) -> bool:
    """Check if text is non-reference content (checklists, acknowledgments)."""
    for pattern in NON_REFERENCE_PATTERNS:
        if pattern.match(text):
            return True
    return False


def test_non_reference_content():
    """Test non-reference content detection."""
    print("=" * 60)
    print("FIX 4: Non-Reference Content Detection")
    print("=" * 60)

    # Should be detected as non-reference (rejected)
    non_ref = [
        "• The answer NA means that the paper has no limitation",
        "- Released models that have a high risk for misuse",
        "We gratefully acknowledge the support of the OpenReview sponsors",
    ]

    # Should NOT be detected (valid titles)
    valid_titles = [
        "The Answer to Everything: A Survey",
        "We Present a Novel Approach to...",
        "Released: A New Dataset for...",
    ]

    print("  Should be detected as non-reference:")
    for text in non_ref:
        result = is_non_reference_content(text)
        status = "OK" if result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print("  Should NOT be detected as non-reference:")
    for text in valid_titles:
        result = is_non_reference_content(text)
        status = "OK" if not result else "FAIL"
        print(f"    {status}: '{text[:50]}...'")

    print()


# Rust implementation pattern:
RUST_NON_REFERENCE = '''
// In title.rs:

static NON_REFERENCE_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| vec![
    // NeurIPS checklist bullet points
    Regex::new(r"(?i)^[•\\-]\\s+(?:The answer|Released models|If you are using)").unwrap(),
    // Acknowledgments
    Regex::new(r"(?i)^We gratefully acknowledge").unwrap(),
]);

fn is_non_reference_content(text: &str) -> bool {
    NON_REFERENCE_PATTERNS.iter().any(|re| re.is_match(text))
}
'''


# =============================================================================
# FIX 5: Maximum Title Length
# =============================================================================
# Titles longer than ~300 characters are almost always extraction errors
# (merged references, author lists, etc.). While some legitimate titles can
# be long (especially in medical literature), >300 chars is a strong signal.
#
# Examples from NeurIPS papers:
#   "OpenAI, :, A. Hurst, A. Lerer, A. P. Goucher, A. Perelman, A. Ramesh..."
#   (4,504 chars - entire GPT-4o author list)
#
# Location: hallucinator-pdf/src/title.rs (post-extraction validation)

MAX_TITLE_LENGTH = 300


def is_title_too_long(title: str) -> bool:
    """Check if title exceeds maximum reasonable length."""
    return len(title) > MAX_TITLE_LENGTH


def test_title_length():
    """Test title length check."""
    print("=" * 60)
    print("FIX 5: Maximum Title Length")
    print("=" * 60)

    test_cases = [
        ("A" * 150, False, "150 chars - OK"),
        ("A" * 250, False, "250 chars - OK (long but valid)"),
        ("A" * 300, False, "300 chars - OK (at limit)"),
        ("A" * 301, True, "301 chars - TOO LONG"),
        ("A" * 500, True, "500 chars - TOO LONG"),
    ]

    for title, expected_too_long, desc in test_cases:
        result = is_title_too_long(title)
        status = "OK" if result == expected_too_long else "FAIL"
        print(f"  {status}: {desc}")

    print()


# Rust implementation pattern:
RUST_TITLE_LENGTH = '''
// In title.rs:

const MAX_TITLE_LENGTH: usize = 300;

fn is_title_too_long(title: &str) -> bool {
    title.len() > MAX_TITLE_LENGTH
}
'''


# =============================================================================
# COMBINED VALIDATION
# =============================================================================

def validate_extracted_title(title: str) -> Tuple[str, bool, Optional[str]]:
    """Validate and clean an extracted title.

    Returns:
        (cleaned_title, is_valid, rejection_reason)
    """
    # Truncate venue after ?/!
    title = truncate_title_at_venue(title)

    # Check for venue-only
    if is_venue_only(title):
        return title, False, "venue_only"

    # Check for author initials list
    if is_author_initials_list(title):
        return title, False, "author_initials_list"

    # Check for non-reference content
    if is_non_reference_content(title):
        return title, False, "non_reference_content"

    # Check length
    if is_title_too_long(title):
        return title, False, "too_long"

    return title, True, None


def test_combined_validation():
    """Test combined validation pipeline."""
    print("=" * 60)
    print("COMBINED: Full Validation Pipeline")
    print("=" * 60)

    test_cases = [
        ("Can transformers sort? International Conference on AI", True, "truncate venue"),
        ("SIAM Journal on Scientific Computing", False, "venue_only"),
        ("AL, Andrew Ahn, Nic Becker,", False, "author_initials_list"),
        ("• The answer NA means...", False, "non_reference_content"),
        ("A" * 400, False, "too_long"),
        ("Attention Is All You Need", True, "valid title"),
    ]

    for original, expected_valid, desc in test_cases:
        cleaned, is_valid, reason = validate_extracted_title(original)
        status = "OK" if is_valid == expected_valid else "FAIL"
        print(f"  {status}: {desc}")
        if not is_valid:
            print(f"       Rejected: {reason}")
        if cleaned != original:
            print(f"       Cleaned: '{cleaned[:50]}...'")

    print()


# =============================================================================
# MAIN
# =============================================================================

if __name__ == "__main__":
    test_venue_after_punctuation()
    test_venue_only()
    test_author_initials_list()
    test_non_reference_content()
    test_title_length()
    test_combined_validation()

    print("=" * 60)
    print("All tests completed!")
    print("=" * 60)
