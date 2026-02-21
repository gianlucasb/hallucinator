"""Type stubs for the hallucinator Python package."""

from typing import Callable, Optional

from hallucinator._native import ArxivInfo as ArxivInfo
from hallucinator._native import CheckStats as CheckStats
from hallucinator._native import DbResult as DbResult
from hallucinator._native import DoiInfo as DoiInfo
from hallucinator._native import ExtractionResult as ExtractionResult
from hallucinator._native import ProgressEvent as ProgressEvent
from hallucinator._native import RetractionInfo as RetractionInfo
from hallucinator._native import SkipStats as SkipStats
from hallucinator._native import ValidationResult as ValidationResult
from hallucinator._native import Validator as Validator
from hallucinator._native import ValidatorConfig as ValidatorConfig
from hallucinator._native import ArchiveEntry as ArchiveEntry
from hallucinator._native import ArchiveIterator as ArchiveIterator
from hallucinator._native import is_archive_path as is_archive_path

class Reference:
    """A parsed reference with structured fields.

    Can be created manually for batch validation without PDF extraction.
    """

    def __init__(
        self,
        title: str,
        authors: list[str] = ...,
        doi: Optional[str] = None,
        arxiv_id: Optional[str] = None,
        raw_citation: Optional[str] = None,
    ) -> None: ...

    @property
    def raw_citation(self) -> str: ...
    @property
    def title(self) -> Optional[str]: ...
    @property
    def authors(self) -> list[str]: ...
    @property
    def doi(self) -> Optional[str]: ...
    @property
    def arxiv_id(self) -> Optional[str]: ...
    @property
    def original_number(self) -> int: ...
    @property
    def skip_reason(self) -> Optional[str]: ...

class PdfExtractor:
    """A configurable PDF reference extractor with custom strategy support.

    Wraps the native Rust extractor and adds support for registering
    custom Python segmentation strategies.
    """

    def __init__(self) -> None: ...

    # Config attributes (write-only, forwarded to native Rust extractor)
    section_header_regex: str
    section_end_regex: str
    fallback_fraction: float
    ieee_segment_regex: str
    numbered_segment_regex: str
    fallback_segment_regex: str
    min_title_words: int
    max_authors: int

    # Config methods
    def add_venue_cutoff_pattern(self, pattern: str) -> None: ...
    def set_venue_cutoff_patterns(self, patterns: list[str]) -> None: ...
    def add_quote_pattern(self, pattern: str) -> None: ...
    def set_quote_patterns(self, patterns: list[str]) -> None: ...
    def add_compound_suffix(self, suffix: str) -> None: ...
    def set_compound_suffixes(self, suffixes: list[str]) -> None: ...

    # Strategy registration
    def add_segmentation_strategy(
        self, fn: Callable[[str], Optional[list[str]]]
    ) -> None:
        """Register a custom segmentation callable.

        Args:
            fn: A callable ``(text: str) -> list[str] | None``.
                Return a list of reference strings (3+ items) if this
                strategy applies, or ``None`` to fall through.
        """
        ...

    def clear_segmentation_strategies(self) -> None:
        """Remove all custom segmentation strategies."""
        ...

    # Pipeline methods
    def find_section(self, text: str) -> Optional[str]: ...
    def segment(self, text: str) -> list[str]: ...
    def parse_reference(
        self, text: str, prev_authors: Optional[list[str]] = None
    ) -> Optional[Reference]: ...
    def extract_from_text(self, text: str) -> ExtractionResult: ...
    def extract(self, path: str) -> ExtractionResult: ...
    def extract_text(self, path: str) -> str: ...
    def extract_archive(
        self, path: str, max_size_bytes: int = 0
    ) -> ArchiveIterator: ...
