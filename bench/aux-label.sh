#!/bin/bash
# The course's `label` edit (bench/edits/course.txt) with the .aux loop in
# one rebuild (DESIGN 3.7, "Trips, as built"): one SSA process (the cold
# build, the label, then a rebuild with no edit) against plain partex run
# to its fixed point at each stage (its files but the log and PDF the
# same before and after a run, at most 5 runs), as scripts/ssa-edits
# --fixpoint does. For the accl `cmd` task, which has the course in
# $w/course and its settled aux files in $w/aux:
#
#   scripts/accl/accl run cmd bash bench/aux-label.sh [NAME=VALUE...]
#
# NAME=VALUE pairs go to the SSA process (PARTEX_SSA_TRIPS=1: one trip a
# rebuild, against one plain run a stage). Results in $r: summary.txt,
# the SSA process's stderr, each stage's files.
set -uo pipefail
: "${w:?the accl workspace}" "${r:?the results directory}"
P=$PWD; B=$P/target/release/partex; C=$w/course; A=$w/aux
S=$r/summary.txt
say() { echo "$*" | tee -a "$S"; }
one=0
for kv in "$@"; do [ "$kv" = PARTEX_SSA_TRIPS=1 ] && one=1; done

cargo build --release -p partex-cli -q 2>&1 | tail -5
[ -x $B ] || { say "BUILD FAILED"; exit 1; }
F=$w/fmt/pdflatex
mkdir -p $w/fmt && (cd $w/fmt && ln -sf $B pdftex && timeout -s KILL 600 ./pdftex -ini \
  -interaction=batchmode -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' \
  >/dev/null 2>&1; rm -f pdftex)
[ -f $F.fmt ] || { say "FORMAT FAILED"; exit 1; }

env0=(SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1)
st='strings out of\|string characters out of\|words of memory out of\|multiletter control sequences out of\|words of font info for\|hyphenation exceptions out of\|stack positions out of'
masklog() { tail -n +2 "$1" | grep -av "$st"; }
# build <out> [NAME=VALUE...]: one run of the course into <out>
build() {
  local o=$1; shift
  (cd $C && /usr/bin/time -v -o $o/time.txt timeout -s KILL 7200 env "${env0[@]}" "$@" \
    $B --compat=pdftex -fmt=$F -interaction=batchmode -output-directory=$o course.tex \
    </dev/null >$o/term.txt 2>$o/err.txt)
  local e=$?
  echo "exit $e wall $(grep -a Elapsed $o/time.txt | awk '{print $NF}')" \
    "maxrss_kb $(grep -a 'Maximum resident' $o/time.txt | awk '{print $NF}')"
}
# written <dir>: what the fixed point compares, every file but the log,
# the PDF and the run's own records
written() { (cd "$1" && for f in course.*; do case $f in *.log|*.pdf) ;; *) md5sum "$f" ;; esac; done); }
# fixpoint <dir> <stage>: plain runs until its files settle
fixpoint() {
  local o=$1 n=0 before
  while :; do
    before=$(written $o)
    say "  plain stage $2 run $((n + 1)): $(build $o)"
    n=$((n + 1))
    [ $one = 1 ] && break
    [ "$(written $o)" = "$before" ] && break
    [ $n -ge 5 ] && break
  done
  say "  plain stage $2: $n runs"
}
edit="s/zero where the constraint holds\\./zero where the constraint holds.\\\\label{partex:bench}/"

cp $C/ch15.tex $w/ch15.orig
o=$r/ssa; rm -rf $o; mkdir -p $o; cp $A/course.* $o/
cmds="mkdir -p $o/s0 && cp $o/course.* $o/s0/; sed -i '$edit' ch15.tex"
cmds="$cmds"$'\n'"mkdir -p $o/s1 && cp $o/course.* $o/s1/; true"
say "ssa: $(build $o PARTEX_SSA=1 PARTEX_SSA_REBUILD_TRACE=1 "$@" "PARTEX_SSA_REBUILD=$cmds")"
mkdir -p $o/s2 && cp $o/course.* $o/s2/
grep -q 'partex:bench' $C/ch15.tex || say "EDIT NOT APPLIED"
grep -a "ssa build 0: [0-9]\|ssa build 0: trips\|ssa rebuild [0-9]*: [0-9]\|ssa rebuild [0-9]*: trips\|bibtex\|makeindex\|stopped\|panicked" \
  $o/err.txt | cut -c1-900 | tee -a "$S"

ref=$r/ref; rm -rf $ref; mkdir -p $ref; cp $A/course.* $ref/
cp --remove-destination $w/ch15.orig $C/ch15.tex
for i in 0 1 2; do
  [ $i = 1 ] && sed -i "$edit" $C/ch15.tex
  fixpoint $ref $i
  mkdir -p $ref/s$i && cp $ref/course.* $ref/s$i/
done
cp --remove-destination $w/ch15.orig $C/ch15.tex

for i in 0 1 2; do
  a=$o/s$i; b=$ref/s$i
  cmp -s $a/course.pdf $b/course.pdf && p=identical || {
    $P/scripts/pdfcheck same $a/course.pdf $b/course.pdf >/dev/null 2>&1 && p="same content" || p=DIFF; }
  diff <(masklog $a/course.log) <(masklog $b/course.log) >$r/log-$i.diff && l=same || l=DIFF
  x=""
  for e in aux toc out; do cmp -s $a/course.$e $b/course.$e && x="$x $e same" || x="$x $e DIFF"; done
  say "stage $i: pdf $p, log $l,$x; rerun warnings $(grep -c 'may have changed' $a/course.log)"
done
find $r -name '*.pdf' -delete
