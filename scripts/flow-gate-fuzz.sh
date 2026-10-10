#!/usr/bin/env bash
# A ship candidate's fuzz gate, as one cluster job:
#
#   ACCL_COMMIT=REV ACCL_OPTS="-c 12 --mem=48G -t 03:00:00 --exclude=harrier" \
#     scripts/accl/accl submit cmd bash scripts/flow-gate-fuzz.sh
#
# 1. the plain corpus: SEEDS (5000) seeds of 6 edits at 1 thread, SEEDS/5
#    at 8 (scripts/flow-fuzz.sh), JOBS (10) at a time, TIMEOUT (120) s a
#    seed; classified (a TRIP timeout tex loops on too is "tex-loops
#    trip"), one of each new class cut down (MIN, 1);
# 2. the LaTeX documents: LATEX_SEEDS (30) of each of B's ten on the saved
#    format, LATEX_JOBS (6) at a time, LATEX_TIMEOUT (180) s a seed.
# Results in $r (the job's results directory; target/flow-gate outside a
# job): fuzz-plain/, fuzz-latex/ (each with by-class/, min/, the
# repros), summary.txt (both summaries and class counts).
# Sized from job 7899 (5000 plain seeds at JOBS=10: 7.9 cores, 13 GB, 38
# min) and 7878 (LaTeX at JOBS=6: 45 GB at its peak, before S's fixes).
set -uo pipefail
cd "$(dirname "$0")/.."
R=${r:-target/flow-gate}
mkdir -p "$R"
JOBS=${JOBS:-10} TIMEOUT=${TIMEOUT:-120} MIN=${MIN:-1} \
	scripts/flow-fuzz.sh "${SEEDS:-5000}" "$R/fuzz-plain" >"$R/fuzz-plain.txt" 2>&1
LATEX=1 JOBS=${LATEX_JOBS:-6} TIMEOUT=${LATEX_TIMEOUT:-180} MIN=0 \
	scripts/flow-fuzz.sh "${LATEX_SEEDS:-30}" "$R/fuzz-latex" >"$R/fuzz-latex.txt" 2>&1
{
	echo "# fuzz gate on $(git rev-parse --short HEAD 2>/dev/null || echo '?')"
	for k in plain latex; do
		echo "== $k"
		cat "$R/fuzz-$k/summary.txt" 2>/dev/null || echo "(no summary: see fuzz-$k.txt)"
		echo "-- classes"
		cat "$R/fuzz-$k/classes-count.txt" 2>/dev/null
	done
} | tee "$R/summary.txt"
