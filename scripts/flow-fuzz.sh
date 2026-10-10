#!/usr/bin/env bash
# The incremental fuzz gate at scale (crates/phitex-flow/src/fuzz.rs).
#
#   scripts/flow-fuzz.sh [SEEDS] [OUT]
#
# SEEDS seeds (default 5000) of 6 edits each at 1 thread, then SEEDS/5 more
# seeds (other seeds) at 8 threads (random schedules: waiting or predicted
# reads, priority or random order), in chunks of 100 seeds run JOBS at a
# time (default: the number of processors). Then:
#   OUT/summary.txt  passed / failed / over-invalidated by document;
#   OUT/classes.txt  each failure's class (phitex-flow --fuzz-classify):
#                    F1, F2?, F6 are open findings (CORE.md "Fuzz
#                    findings"), the other classes are new or unsorted;
#   OUT/min/         one reproduction per new class, cut down by
#                    scripts/flow-fuzz-min.py (MIN=0 skips it).
# Everything runs under scripts/sandbox (directly when already inside it).
# Env: JOBS, EDITS (6), TIMEOUT (seconds a seed may take, 600), MIN (1).
set -euo pipefail
cd "$(dirname "$0")/.."
SEEDS=${1:-5000}
OUT=${2:-target/flow-fuzz}
JOBS=${JOBS:-$(nproc)}
EDITS=${EDITS:-6}
TIMEOUT=${TIMEOUT:-600}
MIN=${MIN:-1}
run() { if [ -n "${PARTEX_SANDBOX:-}" ]; then "$@"; else scripts/sandbox "$@"; fi; }
BIN=target/release/phitex-flow
run cargo build --release -q -p phitex-flow
mkdir -p "$OUT"
J8=$((SEEDS / 5))
# (one line per chunk: threads, first seed, count)
{
	for ((s = 0; s < SEEDS; s += 100)); do echo "1 $s $((SEEDS - s < 100 ? SEEDS - s : 100))"; done
	for ((s = 0; s < J8; s += 100)); do echo "8 $((SEEDS + s)) $((J8 - s < 100 ? J8 - s : 100))"; done
} >"$OUT/chunks.txt"
export BIN OUT EDITS TIMEOUT
chunk() {
	local t=$1 s=$2 n=$3
	local dir="$OUT/j$t"
	mkdir -p "$dir"
	"$BIN" --fuzz "$s" "$EDITS" "$n" --threads "$t" --timeout "$TIMEOUT" --out "$dir" >"$dir/chunk-$s.txt" 2>&1 || true
}
export -f chunk
if [ -n "${PARTEX_SANDBOX:-}" ]; then
	xargs -P "$JOBS" -L 1 bash -c 'chunk "$@"' _ <"$OUT/chunks.txt"
else
	scripts/sandbox env BIN="$BIN" OUT="$OUT" EDITS="$EDITS" TIMEOUT="$TIMEOUT" bash -c "$(declare -f chunk); export -f chunk; xargs -P $JOBS -L 1 bash -c 'chunk \"\$@\"' _ <'$OUT/chunks.txt'"
fi
# (the summary: each failure line names its document)
{
	for t in 1 8; do
		[ -d "$OUT/j$t" ] || continue
		echo "== $t thread(s)"
		cat "$OUT/j$t"/chunk-*.txt | awk '
			/^fuzz: / { split($0, a, " "); seeds += a[2]; next }
			/^seed / { d = $3; sub(/^\(/, "", d); sub(/,$/, "", d); if ($0 ~ /Over \{/) over[d]++; else fail[d]++; bad++ ; docs[d] = 1 }
			END {
				printf "%d seeds: %d passed, %d failed or over-invalidated\n", seeds, seeds - bad, bad
				for (d in docs) printf "  %-8s failed %d, over-invalidated %d\n", d, fail[d], over[d]
			}'
	done
} | tee "$OUT/summary.txt"
# (the classes, and one cut-down reproduction per new class)
find "$OUT"/j1 "$OUT"/j8 -name '*.repro' 2>/dev/null | sort >"$OUT/repros.txt"
: >"$OUT/classes.txt"
if [ -s "$OUT/repros.txt" ]; then
	run bash -c "xargs -P $JOBS -n 4 $BIN --fuzz-classify <'$OUT/repros.txt'" | sort >"$OUT/classes.txt"
fi
echo "== classes"
cut -f1 "$OUT/classes.txt" | sort | uniq -c | sort -rn | tee "$OUT/classes-count.txt"
if [ "$MIN" = 1 ]; then
	mkdir -p "$OUT/min"
	cut -f1 "$OUT/classes.txt" | sort -u | grep -v -e '^F1 ' -e '^F6 ' -e '^F2? ' -e '^passes' | while IFS= read -r c; do
		f=$(C="$c" awk -F'\t' '$1 == ENVIRON["C"] { print $2; exit }' "$OUT/classes.txt")
		name=$(basename "$f" .repro)
		[ -n "$f" ] || continue
		echo "== new class: $c ($f)"
		run python3 scripts/flow-fuzz-min.py "$BIN" "$f" "$OUT/min/$name.repro" | tail -1
	done
fi
