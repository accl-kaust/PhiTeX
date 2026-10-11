#!/bin/sh
# Profiling build of the typing benchmark: engine probes (counters, span
# times) printed per phase, and a Chrome trace in target/trace.json (open in
# https://ui.perfetto.dev). Release builds carry none of this.
# Usage: scripts/profile.sh [typing args...]   e.g. scripts/profile.sh 10000 --edits 5 --no-check
cd "$(dirname "$0")/.." || exit 1
scripts/nc.sh build -q --profile profiling --features profile --example typing || exit 1
exec nice -n 19 ionice -c3 /home/lambda/.local/bin/bw target/profiling/examples/typing --trace target/trace.json "$@"
