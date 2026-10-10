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
# LATEX=1: the LaTeX documents instead (fuzz::LATEX_DOCS but tiny, on the
# saved format, made once into OUT/latex.fmt): SEEDS seeds (default 30)
# of each document, one chunk a document, at 1 or 8 threads at random
# (each chunk takes 8 processors: JOBS defaults to nproc/8); MIN
# defaults to 0. MIN=timeouts cuts down one of each timeout class only.
# A TRIP timeout on which the system's tex loops too is classed
# "tex-loops trip" (scripts/flow-fuzz-tex.py), not ours.
# DOCS="trip pages ...": the plain corpus's documents named only, each by
# `--doc` (its own seeds: seed N is not the same case as without DOCS),
# in chunks of 50 seeds, SEEDS of each at 1 thread and SEEDS/5 at 8.
# In a cluster job (job.sbatch's `cmd`, which exports the results
# directory $r into the container) OUT is copied at the end to
# $r/$(basename OUT) (but the format), which the job copies to
# ~/code/flinner/partex-phitex-runs/cmd-JOBID/; else under Slurm to
# $HOME/code/flinner/partex-phitex-runs/cmd-$SLURM_JOB_ID/$(basename OUT),
# where the runner fetches a job's results from (the run itself writes
# inside the checkout, where the sandbox may write).
set -euo pipefail
cd "$(dirname "$0")/.."
LATEX=${LATEX:-0}
if [ "$LATEX" = 1 ]; then
	SEEDS=${1:-30}
	JOBS=${JOBS:-$(($(nproc) / 8 > 0 ? $(nproc) / 8 : 1))}
	MIN=${MIN:-0}
	LATEX_DOCS="sections math tables figures titlepage packages ams gfx layout rules"
else
	SEEDS=${1:-5000}
	JOBS=${JOBS:-$(nproc)}
	MIN=${MIN:-1}
fi
OUT=${2:-target/flow-fuzz}
EDITS=${EDITS:-6}
TIMEOUT=${TIMEOUT:-600}
if [ -n "${r:-}" ]; then
	KEEP="$r/$(basename "$OUT")"
elif [ -n "${SLURM_JOB_ID:-}" ]; then
	KEEP="$HOME/code/flinner/partex-phitex-runs/cmd-$SLURM_JOB_ID/$(basename "$OUT")"
else
	KEEP=
fi
run() { if [ -n "${PARTEX_SANDBOX:-}" ]; then "$@"; else scripts/sandbox "$@"; fi; }
BIN=target/release/phitex-flow
run cargo build --release -q -p phitex-flow
mkdir -p "$OUT"
J8=$((SEEDS / 5))
# (one line per chunk: threads, first seed, count[, LaTeX document])
if [ "$LATEX" = 1 ]; then
	# (the format, made once: every chunk reads it)
	export PHITEX_FUZZ_LATEX_FMT="$(cd "$OUT" && pwd)/latex.fmt"
	run "$BIN" --fuzz 1 1 1 --latex --doc latex-tiny --out "$OUT/tiny" >"$OUT/format.txt" 2>&1 || true
	for d in $LATEX_DOCS; do echo "1,8 1 $SEEDS latex-$d"; done >"$OUT/chunks.txt"
elif [ -n "${DOCS:-}" ]; then
	for d in $DOCS; do
		for ((s = 0; s < SEEDS; s += 50)); do echo "1 $s $((SEEDS - s < 50 ? SEEDS - s : 50)) $d"; done
		for ((s = 0; s < J8; s += 50)); do echo "8 $((SEEDS + s)) $((J8 - s < 50 ? J8 - s : 50)) $d"; done
	done >"$OUT/chunks.txt"
else
	{
		for ((s = 0; s < SEEDS; s += 100)); do echo "1 $s $((SEEDS - s < 100 ? SEEDS - s : 100))"; done
		for ((s = 0; s < J8; s += 100)); do echo "8 $((SEEDS + s)) $((J8 - s < 100 ? J8 - s : 100))"; done
	} >"$OUT/chunks.txt"
fi
export BIN OUT EDITS TIMEOUT
chunk() {
	local t=$1 s=$2 n=$3 doc=${4:-}
	if [ -n "$doc" ] && [ "${doc#latex-}" = "$doc" ]; then
		# (a plain document by name)
		local dir="$OUT/j$t"
		mkdir -p "$dir"
		fuzz_from "$s" "$n" "$dir/chunk-$doc-$s.txt" --doc "$doc" --threads "$t" --out "$dir"
		return
	fi
	if [ -n "$doc" ]; then
		local dir="$OUT/latex"
		mkdir -p "$dir"
		fuzz_from "$s" "$n" "$dir/chunk-$doc.txt" --latex --doc "$doc" --threads "$t" --out "$dir"
		return
	fi
	local dir="$OUT/j$t"
	mkdir -p "$dir"
	fuzz_from "$s" "$n" "$dir/chunk-$s.txt" --threads "$t" --out "$dir"
}
# (seeds S.. N of them, resumed after a seed that timed out: the run
# stops there, its thread left behind; OPTS after the log file)
fuzz_from() {
	local s=$1 n=$2 log=$3
	shift 3
	local end=$((s + n)) st=0
	: >"$log"
	while [ "$s" -lt "$end" ]; do
		st=0
		"$BIN" --fuzz "$s" "$EDITS" "$((end - s))" --timeout "$TIMEOUT" "$@" >>"$log" 2>&1 || st=$?
		[ "$st" = 3 ] || break
		s=$(sed -n 's/^fuzz: stopped after a timeout; resume at seed //p' "$log" | tail -1)
		[ -n "$s" ] || break
	done
}
export -f chunk fuzz_from
if [ -n "${PARTEX_SANDBOX:-}" ]; then
	xargs -P "$JOBS" -L 1 bash -c 'chunk "$@"' _ <"$OUT/chunks.txt"
else
	scripts/sandbox env BIN="$BIN" OUT="$OUT" EDITS="$EDITS" TIMEOUT="$TIMEOUT" bash -c "$(declare -f chunk fuzz_from); export -f chunk fuzz_from; xargs -P $JOBS -L 1 bash -c 'chunk \"\$@\"' _ <'$OUT/chunks.txt'"
fi
# (the summary: each failure line names its document)
{
	for t in 1 8 latex; do
		dir="$OUT/j$t"
		[ "$t" = latex ] && dir="$OUT/latex"
		[ -d "$dir" ] || continue
		echo "== $t"
		cat "$dir"/chunk-*.txt | awk '
			/^fuzz: / { split($0, a, " "); seeds += a[2]; next }
			/^seed / { d = $3; sub(/^\(/, "", d); sub(/,$/, "", d); if ($0 ~ /Over \{/) over[d]++; else fail[d]++; bad++ ; docs[d] = 1 }
			END {
				printf "%d seeds: %d passed, %d failed or over-invalidated\n", seeds, seeds - bad, bad
				for (d in docs) printf "  %-8s failed %d, over-invalidated %d\n", d, fail[d], over[d]
			}'
	done
} | tee "$OUT/summary.txt"
# (the classes, and one cut-down reproduction per new class)
# (some of the directories may not be there: find's status is not the run's)
{ find "$OUT"/j1 "$OUT"/j8 "$OUT"/latex -name '*.repro' 2>/dev/null || true; } | sort >"$OUT/repros.txt"
: >"$OUT/classes.txt"
if [ -s "$OUT/repros.txt" ]; then
	run bash -c "xargs -P $JOBS -n 4 $BIN --fuzz-classify <'$OUT/repros.txt'" | sort >"$OUT/classes.txt" || true
fi
# (a TRIP timeout where the system's tex loops too, on either text, is
# TRIP edited into a loop, not ours: classed "tex-loops trip")
if grep -q '^timeout trip' "$OUT/classes.txt" && command -v tex >/dev/null; then
	mkdir -p "$OUT/tmp"
	grep '^timeout trip' "$OUT/classes.txt" | cut -f2 |
		TMPDIR="$OUT/tmp" run bash -c "xargs -P $JOBS -n 4 python3 scripts/flow-fuzz-tex.py bench/trip" >"$OUT/trip-tex.txt" || true
	awk -F'\t' 'NR == FNR { if ($0 ~ /loops/) loop[$1] = 1; next }
		$1 ~ /^timeout trip/ && ($2 in loop) { $1 = "tex-loops trip" } { print $1 "\t" $2 }' \
		"$OUT/trip-tex.txt" "$OUT/classes.txt" | sort >"$OUT/classes.txt.new" && mv "$OUT/classes.txt.new" "$OUT/classes.txt"
	rm -rf "$OUT/tmp"
fi
echo "== classes"
cut -f1 "$OUT/classes.txt" | sort | uniq -c | sort -rn | tee "$OUT/classes-count.txt"
# (each reproduction under its class: OUT/by-class/CLASS/, the class's
# words joined by '-', what follows " | " left out)
while IFS=$'\t' read -r c f; do
	d="$OUT/by-class/$(printf '%s' "${c%% | *}" | tr -c 'A-Za-z0-9?.\n' '-' | tr -s '-')"
	mkdir -p "$d"
	cp "$f" "$d/"
done <"$OUT/classes.txt"
# (MIN=1: one of each new class cut down; MIN=timeouts: of the timeout
# classes only, while the last edit alone hangs)
if [ "$MIN" = 1 ] || [ "$MIN" = timeouts ]; then
	mkdir -p "$OUT/min"
	only='.'
	[ "$MIN" = timeouts ] && only='^timeout '
	cut -f1 "$OUT/classes.txt" | sort -u | grep -e "$only" | grep -v -e '^F1 ' -e '^F6 ' -e '^F2? ' -e '^passes' | while IFS= read -r c; do
		f=$(C="$c" awk -F'\t' '$1 == ENVIRON["C"] { print $2; exit }' "$OUT/classes.txt")
		name=$(basename "$f" .repro)
		[ -n "$f" ] || continue
		echo "== new class: $c ($f)"
		run python3 scripts/flow-fuzz-min.py "$BIN" "$f" "$OUT/min/$name.repro" | tail -1 || true
	done
fi
# (the summary and the classes again, at the end of the output; and OUT
# where the runner fetches it from)
echo "== summary"
cat "$OUT/summary.txt"
echo "== classes"
cat "$OUT/classes-count.txt"
if [ -d "$OUT/min" ]; then
	echo "== cut-down reproductions"
	ls -1 "$OUT/min"/*.repro 2>/dev/null || echo "(none)"
fi
# (OUT may be inside the results already: nothing to copy then)
if [ -n "$KEEP" ] && [ "$(mkdir -p "$KEEP" && cd "$KEEP" && pwd)" != "$(cd "$OUT" && pwd)" ]; then
	cp -r "$OUT"/. "$KEEP"/
	rm -f "$KEEP/latex.fmt"
	echo "copied $OUT to $KEEP"
fi
