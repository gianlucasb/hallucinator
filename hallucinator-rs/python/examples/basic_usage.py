"""Basic usage of the hallucinator Python bindings.

Extract references from a PDF and inspect the results.

    pip install .          # from hallucinator-rs/
    python examples/basic_usage.py path/to/paper.pdf
"""

import sys

from hallucinator import PdfExtractor


def main():
    if len(sys.argv) < 2:
        print("Usage: python basic_usage.py <path_to_pdf>")
        sys.exit(1)

    pdf_path = sys.argv[1]
    ext = PdfExtractor()

    # ── Full pipeline: PDF → structured references ──
    result = ext.extract(pdf_path)

    print(
        f"Found {len(result)} references (from {result.skip_stats.total_raw} raw segments)\n"
    )

    # Show skip stats
    stats = result.skip_stats
    if stats.url_only or stats.short_title or stats.no_title or stats.no_authors:
        print("Skipped:")
        if stats.url_only:
            print(f"  URL-only:    {stats.url_only}")
        if stats.short_title:
            print(f"  Short title: {stats.short_title}")
        if stats.no_title:
            print(f"  No title:    {stats.no_title}")
        if stats.no_authors:
            print(f"  No authors:  {stats.no_authors}")
        print()

    # Print each reference
    for i, ref in enumerate(result.references, 1):
        print(f"[{i}] {ref.title}")
        print(f"    Authors: {', '.join(ref.authors)}")
        if ref.doi:
            print(f"    DOI: {ref.doi}")
        if ref.arxiv_id:
            print(f"    arXiv: {ref.arxiv_id}")
        print()


if __name__ == "__main__":
    main()
