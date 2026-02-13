"""Hallucinator PDF reference extraction and validation library.

Provides a configurable pipeline for extracting and parsing academic
references from PDF documents, and validating them against academic
databases. Custom segmentation strategies can be registered to handle
non-standard reference formats.
"""

from hallucinator._native import (
    NativePdfExtractor,
    Reference,
    ExtractionResult,
    SkipStats,
    # Archive extraction
    ArchiveEntry,
    ArchiveIterator,
    is_archive_path,
    # Validation pipeline
    ValidatorConfig,
    Validator,
    ValidationResult,
    DbResult,
    DoiInfo,
    ArxivInfo,
    RetractionInfo,
    ProgressEvent,
    CheckStats,
)

__all__ = [
    # PDF extraction
    "PdfExtractor",
    "Reference",
    "ExtractionResult",
    "SkipStats",
    # Archive extraction
    "ArchiveEntry",
    "ArchiveIterator",
    "is_archive_path",
    # Validation pipeline
    "Validator",
    "ValidatorConfig",
    "ValidationResult",
    "DbResult",
    "DoiInfo",
    "ArxivInfo",
    "RetractionInfo",
    "ProgressEvent",
    "CheckStats",
]


class PdfExtractor:
    """A configurable PDF reference extractor.

    Wraps the native Rust extractor and adds support for custom Python
    segmentation strategies. When no custom strategies are registered,
    all work is delegated to the fast Rust implementation.

    Example::

        ext = PdfExtractor()
        ext.section_header_regex = r"(?i)\\n\\s*Bibliografía\\s*\\n"
        ext.min_title_words = 3
        result = ext.extract("paper.pdf")

    Custom segmentation::

        def paren_segmenter(text: str) -> list[str] | None:
            import re
            parts = re.split(r'\\n\\s*\\(\\d+\\)\\s+', text)
            return parts if len(parts) >= 3 else None

        ext.add_segmentation_strategy(paren_segmenter)
        result = ext.extract_from_text(text)
    """

    # Config attributes forwarded to the native extractor via __setattr__.
    _CONFIG_ATTRS = frozenset(
        {
            "section_header_regex",
            "section_end_regex",
            "fallback_fraction",
            "ieee_segment_regex",
            "numbered_segment_regex",
            "fallback_segment_regex",
            "min_title_words",
            "max_authors",
        }
    )

    def __init__(self):
        self._native = NativePdfExtractor()
        self._custom_strategies = []

    def __setattr__(self, name, value):
        if name in PdfExtractor._CONFIG_ATTRS:
            setattr(self._native, name, value)
        else:
            super().__setattr__(name, value)

    # ── Config methods (forwarded to native) ──

    def add_venue_cutoff_pattern(self, pattern):
        self._native.add_venue_cutoff_pattern(pattern)

    def set_venue_cutoff_patterns(self, patterns):
        self._native.set_venue_cutoff_patterns(patterns)

    def add_quote_pattern(self, pattern):
        self._native.add_quote_pattern(pattern)

    def set_quote_patterns(self, patterns):
        self._native.set_quote_patterns(patterns)

    def add_compound_suffix(self, suffix):
        self._native.add_compound_suffix(suffix)

    def set_compound_suffixes(self, suffixes):
        self._native.set_compound_suffixes(suffixes)

    # ── Strategy registration ──

    def add_segmentation_strategy(self, fn):
        """Register a custom segmentation callable.

        Args:
            fn: A callable with signature ``(text: str) -> list[str] | None``.
                Return a list of reference strings if this strategy applies,
                or ``None`` to fall through to the next strategy.
                The result must contain at least 3 items to be accepted.
        """
        self._custom_strategies.append(fn)

    def clear_segmentation_strategies(self):
        """Remove all custom segmentation strategies."""
        self._custom_strategies.clear()

    # ── Pipeline methods ──

    def find_section(self, text):
        """Locate the references section in document text."""
        return self._native.find_section(text)

    def segment(self, text):
        """Segment references text into individual reference strings.

        Tries custom strategies first (in registration order), then
        falls back to the Rust built-in strategies.
        """
        for strategy in self._custom_strategies:
            result = strategy(text)
            if result is not None and len(result) >= 3:
                return result
        return self._native.segment(text)

    def parse_reference(self, text, prev_authors=None):
        """Parse a single reference string.

        Returns a ``Reference`` or ``None`` if the reference was skipped.
        """
        return self._native.parse_reference(text, prev_authors=prev_authors)

    def extract_from_text(self, text):
        """Run the full extraction pipeline on already-extracted text.

        When custom segmentation strategies are registered, the pipeline
        is orchestrated in Python (find_section -> segment -> parse loop).
        Otherwise delegates entirely to the fast Rust implementation.
        """
        if not self._custom_strategies:
            return self._native.extract_from_text(text)

        section = self.find_section(text)
        if section is None:
            return ExtractionResult._from_parts([], 0, 0, 0, 0, 0)

        segments = self.segment(section)
        return self._parse_segments(segments)

    def extract(self, path):
        """Run the full extraction pipeline on a PDF file."""
        if not self._custom_strategies:
            return self._native.extract(path)

        text = self._native.extract_text(path)
        return self.extract_from_text(text)

    def extract_archive(self, path, max_size_bytes=0):
        """Extract and parse references from a ZIP or tar.gz archive.

        Yields ArchiveEntry items as each file is processed.
        PDFs get full reference extraction; BBL/BIB files yield raw content.

        Access ``.warnings`` on the returned iterator for any size-limit warnings.
        """
        return self._native.extract_archive(path, max_size_bytes=max_size_bytes)

    def extract_text(self, path):
        """Extract raw text from a PDF file."""
        return self._native.extract_text(path)

    # ── Internal ──

    def _parse_segments(self, segments):
        """Parse a list of reference segments into an ExtractionResult."""
        refs = []
        prev_authors = None
        total_raw = len(segments)
        url_only = 0
        short_title = 0
        no_title = 0
        no_authors = 0

        for seg in segments:
            ref, skip_reason = self._native.parse_reference_detailed(
                seg, prev_authors=prev_authors
            )
            if skip_reason:
                if skip_reason == "url_only":
                    url_only += 1
                elif skip_reason == "short_title":
                    short_title += 1
            elif ref is not None:
                if ref.title is None:
                    no_title += 1
                if not ref.authors:
                    no_authors += 1
                else:
                    prev_authors = ref.authors
                refs.append(ref)

        return ExtractionResult._from_parts(
            refs, total_raw, url_only, short_title, no_title, no_authors
        )

    def __repr__(self):
        n = len(self._custom_strategies)
        if n:
            return f"PdfExtractor(custom_strategies={n})"
        return "PdfExtractor(...)"
