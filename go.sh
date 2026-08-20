#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

usage() {
  cat >&2 <<EOF
Usage: $0 [pdf ...]

  (no args)  check every *.pdf in ./runs (default)
  pdf ...    check just these files instead

Per-input results land next to the input as <name>.results.txt / <name>.results.json.
EOF
  exit 1
}

case "${1:-}" in
  -h|--help) usage ;;
esac

# ./runs works on any host and is what sync-runs.sh keeps in step with pdd.
BASE_DIR="./runs"
mkdir -p "$BASE_DIR"

PDFS=("$@")
if [[ ${#PDFS[@]} -eq 0 ]]; then
  # nullglob so an empty runs/ yields no args rather than a literal '*.pdf'
  shopt -s nullglob
  PDFS=("$BASE_DIR"/*.pdf)
  shopt -u nullglob
fi

if [[ ${#PDFS[@]} -eq 0 ]]; then
  echo "no PDFs to check in $BASE_DIR" >&2
  exit 1
fi

ts() { date -u +%H:%M:%SZ; }

# Offline backends. Each of these replaces its online counterpart when the
# local index is present (IACR has no online counterpart at all), so a
# missing path must not be passed: the CLI hard-errors instead of falling
# back. Guard each one and say out loud when we drop to online.
OFFLINE_ARGS=()
offline() {  # offline <flag> <path> <label> [fallback-note]
  if [[ -e "$2" ]]; then
    OFFLINE_ARGS+=("$1=$2")
  else
    echo "=== WARN  $3: $2 missing, ${4:-using online backend}" >&2
  fi
}

offline --dblp-offline        ./dblp.db        DBLP
offline --acl-offline         ./acl.db         "ACL Anthology"
offline --iacr-eprint-offline ./iacr.db        "IACR ePrint"
# OpenAlex online only registers when an API key is configured, so with no
# local index and no key the backend is skipped outright rather than degraded.
offline --openalex-offline ./openalex-index OpenAlex \
        "and online OpenAlex needs --openalex-key -- backend skipped"

# arXiv needs one extra check: `update-arxiv` creates the schema before it
# ingests, so an interrupted build leaves a valid-but-empty database. That
# still satisfies -e and would silence arXiv checking altogether rather than
# fall back online. A real Kaggle ingest is multi-GB; anything tiny is a stub.
# (wc -c rather than stat, whose size flag differs between GNU and BSD)
if [[ -e ./arxiv.db && $(wc -c < ./arxiv.db) -lt 10000000 ]]; then
  echo "=== WARN  arXiv: ./arxiv.db is an empty stub, using online backend" >&2
  echo "===       rebuild with: hallucinator-cli update-arxiv ./arxiv.db" >&2
else
  offline --arxiv-offline ./arxiv.db arXiv
fi

failed=()
for pdf in "${PDFS[@]}"; do
  stem="${pdf##*/}"      # strip directory
  stem="${stem%.*}"      # strip extension
  out_dir="$(dirname "$pdf")"

  echo "=== [$(ts)] CHECK $pdf"
  # capture the exit code explicitly: $? inside an if/else reports the
  # condition's status, not the command's
  rc=0
  ./hallucinator-rs/target/release/hallucinator-cli check \
      "${OFFLINE_ARGS[@]}" \
      --disable-dbs="Semantic Scholar" \
      --url-match \
      --disable-dbs="Semantic Scholar" \
      --output="$out_dir/$stem.results.txt" \
      --json="$out_dir/$stem.results.json" \
      "$pdf" || rc=$?

  if [[ $rc -eq 0 ]]; then
    echo "=== [$(ts)] OK    $pdf"
  else
    echo "=== [$(ts)] FAIL  $pdf (exit $rc)"
    failed+=("$pdf")
  fi
done

if [[ ${#failed[@]} -gt 0 ]]; then
  echo "=== ${#failed[@]}/${#PDFS[@]} failed: ${failed[*]}" >&2
  exit 1
fi

echo "=== [$(ts)] all ${#PDFS[@]} checked"
