#!/bin/bash
# The course's measurements, inside the cluster container (job.sbatch
# `course` task; $w is the workspace, $r the results directory):
#
#   course.sh plain [NAME=VALUE...]   a cold plain build, with those switches
#   course.sh ab NAME=VALUE [runs]    plain builds alternating without and
#                                     with a switch, `runs` times each (3)
#   course.sh ssa                     a cold SSA build
#   course.sh rebuild                 the word edit of ch15 rebuilt in the
#                                     same process, against plain partex
#   course.sh label                   a \label added in ch15, then two passes
#                                     with no edit, against three plain runs
#
# Each prints its numbers and appends them to $r/summary.txt. Times are
# wall clock and peak memory of partex itself (GNU time, no wrapper).
set -uo pipefail
what=${1:?what}; shift
# (the binary: phitex, or partex in a commit from before the rename)
rel() { if [ -x "$1/phitex" ]; then echo "$1/phitex"; else echo "$1/partex"; fi; }
P=$w/partex; B=$(rel "$P/target/release"); C=$w/course; A=$w/aux
S=$r/summary.txt
env0=(SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1)
# statistics masked in log comparisons, as in e2e (xtask/src/mask.rs)
st='strings out of\|string characters out of\|words of memory out of\|multiletter control sequences out of\|words of font info for\|hyphenation exceptions out of\|stack positions out of'

say() { echo "$*" | tee -a "$S"; }

# the pdflatex format made by this binary, once
F=$w/fmt/pdflatex
if [ ! -f $F.fmt ]; then
  mkdir -p $w/fmt && (cd $w/fmt && ln -sf $B pdftex && timeout -s KILL 600 ./pdftex -ini -interaction=batchmode \
    -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1; rm -f pdftex)
  [ -f $F.fmt ] || { say "FORMAT FAILED"; exit 1; }
fi

# fresh <dir>: an output directory with the settled aux files
fresh() { rm -rf "$1"; mkdir -p "$1"; cp $A/course.* "$1"/ 2>/dev/null; true; }
# build <out> [NAME=VALUE...]: one build of the course into <out>
build() {
  local o=$1; shift
  (cd $C && /usr/bin/time -v -o $o/time.txt timeout -s KILL 7200 env "${env0[@]}" "$@" \
    $B --compat=pdftex -fmt=$F -interaction=batchmode -output-directory=$o course.tex \
    </dev/null >$o/term.txt 2>$o/err.txt)
  local e=$?
  local t m
  t=$(grep -a "Elapsed" $o/time.txt | awk '{print $NF}')
  m=$(grep -a "Maximum resident" $o/time.txt | awk '{print $NF}')
  echo "exit $e wall $t maxrss_kb $m"
}
masklog() { grep -av "$st" "$1"; }

case $what in
  plain)
    o=$r/plain; fresh $o
    say "plain $*: $(build $o "$@")"
    grep -a "defprobe\|panicked" $o/err.txt | cut -c1-3000 | tee -a "$S"
    ;;
  ab)
    sw=${1:?switch}; n=${2:-3}
    ref=
    for i in $(seq $n); do
      for x in off on; do
        o=$r/ab-$x; fresh $o
        if [ $x = on ]; then line=$(build $o "$sw"); else line=$(build $o PARTEX_NOTHING=1); fi
        say "ab $x run $i: $line"
        if [ $x = on ] && [ $i = 1 ]; then grep -a "defprobe\|panicked" $o/err.txt | cut -c1-3000 | tee -a "$S"; fi
        if [ -z "$ref" ]; then ref=$w/ab-ref.pdf; cp $o/course.pdf $ref; fi
        $P/scripts/pdfcheck same $o/course.pdf $ref >/dev/null 2>&1 || say "ab $x run $i: PDF DIFFERS"
      done
    done
    ;;
  ssa)
    o=$r/ssa; fresh $o
    say "ssa cold: $(build $o PARTEX_SSA=1 PARTEX_SSA_APPLY=0)"
    grep -a "ssa build\|panicked" $o/err.txt | cut -c1-900 | tee -a "$S"
    ;;
  rebuild)
    cp $C/ch15.tex $w/ch15.orig
    o=$r/rebuild-ssa; fresh $o
    line=$(build $o PARTEX_SSA=1 PARTEX_SSA_APPLY=${APPLY-} PARTEX_SSA_REBUILD_TRACE=${TRACE:-0} \
      "PARTEX_SSA_REBUILD=mkdir -p $o/cold && cp $o/course.* $o/cold/; sed -i 's/That limit is expensive\./That limit is dear./' ch15.tex")
    say "rebuild ssa: $line"
    grep -q "dear" $C/ch15.tex || say "EDIT NOT APPLIED"
    ref=$r/rebuild-ref; rm -rf $ref; mkdir -p $ref; cp $o/cold/course.* $ref/ 2>/dev/null
    rm -f $ref/course.pdf $ref/course.log
    say "rebuild ref: $(build $ref)"
    cp --remove-destination $w/ch15.orig $C/ch15.tex
    grep -a "ssa build 0: [0-9]\|ssa rebuild 1: [0-9]\|stopped\|panicked" $o/err.txt | cut -c1-900 | tee -a "$S"
    $P/scripts/pdfcheck same $o/course.pdf $ref/course.pdf 2>&1 | tee -a "$S"
    if diff <(masklog $o/course.log) <(masklog $ref/course.log) >$r/rebuild-log.diff; then
      say "log same (statistics masked)"
    else
      say "log DIFF: $(wc -l <$r/rebuild-log.diff) lines"
    fi
    ;;
  label)
    cp $C/ch15.tex $w/ch15.orig
    o=$r/label; fresh $o
    cmds="mkdir -p $o/p0 && cp $o/course.* $o/p0/; sed -i 's/That limit is expensive\./That limit is expensive.\\\\label{new-lbl}/' ch15.tex"
    cmds="$cmds"$'\n'"mkdir -p $o/p1 && cp $o/course.* $o/p1/; true"
    cmds="$cmds"$'\n'"mkdir -p $o/p2 && cp $o/course.* $o/p2/; true"
    say "label ssa: $(build $o PARTEX_SSA=1 PARTEX_SSA_REBUILD_TRACE=1 "PARTEX_SSA_REBUILD=$cmds")"
    mkdir -p $o/p3 && cp $o/course.* $o/p3/
    grep -q "new-lbl" $C/ch15.tex || say "EDIT NOT APPLIED"
    grep -a "ssa rebuild [0-9]: [0-9]" $o/err.txt | cut -c1-600 | tee -a "$S"
    ref=$r/label-ref; rm -rf $ref; mkdir -p $ref; cp $o/p0/course.* $ref/ 2>/dev/null
    rm -f $ref/course.pdf $ref/course.log
    for i in 1 2 3; do
      say "label ref $i: $(build $ref)"
      mkdir -p $ref/o$i && cp $ref/course.* $ref/o$i/
    done
    cp --remove-destination $w/ch15.orig $C/ch15.tex
    for i in 1 2 3; do
      a=$o/p$i; b=$ref/o$i
      $P/scripts/pdfcheck same $a/course.pdf $b/course.pdf >/dev/null 2>&1 && p=same || p=DIFF
      diff <(masklog $a/course.log) <(masklog $b/course.log) >$r/label-log-$i.diff && l=same || l=DIFF
      cmp -s $a/course.aux $b/course.aux && x=same || x=DIFF
      say "label pass $i: pdf $p, log $l, aux $x; warning: $(grep -c 'Label(s) may have changed' $a/course.log)"
    done
    ;;
  *) say "unknown course task $what"; exit 2 ;;
esac
# (the PDFs and the big outputs stay on the node)
find $r -name '*.pdf' -delete
