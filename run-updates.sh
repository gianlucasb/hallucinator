#!/usr/bin/env bash
# Sequentially refresh the offline cached databases.
# Runs small/fast updaters first, then the large downloads, checking disk
# headroom before each big in-place rebuild.
set -uo pipefail
cd "$(dirname "$0")"

CLI=./hallucinator-rs/target/release/hallucinator-cli
LOG=update.log

ts() { date -u +%H:%M:%SZ; }
free_gb() { df -g . | tail -1 | awk '{print $4}'; }

run() {
  local name="$1"; shift
  echo "=== [$(ts)] START $name :: $* (free $(free_gb)GB)"
  if "$CLI" "$@"; then
    echo "=== [$(ts)] OK    $name (free $(free_gb)GB)"
  else
    echo "=== [$(ts)] FAIL  $name (exit $?)"
  fi
}

require_disk() {
  local need=$1 name=$2
  local have; have=$(free_gb)
  if (( have < need )); then
    echo "=== [$(ts)] SKIP  $name :: only ${have}GB free, need ${need}GB"
    return 1
  fi
  return 0
}

echo "######## update run started $(ts) ########"

run iacr update-iacr-eprint ./iacr.db
run acl  update-acl ./acl.db

if require_disk 8 dblp; then
  run dblp update-dblp ./dblp.db
fi

if require_disk 10 arxiv; then
  run arxiv update-arxiv ./arxiv.db
fi

echo "######## update run finished $(ts) ########"
