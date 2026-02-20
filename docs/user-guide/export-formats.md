# Export Formats

Hallucinator can export validation results in five formats. The TUI supports all formats via its export dialog; the CLI writes text output by default (use `--output` to save to a file).

## Formats

| Format | Extension | Best for |
|--------|-----------|----------|
| JSON | `.json` | Programmatic processing, data pipelines |
| CSV | `.csv` | Spreadsheets, bulk analysis |
| Markdown | `.md` | Reports, GitHub issues, documentation |
| Text | `.txt` | Plain-text records, email |
| HTML | `.html` | Standalone visual reports |

## Sorting Order

All formats use the same reference ordering within each paper:

1. **Retracted** — Highest priority (most critical)
2. **Not Found** — Potential hallucinations
3. **Author Mismatch** — Title found, wrong authors
4. **DOI/arXiv Issues** — Verified but with invalid identifiers
5. **FP-overridden** — User-verified false positives
6. **Clean Verified** — Confirmed references
7. **Skipped** — References excluded from validation

Within each category, references are ordered by their original reference number.

## False Positive Handling

When a reference has a false-positive override (from the TUI):

- **Original status** is preserved (e.g., `not_found`)
- **Effective status** becomes `verified`
- **FP reason** is included (e.g., `broken_parse`, `exists_elsewhere`)
- **Adjusted statistics** move FP-overridden references from their original bucket into `verified`

## JSON Schema

The JSON export produces an array of paper objects:

```json
[
  {
    "filename": "paper.pdf",
    "verdict": "safe",
    "stats": {
      "total": 42,
      "verified": 38,
      "not_found": 3,
      "author_mismatch": 1,
      "retracted": 0,
      "skipped": 5,
      "problematic_pct": 10.8
    },
    "references": [
      {
        "index": 0,
        "original_number": 1,
        "title": "Attention Is All You Need",
        "raw_citation": "[1] A. Vaswani et al., ...",
        "status": "verified",
        "effective_status": "verified",
        "fp_reason": null,
        "source": "CrossRef",
        "ref_authors": ["A. Vaswani", "N. Shazeer"],
        "found_authors": ["Ashish Vaswani", "Noam Shazeer"],
        "paper_url": "https://doi.org/10.5555/3295222.3295349",
        "failed_dbs": [],
        "doi_info": {
          "doi": "10.5555/3295222.3295349",
          "valid": true,
          "title": null
        },
        "arxiv_info": null,
        "retraction_info": null,
        "db_results": [
          {
            "db": "CrossRef",
            "status": "match",
            "elapsed_ms": 234,
            "authors": ["Ashish Vaswani", "Noam Shazeer"],
            "url": "https://doi.org/10.5555/3295222.3295349"
          },
          {
            "db": "arXiv",
            "status": "skipped",
            "elapsed_ms": 0,
            "authors": [],
            "url": null
          }
        ]
      }
    ]
  }
]
```

### Per-Reference Fields

| Field | Type | Description |
|-------|------|-------------|
| `index` | number | Zero-based index in the results array |
| `original_number` | number | Original reference number from the paper (1-based) |
| `title` | string | Extracted reference title |
| `raw_citation` | string | Full raw citation text from PDF |
| `status` | string | Original status: `verified`, `not_found`, `author_mismatch` |
| `effective_status` | string | Status after FP overrides |
| `fp_reason` | string? | FP reason if overridden: `broken_parse`, `exists_elsewhere`, `all_timed_out`, `known_good`, `non_academic` |
| `source` | string? | Database that verified the reference |
| `ref_authors` | string[] | Authors extracted from the PDF |
| `found_authors` | string[] | Authors returned by the verifying database |
| `paper_url` | string? | URL to the paper in the source database |
| `failed_dbs` | string[] | Databases that timed out or errored |
| `doi_info` | object? | DOI validation: `{doi, valid, title}` |
| `arxiv_info` | object? | arXiv validation: `{arxiv_id, valid, title}` |
| `retraction_info` | object? | Retraction data: `{is_retracted, retraction_doi, retraction_source}` |
| `db_results` | object[] | Per-database query results |

### Skipped Reference Fields

Skipped references include a `skip_reason` field instead of validation data:

```json
{
  "index": 5,
  "original_number": 6,
  "title": "GitHub repo",
  "status": "skipped",
  "effective_status": "skipped",
  "skip_reason": "url_only",
  ...
}
```

### Per-DB Result Fields

| Field | Type | Description |
|-------|------|-------------|
| `db` | string | Database name |
| `status` | string | `match`, `no_match`, `author_mismatch`, `timeout`, `rate_limited`, `error`, `skipped` |
| `elapsed_ms` | number | Query time in milliseconds |
| `authors` | string[] | Authors returned (if found) |
| `url` | string? | Paper URL in this database |

## CSV Schema

One row per reference, with these columns:

```
Filename,Verdict,Ref#,Title,Status,EffectiveStatus,FpReason,Source,Retracted,Authors,FoundAuthors,PaperURL,DOI,ArxivID,FailedDBs
```

Multi-value fields (Authors, FoundAuthors, FailedDBs) use semicolons as separators within the CSV field.

## Markdown Structure

```markdown
# Hallucinator Results

## paper.pdf [SAFE]

**42** references | **38** verified | **3** not found | ...

### Problematic References

**[7]** Suspicious Paper Title — ✗ Not Found
- [Google Scholar](...)

### Verified References

| # | Title | Source | URL |
|---|-------|--------|-----|
| 1 | Attention Is All You Need | CrossRef | [link](...) |

### Skipped References

| # | Title | Reason |
|---|-------|--------|
| 6 | GitHub repo | URL-only |
```

Sections are only included if they contain references (no empty "Problematic References" heading when everything is verified).

## Text Format

Plain-text with fixed-width formatting:

```
Hallucinator Results
============================================================

paper.pdf [SAFE]
-----------------
  42 total | 38 verified | 3 not found | 1 mismatch | 0 retracted | 5 skipped | 10.8% problematic

  [1] Attention Is All You Need - Verified (CrossRef)
       Authors (PDF): A. Vaswani, N. Shazeer
       DOI: 10.5555/3295222.3295349 (valid)
       URL: https://doi.org/...
  [7] Suspicious Paper Title - NOT FOUND
       Authors (PDF): J. Doe, A. Smith
       Timed out: Semantic Scholar, Europe PMC
```

## HTML Format

A self-contained HTML file with:

- Dark theme with CSS variables
- Stat cards showing totals across all papers
- Collapsible per-paper sections
- Color-coded badges (green: verified, red: not found, yellow: mismatch, dark red: retracted)
- Author comparison grid for mismatches
- Retraction warning boxes
- Google Scholar and paper URL links
- Raw citation in expandable details blocks
- Timestamp in footer

The HTML requires no external dependencies — all CSS is inlined.
