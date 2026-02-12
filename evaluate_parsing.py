#!/usr/bin/env python3
"""
Reference Parsing Evaluation Script

Compares PDF reference extraction against ground truth from .bib files,
identifies parsing failures, and helps improve check_hallucinated_references.py.

Usage:
    python evaluate_parsing.py /path/to/downloads           # Basic usage
    python evaluate_parsing.py /path/to/downloads -o report.md  # With output
    python evaluate_parsing.py /path/to/downloads --json    # JSON output
    python evaluate_parsing.py /path/to/downloads -v        # Verbose
    python evaluate_parsing.py /path/to/downloads --limit 10    # Test mode
"""

import argparse
import json
import os
import re
import sys
import tarfile
from collections import defaultdict
from dataclasses import dataclass, field
from io import BytesIO
from pathlib import Path
from typing import Optional

import logging
import warnings
import bibtexparser
from bibtexparser.bparser import BibTexParser
from bibtexparser.customization import convert_to_unicode
from rapidfuzz import fuzz

# Suppress bibtexparser warnings about non-standard entry types
logging.getLogger('bibtexparser.bparser').setLevel(logging.ERROR)

# Import functions from the main script
from check_hallucinated_references import (
    extract_references_with_titles_and_authors,
    normalize_title,
)

# Minimum words for a valid title
MIN_TITLE_WORDS = 4

# Fuzzy match threshold
FUZZY_THRESHOLD = 90


@dataclass
class BibEntry:
    """A parsed .bib entry."""
    key: str
    title: str
    authors: list[str]
    entry_type: str
    year: str = ""
    raw_entry: dict = field(default_factory=dict)


@dataclass
class MatchResult:
    """Result of matching a PDF reference to a .bib entry."""
    pdf_title: str
    pdf_authors: list[str]
    pdf_raw: str
    bib_entry: Optional[BibEntry]
    match_type: str  # 'exact', 'fuzzy', 'title_only', 'unmatched', 'likely_valid_unmatched'
    match_score: float
    failure_patterns: list[str] = field(default_factory=list)


@dataclass
class PaperReport:
    """Report for a single paper."""
    pdf_path: str
    tar_path: str
    pdf_refs: list[tuple]
    bib_entries: list[BibEntry]
    matches: list[MatchResult]
    unmatched_bib: list[BibEntry]
    error: Optional[str] = None

    @property
    def match_rate(self) -> float:
        """Rate of PDF refs that matched a bib entry (measures extraction quality)."""
        if not self.matches:
            return 0.0
        matched = sum(1 for m in self.matches if m.match_type in ('exact', 'fuzzy'))
        return matched / len(self.matches) * 100

    @property
    def extraction_quality(self) -> float:
        """Rate of PDF refs that look valid (matched or valid-looking unmatched).

        This counts successful extractions even if the bib is incomplete.
        """
        if not self.matches:
            return 0.0
        valid = sum(1 for m in self.matches
                    if m.match_type in ('exact', 'fuzzy', 'likely_valid_unmatched'))
        return valid / len(self.matches) * 100

    @property
    def coverage_rate(self) -> float:
        """Rate of bib entries that were found in PDF (may be <100% if not all cited)."""
        if not self.bib_entries:
            return 0.0
        matched = sum(1 for m in self.matches if m.match_type in ('exact', 'fuzzy'))
        return matched / len(self.bib_entries) * 100


def extract_bib_from_tarball(tar_path: str) -> list[tuple[str, str]]:
    """Extract .bib file contents from a tar.gz archive.

    Returns list of (filename, content) tuples.
    Handles nested directories, multiple .bib files, and encoding fallbacks.
    """
    bib_files = []

    try:
        with tarfile.open(tar_path, 'r:gz') as tar:
            for member in tar.getmembers():
                if member.name.endswith('.bib') and member.isfile():
                    f = tar.extractfile(member)
                    if f:
                        raw_content = f.read()
                        # Try different encodings
                        for encoding in ['utf-8', 'latin-1', 'cp1252']:
                            try:
                                content = raw_content.decode(encoding)
                                bib_files.append((member.name, content))
                                break
                            except UnicodeDecodeError:
                                continue
    except Exception as e:
        print(f"[Error] Failed to read tarball {tar_path}: {e}", file=sys.stderr)

    return bib_files


def parse_bib_content(content: str) -> list[BibEntry]:
    """Parse .bib content into structured entries.

    Uses bibtexparser with LaTeX accent decoding.
    Skips entries without title or with <4 word titles.
    """
    entries = []

    try:
        # Configure parser for LaTeX decoding
        parser = BibTexParser(common_strings=True)
        parser.customization = convert_to_unicode

        bib_db = bibtexparser.loads(content, parser=parser)

        for entry in bib_db.entries:
            title = entry.get('title', '')
            if not title:
                continue

            # Clean up braces from title
            title = re.sub(r'[{}]', '', title)
            title = re.sub(r'\s+', ' ', title).strip()

            # Skip short titles
            if len(title.split()) < MIN_TITLE_WORDS:
                continue

            # Parse authors
            authors = []
            author_str = entry.get('author', '')
            if author_str:
                # bibtexparser uses " and " as separator
                author_parts = re.split(r'\s+and\s+', author_str)
                for author in author_parts:
                    # Clean up and extract name
                    author = re.sub(r'[{}]', '', author).strip()
                    if author:
                        # Handle "Last, First" format
                        if ',' in author:
                            parts = author.split(',', 1)
                            author = f"{parts[1].strip()} {parts[0].strip()}"
                        authors.append(author)

            entries.append(BibEntry(
                key=entry.get('ID', ''),
                title=title,
                authors=authors,
                entry_type=entry.get('ENTRYTYPE', ''),
                year=entry.get('year', ''),
                raw_entry=entry
            ))
    except Exception as e:
        print(f"[Warning] Failed to parse bib content: {e}", file=sys.stderr)

    return entries


def looks_like_valid_title(title: str) -> bool:
    """Heuristic to check if a string looks like a valid paper title vs venue/metadata."""
    # Too short
    if len(title.split()) < 4:
        return False

    # Starts with venue keywords
    venue_starts = [
        r'^[Ii]n[:,]?\s+',
        r'^(?:Proceedings|Conference|Workshop|Symposium|Journal|Advances)\s',
        r'^(?:IEEE|ACM|AAAI|International)\s',
    ]
    for vp in venue_starts:
        if re.match(vp, title):
            return False

    # Contains page/volume numbers
    if re.search(r'\d+\s*\(\d+\)\s*,\s*\d+[–\-]\d+', title):
        return False
    if re.search(r',\s*\d+[–\-]\d+\s*\(\d{4}\)', title):
        return False

    # Just numbers/years
    if re.match(r'^[\d\s\(\)\-–,]+$', title):
        return False

    # Has title-like features: capitalized words, colon for subtitle
    has_uppercase_word = bool(re.search(r'\b[A-Z][a-z]{2,}', title))

    return has_uppercase_word


def identify_failure_pattern(pdf_title: str, pdf_raw: str, bib_title: str) -> list[str]:
    """Identify why a match failed.

    Categories:
    - subtitle_truncation: PDF title cut off (missing subtitle after colon)
    - venue_leak: Venue/journal info leaked into extracted title
    - venue_instead_of_title: Extracted venue name instead of actual title (e.g., "In: Conference...")
    - year_page_leak: Year, page numbers, or volume info in title
    - hyphenation: Word broken incorrectly across lines
    - ligature: Ligature character not expanded properly
    - latex_residue: LaTeX commands remaining in title
    - author_in_title: Author names mixed into title
    - parenthetical_truncation: Missing content in parentheses (e.g., "(2022–2032)")
    - format_unrecognized: Reference format not properly parsed
    """
    patterns = []

    pdf_norm = normalize_title(pdf_title)
    bib_norm = normalize_title(bib_title)
    pdf_lower = pdf_title.lower()
    bib_lower = bib_title.lower()

    # Check for subtitle truncation (bib has colon, pdf doesn't have content after it)
    if ':' in bib_title:
        bib_before_colon = bib_title.split(':')[0].strip()
        bib_before_norm = normalize_title(bib_before_colon)
        if fuzz.ratio(pdf_norm, bib_before_norm) >= 90:
            patterns.append('subtitle_truncation')

    # Check for parenthetical truncation (bib has parenthetical content pdf is missing)
    paren_match = re.search(r'\([^)]+\)\s*$', bib_title)
    if paren_match:
        bib_without_paren = bib_title[:paren_match.start()].strip()
        bib_without_paren_norm = normalize_title(bib_without_paren)
        if fuzz.ratio(pdf_norm, bib_without_paren_norm) >= 90:
            patterns.append('parenthetical_truncation')

    # Check for venue instead of title (starts with "In:" or is a venue/journal name)
    venue_start_patterns = [
        r'^[Ii]n[:,]?\s+',
        r'^(?:Proceedings|Conference|Workshop|Symposium|Journal)\s',
        r'^(?:Advances|IEEE|ACM|AAAI|International)\s',
        r'^[A-Z][a-z]+\s+of\s+(?:automated|artificial|computer)',  # "Journal of automated..."
    ]
    for vp in venue_start_patterns:
        if re.match(vp, pdf_title, re.IGNORECASE):
            patterns.append('venue_instead_of_title')
            break

    # Check for venue leak (common venue keywords in pdf title but not in bib)
    venue_keywords = ['proceedings', 'conference', 'journal', 'transactions',
                      'symposium', 'workshop', 'ieee', 'acm', 'arxiv', 'preprint',
                      'advances in', 'international']
    for kw in venue_keywords:
        if kw in pdf_lower and kw not in bib_lower:
            patterns.append('venue_leak')
            break

    # Check for year/page/volume leak
    if re.search(r'\d{4}\)', pdf_title) or re.search(r'\d+[:\-–]\d+', pdf_title):
        if not re.search(r'\d{4}\)', bib_title) and not re.search(r'\d+[:\-–]\d+', bib_title):
            patterns.append('year_page_leak')

    # Check for hyphenation issues (look for unusual letter combinations like "detec tion")
    hyphen_artifacts = re.findall(r'[a-z]{2,}[A-Z][a-z]', pdf_title)
    if hyphen_artifacts:
        patterns.append('hyphenation')

    # Check for ligature issues (fi, fl, ff combinations missing)
    ligature_combos = ['fi', 'fl', 'ff', 'ffi', 'ffl']
    for combo in ligature_combos:
        if combo in bib_lower and combo not in pdf_lower:
            # Check if it's truly a ligature issue (chars missing vs different word)
            if combo in bib_norm and combo not in pdf_norm:
                patterns.append('ligature')
                break

    # Check for LaTeX residue
    latex_patterns = [r'\\[a-zA-Z]+', r'\$[^$]+\$', r'\\{', r'\\}', r'~', r'`', r"''"]
    for pat in latex_patterns:
        if re.search(pat, pdf_title):
            patterns.append('latex_residue')
            break

    # Check for author contamination (common author patterns in title)
    author_patterns = [r'\b[A-Z]\.\s*[A-Z]\.', r'\bet\s+al\b', r'\b(?:Jr|Sr)\.',
                       r'^[A-Z][a-z]+,\s+[A-Z]\.']
    for pat in author_patterns:
        if re.search(pat, pdf_title):
            patterns.append('author_in_title')
            break

    # If no specific pattern found but match failed
    if not patterns and fuzz.ratio(pdf_norm, bib_norm) < FUZZY_THRESHOLD:
        patterns.append('format_unrecognized')

    return list(set(patterns))  # Remove duplicates


def match_references(pdf_refs: list[tuple], bib_entries: list[BibEntry]) -> tuple[list[MatchResult], list[BibEntry]]:
    """Match PDF references to .bib entries.

    Returns (matches, unmatched_bib_entries).
    """
    matches = []
    used_bib_indices = set()

    for pdf_ref in pdf_refs:
        title, authors, doi, arxiv_id, raw_citation = pdf_ref
        pdf_norm = normalize_title(title)

        best_match = None
        best_score = 0
        best_idx = -1

        for i, bib in enumerate(bib_entries):
            if i in used_bib_indices:
                continue

            bib_norm = normalize_title(bib.title)
            score = fuzz.ratio(pdf_norm, bib_norm)

            if score > best_score:
                best_score = score
                best_match = bib
                best_idx = i

        if best_score >= 95:
            match_type = 'exact'
            used_bib_indices.add(best_idx)
        elif best_score >= FUZZY_THRESHOLD:
            match_type = 'fuzzy'
            used_bib_indices.add(best_idx)
        elif best_score >= 70:
            match_type = 'title_only'
            # Don't mark as used - might be wrong match
        else:
            # Unmatched - but distinguish between likely valid titles (coverage gap)
            # and likely parsing failures
            if looks_like_valid_title(title):
                match_type = 'likely_valid_unmatched'
            else:
                match_type = 'unmatched'

        failure_patterns = []
        if match_type in ('title_only', 'unmatched') and best_match:
            failure_patterns = identify_failure_pattern(title, raw_citation, best_match.title)

        # For unmatched refs, still include best_match for debugging (but mark as unmatched)
        matches.append(MatchResult(
            pdf_title=title,
            pdf_authors=authors,
            pdf_raw=raw_citation,
            bib_entry=best_match,  # Include for debugging even if unmatched
            match_type=match_type,
            match_score=best_score,
            failure_patterns=failure_patterns
        ))

    # Find unmatched bib entries
    unmatched_bib = [bib for i, bib in enumerate(bib_entries) if i not in used_bib_indices]

    return matches, unmatched_bib


def compare_paper(pdf_path: str, tar_path: str, verbose: bool = False, max_refs: int = 0) -> PaperReport:
    """Compare one PDF against its .bib ground truth."""

    # Extract bib files from tarball
    bib_files = extract_bib_from_tarball(tar_path)
    if not bib_files:
        return PaperReport(
            pdf_path=pdf_path,
            tar_path=tar_path,
            pdf_refs=[],
            bib_entries=[],
            matches=[],
            unmatched_bib=[],
            error="No .bib files found in tarball"
        )

    # Parse all bib entries
    all_bib_entries = []
    for filename, content in bib_files:
        entries = parse_bib_content(content)
        all_bib_entries.extend(entries)
        if verbose:
            print(f"  Parsed {len(entries)} entries from {filename}")

    if not all_bib_entries:
        return PaperReport(
            pdf_path=pdf_path,
            tar_path=tar_path,
            pdf_refs=[],
            bib_entries=[],
            matches=[],
            unmatched_bib=[],
            error="No valid entries in .bib files"
        )

    # Extract references from PDF
    try:
        pdf_refs, stats = extract_references_with_titles_and_authors(pdf_path, return_stats=True)
    except Exception as e:
        return PaperReport(
            pdf_path=pdf_path,
            tar_path=tar_path,
            pdf_refs=[],
            bib_entries=all_bib_entries,
            matches=[],
            unmatched_bib=all_bib_entries,
            error=f"PDF extraction failed: {e}"
        )

    if verbose:
        print(f"  Extracted {len(pdf_refs)} refs from PDF (raw: {stats['total_raw']}, "
              f"skipped: url={stats['skipped_url']}, short={stats['skipped_short_title']}, "
              f"no_authors={stats['skipped_no_authors']})")

    # Apply max_refs limit if specified
    if max_refs > 0:
        pdf_refs = pdf_refs[:max_refs]
        all_bib_entries = all_bib_entries[:max_refs]

    # Match references
    matches, unmatched_bib = match_references(pdf_refs, all_bib_entries)

    return PaperReport(
        pdf_path=pdf_path,
        tar_path=tar_path,
        pdf_refs=pdf_refs,
        bib_entries=all_bib_entries,
        matches=matches,
        unmatched_bib=unmatched_bib
    )


def process_directory(downloads_dir: str, limit: Optional[int] = None,
                      verbose: bool = False, max_refs: int = 0,
                      offset: int = 0) -> list[PaperReport]:
    """Process all PDF+tar.gz pairs in a directory."""

    downloads_path = Path(downloads_dir)
    if not downloads_path.exists():
        print(f"Error: Directory not found: {downloads_dir}", file=sys.stderr)
        sys.exit(1)

    # Find PDF files and their corresponding tar.gz
    reports = []
    pdf_files = sorted(downloads_path.glob("*.pdf"))

    # Apply offset first, then limit
    if offset:
        pdf_files = pdf_files[offset:]
    if limit:
        pdf_files = pdf_files[:limit]

    for pdf_path in pdf_files:
        # Look for matching tar.gz
        # Try patterns: same_name.tar.gz, same_name_source.tar.gz
        stem = pdf_path.stem
        tar_candidates = [
            pdf_path.with_suffix('.tar.gz'),
            downloads_path / f"{stem}_source.tar.gz",
            downloads_path / f"{stem}.gz.tar",
        ]

        tar_path = None
        for candidate in tar_candidates:
            if candidate.exists():
                tar_path = candidate
                break

        if not tar_path:
            if verbose:
                print(f"Skipping {pdf_path.name}: no matching tar.gz found")
            continue

        if verbose:
            print(f"\nProcessing: {pdf_path.name}")

        report = compare_paper(str(pdf_path), str(tar_path), verbose=verbose, max_refs=max_refs)
        reports.append(report)

        if verbose:
            matched = sum(1 for m in report.matches if m.match_type in ('exact', 'fuzzy'))
            valid = sum(1 for m in report.matches
                        if m.match_type in ('exact', 'fuzzy', 'likely_valid_unmatched'))
            print(f"  Extraction quality: {report.extraction_quality:.1f}% ({valid}/{len(report.matches)} look valid)")
            print(f"  Match rate: {report.match_rate:.1f}% ({matched}/{len(report.matches)} matched bib)")
            print(f"  Coverage: {report.coverage_rate:.1f}% ({matched}/{len(report.bib_entries)} bib entries found)")

    return reports


def aggregate_statistics(reports: list[PaperReport]) -> dict:
    """Generate aggregate statistics from all reports."""

    stats = {
        'total_papers': len(reports),
        'papers_with_errors': sum(1 for r in reports if r.error),
        'total_bib_entries': sum(len(r.bib_entries) for r in reports),
        'total_pdf_refs': sum(len(r.pdf_refs) for r in reports),
        'matches': {
            'exact': 0,
            'fuzzy': 0,
            'title_only': 0,
            'likely_valid_unmatched': 0,  # Looks like a valid title but not in bib (coverage gap)
            'unmatched': 0,  # Doesn't look like a valid title (parsing failure)
        },
        'failure_patterns': defaultdict(int),
        'avg_match_rate': 0.0,
        'avg_extraction_quality': 0.0,
        'avg_coverage_rate': 0.0,
    }

    for report in reports:
        for match in report.matches:
            stats['matches'][match.match_type] += 1
            for pattern in match.failure_patterns:
                stats['failure_patterns'][pattern] += 1

    # Calculate average rates (excluding papers with errors)
    valid_reports = [r for r in reports if not r.error and r.matches]
    if valid_reports:
        stats['avg_match_rate'] = sum(r.match_rate for r in valid_reports) / len(valid_reports)
        stats['avg_extraction_quality'] = sum(r.extraction_quality for r in valid_reports) / len(valid_reports)
        stats['avg_coverage_rate'] = sum(r.coverage_rate for r in valid_reports) / len(valid_reports)

    return stats


def generate_markdown_report(reports: list[PaperReport], stats: dict) -> str:
    """Generate a markdown report."""

    lines = []
    lines.append("# Reference Parsing Evaluation Report\n")

    # Summary
    lines.append("## Summary\n")
    lines.append(f"- **Papers analyzed:** {stats['total_papers']}")
    lines.append(f"- **Papers with errors:** {stats['papers_with_errors']}")
    lines.append(f"- **Total .bib entries:** {stats['total_bib_entries']}")
    lines.append(f"- **Total PDF references extracted:** {stats['total_pdf_refs']}")
    lines.append(f"- **Average extraction quality:** {stats['avg_extraction_quality']:.1f}% (PDF refs that look valid)")
    lines.append(f"- **Average match rate:** {stats['avg_match_rate']:.1f}% (PDF refs matched to bib)")
    lines.append(f"- **Average coverage:** {stats['avg_coverage_rate']:.1f}% (bib entries found in PDF)\n")

    # Match breakdown
    lines.append("### Match Types\n")
    lines.append("| Type | Count | Percentage |")
    lines.append("|------|-------|------------|")
    total_matches = sum(stats['matches'].values())
    if total_matches > 0:
        for match_type, count in stats['matches'].items():
            pct = count / total_matches * 100
            lines.append(f"| {match_type} | {count} | {pct:.1f}% |")
    lines.append("")

    # Failure patterns
    if stats['failure_patterns']:
        lines.append("## Failure Patterns\n")
        lines.append("| Pattern | Count |")
        lines.append("|---------|-------|")
        for pattern, count in sorted(stats['failure_patterns'].items(),
                                      key=lambda x: -x[1]):
            lines.append(f"| {pattern} | {count} |")
        lines.append("")

    # Per-paper results (sorted by match rate)
    lines.append("## Per-Paper Results\n")
    valid_reports = [r for r in reports if not r.error]
    valid_reports.sort(key=lambda r: r.match_rate)

    for report in valid_reports:
        pdf_name = Path(report.pdf_path).name
        matched = sum(1 for m in report.matches if m.match_type in ('exact', 'fuzzy'))
        valid = sum(1 for m in report.matches
                    if m.match_type in ('exact', 'fuzzy', 'likely_valid_unmatched'))
        lines.append(f"### {pdf_name}")
        lines.append(f"- Extraction quality: {report.extraction_quality:.1f}% ({valid}/{len(report.matches)} look valid)")
        lines.append(f"- Match rate: {report.match_rate:.1f}% ({matched}/{len(report.matches)} matched bib)")
        lines.append(f"- Coverage: {report.coverage_rate:.1f}% ({matched}/{len(report.bib_entries)} bib entries)")

        # Show mismatches
        mismatches = [m for m in report.matches if m.match_type in ('title_only', 'unmatched')]
        if mismatches:
            lines.append("\n**Mismatches:**\n")
            for m in mismatches[:5]:  # Limit to 5 per paper
                lines.append(f"- **PDF:** `{m.pdf_title[:80]}{'...' if len(m.pdf_title) > 80 else ''}`")
                if m.bib_entry:
                    lines.append(f"  - **Best BIB match:** `{m.bib_entry.title[:80]}{'...' if len(m.bib_entry.title) > 80 else ''}`")
                lines.append(f"  - Score: {m.match_score:.0f}%, Patterns: {', '.join(m.failure_patterns) or 'none'}")
                lines.append(f"  - Raw: `{m.pdf_raw[:100]}{'...' if len(m.pdf_raw) > 100 else ''}`")

        # Show unmatched bib entries
        if report.unmatched_bib:
            lines.append(f"\n**Unmatched .bib entries ({len(report.unmatched_bib)}):**\n")
            for bib in report.unmatched_bib[:3]:
                lines.append(f"- `{bib.title[:80]}{'...' if len(bib.title) > 80 else ''}`")

        lines.append("")

    # Papers with errors
    error_reports = [r for r in reports if r.error]
    if error_reports:
        lines.append("## Papers with Errors\n")
        for report in error_reports:
            pdf_name = Path(report.pdf_path).name
            lines.append(f"- **{pdf_name}**: {report.error}")
        lines.append("")

    return "\n".join(lines)


def generate_json_report(reports: list[PaperReport], stats: dict) -> str:
    """Generate a JSON report."""

    data = {
        'statistics': stats,
        'papers': []
    }

    # Convert failure_patterns defaultdict to regular dict
    data['statistics']['failure_patterns'] = dict(stats['failure_patterns'])

    for report in reports:
        paper_data = {
            'pdf': Path(report.pdf_path).name,
            'tar': Path(report.tar_path).name,
            'match_rate': report.match_rate,
            'bib_count': len(report.bib_entries),
            'pdf_ref_count': len(report.pdf_refs),
            'error': report.error,
            'matches': []
        }

        for match in report.matches:
            match_data = {
                'pdf_title': match.pdf_title,
                'pdf_authors': match.pdf_authors,
                'match_type': match.match_type,
                'match_score': match.match_score,
                'failure_patterns': match.failure_patterns,
            }
            if match.bib_entry:
                match_data['bib_title'] = match.bib_entry.title
                match_data['bib_authors'] = match.bib_entry.authors
            paper_data['matches'].append(match_data)

        paper_data['unmatched_bib'] = [
            {'title': bib.title, 'authors': bib.authors}
            for bib in report.unmatched_bib
        ]

        data['papers'].append(paper_data)

    return json.dumps(data, indent=2)


def main():
    parser = argparse.ArgumentParser(
        description="Evaluate reference parsing against .bib ground truth"
    )
    parser.add_argument(
        'downloads_dir',
        help="Directory containing PDF files and their source tar.gz archives"
    )
    parser.add_argument(
        '-o', '--output',
        help="Output file path (markdown format unless --json)"
    )
    parser.add_argument(
        '--json',
        action='store_true',
        help="Output in JSON format"
    )
    parser.add_argument(
        '-v', '--verbose',
        action='store_true',
        help="Verbose output during processing"
    )
    parser.add_argument(
        '--limit',
        type=int,
        help="Limit number of papers to process (for testing)"
    )
    parser.add_argument(
        '--offset',
        type=int,
        default=0,
        help="Skip first N papers (for batch processing)"
    )
    parser.add_argument(
        '--max-refs',
        type=int,
        default=0,
        help="Limit references per paper (0 = unlimited)"
    )

    args = parser.parse_args()

    print(f"Processing papers in: {args.downloads_dir}")
    if args.offset:
        print(f"Starting at paper {args.offset}")
    if args.limit:
        print(f"Limiting to {args.limit} papers")
    if args.max_refs:
        print(f"Limiting to {args.max_refs} refs per paper")

    # Process papers
    reports = process_directory(
        args.downloads_dir,
        limit=args.limit,
        verbose=args.verbose,
        max_refs=args.max_refs,
        offset=args.offset
    )

    if not reports:
        print("No papers with matching PDF and tar.gz files found.")
        sys.exit(1)

    # Generate statistics
    stats = aggregate_statistics(reports)

    # Generate report
    if args.json:
        report_content = generate_json_report(reports, stats)
    else:
        report_content = generate_markdown_report(reports, stats)

    # Output
    if args.output:
        with open(args.output, 'w', encoding='utf-8') as f:
            f.write(report_content)
        print(f"\nReport written to: {args.output}")
    else:
        print("\n" + "="*60 + "\n")
        print(report_content)

    # Print summary
    print(f"\n{'='*60}")
    print(f"Summary: {stats['total_papers']} papers")
    print(f"  Extraction quality: {stats['avg_extraction_quality']:.1f}% (refs that look valid)")
    print(f"  Match rate: {stats['avg_match_rate']:.1f}% (refs matched to bib)")
    print(f"Match breakdown: exact={stats['matches']['exact']}, "
          f"fuzzy={stats['matches']['fuzzy']}, "
          f"title_only={stats['matches']['title_only']}, "
          f"valid_unmatched={stats['matches']['likely_valid_unmatched']}, "
          f"parse_failures={stats['matches']['unmatched']}")

    if stats['failure_patterns']:
        print(f"Failure patterns: {dict(sorted(stats['failure_patterns'].items(), key=lambda x: -x[1])[:5])}")


if __name__ == '__main__':
    main()
