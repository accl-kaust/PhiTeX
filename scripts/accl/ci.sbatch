#!/bin/bash
# One stage of the TeX Live oracle (docs/ci.md), on accl, submitted by
# `accl ci-oracle` and `accl ci-discover`:
#
#   sbatch [slurm options] ci.sbatch <stage> <run> [args...]
#
# Stages:
#   oracle <list>         an array task: TeX Live's builds of its shard of
#                         <list> (the candidates or the pinned list) into
#                         the oracle cache
#   discover              the candidates (manuals.py discover) into <run>
#
# <run> is a directory under $CI/runs. Documents are untrusted code: each
# stage runs them in `box`, an Apptainer container with no network, no
# home, the environment cleared, the image read-only, and only the job's
# node-local scratch writable (bound at /w); the harness adds shell escape
# off, kpathsea's paranoid file access and per-run limits (scripts/ci/
# common.py). Results leave the container as files in /w/results, copied
# to <run> by this script when the stage ends; the oracle cache is read-only
# inside and filled from /w/oracle-new after.
#SBATCH -J phitex-ci -o phitex-ci-%A_%a.out
set -euo pipefail
stage=$1 run=$2
shift 2
CI=$HOME/code/flinner/phitex-ci
image=${CI_IMAGE:-phitex-ci-2026-09-25}
sif=$HOME/shared-cache/phitex/$image.sif
cache=$HOME/shared-cache/phitex
R=$CI/runs/$run
oracle=$CI/oracle/$image
w=/tmp/$USER-${SLURM_JOB_ID}-${SLURM_ARRAY_TASK_ID:-0}
mkdir -p "$w"/{results,oracle-new,home,texmf-var,cargo,noresolve} "$R" "$oracle"
trap 'rm -rf "$w"' EXIT
echo "stage $stage, run $run, node $(hostname), job ${SLURM_JOB_ID} task ${SLURM_ARRAY_TASK_ID:-}, image $image, $(date -Is)"

# box CMD...: CMD in the container, as described above (/h: the harness,
# read-only; /o: the oracle cache, read-only; /w: the scratch). Apptainer
# binds the host's /run/systemd/resolve, whose resolved socket answers DNS
# queries on some nodes even without a network: an empty directory hides it.
box() {
  apptainer exec --containall --cleanenv --no-home --net --network none \
    --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w:/w" --bind "$CI/harness:/h:ro" --bind "$oracle:/o:ro" --pwd /w \
    --env TEXMFVAR=/w/texmf-var,CARGO_HOME=/w/cargo,CARGO_NET_OFFLINE=true,PHITEX_SANDBOX=1,CI_WORK=/w \
    "$sif" "$@"
}
# (no name resolves inside: no channel out)
if box getent hosts export.arxiv.org >/dev/null 2>&1; then echo "FAILED: the box resolves names on $(hostname)"; exit 76; fi
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
case $stage in
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
  *) echo "unknown stage $stage"; exit 2 ;;
esac
