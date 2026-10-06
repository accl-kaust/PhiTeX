#!/bin/bash
# The course's word edit and its revert rebuilt in one SSA process, N
# times, by this checkout's partex and, back to back on the same node, by
# a base commit's: each rebuild's link line, and every stage's PDF, log
# and .aux against plain partex on the same sources (PDF and .aux byte
# for byte, the log with its statistics masked).
#
#   bench/link-course.sh [BASE [N [NAME=VALUE...]]]
#
# Run by the accl `cmd` task (scripts/accl/accl run cmd bench/link-course.sh
# ...): the course in $w/course, its settled aux files in $w/aux, results
# in $r. BASE (default main; `none`: this checkout only) is built in a
# worktree of the checkout; N rebuilds (default 10) alternate the edit and
# its revert; NAME=VALUE switches go to both SSA processes.
set -uo pipefail
base=${1:-main}
n=${2:-10}
shift $(($# < 2 ? $# : 2))
P=$(pwd)
C=$w/course
A=$w/aux
O=$C/out
S=$r/summary.txt
env0=(SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1)
st='strings out of\|string characters out of\|words of memory out of\|multiletter control sequences out of\|words of font info for\|hyphenation exceptions out of\|stack positions out of'
say() { echo "$*" | tee -a "$S"; }
masklog() { grep -av "$st" "$1" | tail -n +2; }

# (the binary: phitex, or partex in a commit from before the rename)
rel() { if [ -x "$1/phitex" ]; then echo "$1/phitex"; else echo "$1/partex"; fi; }
# the binaries: this checkout's, and the base's in a worktree
cargo build --release -p partex-cli -q || { say "BUILD FAILED"; exit 1; }
declare -A bin=([new]=$P/target/release/phitex)
whichs=(new)
if [ "$base" != none ]; then
  git worktree add -q --detach "$w/base" "$base" || { say "NO BASE $base"; exit 1; }
  (cd "$w/base" && CARGO_TARGET_DIR=$w/base-target cargo build --release -p partex-cli -q) ||
    { say "BASE BUILD FAILED"; exit 1; }
  bin[base]=$(rel "$w/base-target/release")
  whichs=(base new)
fi
say "node $(uname -n), new $(git rev-parse --short=12 HEAD), base $base, $n rebuilds, switches: $*; $(nproc) CPUs, load $(cut -d' ' -f1-3 /proc/loadavg)"

# each binary's pdflatex format
for b in "${whichs[@]}"; do
  d=$w/fmt-$b
  mkdir -p "$d" && (cd "$d" && ln -sf "${bin[$b]}" pdftex && timeout -s KILL 600 ./pdftex -ini -interaction=batchmode \
    -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1; rm -f pdftex)
  [ -f "$d/pdflatex.fmt" ] || { say "FORMAT FAILED ($b)"; exit 1; }
done

# the output directory, the same path for every run (the log names it)
fresh() { rm -rf "$O"; mkdir -p "$O"; cp "$A"/course.* "$O"/ 2>/dev/null; true; }
cp "$C/ch15.tex" "$w/ch15.orig"
sed 's/That limit is expensive\./That limit is dear./' "$w/ch15.orig" >"$w/ch15.edit"
cmp -s "$w/ch15.orig" "$w/ch15.edit" && { say "EDIT NOT FOUND"; exit 1; }

# the oracle: plain partex (this checkout's) on the original and the
# edited source
for v in orig edit; do
  fresh
  cp "$w/ch15.$v" "$C/ch15.tex"
  (cd "$C" && /usr/bin/time -f "%e s %M kB" -o "$w/time.txt" env "${env0[@]}" "${bin[new]}" --compat=pdftex \
    -fmt="$w/fmt-new/pdflatex" -interaction=batchmode -output-directory=out course.tex </dev/null >/dev/null 2>"$w/err.txt")
  say "plain $v: exit $?, $(cat "$w/time.txt")"
  rm -rf "$w/ref-$v" && cp -a "$O" "$w/ref-$v"
done

# each SSA process: stage k is after rebuild k (0: the cold build); odd
# stages are the edited source, even ones the original
for which in "${whichs[@]}"; do
  s=$w/snap-$which
  rm -rf "$s"
  cmds=""
  for k in $(seq 1 "$n"); do
    v=$([ $((k % 2)) = 1 ] && echo edit || echo orig)
    cmds+="mkdir -p $s/$((k - 1)) && cp $O/course.pdf $O/course.log $O/course.aux $s/$((k - 1))/; cp $w/ch15.$v $C/ch15.tex"$'\n'
  done
  fresh
  cp "$w/ch15.orig" "$C/ch15.tex"
  (cd "$C" && /usr/bin/time -f "%e s %M kB" -o "$w/time.txt" env "${env0[@]}" PARTEX_SSA=1 "$@" \
    "PARTEX_SSA_REBUILD=$cmds" "${bin[$which]}" --compat=pdftex -fmt="$w/fmt-$which/pdflatex" \
    -interaction=batchmode -output-directory=out course.tex </dev/null >/dev/null 2>"$w/err.txt")
  e=$?
  mkdir -p "$s/$n" && cp "$O"/course.{pdf,log,aux} "$s/$n/"
  cp "$w/ch15.orig" "$C/ch15.tex"
  say "ssa $which: exit $e, $(cat "$w/time.txt")"
  cp "$w/err.txt" "$r/ssa-$which.err"
  grep -a "ssa build 0: link\|ssa rebuild [0-9]*: link\|ssa rebuild [0-9]*: [0-9.]* ms\|panicked\|stopped" "$w/err.txt" |
    sed 's/, edits [0-9].*//' | cut -c1-700 | tee -a "$S"
  for k in $(seq 0 "$n"); do
    v=$([ $((k % 2)) = 1 ] && echo edit || echo orig)
    ref=$w/ref-$v
    cmp -s "$s/$k/course.pdf" "$ref/course.pdf" && p=same || p=DIFF
    cmp -s "$s/$k/course.aux" "$ref/course.aux" && x=same || x=DIFF
    diff <(masklog "$s/$k/course.log") <(masklog "$ref/course.log") >"$r/log-$which-$k.diff" && l=same || l=DIFF
    say "ssa $which stage $k ($v): pdf $p, log $l, aux $x"
  done
done
