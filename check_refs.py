#!/usr/bin/env python3
"""
Check plain-text reference lists for hallucinated references.

hallucinator-cli reads PDF, .bbl, .bib and GROBID TEI XML, but not plain
text, so each reference takes one of two routes into it:

  * APA-style references ("Authors (2023). Title. Venue.") are parsed here
    and handed over as structured TEI. hallucinator's free-text parser
    misreads this style: some come back with "(2023)" as the title (and are
    skipped as too short), others get the venue glued onto the title. (A
    .bib would also work, but the BibTeX path drops URLs, so GitHub and
    blog references would never get a URL check.)
  * Everything else (IEEE, ACM, ...) goes through hallucinator's own
    free-text parser, the one it runs on PDFs.

Either way hallucinator does all of the checking. Offline indexes found next
to this script (dblp.db, acl.db, iacr.db, openalex.idx, arxiv.db) replace
their online counterparts, Semantic Scholar is disabled (1 request/s without
an API key), and --url-match checks the non-academic URLs that no database
indexes.

Usage:
    ./check_refs.py                  # every reference list (*.txt) in ./runs
    ./check_refs.py a.txt b.txt      # just these

A list holds one reference per paragraph, or per line if it has no blank lines.

Results land next to each input:
    <name>.summary.txt   every reference with its verdict, flagged ones first
    <name>.results.txt   hallucinator's report, and <name>.results.json its
                         per-reference results (hallucinator-tui --load)
    <name>.tei.xml       the APA references as hallucinator was given them
Non-APA references get their own <name>.freetext.results.* (from the page
hallucinator read, <name>.freetext.html). The summary numbers references as
the input does; the other files number them in the order hallucinator read
them, which only differs when a list mixes APA with other styles.
"""

import argparse
import html
import json
import re
import subprocess
import sys
import unicodedata
from pathlib import Path
from typing import Optional
from xml.sax.saxutils import escape, quoteattr

ROOT = Path(__file__).resolve().parent
CLI = ROOT / "hallucinator-rs" / "target" / "release" / "hallucinator-cli"
RUNS = ROOT / "runs"  # default input folder, next to this script

# The offline indexes, as (flag, path under ROOT, backend); each is used if present.
OFFLINE_DBS = [
    ("--dblp-offline", "dblp.db", "DBLP"),
    ("--acl-offline", "acl.db", "ACL Anthology"),
    ("--iacr-eprint-offline", "iacr.db", "IACR ePrint"),
    ("--openalex-offline", "openalex.idx", "OpenAlex"),
    ("--arxiv-offline", "arxiv.db", "arXiv"),
]

# "Authors (2023). Title. Venue." -- also (2022a), (2024, May 17) and (n.d.).
APA_RE = re.compile(
    r"^(?P<authors>.*?)\s*\((?P<year>\d{4}[a-z]?|n\.d\.)(?:,[^)]*)?\)\.?\s*(?P<rest>.+)$"
)
# A title ends at its first sentence-ending period. "?" doesn't end it, so
# "Who Is AI Replacing? The Impact of ..." stays in one piece.
PERIOD_RE = re.compile(r"(?<=[^\s.])\.(?=\s|$)")
ABBREVIATIONS = {"vs", "e.g", "i.e", "U.S", "U.K", "St", "Dr", "Mr", "Mrs", "Ms", "No", "Vol"}
INITIAL_RE = re.compile(r"[^\W\d_]{1,2}\.")  # "A.", "Th.", either half of "Y.-S."
DOI_RE = re.compile(r"\b10\.\d{4,9}/\S+")
ARXIV_RE = re.compile(
    r"(?i)\barxiv(?:\s*:\s*|\.org/(?:abs|pdf)/)"
    r"(\d{4}\.\d{4,5}(?:v\d+)?|[a-z-]+(?:\.[a-z]{2})?/\d{7})"
)
# Scheme-less URLs ("github.com/rom1504/clip-retrieval") count too.
URL_RE = re.compile(r"https?://\S+|(?<![\w@/.:-])(?:[a-z0-9-]+\.)+[a-z]{2,}/\S*", re.I)
# Domains hallucinator leaves to its dedicated backends rather than URL Check
# (ACADEMIC_DOMAINS in hallucinator-core's text_utils.rs).
ACADEMIC_RE = re.compile(
    r"(?i)doi\.org|arxiv\.org|acm\.org|ieee\.org|usenix\.org|semanticscholar\.org|"
    r"dblp\.org|aclanthology\.org|openreview\.net|neurips\.cc|proceedings\.mlr\.press"
)
TRAILING_PUNCT = ".,;:)]}\"'\u201c\u201d\u2018\u2019"


def read_references(path: Path) -> list[str]:
    """One reference per paragraph, or per line if there are no blank lines."""
    text = path.read_text(encoding="utf-8").strip()
    blocks = re.split(r"\n\s*\n", text) if re.search(r"\n\s*\n", text) else text.splitlines()
    refs = (" ".join(block.split()) for block in blocks)
    return [re.sub(r"^(?:\[\d+\]|\d+[.)])\s+", "", ref) for ref in refs if ref]


def is_initials(token: str) -> bool:
    """'A.', 'A. J. B.', 'Y.-S.', 'W.-t.'"""
    return token[:1].isupper() and all(
        INITIAL_RE.fullmatch(part) for part in re.split(r"\s+|-", token)
    )


def parse_authors(text: str) -> list[tuple[str, str]]:
    """'Chaney, A. J. B., Stewart, B. M., & Engelhardt, B. E.' -> [(initials, surname), ...]

    A name with no initials after it, such as an organisation ("CompVis."),
    becomes a surname with no forename.
    """
    text = re.sub(r"\bet al\b\.?|\.{3}|\u2026", "", text)
    text = re.sub(r"\s*,?\s*&\s*", ", ", text)
    authors, surname = [], None
    for token in (t.strip() for t in text.split(",")):
        if not token:
            continue
        if is_initials(token):
            if surname:
                authors.append((token, surname))
                surname = None
            continue
        if surname:
            authors.append(("", surname))
        surname = token.rstrip(".")
    if surname:
        authors.append(("", surname))
    return authors


def split_title(rest: str) -> tuple[str, str]:
    """'Title. Venue. URL' -> ('Title', 'Venue. URL')"""
    for m in PERIOD_RE.finditer(rest):
        if rest[: m.start()].rsplit(None, 1)[-1] not in ABBREVIATIONS:
            return rest[: m.start()], rest[m.end():].strip()
    return rest, ""


def parse_reference(ref: str) -> Optional[dict]:
    """Pull the fields hallucinator checks out of one APA reference."""
    m = APA_RE.match(ref)
    if not m:
        return None
    title, venue = split_title(m["rest"])
    title = re.split(r"\s+(?=https?://)", title)[0]
    title = re.sub(r"\s*\[[^\]]*\]$", "", title).strip()  # "[Computer software]"
    if not title or URL_RE.match(title):
        return None

    urls = []
    for url in URL_RE.findall(ref):
        url = url.rstrip(TRAILING_PUNCT)
        if ACADEMIC_RE.search(url) or DOI_RE.search(url):
            continue
        url = url if url.lower().startswith("http") else "https://" + url
        if url not in urls:
            urls.append(url)
    doi = DOI_RE.search(ref)
    arxiv = ARXIV_RE.search(ref)
    return {
        "authors": parse_authors(m["authors"]),
        "title": title,
        "venue": venue,
        "doi": doi[0].rstrip(TRAILING_PUNCT) if doi else None,
        "arxiv": arxiv[1] if arxiv else None,
        "urls": urls,
    }


def to_tei(parsed: list[dict]) -> str:
    """Render parsed references as GROBID TEI, the shape hallucinator-grobid reads."""
    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<TEI xmlns="http://www.tei-c.org/ns/1.0">',
        '<text><back><div type="references"><listBibl>',
    ]
    for n, p in enumerate(parsed, 1):
        lines.append(f'<biblStruct xml:id="b{n}">')
        lines.append("  <analytic>")
        lines.append(f'    <title level="a" type="main">{escape(p["title"])}</title>')
        for forename, surname in p["authors"]:
            fore = f'<forename type="first">{escape(forename)}</forename>' if forename else ""
            lines.append(
                f"    <author><persName>{fore}<surname>{escape(surname)}</surname></persName></author>"
            )
        lines.append("  </analytic>")
        if p["venue"]:
            lines.append(f'  <monogr><title level="m">{escape(p["venue"])}</title></monogr>')
        if p["doi"]:
            lines.append(f'  <idno type="DOI">{escape(p["doi"])}</idno>')
        if p["arxiv"]:
            lines.append(f'  <idno type="arXiv">{escape(p["arxiv"])}</idno>')
        lines.extend(f"  <ptr target={quoteattr(url)}/>" for url in p["urls"])
        lines.append("</biblStruct>")
    lines.append("</listBibl></div></back></text></TEI>")
    return "\n".join(lines) + "\n"


def to_freetext_page(refs: list[str], name: str) -> str:
    """Lay references out the way hallucinator's PDF pipeline expects them.

    The CLI hands any extension it doesn't recognise to MuPDF, which renders
    HTML like a PDF. The "References" heading needs a line above it to be
    found, the [N] markers keep entries apart (only if they run 1, 2, 3, ...),
    and the 1pt font keeps each reference on one line, so nothing is wrapped
    or hyphenated.
    """
    body = "".join(f"<p>[{n}] {html.escape(ref)}</p>" for n, ref in enumerate(refs, 1))
    return (
        '<!DOCTYPE html><html><head><meta charset="utf-8">'
        "<style>body { font-size: 1pt }</style></head><body>"
        f"<p>{html.escape(name)}</p><h1>References</h1>{body}</body></html>\n"
    )


def offline_args() -> list[str]:
    """The --*-offline flags for the indexes present next to this script."""
    args, offline = [], []
    for flag, name, backend in OFFLINE_DBS:
        path = ROOT / name
        # A missing path must not be passed (the CLI hard-errors instead of
        # falling back), and an interrupted update-arxiv leaves a
        # valid-but-empty stub that would silence arXiv rather than fall back.
        if not path.exists() or (name == "arxiv.db" and path.stat().st_size < 10_000_000):
            print(f"=== WARN  {backend}: no usable {path}, online backend only (if it has one)",
                  file=sys.stderr)
            continue
        args.append(f"{flag}={path}")
        offline.append(backend)
    print(f"=== offline: {', '.join(offline) or 'none'}", file=sys.stderr)
    return args


def run_hallucinator(page: Path, prefix: str, offline: list[str]) -> tuple[list[dict], Path, Path]:
    """Check one file; returns the papers in its JSON plus the report/JSON paths."""
    report, results = Path(f"{prefix}.results.txt"), Path(f"{prefix}.results.json")
    # The CLI writes no JSON when it finds nothing to check, so don't let a
    # previous run's file stand in for this one.
    results.unlink(missing_ok=True)
    # cwd=ROOT so the CLI picks up the repo's .hallucinator.toml and .env.
    subprocess.run(
        [str(CLI), "check", *offline, "--disable-dbs=Semantic Scholar", "--url-match",
         f"--output={report}", f"--json={results}", str(page)],
        cwd=ROOT,
        check=True,
    )
    papers = json.loads(results.read_text(encoding="utf-8")) if results.exists() else []
    return papers, report, results


def same_reference(result: dict, ref: str) -> bool:
    """Whether a free-text result is about `ref`.

    The raw citation keeps the reference's text up to punctuation and spacing;
    skipped entries carry only their title.
    """
    def norm(text: str) -> str:
        return re.sub(r"\W+", "", unicodedata.normalize("NFKC", text)).lower()

    raw, title = result.get("raw_citation") or "", result.get("title") or ""
    if raw:
        return norm(ref)[:40] in norm(raw)
    return bool(title) and norm(title) in norm(ref)


def describe(result: Optional[dict], cited_arxiv: Optional[str]) -> tuple[str, str, list[str]]:
    """(group, verdict, detail lines) for one reference's entry in results.json."""
    if result is None:
        return "unchecked", "NOT CHECKED (no result from hallucinator)", []
    status = result.get("effective_status") or result.get("status")
    if status == "skipped":
        reason = {"short_title": "title under 4 words", "no_title": "no title"}.get(
            result.get("skip_reason"), result.get("skip_reason") or "no URL check")
        return "unchecked", f"SKIPPED ({reason})", []
    if status == "inconclusive":
        # hallucinator's "we could not check": a lookup failed, so not a finding.
        failed = ", ".join(result.get("failed_dbs", []))
        return "unchecked", f"INCONCLUSIVE (not found, but {failed} failed)", []

    source = result.get("source") or "?"
    doi, arxiv = result.get("doi_info") or {}, result.get("arxiv_info") or {}
    # The JSON doesn't carry the mismatch kind, so rebuild it the way the CLI
    # words it. A verified ref with a dead DOI/arXiv ID counts as an issue too.
    issues = []
    if doi and not doi.get("valid"):
        issues.append(f"DOI {doi.get('doi')} doesn't resolve")
    if arxiv and not arxiv.get("valid"):
        issues.append(f"arXiv ID {arxiv.get('arxiv_id')} doesn't resolve")
    # A match in an offline index skips validating the cited arXiv ID (see
    # TODO(#124) in hallucinator-core's pool.rs), so a real title carrying a
    # made-up ID passes. Compare the ID with the record the title matched.
    matched = re.search(r"arxiv\.org/abs/(.+?)(?:v\d+)?$", result.get("paper_url") or "")
    if cited_arxiv and matched and matched[1] != re.sub(r"v\d+$", "", cited_arxiv):
        issues.append(f"cites arXiv:{cited_arxiv}, but the title matches arXiv:{matched[1]}")
    authors_differ = status == "mismatch" and (
        not issues
        or any(db.get("status") == "author_mismatch" for db in result.get("db_results", []))
    )
    if authors_differ:
        issues.insert(0, "authors don't match")

    details = []
    if status == "not_found":
        group, verdict = "review", "NOT FOUND in any database"
        details.extend(issues)
    elif issues:
        group, verdict = "review", f"MISMATCH on {source}: {'; '.join(issues)}"
        if authors_differ:
            details.append(f"cited: {'; '.join(result.get('ref_authors', []))}")
            details.append(f"{source}: {'; '.join(result.get('found_authors', []))}")
    else:
        group, verdict = "verified", f"verified on {source}"
    if result.get("paper_url"):
        details.append(result["paper_url"])
    if (result.get("retraction_info") or {}).get("is_retracted"):
        group, verdict = "review", f"RETRACTED, {verdict}"
    return group, verdict, details


def write_summary(refs: list[str], by_number: dict[int, dict], summary: Path, source: Path) -> str:
    groups = {"review": [], "unchecked": [], "verified": []}
    for n, ref in enumerate(refs, 1):
        cited_arxiv = ARXIV_RE.search(ref)
        group, verdict, details = describe(by_number.get(n), cited_arxiv and cited_arxiv[1])
        groups[group].append("\n     ".join([f"[{n}] {verdict}", ref, *details]))

    sections = [
        ("Needs review", groups["review"], ""),
        ("Not checked", groups["unchecked"],
         "hallucinator skips titles under 4 words as too prone to false matches, and a lookup\n"
         "is inconclusive when a database failed. Check these by hand. (results.txt still lists\n"
         "skipped titles it couldn't match as potential hallucinations; results.json doesn't.)\n"),
        ("Verified", groups["verified"], ""),
    ]
    header = f"{source.name}: {len(refs)} references, " + ", ".join(
        f"{len(entries)} {title.lower()}" for title, entries, _ in sections)

    def render(chosen: list[tuple[str, list[str], str]]) -> str:
        return header + "".join(f"\n\n== {title} ({len(entries)}) ==\n{note}" + "\n".join(entries)
                                for title, entries, note in chosen if entries)

    summary.write_text(render(sections) + "\n", encoding="utf-8")
    # The console gets everything but the verified list, which only lives in the file.
    return render(sections[:2])


def check_file(source: Path, offline: list[str]) -> None:
    print(f"=== CHECK {source}", file=sys.stderr)
    refs = read_references(source)
    if not refs:
        print(f"=== WARN  no references found in {source}", file=sys.stderr)
        return
    parsed = [parse_reference(ref) for ref in refs]
    apa = [n for n, p in enumerate(parsed, 1) if p]
    other = [n for n, p in enumerate(parsed, 1) if p is None]
    stem = source.with_suffix("")
    by_number, written = {}, []

    if apa:
        tei = Path(f"{stem}.tei.xml")
        tei.write_text(to_tei([parsed[n - 1] for n in apa]), encoding="utf-8")
        papers, report, results = run_hallucinator(tei, str(stem), offline)
        # The TEI path numbers entries by position, one per <biblStruct>.
        for r in (r for paper in papers for r in paper["references"]):
            by_number[apa[r["original_number"] - 1]] = r
        written += [tei, report, results]

    if other:
        print(f"=== {len(other)} of {len(refs)} references aren't APA style, so hallucinator's "
              f"own parser reads them: {' '.join(f'[{n}]' for n in other)}", file=sys.stderr)
        page = Path(f"{stem}.freetext.html")
        page.write_text(to_freetext_page([refs[n - 1] for n in other], source.name),
                        encoding="utf-8")
        papers, report, results = run_hallucinator(page, f"{stem}.freetext", offline)
        # hallucinator segments the page itself, so a merged or split entry
        # would shift every number after it. Only take a result whose text is
        # the reference at that position on the page.
        for r in (r for paper in papers for r in paper["references"]):
            k = r["original_number"]
            if 1 <= k <= len(other) and same_reference(r, refs[other[k - 1] - 1]):
                by_number.setdefault(other[k - 1], r)
        written += [page, report, results]

    summary = Path(f"{stem}.summary.txt")
    print("\n" + write_summary(refs, by_number, summary, source))
    print("\nWrote " + "\n      ".join(str(p) for p in [summary, *written] if p.exists()) + "\n")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__.strip().splitlines()[0],
        epilog="See the top of this file for how references are routed and where results go.",
    )
    parser.add_argument("refs", nargs="*", type=Path,
                        help="reference lists to check (default: every *.txt in ./runs)")
    # Results (.summary.txt, .results.txt) share the extension, so leave them out.
    sources = parser.parse_args().refs or sorted(
        path for path in RUNS.glob("*.txt")
        if not path.name.endswith((".results.txt", ".summary.txt"))
    )
    if not sources:
        sys.exit(f"no reference lists to check in {RUNS}")
    sources = [path.resolve() for path in sources]
    if not CLI.is_file():
        sys.exit(f"{CLI} not found. Build it with: cargo build --release -p hallucinator-cli")
    offline = offline_args()

    failed = []
    for source in sources:
        try:
            check_file(source, offline)
        except (OSError, UnicodeDecodeError, subprocess.CalledProcessError) as e:
            print(f"=== FAIL  {source}: {e}", file=sys.stderr)
            failed.append(source)
    if failed:
        sys.exit(f"=== {len(failed)}/{len(sources)} failed")


if __name__ == "__main__":
    main()
