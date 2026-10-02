#!/bin/bash
# The course with the view (DESIGN 4.3 item 7), in an accl `cmd` job
# ($w, $r exported; the course in $w/course, its aux in $w/aux):
#   scripts/accl/accl run cmd bench/view-course.sh
# A cold SSA build with PARTEX_SSA_VIEW, the `word` edit of ch15 rebuilt
# in the same process (the view again after it), then the same without
# the view, for its cost. Prints the numbers to $r/summary.txt and keeps
# the windows of ch15 and the windows that changed.
set -uo pipefail
P=$w/partex; B=$P/target/release/partex; C=$w/course; A=$w/aux; S=$r/summary.txt
say() { echo "$*" | tee -a "$S"; }
(cd $P && cargo build --release -p partex-cli -q) || { say "BUILD FAILED"; exit 1; }
F=$w/fmt/pdflatex
mkdir -p $w/fmt && (cd $w/fmt && ln -sf $B pdftex && timeout -s KILL 600 ./pdftex -ini -interaction=batchmode \
  -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1; rm -f pdftex)
[ -f $F.fmt ] || { say "FORMAT FAILED"; exit 1; }
cp $C/ch15.tex $w/ch15.orig
edit="sed -i 's/That limit is expensive\\./That limit is dear./' ch15.tex"
perf=$(command -v perf || true)
run() { # run <out> [VAR=VALUE...]: the cold build and the word edit
  local o=$1; shift
  rm -rf $o; mkdir -p $o; cp $A/course.* $o/ 2>/dev/null
  cp --remove-destination $w/ch15.orig $C/ch15.tex
  local pre=()
  [ -n "$perf" ] && pre=($perf stat -e instructions:u -x, -o $o/perf.txt --)
  (cd $C && /usr/bin/time -v -o $o/time.txt timeout -s KILL 7200 env SOURCE_DATE_EPOCH=1758800000 \
    FORCE_SOURCE_DATE=1 PARTEX_SSA=1 "$@" "PARTEX_SSA_REBUILD=$edit" "${pre[@]}" $B --compat=pdftex -fmt=$F \
    -interaction=batchmode -output-directory=$o course.tex </dev/null >$o/term.txt 2>$o/err.txt)
  say "$o: exit $? wall $(grep -a Elapsed $o/time.txt | awk '{print $NF}') maxrss_kb $(grep -a 'Maximum resident' $o/time.txt | awk '{print $NF}') instructions $(cut -d, -f1 $o/perf.txt 2>/dev/null)"
  grep -a "ssa view\|ssa build 0: [0-9]\|ssa rebuild 1: [0-9]\|panicked\|stopped" $o/err.txt | cut -c1-400 | tee -a "$S"
}
run $w/view PARTEX_SSA_VIEW=$w/view/view.txt PARTEX_SSA_VIEW_STEP=12
run $w/plain PARTEX_NOTHING=1
cp --remove-destination $w/ch15.orig $C/ch15.tex
v=$w/view
cmp -s $v/course.pdf $w/plain/course.pdf && say "pdf same with and without the view" || say "PDF DIFFERS"
ls -la $v/view.txt* | tee -a "$S"
# (the windows of ch15, their comments, and the windows the edit changed)
grep -n 'ch15' $v/view.txt | sed 's/^\([0-9]*\):\(%[0-9]*\) = .*) ; /\1 \2 ; /' > $r/ch15-shows.txt
diff <(sed 's/^\(%[0-9]*\) = .*) ; /\1 ; /' $v/view.txt) <(sed 's/^\(%[0-9]*\) = .*) ; /\1 ; /' $v/view.txt.1) > $r/changed-shows.diff
say "windows whose comment changed: $(grep -c '^>' $r/changed-shows.diff)"
cmp <(cut -c1-100000 $v/view.txt) <(cut -c1-100000 $v/view.txt.1) | head -1 | tee -a "$S"
diff $v/view.txt $v/view.txt.1 | grep '^[<>]' | cut -c1-3000 > $r/changed-windows.txt
say "lines that differ: $(grep -c '^>' $r/changed-windows.txt)"
gzip -c $v/view.txt > $r/view.txt.gz; gzip -c $v/view.txt.1 > $r/view.txt.1.gz
cp $v/view.txt*.step12 $r/ 2>/dev/null
ls -la $r | tee -a "$S"
