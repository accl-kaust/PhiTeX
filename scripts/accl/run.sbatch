#!/bin/bash
# Run a command on a commit of phitex in the cluster image (on accl):
#
#   sbatch [slurm options] ~/code/flinner/phitex-accl/run.sbatch <commit> <command> [args...]
#
# The commit is checked out from ~/code/flinner/phitex.git into node-local
# /tmp, built (release binary and its pdflatex format), and the command run
# in the checkout; bench/results/ and the job's log are copied to
# ~/code/flinner/phitex-runs/<commit>-<job>/.
#SBATCH -J phitex-run -o phitex-run-%j.out
set -euo pipefail
commit=$1
shift
sif=$HOME/shared-cache/phitex/phitex-2026-09-25.sif
cache=$HOME/shared-cache/phitex
w=/tmp/$USER-$SLURM_JOB_ID
trap 'rm -rf "$w"' EXIT
mkdir -p "$w/cargo"
git clone -q "$HOME/code/flinner/phitex.git" "$w/phitex"
git -C "$w/phitex" checkout -q "$commit"
cd "$w/phitex"
# the upstream sources, fetched once per version of the fetch script
key=$(sha256sum scripts/fetch-upstream.sh | cut -c1-16)
run() {
  apptainer exec --containall --bind "$w" --pwd "$w/phitex" \
    --home "$w" --env CARGO_HOME="$w/cargo" "$sif" "$@"
}
if [ ! -d "$cache/upstream-$key" ]; then
  run scripts/fetch-upstream.sh
  cp -a upstream "$cache/upstream-$key.part" && mv "$cache/upstream-$key.part" "$cache/upstream-$key"
else
  cp -a "$cache/upstream-$key" upstream
fi
echo "node $(hostname), commit $(git rev-parse --short=12 HEAD), $(nproc) CPUs"
run cargo build --release -p phitex-cli -q
# (scripts/build-pdflatex-format.sh without its sandbox: the container is one)
shim=target/phitex-shim
mkdir -p $shim/formats
# (the binary: phitex, or phitex in a commit from before the rename)
rel() { if [ -x "$1/phitex" ]; then echo "$1/phitex"; else echo "$1/phitex"; fi; }
ln -sfn "$(rel "$w/phitex/target/release")" $shim/pdftex
ln -sfn "$(rel "$w/phitex/target/release")" $shim/pdflatex
run bash -c "cd $shim/formats && SOURCE_DATE_EPOCH=1700000000 TZ=UTC ../pdftex -ini \
  -interaction=nonstopmode -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >pdflatex-build.log"
test -s $shim/formats/pdflatex.fmt
status=0
run "$@" || status=$?
out=$HOME/code/flinner/phitex-runs/$(git rev-parse --short=12 HEAD)-$SLURM_JOB_ID
mkdir -p "$out"
cp -a bench/results/. "$out/" 2>/dev/null || true
exit $status
