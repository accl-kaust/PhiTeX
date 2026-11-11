#!/bin/bash
# (temporary) the gate's manuals shards (gate-shard.sh manuals K N) for SHARDS (e.g. "0 1") of N=8,
# on this commit's release CLI, in the CI image (ACCL_IMAGE=phitex-ci-2026-09-25): red lines and
# each manual's verdict.
set -u
mkdir -p $w/bin/cli/rel
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
cp target/release/phitex $w/bin/cli/rel/phitex
r0=$r
for k in ${SHARDS:-0}; do
  export r=$r0/m$k; mkdir -p $r
  GATE_MANUALS_PAR=${PAR:-1:8,4:2} bash scripts/accl/gate-shard.sh manuals $k 8 > $r/shard.txt 2>&1
  echo "== shard $k: exit $?"; cat $r/red.txt 2>/dev/null | cut -c1-300
  cat $r/known-red.txt 2>/dev/null | cut -c1-200 | sed 's/^/  known: /'
done
