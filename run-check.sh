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
      --dblp-offline=./dblp.db \
      --acl-offline=./acl.db \
      --arxiv-offline=./arxiv.db \
      --iacr-eprint-offline=./iacr.db \
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
