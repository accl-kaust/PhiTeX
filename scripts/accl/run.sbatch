#!/bin/bash
# Run a command on a commit of partex in the cluster image (on accl):
#
#   sbatch [slurm options] ~/code/flinner/partex-phitex-accl/run.sbatch <commit> <command> [args...]
#
# The commit is checked out from ~/code/flinner/partex-phitex.git into node-local
# /tmp, built (release binary and its pdflatex format), and the command run
# in the checkout; bench/results/ and the job's log are copied to
# ~/code/flinner/partex-phitex-runs/<commit>-<job>/.
#SBATCH -J partex-run -o partex-run-%j.out
set -euo pipefail
commit=$1
shift
sif=$HOME/shared-cache/partex/partex-2026-09-25.sif
cache=$HOME/shared-cache/partex
w=/tmp/$USER-$SLURM_JOB_ID
trap 'rm -rf "$w"' EXIT
mkdir -p "$w/cargo"
git clone -q "$HOME/code/flinner/partex-phitex.git" "$w/partex"
git -C "$w/partex" checkout -q "$commit"
cd "$w/partex"
# the upstream sources, fetched once per version of the fetch script
key=$(sha256sum scripts/fetch-upstream.sh | cut -c1-16)
run() {
  apptainer exec --containall --bind "$w" --pwd "$w/partex" \
    --home "$w" --env CARGO_HOME="$w/cargo" "$sif" "$@"
}
if [ ! -d "$cache/upstream-$key" ]; then
  run scripts/fetch-upstream.sh
  cp -a upstream "$cache/upstream-$key.part" && mv "$cache/upstream-$key.part" "$cache/upstream-$key"
else
  cp -a "$cache/upstream-$key" upstream
fi
echo "node $(hostname), commit $(git rev-parse --short=12 HEAD), $(nproc) CPUs"
run cargo build --release -p partex-cli -q
# (scripts/build-pdflatex-format.sh without its sandbox: the container is one)
shim=target/partex-shim
mkdir -p $shim/formats
# (the binary: phitex, or partex in a commit from before the rename)
rel() { if [ -x "$1/phitex" ]; then echo "$1/phitex"; else echo "$1/partex"; fi; }
ln -sfn "$(rel "$w/partex/target/release")" $shim/pdftex
ln -sfn "$(rel "$w/partex/target/release")" $shim/pdflatex
run bash -c "cd $shim/formats && SOURCE_DATE_EPOCH=1700000000 TZ=UTC ../pdftex -ini \
  -interaction=nonstopmode -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >pdflatex-build.log"
test -s $shim/formats/pdflatex.fmt
status=0
run "$@" || status=$?
out=$HOME/code/flinner/partex-phitex-runs/$(git rev-parse --short=12 HEAD)-$SLURM_JOB_ID
mkdir -p "$out"
cp -a bench/results/. "$out/" 2>/dev/null || true
exit $status
