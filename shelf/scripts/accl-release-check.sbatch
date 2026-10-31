#!/bin/bash
# A release made and checked on mc, published nowhere (no Pages project,
# nothing deployed): build.py from a package database (default
# tl2026.6's), with the release's font index (FONT_INDEX, phitex-otf's
# otf-index over the release's own OpenType/TrueType fonts, relative to
# the tree), release.py on top of the releases made before (PRIOR, a copy
# of releases-v1/: the new release is numbered after them), then
# PhiTeX at COMMIT building documents with --shelf from the new release,
# served by serve.py on the node's loopback, each PDF compared byte for byte
# with the one TeX Live's own engines (the CI image's, TL_IMAGE) make of
# the same document from a texmf tree of the release's files alone, their
# formats made there by fmtutil: a file the release lacks fails TeX
# Live's build, so the release is never checked against itself. And every
# font of TeX Live's own pdftex.map must be in the release's. Any
# difference fails the job.
#
#   scp shelf/scripts/*.sbatch mcmgt01:code/flinner/shelf-scripts/
#   ssh mcmgt01 'export PATH=/opt/slurm/current/bin:$PATH; cd code/flinner/shelf-runs &&
#     sbatch -p mc -C cpu_amd_epyc_9754 -c 32 --mem=96G -t 04:00:00 \
#       ~/code/flinner/shelf-scripts/accl-release-check.sbatch COMMIT'
#
# COMMIT: a PhiTeX commit in ~/code/flinner/phitex.git (a branch's, or a
# snapshot PhiTeX's scripts/accl pushed as refs/wip/COMMIT): Shelf is its
# shelf/, the release check's PhiTeX the same commit's. Results in
# ~/code/flinner/shelf-runs/check-release-<job>/: summary.txt (the
# release's numbers, each document's comparison), build.log, the new
# release's release.json, the logs of the documents that differ.
#SBATCH -J shelf-check -o shelf-check-%j.out
set -euo pipefail
commit=$1 label=${2:-tl2026}
sif=$HOME/shared-cache/phitex/${ACCL_IMAGE:-phitex-2026-09-25-tools}.sif
tlsif=$HOME/shared-cache/phitex/${TL_IMAGE:-phitex-ci-2026-09-25}.sif
w=/tmp/$USER-$SLURM_JOB_ID
out=$HOME/code/flinner/shelf-runs/check-release-$SLURM_JOB_ID
arch=$HOME/shared-cache/shelf/archives
prior=${PRIOR:-$HOME/code/flinner/shelf-releases-v1}
traces=${TRACES:-$HOME/code/flinner/shelf-runs/full-75761}
tlpdb=${TLPDB:-$traces/texlive.tlpdb.xz}
jobs=$((${SLURM_CPUS_ON_NODE:-8} - 2))
mkdir -p "$out" "$arch" "$w/noresolve" "$w/ftree" "$w/results"
trap 'cp -a "$w/results/." "$out/" 2>/dev/null; rm -rf "$w"' EXIT
exec > >(tee "$out/job.log") 2>&1
echo "node $(hostname), $jobs jobs, PhiTeX $commit, start $(date +%T)"

git clone -q "$HOME/code/flinner/phitex.git" "$w/phitex"
git -C "$w/phitex" fetch -q origin "+refs/wip/$commit:refs/wip/$commit" 2>/dev/null || true
git -C "$w/phitex" -c advice.detachedHead=false checkout -q "$commit"
mkdir -p "$w/phitex/shelf/cache"
key=$(sha256sum "$w/phitex/Cargo.lock" | cut -c1-16)
[ -d "$HOME/shared-cache/phitex/cargo-$key" ] && cp -a "$HOME/shared-cache/phitex/cargo-$key" "$w/cargo"
cp "$tlpdb" "$w/phitex/shelf/cache/texlive.tlpdb.xz"
cp -a "$arch/." "$w/phitex/shelf/cache/"
# (the kept traces: a column whose inputs match is reused, else empty)
cp "$traces"/trace-*.json "$w/phitex/shelf/cache/" 2>/dev/null || true

command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
net() {
  apptainer exec --containall --bind "$w" --home "$w" --pwd "$w/phitex/shelf" "$sif" "$@"
}
box() {
  apptainer exec --containall --net --network none --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w" \
    --home "$w" --pwd "$w/phitex/shelf" --env CARGO_HOME="$w/cargo",CARGO_NET_OFFLINE=true "$sif" "$@"
}
if box getent hosts mirror.ctan.org >/dev/null 2>&1; then echo "FAILED: the box resolves names"; exit 76; fi
if [ ! -d "$w/cargo" ]; then
  net env CARGO_HOME="$w/cargo" sh -c "cd $w/phitex && cargo fetch -q"
  p=$HOME/shared-cache/phitex/cargo-$key.part-$SLURM_JOB_ID
  cp -a "$w/cargo" "$p" && { mv -T "$p" "$HOME/shared-cache/phitex/cargo-$key" 2>/dev/null || rm -rf "$p"; }
fi

# (1) the archives (the network: those of packages taken now and not before)
net env FETCH_ONLY=1 FONT_TREE="$w/ftree" TLPDB=cache/texlive.tlpdb.xz python3 build.py "$label"
cp -n "$w/phitex/shelf/cache/"*.tar.xz "$arch/" 2>/dev/null || true
echo "fetched $(date +%T)"

# (2) PhiTeX and the release's font index (fontconfig's order, in the box)
box sh -c "cd $w/phitex && cargo build -q --offline --profile fastdev -p phitex-cli --bin phitex -p phitex-otf --example otf-index"
cat > "$w/phitex/scripts/xetex/fonts.conf" <<CONF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$w/ftree/fonts/opentype</dir>
  <dir>$w/ftree/fonts/truetype</dir>
  <cachedir>$w/fc-cache</cachedir>
</fontconfig>
CONF
box sh -c "cd $w/phitex && target/fastdev/examples/otf-index $w/fontindex-rel.pxfi --relative $w/ftree"
echo "PhiTeX and the font index $(date +%T)"

# (3) the build and (4) the release, after the ones made before
status=0
box env TLPDB=cache/texlive.tlpdb.xz FONT_INDEX="$w/fontindex-rel.pxfi" PACK_JOBS="$jobs" \
  python3 build.py "$label" > "$w/results/build.log" 2>&1 || status=$?
tail -15 "$w/results/build.log"
[ "$status" = 0 ] || { echo "FAILED: build.py exit $status"; exit "$status"; }
cp -a "$prior/." "$w/phitex/shelf/releases-v1/"
before=$(ls "$w/phitex/shelf/releases-v1/$label/h" | wc -l)
old=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['release'])" "$w/phitex/shelf/releases-v1/$label/release.json")
box python3 release.py "$label" --releases releases-v1 2>&1 | tail -5
new=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['release'])" "$w/phitex/shelf/releases-v1/$label/release.json")
cp "$w/phitex/shelf/releases-v1/$label/release.json" "$w/results/"
after=$(ls "$w/phitex/shelf/releases-v1/$label/h" | wc -l)
echo "release $new (after $old): $after pack files kept, $((after - before)) new" | tee "$w/results/summary.txt"

# (5)-(6) the check (scripts/check.sh) in TeX Live's image, without the network
apptainer exec --containall --net --network none --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w" \
  --home "$w" --pwd "$w" --env ROOT="$w/phitex",REL="$w/phitex/shelf/releases-v1",LABEL="$label",NEW="$new",BIN="$w/phitex/target/fastdev/phitex",FTREE="$w/ftree",W="$w/check",OUT="$w/results" \
  "$tlsif" bash "$w/phitex/shelf/scripts/check.sh"; st=$?
echo "end $(date +%T)"
exit $st
