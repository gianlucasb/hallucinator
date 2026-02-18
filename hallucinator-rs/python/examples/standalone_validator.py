"""Standalone validation: build references by hand and validate them.

No PDF required -- construct Reference objects directly and feed them
to the Validator.

Usage:
    python examples/standalone_validator.py

Requires network access to query academic databases.
"""

from hallucinator import Reference, Validator, ValidatorConfig

# Build references by hand -- only raw_citation is required,
# but providing title + authors improves matching accuracy.
refs = [
    Reference(
        raw_citation=(
            'A. Vaswani, N. Shazeer, N. Parmar, J. Uszkoreit, L. Jones, '
            'A. N. Gomez, L. Kaiser, and I. Polosukhin, "Attention Is All '
            'You Need," in NeurIPS, 2017.'
        ),
        title="Attention Is All You Need",
        authors=[
            "Ashish Vaswani",
            "Noam Shazeer",
            "Niki Parmar",
            "Jakob Uszkoreit",
            "Llion Jones",
            "Aidan N. Gomez",
            "Lukasz Kaiser",
            "Illia Polosukhin",
        ],
    ),
    Reference(
        raw_citation=(
            'J. Devlin, M.-W. Chang, K. Lee, and K. Toutanova, "BERT: '
            "Pre-training of Deep Bidirectional Transformers for Language "
            'Understanding," in NAACL-HLT, 2019.'
        ),
        title="BERT: Pre-training of Deep Bidirectional Transformers for Language Understanding",
        authors=["Jacob Devlin", "Ming-Wei Chang", "Kenton Lee", "Kristina Toutanova"],
    ),
    # A completely fabricated reference -- should come back as "not_found".
    Reference(
        raw_citation=(
            'Z. Fakename and Q. Pseudonym, "A Totally Real Paper That '
            'Definitely Exists," in Proceedings of Nowhere, 2099.'
        ),
        title="A Totally Real Paper That Definitely Exists",
        authors=["Z. Fakename", "Q. Pseudonym"],
    ),
]

# Configure the validator (all defaults are fine for a quick test).
config = ValidatorConfig()
# Uncomment to set API keys or tweak settings:
# config.s2_api_key = "your-semantic-scholar-key"
# config.crossref_mailto = "you@example.com"


def on_progress(event):
    if event.event_type == "checking":
        print(f"  [{event.index + 1}/{event.total}] Checking: {event.title}")
    elif event.event_type == "result":
        r = event.result
        icon = {"verified": "+", "not_found": "?", "author_mismatch": "~"}.get(r.status, " ")
        source = f" ({r.source})" if r.source else ""
        print(f"  [{icon}] {r.title}{source}")
    elif event.event_type == "retry_pass":
        print(f"  Retrying {event.count} unresolved references...")


print(f"Validating {len(refs)} hand-built references...\n")
validator = Validator(config)
results = validator.check(refs, progress=on_progress)

# Print summary
stats = Validator.stats(results)
print("\n--- Summary ---")
print(f"Total:            {stats.total}")
print(f"Verified:         {stats.verified}")
print(f"Not found:        {stats.not_found}")
print(f"Author mismatch:  {stats.author_mismatch}")
print(f"Retracted:        {stats.retracted}")
