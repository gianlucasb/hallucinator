#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

PDF="${1:-main.pdf}"

./hallucinator-rs/target/release/hallucinator-cli check \
  --dblp-offline=./dblp.db \
  --acl-offline=./acl.db \
  --arxiv-offline=./arxiv.db \
  --iacr-eprint-offline=./iacr.db \
  --url-match \
  --output=results.txt \
  --json=results.json \
  "$PDF"
