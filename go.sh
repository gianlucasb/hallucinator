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
# local index is present, so a missing path must not be passed: the CLI
# hard-errors instead of falling back. Guard each one and say out loud where
# it landed -- nothing downstream reveals it, because an offline backend
# reports the same name as its online twin (results.json says "arXiv" either
# way).
#
# Three outcomes, not two. "skipped" is the one worth noticing: that backend
# contributes nothing to the run at all.
OFFLINE_ARGS=()
using_offline=""   # comma-joined labels, for the summary lines below
using_online=""
using_skipped=""

# du -sh copes with both the SQLite files and the OpenAlex index directory.
# Printing the size catches a truncated index that -e alone would accept.
size_of() { du -sh "$1" 2>/dev/null | cut -f1; }

# Plain strings rather than arrays: multi-word labels survive, and an empty
# one expands cleanly under `set -u` on any bash.
note_offline() { using_offline+="${using_offline:+, }$1"; }
note_online()  { using_online+="${using_online:+, }$1"; }
note_skipped() { using_skipped+="${using_skipped:+, }$1"; }

offline() {  # offline <flag> <path> <label> <online|skipped> [why-skipped]
  if [[ -e "$2" ]]; then
    OFFLINE_ARGS+=("$1=$2")
    note_offline "$3"
    echo "=== OK    $3: offline, $2 ($(size_of "$2"))"
  elif [[ $4 == online ]]; then
    note_online "$3"
    echo "=== WARN  $3: $2 missing, using online backend" >&2
  else
    note_skipped "$3"
    echo "=== WARN  $3: $2 missing, ${5:-backend skipped}" >&2
  fi
}

offline --dblp-offline ./dblp.db        DBLP            online
offline --acl-offline  ./acl.db         "ACL Anthology" online
# IACR is offline-only: the ePrint archive has no public search API, so
# without a local index this backend never registers. Saying "using online
# backend" here would be a lie.
offline --iacr-eprint-offline ./iacr.db "IACR ePrint"   skipped \
        "IACR has no search API -- backend skipped"

# OpenAlex is the one backend whose online mode needs a key. The CLI resolves
# it from --openalex-key, then $OPENALEX_KEY, then .env (it calls dotenvy at
# startup), then the config file -- so no key belongs in this script. We can
# check the first three; a key set only in the config file reads as absent
# here, which makes the warning a false alarm rather than a missed one.
have_openalex_key() {
  if [[ -n ${OPENALEX_KEY:-} ]]; then
    return 0
  fi
  if [[ -f .env ]] && grep -qE '^[[:space:]]*OPENALEX_KEY=.' .env; then
    return 0
  fi
  return 1
}

if [[ -e ./openalex-index ]]; then
  offline --openalex-offline ./openalex-index OpenAlex online
elif have_openalex_key; then
  note_online OpenAlex
  echo "=== OK    OpenAlex: online, no local index (key found)"
else
  note_skipped OpenAlex
  echo "=== WARN  OpenAlex: no local index and no key -- backend skipped" >&2
  echo "===       free key at https://openalex.org/settings/api, then" >&2
  echo "===       export OPENALEX_KEY=... (or put it in .env)" >&2
fi

# arXiv needs one extra check: `update-arxiv` creates the schema before it
# ingests, so an interrupted build leaves a valid-but-empty database. That
# still satisfies -e and would silence arXiv checking altogether rather than
# fall back online. A real Kaggle ingest is multi-GB; anything tiny is a stub.
# (wc -c rather than stat, whose size flag differs between GNU and BSD)
if [[ -e ./arxiv.db && $(wc -c < ./arxiv.db) -lt 10000000 ]]; then
  echo "=== WARN  arXiv: ./arxiv.db is an empty stub, using online backend" >&2
  echo "===       rebuild with: hallucinator-cli update-arxiv ./arxiv.db" >&2
  note_online arXiv
else
  offline --arxiv-offline ./arxiv.db arXiv online
fi

# The lines to grep in a log when you want to know what a run actually used.
echo "=== [$(ts)] offline: ${using_offline:-none}"
echo "=== [$(ts)] online : ${using_online:-none}"
echo "=== [$(ts)] skipped: ${using_skipped:-none}"

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
