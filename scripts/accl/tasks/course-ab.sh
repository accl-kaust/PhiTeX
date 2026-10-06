#!/bin/bash
# The course's warm word edits by two binaries, alternating on one node
# (job.sbatch `cmd` task, run from the checkout; $w is the workspace, $r
# the results directory, the course in $w/course and its aux in $w/aux):
#
#   course-ab.sh OLD [ROUNDS]   commit OLD built in a worktree, then
#                               bench/ssa-course.sh with OLD's binary and
#                               the checkout's in turn, ROUNDS times (2),
#                               10 warm pairs and the `word` edit each
#
# Each run's table goes to $r/ab-<which>-<round>.md; the warm medians are
# printed, and appended to $r/summary.txt.
set -uo pipefail
old=${1:?old commit} n=${2:-2}
P=$PWD
git worktree add -q --detach "$w/old" "$old" || exit 1
(cd "$w/old" && cargo build --release -p partex-cli -q) || exit 1
cargo build --release -p partex-cli -q || exit 1
new=$(git rev-parse --short HEAD) old=$(git -C "$w/old" rev-parse --short HEAD)
# (the binary: phitex, or partex in a commit from before the rename)
rel() { if [ -x "$1/phitex" ]; then echo "$1/phitex"; else echo "$1/partex"; fi; }
for i in $(seq "$n"); do
  for which in old new; do
    if [ $which = old ]; then b=$(rel "$w/old/target/release"); c=$old; else b=$P/target/release/phitex; c=$new; fi
    BIN=$b REV=$c OUT=$r/ab-$which-$i.json bench/ssa-course.sh --warm 10 --edits word \
      "$w/course" "$w/aux" >"$r/ab-$which-$i.md" 2>"$r/ab-$which-$i.err"
    echo "round $i $which ($c): exit $?" | tee -a "$r/summary.txt"
    grep -a 'warm, median\|cold build' "$r/ab-$which-$i.md" | tee -a "$r/summary.txt"
  done
done
