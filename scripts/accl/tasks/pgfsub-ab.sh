#!/bin/bash
# pgfsub's edits (bench/edits/pgfsub.txt) by two binaries, alternating on
# one node (job.sbatch `cmd` task, run from the checkout; $w is the
# workspace, $r the results directory):
#
#   pgfsub-ab.sh OLD [ROUNDS]   commit OLD built in a worktree; pgfsub's
#                               .aux settled by plain builds (three);
#                               then bench/ssa-course.sh with OLD's
#                               binary and the checkout's in turn, ROUNDS
#                               times (2), 3 warm word pairs each
#
# Each run's table goes to $r/ab-<which>-<round>.md; the cold line and
# the edits' rows are appended to $r/summary.txt.
set -uo pipefail
old=${1:?old commit} n=${2:-2}
P=$PWD
git worktree add -q --detach "$w/old" "$old" || exit 1
(cd "$w/old" && cargo build --release -p partex-cli -q) || exit 1
cargo build --release -p partex-cli -q || exit 1
new=$(git rev-parse --short HEAD) old=$(git -C "$w/old" rev-parse --short HEAD)
B=$P/target/release/phitex
# the manual's sources with pgfsub.tex, and its settled .aux (plain builds)
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
  echo "plain build $i: exit $?" | tee -a "$r/summary.txt"
done
rm -f "$A/pgfsub.pdf" "$A/pgfsub.log"
for i in $(seq "$n"); do
  for which in old new; do
    if [ $which = old ]; then b=$w/old/target/release/phitex c=$old; else b=$B c=$new; fi
    HEAVY=0 BIN=$b REV=$c MAIN=pgfsub.tex EDITS=$P/bench/edits/pgfsub.txt \
      OUT=$r/ab-$which-$i.json bench/ssa-course.sh --warm 3 "$D" "$A" \
      >"$r/ab-$which-$i.md" 2>"$r/ab-$which-$i.err"
    echo "round $i $which ($c): exit $?" | tee -a "$r/summary.txt"
    grep -a 'cold build\|^| [0-9]' "$r/ab-$which-$i.md" | tee -a "$r/summary.txt"
  done
done
