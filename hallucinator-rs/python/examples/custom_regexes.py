"""Customize regex patterns for non-standard paper formats.

Shows how to override section detection, segmentation, title parsing,
and other knobs to handle papers that the defaults don't parse well.

    pip install .          # from hallucinator-rs/
    python examples/custom_regexes.py
"""

from hallucinator import PdfExtractor


def spanish_paper_example():
    """Handle a paper with Spanish section headers."""
    print("-- Spanish section header --")
    ext = PdfExtractor()

    # Override section header to match "Bibliografía" or "Referencias"
    ext.section_header_regex = r"(?i)\n\s*(?:Bibliograf[ií]a|Referencias)\s*\n"

    text = (
        "Cuerpo del artículo.\n\n"
        "Bibliografía\n\n"
        '[1] García, A. "Un estudio sobre la detección de referencias falsas."\n'
        '[2] López, B. "Análisis de citas en documentos académicos modernos."\n'
        '[3] Martínez, C. "Métodos computacionales para verificación bibliográfica."\n'
    )

    result = ext.extract_from_text(text)
    print(f"  Found {len(result)} references")
    for ref in result.references:
        print(f"    - {ref.title}")
    print()


def curly_bracket_segmentation():
    """Handle a paper that uses {1}, {2} instead of [1], [2]."""
    print("-- Custom segmentation regex --")
    ext = PdfExtractor()

    # Override IEEE-style regex to match curly brackets
    ext.ieee_segment_regex = r"\n\s*\{(\d+)\}\s*"

    section = (
        '\n{1} Smith, J. "A Novel Approach to Reference Verification Systems." '
        "Proc. IEEE, 2023.\n"
        '{2} Jones, A. "Machine Learning for Academic Document Analysis." '
        "Proc. AAAI, 2022.\n"
        '{3} Brown, C. "Natural Language Processing in Citation Parsing." '
        "Proc. ACL, 2021.\n"
    )

    segments = ext.segment(section)
    print(f"  Segments: {len(segments)}")
    for i, seg in enumerate(segments, 1):
        ref = ext.parse_reference(seg)
        if ref:
            print(f"    [{i}] {ref.title}")
    print()


def short_titles_and_custom_venue():
    """Allow shorter titles and add a custom venue cutoff pattern."""
    print("-- Short titles + custom venue cutoff --")
    ext = PdfExtractor()

    # Default min is 4 words — lower it to accept 3-word titles
    ext.min_title_words = 3

    # Add a venue cutoff so "My Journal" doesn't leak into the title
    ext.add_venue_cutoff_pattern(r"(?i)\.\s*My Journal\b.*$")

    ref = ext.parse_reference(
        "Smith, J. and Jones, A. 2022. Neural Reference Detection. My Journal, vol 5."
    )
    if ref:
        print(f"  Title: {ref.title}")
        print(f"  Authors: {', '.join(ref.authors)}")
        assert "My Journal" not in ref.title
        print("  (venue correctly excluded from title)")
    print()


def compound_hyphenation():
    """Add a custom compound-word suffix for hyphenation fixing."""
    print("-- Custom compound suffix --")
    ext = PdfExtractor()

    # "AI-powered" would be dehyphenated to "AIpowered" by default
    # because "powered" isn't in the built-in suffix list
    ext.add_compound_suffix("powered")

    ref = ext.parse_reference(
        'Smith, J. "An AI- powered System for Detecting Hallucinated References." '
        "Proc. IEEE, 2023."
    )
    if ref:
        print(f"  Title: {ref.title}")
        assert "AI-powered" in ref.title
        print("  (hyphen correctly preserved)")
    print()


def limit_authors():
    """Cap the number of authors extracted per reference."""
    print("-- Max authors --")
    ext = PdfExtractor()
    ext.max_authors = 3

    ref = ext.parse_reference(
        "A. Smith, B. Jones, C. Williams, D. Brown, and E. Davis, "
        '"A Large Collaborative Study on Reference Validation Methods," '
        "Proc. IEEE, 2023."
    )
    if ref:
        print(f"  Authors ({len(ref.authors)}): {', '.join(ref.authors)}")
        assert len(ref.authors) <= 3
        print("  (capped at 3)")
    print()


def german_appendix_cutoff():
    """Use a custom section-end regex for German papers."""
    print("-- Custom section-end regex --")
    ext = PdfExtractor()

    # German papers might use "Anhang" (Appendix) instead of "Appendix"
    ext.section_end_regex = r"(?i)\n\s*Anhang"

    text = (
        "Body.\n\nReferences\n\n"
        '[1] Schmidt, H. "Referenzerkennung in wissenschaftlichen Arbeiten." 2023.\n'
        '[2] Müller, K. "Automatische Zitationsanalyse mit neuronalen Netzen." 2022.\n'
        "\nAnhang A\n\nAdditional material here.\n"
    )

    section = ext.find_section(text)
    assert "Additional material" not in section
    result = ext.extract_from_text(text)
    print(f"  Found {len(result)} references (appendix excluded)")
    print()


def custom_callable_strategy():
    """Use a Python callable to handle parenthesized reference numbering.

    Some papers use (1), (2) instead of [1], [2]. A regex override won't
    help if the format is sufficiently unusual — but a Python callable can
    implement arbitrary splitting logic.
    """
    import re

    print("-- Custom callable segmentation strategy --")
    ext = PdfExtractor()

    def paren_segmenter(text: str) -> list[str] | None:
        """Split references numbered with parenthesized digits: (1), (2), ..."""
        parts = re.split(r"\n\s*\(\d+\)\s+", text)
        parts = [p.strip() for p in parts if p.strip()]
        return parts if len(parts) >= 3 else None

    ext.add_segmentation_strategy(paren_segmenter)

    text = (
        "Body of the paper.\n\n"
        "References\n\n"
        '(1) García, A. "A Novel Approach to Detecting Hallucinated References." '
        "Proc. IEEE, 2023.\n"
        '(2) López, B. "Machine Learning Methods for Citation Verification." '
        "Proc. AAAI, 2022.\n"
        '(3) Martínez, C. "Computational Approaches to Bibliographic Validation." '
        "Proc. ACL, 2021.\n"
    )

    result = ext.extract_from_text(text)
    print(f"  Found {len(result)} references (via custom callable)")
    for ref in result.references:
        print(f"    - {ref.title}")

    # The callable is tried first; if it returns None, Rust built-ins take over
    ext2 = PdfExtractor()
    ext2.add_segmentation_strategy(paren_segmenter)

    ieee_text = (
        "Body.\n\nReferences\n\n"
        '[1] Smith, J. "A Paper Using Standard IEEE Bracketed Numbering." '
        "Proc. IEEE, 2023.\n"
        '[2] Jones, A. "Another Paper With Normal Reference Formatting." '
        "Proc. AAAI, 2022.\n"
        '[3] Brown, C. "A Third Paper on Natural Language Processing." '
        "Proc. ACL, 2021.\n"
    )

    result2 = ext2.extract_from_text(ieee_text)
    print(f"  Fallback to Rust: found {len(result2)} references")
    print()


if __name__ == "__main__":
    spanish_paper_example()
    curly_bracket_segmentation()
    short_titles_and_custom_venue()
    compound_hyphenation()
    limit_authors()
    german_appendix_cutoff()
    custom_callable_strategy()
    print("All examples ran successfully.")
