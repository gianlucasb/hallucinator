# Understanding Results

This guide explains how to interpret Hallucinator's output, what each verdict means, and how to handle edge cases.

## Verdict Types

Each validated reference receives one of these statuses:

### Verified

The reference was found in at least one academic database with matching authors.

- **Source** is reported (e.g., "CrossRef", "DBLP Offline", "arXiv")
- **Found authors** are listed for comparison
- **Paper URL** links to the database entry when available

A verified reference is almost certainly real. The 95% fuzzy title matching threshold accommodates minor PDF extraction artifacts while remaining strict enough to avoid false matches.

### Not Found

The reference was not found in any queried database.

This does **not** necessarily mean the reference is fabricated. Common legitimate reasons:

- **Very recent publication** — Not yet indexed by databases
- **Book chapters or dissertations** — Less coverage in article-focused databases
- **Workshop or regional conference papers** — May not be in major indices
- **PDF extraction error** — Title was mangled during extraction (ligatures, hyphenation, encoding issues)
- **Database outage** — Temporary API issues (check "Failed DBs" in the output)

**What to do:** Check the "Failed DBs" list. If multiple databases timed out, the reference may simply need rechecking. Use Google Scholar or the paper URL (if available) for manual verification.

### Author Mismatch

The title was found in a database, but the authors don't match.

Possible explanations:

- **Different paper with similar title** — The database returned a different paper
- **Author name variants** — Different transliterations, maiden/married names, inconsistent initials
- **Preprint vs. published version** — Author list changed between versions
- **PDF extraction error** — Authors were incorrectly parsed from the PDF

**What to do:** Compare the "PDF authors" and "DB authors" in the output. If they're clearly the same people with different name formats, this is a false positive. If the authors are completely different, it's worth investigating.

### Retracted

The reference was found but has been retracted. This information comes from CrossRef's retraction metadata.

- **Retraction DOI** links to the retraction notice
- **Retraction source** indicates the type (e.g., retraction, removal, expression of concern)

Citing retracted papers is a serious concern in academic integrity. However, some retractions are for reasons unrelated to the paper's scientific content (e.g., copyright disputes). Always check the retraction notice.

## Skipped References

Some references are excluded from validation:

| Reason | Explanation |
|--------|-------------|
| **URL-only** | Reference is just a URL to a non-academic site (GitHub, documentation) |
| **Short title** | Title has fewer than 5 words (too short for reliable matching) |
| **No title** | No title could be extracted from the reference text |

Skipped references are not counted in the "problematic" percentage.

**Exception:** References with a DOI or arXiv ID are never skipped for short title, since the identifier provides a reliable lookup path.

## Paper Verdicts (TUI)

In the TUI, entire papers can be marked with a verdict:

- **Safe** — All references verified, or issues have been manually reviewed
- **Questionable** — Contains concerning unverified references

These are user-assigned labels for batch triage, not automated judgments.

## Per-Database Results

Each reference includes per-database query results showing:

- **Database name** — Which DB was queried
- **Status** — `match`, `no_match`, `author_mismatch`, `timeout`, `rate_limited`, `error`, `skipped`
- **Elapsed time** — How long the query took
- **Found authors** — Authors returned by the database (if found)
- **Paper URL** — Direct link to the database entry (if found)

Use this to understand why a reference got its verdict. If several databases timed out, the "Not Found" verdict may be unreliable.

## DOI and arXiv Validation

When a reference includes a DOI or arXiv ID:

- **Valid** — The identifier resolves to a real paper
- **Invalid** — The identifier doesn't resolve (possible fabrication signal)

A verified reference with an invalid DOI is flagged separately — the paper exists in some database, but the DOI in the citation is wrong or fabricated.

## False Positive Overrides (TUI)

In the TUI, you can mark results as false positives with a reason:

| Reason | Use when |
|--------|----------|
| **Broken Parse** | PDF extraction mangled the title/authors |
| **Exists Elsewhere** | You verified the paper exists outside indexed databases |
| **All Timed Out** | All databases timed out; the result is inconclusive |
| **Known Good** | You personally know this reference is legitimate |
| **Non-Academic** | The reference is to a non-academic resource (software, standard, etc.) |

FP overrides are reflected in exported results: the `effective_status` changes to `verified` while the original `status` is preserved for transparency.

## Confidence Signals

Higher confidence in a "Not Found" verdict:

- Multiple databases returned `no_match` (not just timeouts)
- No DOI or arXiv ID was present in the reference
- Title was cleanly extracted (no obvious parsing artifacts)
- Paper claims to be from a well-indexed venue (top conferences, major journals)

Lower confidence (consider manual verification):

- Several databases timed out or returned errors
- Title contains unusual characters or formatting
- Reference is to a workshop paper, technical report, or dissertation
- The title is very short (close to the 5-word minimum)

## The Problematic Percentage

The summary reports a "problematic %" calculated as:

```
(not_found + author_mismatch + retracted) / (total - skipped) * 100
```

This gives a quick signal for triage. A high percentage doesn't prove misconduct — it means the paper warrants closer human review. Even legitimate papers checking niche or very recent literature can have a notable percentage of unverified references.

## Manual Verification Workflow

When Hallucinator flags a reference as Not Found:

1. **Check failed databases** — Were most DBs queried, or did many time out?
2. **Search Google Scholar** — The output includes a Google Scholar link for each reference
3. **Check the paper URL** — If available, visit the link directly
4. **Verify the venue** — Is the claimed venue real? Was the paper published there?
5. **Check authors** — Do the listed authors exist and publish in this field?
6. **Look for the DOI** — If a DOI is listed, try resolving it at `doi.org`
