#!/bin/bash
# Build the partex Apptainer image into ~/shared-cache/partex (on accl:
# sbatch --qos=build4 ~/code/flinner/partex-phitex-accl/build-image.sbatch <name>).
#SBATCH -p build -n 1 -c 16 --mem=32G -t 02:00:00 -J partex-image -o partex-image-%j.out
set -euo pipefail
name=${1:?image name, e.g. partex-2026-09-25}
def="$(realpath "${2:-$HOME/code/flinner/partex-phitex-accl/partex.def}")"
w=/tmp/$USER-$SLURM_JOB_ID
mkdir -p "$w/tmp" "$w/cache"
trap 'rm -rf "$w"' EXIT
export APPTAINER_TMPDIR=$w/tmp APPTAINER_CACHEDIR=$w/cache
cd "$w"
apptainer build --fakeroot partex.sif "$def"
apptainer exec partex.sif sh -c 'pacman -Q; pdflatex --version | head -1; rustc --version; valgrind --version; qpdf --version | head -1' > versions.txt
out=$HOME/shared-cache/partex
mkdir -p "$out"
cp partex.sif "$out/$name.sif.part" && mv "$out/$name.sif.part" "$out/$name.sif"
cp versions.txt "$out/$name.versions.txt"
cat versions.txt | grep -E '^(texlive-bin|texlive-basic|zlib|libpng|poppler) |pdfTeX|rustc'
