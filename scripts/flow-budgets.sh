#!/usr/bin/env bash
# The edit budgets (AGENTS_BRIEF.md "Budgets"), measured: the plain
# documents, big (48 pages) and bigout (about 150 pages through plain's
# output routine, marks and footnotes), at 1 and 8 workers, lean or not;
# each edit kind (a word, an inserted line, a page-break move, a \def every
# paragraph reads, a global parameter) REPS times: median, maximum, steps,
# pieces, the join's time and the longest phase, every budget miss flagged.
#
#   scripts/flow-budgets.sh [REPS] [DOC] [OUT]
#
# Writes OUT (default target/flow-budgets.txt) and prints it. Informational:
# the exit status is 0 whatever is missed. Runs under scripts/sandbox
# (directly when already inside it).
set -euo pipefail
cd "$(dirname "$0")/.."
REPS=${1:-5}
DOC=${2:-}
OUT=${3:-target/flow-budgets.txt}
run() { if [ -n "${PHITEX_SANDBOX:-}" ]; then "$@"; else scripts/sandbox "$@"; fi; }
run cargo build --release -q -p phitex-flow
mkdir -p "$(dirname "$OUT")"
{
	echo "# phitex-flow edit budgets, $(git rev-parse --short HEAD 2>/dev/null || echo '?'), $REPS repetitions"
	run target/release/phitex-flow --budgets "$REPS" ${DOC:+"$DOC"}
} | tee "$OUT"
echo "misses: $(grep -c 'MISS' "$OUT" || true) rows (written to $OUT)"
