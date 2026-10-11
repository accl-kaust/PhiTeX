#!/usr/bin/env bash
# The quick gate, on every branch head as it lands (about 10 minutes at
# -c 12; scripts/flow-gate-submit.sh REV quick):
#
#   ACCL_COMMIT=REV ACCL_OPTS="-c 12 --mem=48G -t 00:30:00 --exclude=harrier" \
#     scripts/accl/accl submit cmd bash scripts/flow-gate-quick.sh
#
# At once, after one release build:
# - the plain corpus: SEEDS (1000) seeds at 1 thread and SEEDS/5 at 8
#   (scripts/flow-fuzz.sh), JOBS (7) at a time, TIMEOUT (60) s a seed,
#   classified, nothing cut down;
# - the LaTeX documents: LATEX_SEEDS (10) of each of B's ten (100),
#   LATEX_JOBS (3) at a time, LATEX_TIMEOUT (120) s a seed;
# - clippy (-D warnings) on phitex-flow, all targets, then
#   tests/flow.rs.
# Results in $r (target/flow-gate-quick outside a job): fuzz-plain/,
# fuzz-latex/, clippy.txt, flow.txt, summary.txt.
set -uo pipefail
cd "$(dirname "$0")/.."
R=${r:-target/flow-gate-quick}
mkdir -p "$R"
t0=$(date +%s)
cargo build --release -q -p phitex-flow 2>&1 | tail -3
JOBS=${JOBS:-7} TIMEOUT=${TIMEOUT:-60} MIN=0 \
	scripts/flow-fuzz.sh "${SEEDS:-1000}" "$R/fuzz-plain" >"$R/fuzz-plain.txt" 2>&1 &
p=$!
LATEX=1 JOBS=${LATEX_JOBS:-3} TIMEOUT=${LATEX_TIMEOUT:-120} MIN=0 \
	scripts/flow-fuzz.sh "${LATEX_SEEDS:-10}" "$R/fuzz-latex" >"$R/fuzz-latex.txt" 2>&1 &
l=$!
c=0
cargo clippy -q -p phitex-flow --all-targets -- -D warnings >"$R/clippy.txt" 2>&1 || c=$?
f=0
cargo test --release -p phitex-flow --test flow >"$R/flow.txt" 2>&1 || f=$?
wait $p $l
{
	echo "# quick gate on $(scripts/flow-gate-rev.sh) ($((($(date +%s) - t0) / 60)) min)"
	for k in plain latex; do
		echo "== $k"
		cat "$R/fuzz-$k/summary.txt" 2>/dev/null || echo "(no summary: see fuzz-$k.txt)"
		echo "-- classes"
		cat "$R/fuzz-$k/classes-count.txt" 2>/dev/null
	done
	echo "== clippy -D warnings (exit $c)"
	grep -E '^(error|warning)' "$R/clippy.txt" | sort | uniq -c | head -20
	echo "== tests/flow.rs (exit $f)"
	grep -E '^test result|FAILED|panicked at' "$R/flow.txt" | head -20
} | tee "$R/summary.txt"
