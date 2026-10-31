#!/bin/bash
# The corpus sweep on accl (submitted by scripts/accl/corpus-sweep):
#
#   sbatch [slurm options] corpus-sweep.sbatch <run> <phitex binary> <jobs> <par> <timeout> [ids]
#
# ONE job: every pinned pdfLaTeX manual built cold by <phitex binary> at
# -j 1 and -j 8 (scripts/ci/sweep.py), against the oracle cache, in the
# CI image (the oracle's container: no network, no home, the environment
# cleared, only the job's scratch writable at /w; the harness read-only
# at /h, the binary at /b, the oracle cache at /o). Results stream to
# $CI/runs/<run>/docs as they come (a job killed at its limit keeps them).
#SBATCH -J phitex-sweep -o phitex-sweep-%j.out
set -euo pipefail
# (mc's nodes have no apptainer: the user-space one, apptainer-user.sbatch)
command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
run=$1 bin=$2 jobs=$3 par=$4 tmo=$5 ids=${6:-}
CI=$HOME/code/flinner/phitex-ci
image=${CI_IMAGE:-phitex-ci-2026-09-25}
sif=$HOME/shared-cache/phitex/$image.sif
R=$CI/runs/$run
oracle=$CI/oracle/$image
w=/tmp/$USER-${SLURM_JOB_ID}
free=$(df -Pk /tmp | awk 'NR==2{print $4}'); [ "$free" -ge 20971520 ] || { echo "FAILED: /tmp on $(hostname) has $((free/1048576)) GB free"; exit 75; }
mkdir -p "$w"/{results,home,texmf-var,noresolve,b} "$R/docs"
cp "$bin" "$w/b/phitex"
trap 'cp -a "$w/results/." "$R/docs/" 2>/dev/null; rm -rf "$w"' EXIT
echo "sweep $run, node $(hostname), job ${SLURM_JOB_ID}, $SLURM_CPUS_ON_NODE CPUs, image $image, bin $bin $(md5sum < "$bin" | cut -c1-12), $(date -Is)"
box() {
  apptainer exec --containall --cleanenv --no-home --net --network none \
    --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w:/w" --bind "$R/harness:/h:ro" \
    --bind "$oracle:/o:ro" --bind "$w/b:/b:ro" --pwd /w \
    --env TEXMFVAR=/w/texmf-var,PHITEX_SANDBOX=1,CI_WORK=/w \
    "$sif" "$@"
}
( while sleep 60; do cp -a "$w/results/." "$R/docs/" 2>/dev/null; done ) &
if box getent hosts export.arxiv.org >/dev/null 2>&1; then echo "FAILED: the box resolves names on $(hostname)"; exit 76; fi
box sh -c 'command -v qpdf pdftotext pdfinfo; true'
box python3 /h/ci/sweep.py run --list /h/data/manuals.json --doc /usr/share/doc/texlive \
  --cache /o --bin /b/phitex --out /w/results ${ids:+--ids "$ids"} \
  --jobs "$jobs" --par "$par" --timeout "$tmo"
box python3 /h/ci/sweep.py table /w/results --json /w/results/table.json > "$w/results/table.txt" 2>&1 || true
echo "end $(date -Is)"
