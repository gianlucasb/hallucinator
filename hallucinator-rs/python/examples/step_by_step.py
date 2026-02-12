"""Step-by-step extraction: run each pipeline stage individually.

Useful for debugging why a specific paper's references aren't parsed correctly.

    pip install .          # from hallucinator-rs/
    python examples/step_by_step.py path/to/paper.pdf
"""

import sys

from hallucinator import PdfExtractor


def main():
    if len(sys.argv) < 2:
        print("Usage: python step_by_step.py <path_to_pdf>")
        sys.exit(1)

    pdf_path = sys.argv[1]
    ext = PdfExtractor()

    # ── Step 1: Extract raw text from PDF ──
    print("=" * 60)
    print("STEP 1: Raw text extraction")
    print("=" * 60)
    text = ext.extract_text(pdf_path)
    print(f"Extracted {len(text)} characters")
    print(f"Preview (last 200 chars):\n...{text[-200:]}\n")

    # ── Step 2: Find the references section ──
    print("=" * 60)
    print("STEP 2: Find references section")
    print("=" * 60)
    section = ext.find_section(text)
    if section is None:
        print("ERROR: No references section found!")
        print("Try setting a custom section header regex:")
        print('  ext.section_header_regex = r"(?i)\\n\\s*Works Cited\\s*\\n"')
        sys.exit(1)
    print(f"Section: {len(section)} characters")
    print(f"Preview (first 300 chars):\n{section[:300]}\n")

    # ── Step 3: Segment into individual references ──
    print("=" * 60)
    print("STEP 3: Segment references")
    print("=" * 60)
    segments = ext.segment(section)
    print(f"Found {len(segments)} segments\n")
    for i, seg in enumerate(segments[:5], 1):
        preview = seg[:120].replace("\n", " ")
        print(f"  [{i}] {preview}...")
    if len(segments) > 5:
        print(f"  ... and {len(segments) - 5} more")
    print()

    # ── Step 4: Parse each segment into structured references ──
    print("=" * 60)
    print("STEP 4: Parse references")
    print("=" * 60)
    prev_authors = None
    parsed = 0
    skipped = 0
    for i, seg in enumerate(segments, 1):
        ref = ext.parse_reference(seg, prev_authors=prev_authors)
        if ref is None:
            skipped += 1
            preview = seg[:80].replace("\n", " ")
            print(f"  [{i}] SKIPPED: {preview}...")
        else:
            parsed += 1
            prev_authors = ref.authors
            print(f"  [{i}] {ref.title}")
            print(f"       Authors: {', '.join(ref.authors)}")

    print(f"\nParsed: {parsed}, Skipped: {skipped}")


if __name__ == "__main__":
    main()
