#!/bin/bash
# The PGF manual, inside the cluster container (job.sbatch `cmd` task, run
# from the checkout; $w is the workspace, $r the results directory):
#
#   pgfman.sh FILE OLD NEW   a cold SSA build of upstream/pgf's manual, then
#                            OLD replaced by NEW in FILE, rebuilt in the
#                            same process
#
# TRIPS (1: one trip a build, as the Overleaf extension runs it), TRACE
# (PARTEX_SSA_REBUILD_TRACE) and LTO (false: a quicker build) pass
# through. Its stderr and terminal output land in $r.
set -uo pipefail
file=${1:?file} old=${2:?old} new=${3:?new}
P=$PWD
CARGO_PROFILE_RELEASE_LTO=${LTO:-false} CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  cargo build --release -p partex-cli -q || exit 1
B=$P/target/release/partex
F=$w/fmt/pdflatex
mkdir -p $w/fmt && (cd $w/fmt && ln -sf $B pdftex && timeout -s KILL 600 ./pdftex -ini -interaction=batchmode \
  -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1; rm -f pdftex)
[ -f $F.fmt ] || { echo "FORMAT FAILED"; exit 1; }
D=$w/pgf
cp -a upstream/pgf/doc/generic/pgf $D
# (the edit, as a rebuild line: sed, then a check that it was made)
printf '%s\n' "sed -i 's/$old/$new/' $file && grep -q '$new' $file" > $w/edit.sh
cd $D
/usr/bin/time -v -o $r/time.txt timeout -s KILL 6000 env SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 \
  RUST_BACKTRACE=1 PARTEX_SSA=1 PARTEX_SSA_TRIPS=${TRIPS:-1} PARTEX_SSA_REBUILD_TRACE=${TRACE:-0} \
  "PARTEX_SSA_REBUILD=sh $w/edit.sh" \
  $B --compat=pdftex -fmt=$F -interaction=batchmode pgfmanual </dev/null >$r/term.txt 2>$r/err.txt
e=$?
echo "exit $e" | tee -a $r/summary.txt
grep -a "Elapsed\|Maximum resident" $r/time.txt | tee -a $r/summary.txt
grep -a "ssa build 0: [0-9.]* ms\|ssa rebuild [0-9]*: [0-9.]* ms\|panicked\|stopped" $r/err.txt | cut -c1-600 | tee -a $r/summary.txt
grep -a -A40 "panicked" $r/err.txt | head -60 | tee -a $r/summary.txt
cp -a pgfmanual.log $r/ 2>/dev/null
exit $e
