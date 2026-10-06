#!/bin/bash
# One stage of the CI (docs/ci.md), on accl, submitted by `accl ci`:
#
#   sbatch [slurm options] ci.sbatch <stage> <run> [args...]
#
# Stages:
#   build <commit>        the release binary and its pdflatex and xelatex
#                         formats, into <run>/bin (and the commit's tree,
#                         <run>/src.tar, for the stages that need cargo)
#   manuals               an array task: its shard of ci/manuals.json, in
#                         every mode, against the cached oracle
#   oracle <list>         an array task: TeX Live's builds of its shard of
#                         <list> (the candidates or the pinned list) into
#                         the oracle cache
#   discover              the candidates (manuals.py discover) into <run>
#   torture               trip, etrip (plain and machine mode), the l3build
#                         suites, the escape tests
#   perf [reps]           the measured runs (perf.py) on this node
#   score                 the scoreboard and the perf comparison (trusted:
#                         reads JSON only), the history and its report
#
# <run> is a directory under $CI/runs. Documents are untrusted code: each
# stage runs them in `box`, an Apptainer container with no network, no
# home, the environment cleared, the image read-only, and only the job's
# node-local scratch writable (bound at /w); the harness adds shell escape
# off, kpathsea's paranoid file access and per-run limits (scripts/ci/
# common.py). Results leave the container as files in /w/results, copied
# to <run> by this script when the stage ends; the oracle cache is read-only
# inside and filled from /w/oracle-new after.
#SBATCH -J partex-ci -o partex-ci-%A_%a.out
set -euo pipefail
stage=$1 run=$2
shift 2
CI=$HOME/code/flinner/partex-phitex-ci
image=${CI_IMAGE:-partex-ci-2026-09-25}
sif=$HOME/shared-cache/partex/$image.sif
cache=$HOME/shared-cache/partex
R=$CI/runs/$run
oracle=$CI/oracle/$image
w=/tmp/$USER-${SLURM_JOB_ID}-${SLURM_ARRAY_TASK_ID:-0}
mkdir -p "$w"/{results,oracle-new,home,texmf-var,cargo} "$R" "$oracle"
trap 'rm -rf "$w"' EXIT
echo "stage $stage, run $run, node $(hostname), job ${SLURM_JOB_ID} task ${SLURM_ARRAY_TASK_ID:-}, image $image, $(date -Is)"

# box CMD...: CMD in the container, as described above (/h: the harness,
# read-only; /o: the oracle cache, read-only; /w: the scratch)
box() {
  apptainer exec --containall --cleanenv --no-home --net --network none \
    --bind "$w:/w" --bind "$CI/harness:/h:ro" --bind "$oracle:/o:ro" --pwd /w \
    --env TEXMFVAR=/w/texmf-var,CARGO_HOME=/w/cargo,CARGO_NET_OFFLINE=true,PARTEX_SANDBOX=1,CI_WORK=/w \
    "$sif" "$@"
}
# out: the results to the run's directory; new oracle builds to the cache
out() {
  local into=$1
  mkdir -p "$R/$into"
  cp -a "$w/results/." "$R/$into/"
  if [ -n "$(ls -A "$w/oracle-new")" ]; then
    for d in "$w"/oracle-new/*/; do
      n=$(basename "$d")
      [ -e "$oracle/$n" ] || { cp -a "$d" "$oracle/$n.part-$SLURM_JOB_ID" && mv -T "$oracle/$n.part-$SLURM_JOB_ID" "$oracle/$n" 2>/dev/null || rm -rf "$oracle/$n.part-$SLURM_JOB_ID"; }
    done
  fi
}
shard() { echo "${SLURM_ARRAY_TASK_ID:-0}/${SLURM_ARRAY_TASK_COUNT:-1}"; }
# the binary and formats the build stage made, node-local
bin_in() {
  mkdir -p "$w/bin" "$w/formats"
  cp -a "$R/bin/partex" "$w/bin/"
  cp -a "$R/bin/formats/." "$w/formats/"
}

case $stage in
  build)
    commit=$1
    git clone -q "$HOME/code/flinner/partex-phitex.git" "$w/src"
    git -C "$w/src" -c advice.detachedHead=false checkout -q "$commit"
    git -C "$w/src" log -1 --format='%H%n%cI%n%s' > "$w/results/commit.txt"
    key=$(sha256sum "$w/src/Cargo.lock" | cut -c1-16)
    if [ -d "$cache/cargo-$key" ]; then
      cp -a "$cache/cargo-$key/." "$w/cargo/"
    else
      # (the crates, fetched with the network once per Cargo.lock; no
      # document runs in this container)
      apptainer exec --containall --cleanenv --bind "$w:/w" --pwd /w/src --env CARGO_HOME=/w/cargo "$sif" cargo fetch -q
      p=$cache/cargo-$key.part-$SLURM_JOB_ID
      cp -a "$w/cargo" "$p" && { mv -T "$p" "$cache/cargo-$key" 2>/dev/null || rm -rf "$p"; }
    fi
    t0=$(date +%s)
    status=0
    box bash -c 'cd /w/src && cargo build --release -p partex-cli -q' > "$w/results/build.log" 2>&1 || status=$?
    echo "build_s $(( $(date +%s) - t0 ))" >> "$w/results/commit.txt"
    if [ $status -ne 0 ]; then
      tail -30 "$w/results/build.log"
      echo "build FAILED" | tee "$w/results/summary.txt"
      out build
      exit $status
    fi
    mkdir -p "$w/results/formats"
    cp "$w/src/target/release/partex" "$w/results/partex"
    box python3 /h/ci/formats.py /w/results/partex /w/results/formats || status=$?
    # (the tree, for the torture and perf stages: they build xtask)
    tar -C "$w/src" --exclude=./target -cf "$w/results/src.tar" .
    out bin
    ls -l "$R/bin" "$R/bin/formats"
    exit $status
    ;;
  manuals)
    bin_in
    status=0
    box python3 /h/ci/manuals.py run --list /h/data/manuals.json --doc /usr/share/doc/texlive \
      --cache /o --new-cache /w/oracle-new --bin /w/bin/partex --formats /w/formats \
      --out /w/results --shard "$(shard)" ${CI_MODES:+--modes $CI_MODES} || status=$?
    out manuals
    exit $status
    ;;
  oracle)
    list=$1
    cp "$list" "$w/list.json"
    status=0
    box python3 /h/ci/manuals.py oracle --list /w/list.json --doc /usr/share/doc/texlive \
      --cache /o --new-cache /w/oracle-new --shard "$(shard)" || status=$?
    out oracle-log
    exit $status
    ;;
  discover)
    box python3 /h/ci/manuals.py discover --doc /usr/share/doc/texlive --out /w/results/candidates.json
    out .
    ;;
  torture)
    bin_in
    mkdir -p "$w/src" && tar -C "$w/src" -xf "$R/bin/src.tar"
    key=$(sha256sum "$w/src/Cargo.lock" | cut -c1-16)
    cp -a "$cache/cargo-$key/." "$w/cargo/"
    # (the pinned upstream test suites: fetched once per fetch script by
    # the gate's job, verified by git's commit hashes; never fetched here)
    key=$(sha256sum "$w/src/scripts/fetch-upstream.sh" | cut -c1-16)
    [ -d "$cache/upstream-$key" ] || { echo "no upstream-$key in the cache: run the gate once"; exit 3; }
    cp -a "$cache/upstream-$key" "$w/src/upstream"
    # (the container is the sandbox; Slurm's allocation the memory cap)
    printf '#!/usr/bin/env bash\n[ "${1:-}" = "--net" ] && shift\nexec "$@"\n' > "$w/src/scripts/sandbox"
    printf '#!/usr/bin/env bash\nexec "$@"\n' > "$w/src/scripts/heavy"
    chmod +x "$w/src/scripts/sandbox" "$w/src/scripts/heavy"
    status=0
    # (a file on the host the documents must not see: escape.py)
    mkdir -p "$CI/canary" && echo "host-only $(date -Is)" > "$CI/canary/host-only.txt"
    box python3 /h/ci/torture.py --src /w/src --bin /w/bin/partex --formats /w/formats --out /w/results \
      --l3build-cache /o --l3build-new /w/oracle-new --canary "$CI/canary/host-only.txt" "$@" || status=$?
    out torture
    exit $status
    ;;
  perf)
    bin_in
    mkdir -p "$w/src" && tar -C "$w/src" -xf "$R/bin/src.tar"
    key=$(sha256sum "$w/src/Cargo.lock" | cut -c1-16)
    cp -a "$cache/cargo-$key/." "$w/cargo/"
    # (the course, a private document: node-local only, never in results)
    if [ -d "$HOME/code/tmp/np-course/src" ]; then
      cp -a "$HOME/code/tmp/np-course/src" "$w/course"
    fi
    status=0
    box python3 /h/ci/perf.py --src /w/src --bin /w/bin/partex --formats /w/formats \
      --doc /usr/share/doc/texlive --out /w/results "$@" || status=$?
    out perf
    exit $status
    ;;
  score)
    # (trusted: reads the stages' JSON, runs no document; the CI
    # directory writable for the baseline, the history and the report)
    apptainer exec --containall --cleanenv --no-home --net --network none \
      --bind "$CI:/ci" --bind "$CI/harness:/h:ro" "$sif" \
      python3 /h/ci/score.py --run "/ci/runs/$run" --ci /ci --known /h/data/known-failing.txt "$@"
    ;;
  *) echo "unknown stage $stage"; exit 2 ;;
esac
