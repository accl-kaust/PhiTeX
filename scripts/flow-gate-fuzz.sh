#!/usr/bin/env bash
# A ship candidate's fuzz gate, as one cluster job, or sharded into four
# (scripts/flow-gate-submit.sh REV fuzz submits the four, and
# scripts/flow-gate-tally.sh JOB... joins their results):
#
#   ACCL_COMMIT=REV ACCL_OPTS="-c 12 --mem=48G -t 01:00:00 --exclude=harrier" \
#     scripts/accl/accl submit cmd bash scripts/flow-gate-fuzz.sh [SHARD]
#
# 1. the plain corpus: SEEDS (5000) seeds of 6 edits at 1 thread, SEEDS/5
#    at 8 (scripts/flow-fuzz.sh), JOBS (10) at a time, TIMEOUT (120) s a
#    seed; classified (a TRIP timeout tex loops on too is "tex-loops
#    trip"), one of each new class cut down (MIN, 1);
# 2. the LaTeX documents: LATEX_SEEDS (30) of each of B's ten on the saved
#    format, LATEX_JOBS (6) at a time, LATEX_TIMEOUT (180) s a seed.
# SHARD (the argument, or the env) runs one part (the same seeds as the whole): plain-a (1-thread
# seeds 0..SEEDS/2), plain-b (SEEDS/2..SEEDS), plain-j8 (the SEEDS/5 at 8
# threads), latex; unset, all of it in this job.
# Results in $r (the job's results directory; target/flow-gate outside a
# job): fuzz-plain/ (fuzz-plain-a/ ... for a shard), fuzz-latex/ (each
# with by-class/, min/, the repros), summary.txt (the summaries and class
# counts).
# Sized from job 8163 (b8fd8022, one job, -c 12: plain 6000 seeds 27 min
# and their cut-downs 5, LaTeX 300 seeds 13 min; 45 min in all).
set -uo pipefail
cd "$(dirname "$0")/.."
R=${r:-target/flow-gate}
mkdir -p "$R"
SEEDS=${SEEDS:-5000}
SHARD=${1:-${SHARD:-}}
half=$((SEEDS / 2))
plain() { # NAME, then flow-fuzz.sh's env
	local name=$1
	shift
	env JOBS="${JOBS:-10}" TIMEOUT="${TIMEOUT:-120}" MIN="${MIN:-1}" MIN_LIMIT="${MIN_LIMIT:-300}" "$@" \
		scripts/flow-fuzz.sh "$SEEDS" "$R/fuzz-$name" >"$R/fuzz-$name.txt" 2>&1
}
latex() {
	LATEX=1 JOBS=${LATEX_JOBS:-6} TIMEOUT=${LATEX_TIMEOUT:-180} MIN=0 \
		scripts/flow-fuzz.sh "${LATEX_SEEDS:-30}" "$R/fuzz-latex" >"$R/fuzz-latex.txt" 2>&1
}
case "$SHARD" in
'') plain plain; latex; parts="plain latex" ;;
plain-a) plain plain-a FROM=0 TO=$half J8=0; parts=plain-a ;;
plain-b) plain plain-b FROM=$half TO=$SEEDS J8=0; parts=plain-b ;;
plain-j8) plain plain-j8 FROM=0 TO=0; parts=plain-j8 ;;
latex) latex; parts=latex ;;
*) echo "SHARD: plain-a, plain-b, plain-j8 or latex, not $SHARD" >&2; exit 2 ;;
esac
{
	echo "# fuzz gate${SHARD:+ shard $SHARD} on $(scripts/flow-gate-rev.sh)"
	for k in $parts; do
		echo "== $k"
		cat "$R/fuzz-$k/summary.txt" 2>/dev/null || echo "(no summary: see fuzz-$k.txt)"
		echo "-- classes"
		cat "$R/fuzz-$k/classes-count.txt" 2>/dev/null
	done
} | tee "$R/summary.txt"
