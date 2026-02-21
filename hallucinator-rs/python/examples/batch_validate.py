#!/usr/bin/env python3
"""Batch-validate references without PDF extraction.

Creates Reference objects directly from structured data (e.g. a CSV or
BibTeX parser) and validates them against academic databases.

This addresses the use case from issue #178 — validating references when
you already have titles, authors, and DOIs without needing a PDF.
"""

from hallucinator import Reference, Validator, ValidatorConfig

# Build references from structured data — no PDF needed.
refs = [
    Reference(
        "Attention Is All You Need",
        authors=["Vaswani", "Shazeer", "Parmar", "Uszkoreit", "Jones"],
    ),
    Reference(
        "BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding",
        authors=["Devlin", "Chang", "Lee", "Toutanova"],
        doi="10.18653/v1/N19-1423",
    ),
    Reference(
        "A Completely Made Up Paper Title That Does Not Exist Anywhere",
        authors=["Fakename", "Notreal"],
    ),
]

# Configure and validate
config = ValidatorConfig()
validator = Validator(config)


def on_progress(event):
    if event.event_type == "checking":
        print(f"  [{event.index + 1}/{event.total}] Checking: {event.title}")
    elif event.event_type == "result":
        r = event.result
        icon = {"verified": "+", "not_found": "?", "author_mismatch": "~"}[r.status]
        src = f" ({r.source})" if r.source else ""
        print(f"  [{icon}] {r.title}{src}")


print(f"Validating {len(refs)} references...\n")
results = validator.check(refs, progress=on_progress)

# Summary
stats = Validator.stats(results)
print(f"\nVerified: {stats.verified}/{stats.total}")
if stats.not_found:
    print(f"Potentially hallucinated: {stats.not_found}")
