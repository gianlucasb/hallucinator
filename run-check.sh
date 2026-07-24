#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

usage() {
  echo "Usage: $0 [mac|linux] [pdf]" >&2
  exit 1
}

PLATFORM="mac"
if [[ $# -gt 0 ]]; then
  case "$1" in
    mac|linux) PLATFORM="$1"; shift ;;
    -h|--help) usage ;;
  esac
fi

case "$PLATFORM" in
  mac)   BASE_DIR="/Users/emidec/ownCloud/hallucinator" ;;
  linux) BASE_DIR="/home/edecrist/ownCloud/hallucinator" ;;
  *)     usage ;;
esac

PDF="${1:-$BASE_DIR/main.pdf}"

./hallucinator-rs/target/release/hallucinator-cli check \
  --dblp-offline=./dblp.db \
  --acl-offline=./acl.db \
  --arxiv-offline=./arxiv.db \
  --iacr-eprint-offline=./iacr.db \
  --url-match \
  --output="$BASE_DIR/results.txt" \
  --json="$BASE_DIR/results.json" \
  "$PDF"
