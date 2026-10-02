#!/bin/bash
# A/B timing of two commits on one node, runs alternating:
#   sbatch [slurm options] ab.sbatch <commit-a> <commit-b> [runs]
# Plain runs of the PGF subset (settled aux from pdflatex) and the article.
#SBATCH -J partex-ab -o partex-ab-%j.out
set -euo pipefail
a=$1 b=$2 n=${3:-5}
sif=$HOME/shared-cache/partex/partex-2026-09-25.sif
cache=$HOME/shared-cache/partex
w=/tmp/$USER-$SLURM_JOB_ID
trap 'rm -rf "$w"' EXIT
mkdir -p "$w/cargo"
run() { apptainer exec --containall --bind "$w" --home "$w" --env CARGO_HOME="$w/cargo" "$sif" "$@"; }
for c in $a $b; do
  git clone -q "$HOME/code/flinner/partex-phitex.git" "$w/$c"
  git -C "$w/$c" checkout -q "$c"
  run bash -c "cd $w/$c && cargo build --release -p partex-cli -q"
done
src=$w/$a
key=$(sha256sum "$src/scripts/fetch-upstream.sh" | cut -c1-16)
cp -a "$cache/upstream-$key/pgf" "$w/pgf"
shim=$w/shim; mkdir -p $shim/formats; ln -sfn "$w/$a/target/release/partex" $shim/pdftex
run bash -c "cd $shim/formats && SOURCE_DATE_EPOCH=1700000000 TZ=UTC ../pdftex -ini -interaction=nonstopmode -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null"
env=(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC)
# settle the PGF subset's aux with pdflatex
o=$w/o; mkdir -p $o
for i in 1 2 3; do run env "${env[@]}" TEXINPUTS=$o:.: bash -c "cd $w/pgf/doc/generic/pgf && pdflatex -interaction=batchmode -output-directory=$o $src/bench/inputs/pgfsub.tex >/dev/null" || true; done
echo "node $(hostname), $(nproc) CPUs, $n alternating runs"
for i in $(seq $n); do
  for c in $a $b; do
    d=$w/run-$c; rm -rf $d; mkdir -p $d; cp $o/*.aux $o/*.out $o/*.toc $d/
    t=$( { run env "${env[@]}" TEXFORMATS=$shim/formats: TEXINPUTS=$d:.: bash -c "cd $w/pgf/doc/generic/pgf && /usr/bin/time -f %e $w/$c/target/release/partex -engine=pdftex -fmt=pdflatex -interaction=batchmode -output-directory=$d $src/bench/inputs/pgfsub.tex >/dev/null" ; } 2>&1 | tail -1)
    echo "pgfsub $c $t"
  done
done
