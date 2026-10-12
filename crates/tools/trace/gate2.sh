#!/usr/bin/env bash
# Gate 2: the replay client (crates/vendor/defuse/examples/replay.rs) over
# the format v1 corpus that replay.sh writes (target/corpus/replay/<doc>).
# Per document, smallest first, one run at a time, all at --threads 1:
# cold (no edits), then the 200 edits with --check-every 50 at speculation
# off and predicted (each again without checks when a check fails, for
# the numbers). A failure is recorded and the next run goes on. Logs to
# target/corpus/gate2/<doc>.<run>.log; each run is capped at
# PHITEX_GATE_TIMEOUT seconds (default 7200). Run through the sandbox:
#
#   scripts/sandbox crates/tools/trace/gate2.sh [DOC...]
set -uo pipefail
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
in="$repo/target/corpus/replay"
out="$repo/target/corpus/gate2"
mkdir -p "$out"
cargo build -q --release -p defuse --example replay || exit 1
bin="$repo/target/release/examples/replay"
cap="${PHITEX_GATE_TIMEOUT:-7200}"
want=" $* "
run() { # name tag args...
  local name=$1 tag=$2
  shift 2
  echo "$name $tag" >&2
  timeout "$cap" "$bin" "$@" >"$out/$name.$tag.log" 2>&1
  echo "exit $?" >>"$out/$name.$tag.log"
}
for d in $(ls -S -r "$in"/*/trace.jsonl 2>/dev/null); do
  name=$(basename "$(dirname "$d")")
  [ $# -eq 0 ] || [[ $want == *" $name "* ]] || continue
  e="$in/$name/edits.jsonl"
  run "$name" cold1 "$d" /dev/null --threads 1
  for spec in off predicted; do
    run "$name" "$spec" "$d" "$e" --threads 1 --spec "$spec" --check-every 50
    if ! grep -q "^exit 0" "$out/$name.$spec.log"; then
      run "$name" "$spec-nocheck" "$d" "$e" --threads 1 --spec "$spec"
    fi
  done
done
