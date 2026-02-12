"""End-to-end example: extract PDF references and validate them.

Usage:
    python examples/validate_references.py paper.pdf

Requires network access to query academic databases.
"""

import sys

from hallucinator import PdfExtractor, Validator, ValidatorConfig


def main():
    if len(sys.argv) < 2:
        print("Usage: python examples/validate_references.py <pdf_path>")
        sys.exit(1)

    pdf_path = sys.argv[1]

    # Step 1: Extract references from PDF
    print(f"Extracting references from {pdf_path}...")
    ext = PdfExtractor()
    result = ext.extract(pdf_path)
    refs = result.references
    stats = result.skip_stats
    print(
        f"Found {len(refs)} references "
        f"({stats.total_raw} raw, {stats.url_only} URL-only, "
        f"{stats.short_title} short-title skipped)"
    )

    if not refs:
        print("No references to validate.")
        return

    # Step 2: Configure and run validation
    config = ValidatorConfig()
    # Uncomment to set API keys:
    # config.s2_api_key = "your-semantic-scholar-key"
    # config.openalex_key = "your-openalex-key"
    # config.crossref_mailto = "you@example.com"

    def on_progress(event):
        if event.event_type == "checking":
            print(f"  [{event.index + 1}/{event.total}] Checking: {event.title}")
        elif event.event_type == "result":
            r = event.result
            status_icon = {
                "verified": "+",
                "not_found": "?",
                "author_mismatch": "~",
            }.get(r.status, " ")
            source = f" ({r.source})" if r.source else ""
            print(f"  [{status_icon}] {r.title}{source}")
        elif event.event_type == "retry_pass":
            print(f"  Retrying {event.count} unresolved references...")

    print("\nValidating references...")
    validator = Validator(config)
    results = validator.check(refs, progress=on_progress)

    # Step 3: Summary
    check_stats = Validator.stats(results)
    print("\n--- Summary ---")
    print(f"Total:            {check_stats.total}")
    print(f"Verified:         {check_stats.verified}")
    print(f"Not found:        {check_stats.not_found}")
    print(f"Author mismatch:  {check_stats.author_mismatch}")
    print(f"Retracted:        {check_stats.retracted}")

    # Step 4: Show details for suspicious references
    suspicious = [r for r in results if r.status != "verified"]
    if suspicious:
        print(f"\n--- Suspicious references ({len(suspicious)}) ---")
        for r in suspicious:
            print(f"\n  [{r.status.upper()}] {r.title}")
            if r.ref_authors:
                print(f"    Authors: {', '.join(r.ref_authors)}")
            if r.failed_dbs:
                print(f"    Failed DBs: {', '.join(r.failed_dbs)}")
            if r.doi_info:
                print(f"    DOI: {r.doi_info.doi} (valid={r.doi_info.valid})")
            if r.retraction_info and r.retraction_info.is_retracted:
                print(f"    RETRACTED! Source: {r.retraction_info.retraction_source}")


if __name__ == "__main__":
    main()
