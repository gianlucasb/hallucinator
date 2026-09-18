#!/usr/bin/env bash
# Sequentially refresh the offline cached databases.
# Runs small/fast updaters first, then the large downloads, checking disk
# headroom before each big in-place rebuild.
set -uo pipefail
cd "$(dirname "$0")"

CLI=./hallucinator-rs/target/release/hallucinator-cli
LOG=update.log

# Mirror everything to the log as well as the console.
exec > >(tee -a "$LOG") 2>&1

ts() { date -u +%H:%M:%SZ; }
# -P forces single-line (unwrapped) POSIX output, -k gives 1024-byte blocks.
# Plain `df -g` is a BSD/macOS spelling and is rejected by GNU coreutils.
free_gb() { df -Pk . | awk 'NR==2 {print int($4/1048576)}'; }

run() {
  local name="$1"; shift
  echo "=== [$(ts)] START $name :: $* (free $(free_gb)GB)"
  if "$CLI" "$@"; then
    echo "=== [$(ts)] OK    $name (free $(free_gb)GB)"
  else
    # Capture before anything else runs: a command substitution such as
    # $(ts) would overwrite $? with date's exit status.
    local rc=$?
    echo "=== [$(ts)] FAIL  $name (exit $rc)"
  fi
}

require_disk() {
  local need=$1 name=$2
  local have; have=$(free_gb)
  if [[ ! $have =~ ^[0-9]+$ ]]; then
    echo "=== [$(ts)] WARN  $name :: could not determine free space, running anyway"
    return 0
  fi
  if (( have < need )); then
    echo "=== [$(ts)] SKIP  $name :: only ${have}GB free, need ${need}GB"
    return 1
  fi
  return 0
}

echo "######## update run started $(ts) ########"

if [[ ! -x $CLI ]]; then
  echo "=== [$(ts)] ABORT :: $CLI not found or not executable."
  echo "                     Build it first: cargo build --release -p hallucinator-cli"
  exit 1
fi

run iacr update-iacr-eprint ./iacr.db
run acl  update-acl ./acl.db

if require_disk 8 dblp; then
  run dblp update-dblp ./dblp.db
fi

if require_disk 10 arxiv; then
  run arxiv update-arxiv ./arxiv.db
fi

echo "######## update run finished $(ts) ########"
