"""Tests for the hallucinator Python bindings (Phases 1–2A: PDF extraction + custom strategies)."""

import pytest

from hallucinator import PdfExtractor, Reference, ExtractionResult
from hallucinator._native import NativePdfExtractor


# ── Construction ──


def test_default_constructor():
    ext = PdfExtractor()
    assert repr(ext) == "PdfExtractor(...)"


# ── find_section ──


def test_find_section_basic():
    ext = PdfExtractor()
    section = ext.find_section("Body.\n\nReferences\n\nSome refs here.\n")
    assert section is not None
    assert "Some refs here." in section


def test_find_section_bibliography():
    ext = PdfExtractor()
    section = ext.find_section("Body.\n\nBibliography\n\nRef A.\nRef B.\n")
    assert section is not None
    assert "Ref A." in section


def test_find_section_with_appendix():
    ext = PdfExtractor()
    section = ext.find_section(
        "Body.\n\nReferences\n\n[1] Ref one.\n\nAppendix A\n\nExtra stuff."
    )
    assert "[1] Ref one." in section
    assert "Extra stuff" not in section


def test_find_section_custom_header():
    ext = PdfExtractor()
    ext.section_header_regex = r"(?i)\n\s*Bibliografía\s*\n"
    section = ext.find_section("Body.\n\nBibliografía\n\nRef Spanish.\n")
    assert section is not None
    assert "Ref Spanish." in section


def test_find_section_custom_end():
    ext = PdfExtractor()
    ext.section_end_regex = r"(?i)\n\s*Anhang"
    section = ext.find_section("Body.\n\nReferences\n\nRef one.\n\nAnhang\n\nExtra.")
    assert "Ref one." in section
    assert "Extra" not in section


def test_find_section_none():
    ext = PdfExtractor()
    # Very short text — fallback returns something, never None
    section = ext.find_section("Short.")
    assert section is not None


# ── segment ──


def test_segment_ieee():
    ext = PdfExtractor()
    segs = ext.segment("\n[1] First ref.\n[2] Second ref.\n[3] Third ref.\n")
    assert len(segs) == 3
    assert segs[0].startswith("First")


def test_segment_custom_ieee_regex():
    ext = PdfExtractor()
    ext.ieee_segment_regex = r"\n\s*\{(\d+)\}\s*"
    segs = ext.segment("\n{1} First ref.\n{2} Second ref.\n{3} Third ref.\n")
    assert len(segs) == 3
    assert segs[0].startswith("First")


# ── parse_reference ──


def test_parse_reference_ieee():
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'J. Smith, A. Jones, "Detecting Fake References in Academic Papers," '
        "in Proc. IEEE Conf., 2023."
    )
    assert ref is not None
    assert "Detecting Fake References" in ref.title
    assert len(ref.authors) >= 1


def test_parse_reference_skips_url_only():
    ext = PdfExtractor()
    ref = ext.parse_reference(
        "See https://github.com/some/repo for implementation details."
    )
    assert ref is None


def test_parse_reference_academic_url_not_skipped():
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'J. Smith, "A Paper Title About Reference Detection Systems," '
        "https://doi.org/10.1234/test, 2023."
    )
    assert ref is not None


def test_parse_reference_prev_authors():
    ext = PdfExtractor()
    prev = ["J. Smith", "A. Jones"]
    ref = ext.parse_reference(
        '\u2014\u2014\u2014, "Another Paper on Machine Learning Systems," '
        "in Proc. IEEE, 2023.",
        prev_authors=prev,
    )
    assert ref is not None
    assert ref.authors == prev


def test_parse_reference_no_prev_authors():
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'J. Smith, "Detecting Fake References in Academic Papers," in Proc. IEEE, 2023.'
    )
    assert ref is not None
    # prev_authors defaults to None/empty
    assert ref.doi is None or ref.doi == ""


# ── Config: min_title_words ──


def test_min_title_words_default_skips_short():
    ext = PdfExtractor()
    ref = ext.parse_reference('J. Smith, "Three Word Title," in Proc. IEEE, 2023.')
    assert ref is None  # default min=4, "Three Word Title" = 3 words


def test_min_title_words_custom_keeps_short():
    ext = PdfExtractor()
    ext.min_title_words = 3
    ref = ext.parse_reference('J. Smith, "Three Word Title," in Proc. IEEE, 2023.')
    assert ref is not None
    assert "Three Word Title" in ref.title


# ── Config: max_authors ──


def test_max_authors():
    ext = PdfExtractor()
    ext.max_authors = 2
    ref = ext.parse_reference(
        "A. Smith, B. Jones, C. Williams, and D. Brown, "
        '"A Paper About Testing Maximum Author Limits in Reference Parsing," '
        "in Proc. IEEE, 2023."
    )
    assert ref is not None
    assert len(ref.authors) <= 2


# ── Config: compound suffixes ──


def test_add_compound_suffix():
    ext = PdfExtractor()
    ext.add_compound_suffix("powered")
    ref = ext.parse_reference(
        'J. Smith, "An AI- powered Approach to Detecting Hallucinated References," '
        "in Proc. IEEE, 2023."
    )
    assert ref is not None
    assert "AI-powered" in ref.title


# ── Config: venue cutoff patterns ──


def test_add_venue_cutoff_pattern():
    ext = PdfExtractor()
    ext.add_venue_cutoff_pattern(r"(?i)\.\s*My Niche Journal\b.*$")
    ref = ext.parse_reference(
        "Smith, J. and Jones, A. 2022. A Novel Approach to Reference Detection. "
        "My Niche Journal, vol 5."
    )
    assert ref is not None
    assert "My Niche Journal" not in ref.title


# ── extract_from_text ──


def test_extract_from_text():
    ext = PdfExtractor()
    text = (
        "Body text.\n\nReferences\n"
        "42\n"  # page number line to provide \n before [1]
        '[1] J. Smith, "Detecting Fake References in Academic Papers," '
        "in Proc. IEEE, 2023.\n"
        '[2] A. Brown, "Another Important Paper on Machine Learning," '
        "in Proc. AAAI, 2022.\n"
        '[3] C. Wilson, "A Third Paper About NLP Systems," '
        "in Proc. ACL, 2021.\n"
    )
    result = ext.extract_from_text(text)
    assert isinstance(result, ExtractionResult)
    assert len(result) == 3
    assert result.skip_stats.total_raw == 3
    assert result.skip_stats.url_only == 0

    refs = result.references
    assert all(isinstance(r, Reference) for r in refs)
    assert "Detecting Fake References" in refs[0].title


# ── Type checks ──


def test_reference_properties():
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'J. Smith, "A Paper About Testing Property Access on References," '
        "in Proc. IEEE, 2023. doi:10.1234/test"
    )
    assert ref is not None
    assert isinstance(ref.title, str)
    assert isinstance(ref.authors, list)
    assert isinstance(ref.raw_citation, str)
    # doi/arxiv_id may or may not be found
    assert ref.arxiv_id is None


def test_skip_stats_properties():
    ext = PdfExtractor()
    result = ext.extract_from_text("Body.\n\nReferences\n\nNothing useful.\n")
    stats = result.skip_stats
    assert isinstance(stats.total_raw, int)
    assert isinstance(stats.url_only, int)
    assert isinstance(stats.short_title, int)
    assert isinstance(stats.no_title, int)
    assert isinstance(stats.no_authors, int)


# ── Invalid regex ──


def test_invalid_regex_raises():
    ext = PdfExtractor()
    ext.section_header_regex = r"[invalid"
    with pytest.raises(ValueError, match="Invalid regex"):
        ext.find_section("anything")


# ── Native class access ──


def test_native_extractor_accessible():
    native = NativePdfExtractor()
    assert repr(native) == "NativePdfExtractor(...)"


# ── Custom segmentation strategies ──


IEEE_TEXT = (
    "Body text.\n\nReferences\n"
    "42\n"
    '[1] J. Smith, "Detecting Fake References in Academic Papers," '
    "in Proc. IEEE, 2023.\n"
    '[2] A. Brown, "Another Important Paper on Machine Learning," '
    "in Proc. AAAI, 2022.\n"
    '[3] C. Wilson, "A Third Paper About NLP Systems," '
    "in Proc. ACL, 2021.\n"
)


def test_add_custom_strategy_runs_first():
    """A custom strategy that returns segments is used instead of Rust built-ins."""
    ext = PdfExtractor()

    custom_segments = [
        'J. Smith, "Custom Parsed Reference One About Academic Systems," Proc. IEEE, 2023.',
        'A. Brown, "Custom Parsed Reference Two About Machine Learning," Proc. AAAI, 2022.',
        'C. Wilson, "Custom Parsed Reference Three About NLP Methods," Proc. ACL, 2021.',
    ]

    def custom_strategy(text):
        return custom_segments

    ext.add_segmentation_strategy(custom_strategy)
    segments = ext.segment("any text")
    assert segments == custom_segments


def test_custom_strategy_fallthrough():
    """A strategy returning None falls through to Rust built-ins."""
    ext = PdfExtractor()

    def returns_none(text):
        return None

    ext.add_segmentation_strategy(returns_none)
    segs = ext.segment("\n[1] First ref.\n[2] Second ref.\n[3] Third ref.\n")
    assert len(segs) == 3  # Rust built-in IEEE strategy handles it


def test_custom_strategy_with_extract_from_text():
    """Full pipeline uses custom strategy when registered."""
    ext = PdfExtractor()
    call_count = 0

    def paren_strategy(text):
        nonlocal call_count
        call_count += 1
        # Split by parenthesized numbers
        import re

        parts = re.split(r"\n\s*\(\d+\)\s+", text)
        parts = [p.strip() for p in parts if p.strip()]
        return parts if len(parts) >= 3 else None

    ext.add_segmentation_strategy(paren_strategy)

    text = (
        "Body text.\n\nReferences\n\n"
        '(1) J. Smith, "Detecting Fake References in Academic Papers," '
        "in Proc. IEEE, 2023.\n"
        '(2) A. Brown, "Another Important Paper on Machine Learning," '
        "in Proc. AAAI, 2022.\n"
        '(3) C. Wilson, "A Third Paper About Natural Language Processing," '
        "in Proc. ACL, 2021.\n"
    )

    result = ext.extract_from_text(text)
    assert call_count >= 1
    assert isinstance(result, ExtractionResult)
    assert len(result) >= 2  # some refs should parse


def test_clear_strategies():
    """After clearing, fast path is restored."""
    ext = PdfExtractor()

    def custom(text):
        return ["a", "b", "c"]

    ext.add_segmentation_strategy(custom)
    assert repr(ext) == "PdfExtractor(custom_strategies=1)"

    ext.clear_segmentation_strategies()
    assert repr(ext) == "PdfExtractor(...)"

    # Now extract_from_text delegates to Rust (fast path)
    result = ext.extract_from_text(IEEE_TEXT)
    assert isinstance(result, ExtractionResult)
    assert len(result) == 3


def test_multiple_strategies_priority_order():
    """First matching callable wins; later ones are not called."""
    ext = PdfExtractor()
    calls = []

    def first(text):
        calls.append("first")
        return None  # fall through

    def second(text):
        calls.append("second")
        return ["seg1", "seg2", "seg3"]

    def third(text):
        calls.append("third")
        return ["other1", "other2", "other3"]

    ext.add_segmentation_strategy(first)
    ext.add_segmentation_strategy(second)
    ext.add_segmentation_strategy(third)

    result = ext.segment("any text")
    assert result == ["seg1", "seg2", "seg3"]
    assert calls == ["first", "second"]  # third never called


def test_custom_strategy_exception_propagates():
    """If a custom strategy raises, the exception propagates to the caller."""
    ext = PdfExtractor()

    def bad_strategy(text):
        raise RuntimeError("strategy failed")

    ext.add_segmentation_strategy(bad_strategy)
    with pytest.raises(RuntimeError, match="strategy failed"):
        ext.segment("any text")


def test_custom_strategy_too_few_results_falls_through():
    """A strategy returning fewer than 3 items is treated as non-matching."""
    ext = PdfExtractor()

    def returns_two(text):
        return ["a", "b"]

    ext.add_segmentation_strategy(returns_two)
    segs = ext.segment("\n[1] First ref.\n[2] Second ref.\n[3] Third ref.\n")
    assert len(segs) == 3  # fell through to Rust


def test_parse_reference_detailed():
    """parse_reference_detailed returns (ref, None) or (None, reason)."""
    ext = PdfExtractor()

    # Successful parse
    ref, reason = ext._native.parse_reference_detailed(
        'J. Smith, "Detecting Fake References in Academic Papers," in Proc. IEEE, 2023.'
    )
    assert ref is not None
    assert reason is None
    assert "Detecting Fake References" in ref.title

    # URL-only skip
    ref, reason = ext._native.parse_reference_detailed(
        "See https://github.com/some/repo for details."
    )
    assert ref is None
    assert reason == "url_only"

    # Short title skip
    ref, reason = ext._native.parse_reference_detailed(
        'J. Smith, "Short Title," in Proc. IEEE, 2023.'
    )
    assert ref is None
    assert reason == "short_title"


def test_extraction_result_from_parts():
    """ExtractionResult._from_parts() constructs a valid result."""
    ext = PdfExtractor()
    ref = ext.parse_reference(
        'J. Smith, "Detecting Fake References in Academic Papers," in Proc. IEEE, 2023.'
    )
    assert ref is not None

    result = ExtractionResult._from_parts([ref], 5, 1, 1, 0, 0)
    assert len(result) == 1
    assert result.skip_stats.total_raw == 5
    assert result.skip_stats.url_only == 1
    assert result.skip_stats.short_title == 1
    assert result.references[0].title == ref.title
