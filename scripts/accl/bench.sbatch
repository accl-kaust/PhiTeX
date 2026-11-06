#!/bin/bash
# The proof page's benchmark on accl (scripts/ci/bench.py; bench.json):
#
#   sbatch [slurm options] bench.sbatch <run dir> <phitex binary> <steps> [edit docs]
#
# <run dir> (under $HOME, shared by every job of one benchmark) holds the
# harness (harness/: scripts/, ci/, bench/amsldoc, bench/para4000.tex),
# the Grail source (grail/, for the edit matrix) and the results (out/).
# <steps>, ':'-separated, in order: research and manuals (cold builds,
# each document claimed by one job at a time: several jobs on several
# nodes share the list), edits (the edit matrix's documents, one at a time).
# BENCH_ARGS: more bench.py cold options (--ids a,b --runs 1: a smoke run).
# Everything in the CI image (no network, the environment cleared, only
# the job's scratch and out/ writable), on the job's whole cores: one
# hardware thread of each, its sibling left idle.
#SBATCH -J phitex-bench -o phitex-bench-%j.out
set -euo pipefail
command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
R=$1 bin=$2 steps=$3 edocs=${4:-amsldoc:tikzcd:beamer:biblatex:para4000:pgfplots:grail:circuitikz}
image=${CI_IMAGE:-phitex-ci-2026-09-25}
sif=$HOME/shared-cache/phitex/$image.sif
w=/tmp/$USER-${SLURM_JOB_ID}
free=$(df -Pk /tmp | awk 'NR==2{print $4}'); [ "$free" -ge 20971520 ] || { echo "FAILED: /tmp on $(hostname) has $((free/1048576)) GB free"; exit 75; }
mkdir -p "$w"/{home,texmf-var,noresolve,b} "$R/out"
cp "$bin" "$w/b/phitex"
trap 'rm -rf "$w"' EXIT
cores=$(python3 -I -c '
import os
aff = os.sched_getaffinity(0)
out = []
for c in sorted(aff):
    try:
        sib = open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip()
    except OSError:
        out.append(c); continue
    ids = set()
    for part in sib.split(","):
        a, _, b = part.partition("-")
        ids.update(range(int(a), int(b or a) + 1))
    if ids <= aff and c == min(ids):
        out.append(c)
print(",".join(map(str, out)))')
host=$(hostname -s)
echo "bench $steps, node $host, job $SLURM_JOB_ID, cpus $(taskset -pc $$ | sed 's/.*: //'), whole cores $cores, bin $(md5sum < "$bin" | cut -c1-12), $(date -Is)"
{ lscpu; echo; free -g; echo; nproc --all; } > "$R/out/machine-$host-$SLURM_JOB_ID.txt"
box() {
  taskset -c "$cores" apptainer exec --containall --cleanenv --no-home --net --network none \
    --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w:/w" --bind "$R/harness:/h:ro" \
    --bind "$R/out:/r" --bind "$w/b:/b:ro" ${grail:+--bind "$R/grail:/g:ro"} --pwd /w \
    --env TEXMFVAR=/w/texmf-var,PHITEX_SANDBOX=1,CI_WORK=/w \
    "$sif" "$@"
}
grail=; [ -d "$R/grail" ] && grail=1
for step in ${steps//:/ }; do
  case $step in
    edits) box python3 -I /h/scripts/ci/bench.py edits --bin /b/phitex --docs "$edocs" --repo /h --grail /g --out /r/edits/$host ;;
    research) box python3 -I /h/scripts/ci/bench.py cold --list /h/ci/research.json --corpus research --doc /h/ci/research --bin /b/phitex --out /r ${BENCH_ARGS:-} ;;
    manuals) box python3 -I /h/scripts/ci/bench.py cold --list /h/ci/manuals.json --corpus manuals --doc /usr/share/doc/texlive --bin /b/phitex --out /r ${BENCH_ARGS:-} ;;
    *) echo "FAILED: no step $step"; exit 2 ;;
  esac
done
echo "end $(date -Is)"
