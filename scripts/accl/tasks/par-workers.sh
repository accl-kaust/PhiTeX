#!/bin/bash
# Parallel SSA builds measured (job.sbatch `cmd` task, run from the
# checkout; $w is the workspace, $r the results directory, the course in
# $w/course and its aux in $w/aux):
#
#   par-workers.sh [WORKERS...]   (default 1 8 16 48)
#
# DOCS (default `course pgf`) names the documents; the course's edits
# (bench/edits/par.txt) are the course copy's that `ACCL_COURSE` names.
#
# For each worker count N: the course and pgfsub, each a cold build and
# its edits (bench/edits/par.txt, bench/edits/pgfsub.txt) as the rebuilds
# of one process, with the rebuilds' steps on N workers (A); then the
# cold build with its paragraphs on N workers too (B, PHITEX_SSA_COLD=1).
# pdflatex's time for each document, if the node has it. Each run's table
# goes to $r/<doc>-<mode>-<N>.md, its stderr beside it; the cold lines,
# the edits' rows and the workers' reports are appended to
# $r/summary.txt.
set -uo pipefail
P=$PWD
cargo build --release -p partex-cli -q || exit 1
B=$P/target/release/phitex
ns=("$@")
[ ${#ns[@]} -eq 0 ] && ns=(1 8 16 48)

# pgfsub's sources and settled .aux (as pgfsub-ab.sh makes them)
D=$w/pgf A=$w/pgfaux F=$w/fmt
cp -a upstream/pgf/doc/generic/pgf "$D" && cp bench/inputs/pgfsub.tex "$D/"
mkdir -p "$F" "$A"
(cd "$F" && env SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 "$B" --compat=pdftex -ini \
  -interaction=batchmode -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' \
  </dev/null >/dev/null 2>&1)
[ -f "$F/pdflatex.fmt" ] || { echo "FORMAT FAILED"; exit 1; }
for i in 1 2 3; do
  (cd "$D" && env SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 "$B" --compat=pdftex \
    -fmt="$F/pdflatex" -interaction=batchmode -output-directory="$A" pgfsub.tex \
    </dev/null >/dev/null 2>&1)
done
rm -f "$A/pgfsub.pdf" "$A/pgfsub.log"

if command -v pdflatex >/dev/null; then
  for doc in course pgf; do
    if [ $doc = course ]; then src=$w/course main=course.tex; else src=$D main=pgfsub.tex; fi
    mkdir -p "$w/pdflatex-$doc"
    t0=$(date +%s.%N)
    (cd "$src" && pdflatex -interaction=batchmode -output-directory="$w/pdflatex-$doc" "$main" \
      </dev/null >/dev/null 2>&1)
    t1=$(date +%s.%N)
    echo "pdflatex $doc: $(echo "$t1 - $t0" | bc) s (one pass)" | tee -a "$r/summary.txt"
  done
fi

run() { # run DOC MODE N
  local doc=$1 mode=$2 n=$3 out=$r/$1-$2-$3 cold=0
  [ "$mode" = b ] && cold=1
  if [ "$doc" = course ]; then
    PHITEX_SSA_WORKERS=$n PHITEX_SSA_COLD=$cold HEAVY=0 BIN=$B OUT=$out.json \
      EDITS=$P/bench/edits/par.txt bench/ssa-course.sh --warm 1 "$w/course" "$w/aux" \
      >"$out.md" 2>"$out.err"
  else
    PHITEX_SSA_WORKERS=$n PHITEX_SSA_COLD=$cold HEAVY=0 BIN=$B OUT=$out.json MAIN=pgfsub.tex \
      EDITS=$P/bench/edits/pgfsub.txt bench/ssa-course.sh --warm 1 "$D" "$A" \
      >"$out.md" 2>"$out.err"
  fi
  echo "== $doc $mode N=$n: exit $?" | tee -a "$r/summary.txt"
  grep -a 'cold build\|^| [0-9]' "$out.md" | tee -a "$r/summary.txt"
  cp target/ssa-course/err.txt "$out.phitex.err" 2>/dev/null
  grep -a 'workers [0-9]*:\|cold: .*steps taken\|correction' "$out.phitex.err" | cut -c1-400 \
    | tee -a "$r/summary.txt"
}
for n in "${ns[@]}"; do
  for doc in ${DOCS:-course pgf}; do
    run "$doc" a "$n"
    [ "$n" -gt 1 ] && run "$doc" b "$n"
  done
done
