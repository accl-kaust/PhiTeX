#!/bin/bash
# The perf history's backfill (docs/ci.md), one array task per commit of
# $CI/harness/data/backfill.txt, on the perf node: the commit built in the
# CI container, its formats made, perf.py run (the harness of the tree that
# submitted it, not the commit's), and the medians appended to
# $CI/history.jsonl as a main entry marked `backfill`. A commit that does
# not build is skipped (its log kept). Submitted by `accl ci-backfill`.
#SBATCH -J ci-backfill -o partex-ci-backfill-%A_%a.out
set -euo pipefail
CI=$HOME/code/flinner/partex-phitex-ci
image=${CI_IMAGE:-partex-ci-2026-09-25}
sif=$HOME/shared-cache/partex/$image.sif
cache=$HOME/shared-cache/partex
commit=$(sed -n "$(( SLURM_ARRAY_TASK_ID + 1 ))p" "$CI/harness/data/backfill.txt")
run=backfill-${commit:0:12}
R=$CI/runs/$run
w=/tmp/$USER-${SLURM_JOB_ID}-${SLURM_ARRAY_TASK_ID}
mkdir -p "$w"/{results/bin,results/perf,home,texmf-var,cargo} "$R"
trap 'rm -rf "$w"' EXIT
echo "backfill $commit on $(hostname), $(date -Is)"
box() {
  apptainer exec --containall --cleanenv --no-home --net --network none \
    --bind "$w:/w" --bind "$CI/harness:/h:ro" --pwd /w \
    --env TEXMFVAR=/w/texmf-var,CARGO_HOME=/w/cargo,CARGO_NET_OFFLINE=true,PARTEX_SANDBOX=1,CI_WORK=/w \
    "$sif" "$@"
}
git clone -q "$HOME/code/flinner/partex-phitex.git" "$w/src"
git -C "$w/src" -c advice.detachedHead=false checkout -q "$commit"
git -C "$w/src" log -1 --format='%H%n%cI%n%s' > "$w/results/bin/commit.txt"
key=$(sha256sum "$w/src/Cargo.lock" | cut -c1-16)
if [ -d "$cache/cargo-$key" ]; then
  cp -a "$cache/cargo-$key/." "$w/cargo/"
else
  apptainer exec --containall --cleanenv --bind "$w:/w" --pwd /w/src --env CARGO_HOME=/w/cargo "$sif" cargo fetch -q || true
fi
if ! box bash -c 'cd /w/src && cargo build --release -p partex-cli -q' > "$w/results/build.log" 2>&1; then
  echo "build FAILED: skipped"; tail -5 "$w/results/build.log"
  cp -a "$w/results/." "$R/"; exit 0
fi
mkdir -p "$w/bin" "$w/formats"
cp "$w/src/target/release/partex" "$w/bin/partex"
box python3 /h/ci/formats.py /w/bin/partex /w/formats || { echo "formats FAILED: skipped"; cp -a "$w/results/." "$R/"; exit 0; }
[ -d "$HOME/code/tmp/np-course/src" ] && cp -a "$HOME/code/tmp/np-course/src" "$w/course"
box python3 /h/ci/perf.py --src /w/src --bin /w/bin/partex --formats /w/formats --doc /usr/share/doc/texlive \
  --out /w/results/perf --reps "${CI_BACKFILL_REPS:-3}" ${CI_BACKFILL_ONLY:+--only $CI_BACKFILL_ONLY} || true
cp -a "$w/results/." "$R/"
apptainer exec --containall --cleanenv --no-home --net --network none \
  --bind "$CI:/ci" --bind "$CI/harness:/h:ro" "$sif" \
  python3 /h/ci/score.py --run "/ci/runs/$run" --ci /ci --perf-only
