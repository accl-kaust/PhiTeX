#!/bin/bash
# Build the phitex Apptainer image into ~/shared-cache/phitex (on accl:
# sbatch --qos=build4 ~/code/flinner/phitex-accl/build-image.sbatch <name>;
# on mc: sbatch -p mc -C cpu_amd_epyc_9754 ... the same).
#SBATCH -p build -n 1 -c 16 --mem=32G -t 02:00:00 -J phitex-image -o phitex-image-%j.out
set -euo pipefail
# (mc's nodes have no apptainer: the user-space one, apptainer-user.sbatch)
command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
name=${1:?image name, e.g. phitex-2026-09-25}
def="$(realpath "${2:-$HOME/code/flinner/phitex-accl/phitex.def}")"
w=/tmp/$USER-$SLURM_JOB_ID
mkdir -p "$w/tmp" "$w/cache"
trap 'rm -rf "$w"' EXIT
export APPTAINER_TMPDIR=$w/tmp APPTAINER_CACHEDIR=$w/cache
cd "$w"
# (mc: no subuid range, so Apptainer's fakeroot is a root-mapped user
# namespace plus the fakeroot command, whose faked died under pacman's
# keyring check and left gpg waiting on its semaphore for good (job
# 75697); BUILD_FLAGS=--ignore-fakeroot-command runs %post as root of the
# user namespace alone)
apptainer build --fakeroot ${BUILD_FLAGS:-} phitex.sif "$def"
apptainer exec phitex.sif sh -c 'pacman -Q; pdflatex --version | head -1; rustc --version; valgrind --version; qpdf --version | head -1; node --version 2>/dev/null; rustup target list --installed 2>/dev/null' > versions.txt
out=$HOME/shared-cache/phitex
mkdir -p "$out"
cp phitex.sif "$out/$name.sif.part" && mv "$out/$name.sif.part" "$out/$name.sif"
cp versions.txt "$out/$name.versions.txt"
cat versions.txt | grep -E '^(texlive-bin|texlive-basic|zlib|libpng|poppler) |pdfTeX|rustc'
