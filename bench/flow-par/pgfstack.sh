#!/bin/bash
# bench/flow-par/pgfstack.sh: where a long run of the pgf manual's build is,
# by stacks sampled under gdb (gdb starts the build, so ptrace_scope 1
# allows it): from START s, every STEP s, N times, the build is sent SIGINT
# (gdb stops it, not passed on) and every thread's backtrace written.
# DEBUG=2: full debug information (sdump.py reads the node's fields).
# The format is made first into a cache of the job's own (pgfrounds.sh).
set -u
R=${r:-/tmp/pgfs-results}; W=${w:-/tmp/pgfs-work}/pgfs; mkdir -p "$R" "$W"
SRC=${SRC:-$PWD/bench/flow-par/pgfman-src}
START=${START:-8}; STEP=${STEP:-4}; N=${N:-16}; TH=${TH:-1}; SDUMP=${SDUMP:-0}
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
CARGO_PROFILE_RELEASE_DEBUG=${DEBUG:-line-tables-only} cargo build --release -q -p phitex-cli > "$R/build.log" 2>&1 || { echo "build failed"; tail -30 "$R/build.log"; exit 1; }
B=$PWD/target/release/phitex
export PHITEX_CACHE_DIR=$W/cache; rm -rf $W/cache $W/fmt; mkdir -p $W/fmt
printf '%s\n' '\documentclass{article}\begin{document}x\end{document}' > $W/fmt/one.tex
(cd $W/fmt && $B --compat=pdflatex -threads=4 -interaction=batchmode one.tex > "$R/format.out" 2>&1)
rm -rf $W/run; cp -a $SRC $W/run
cmds=(-ex 'set pagination off' -ex 'set confirm off' -ex 'handle SIGINT stop print nopass' -ex run)
[ "$SDUMP" = 1 ] && cmds=(-x $PWD/bench/flow-par/sdump.py "${cmds[@]}")
for i in $(seq $N); do cmds+=(-ex "echo ==SAMPLE $i\\n" -ex 'thread apply all bt 60'); [ "$SDUMP" = 1 ] && cmds+=(-ex sdump); cmds+=(-ex continue); done
cd $W/run
PHITEX_SCHED=$R/sched.tsv gdb -batch "${cmds[@]}" --args $B --compat=pdflatex -threads=$TH -interaction=batchmode pgfmanual.tex > "$R/gdb.txt" 2>&1 &
g=$!
t0=$(date +%s%N)
sleep $START
for i in $(seq $N); do
  p=$(pgrep -P $g | head -1); [ -z "$p" ] && break
  echo "sample $i at $(( ($(date +%s%N) - t0) / 1000000 )) ms" | tee -a "$R/summary.txt"
  kill -INT $p; sleep $STEP
done
p=$(pgrep -P $g | head -1); [ -n "$p" ] && kill -TERM $p
wait $g
echo "gdb done; $(grep -c '^==SAMPLE' "$R/gdb.txt") samples" | tee -a "$R/summary.txt"
gzip -f "$R/sched.tsv" 2>/dev/null
