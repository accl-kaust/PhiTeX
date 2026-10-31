#!/bin/bash
# Shelf's build with its dependency traces, on the accl or mc cluster, in
# PhiTeX's cluster image (TeX Live 2026, Rust; PhiTeX's scripts/accl;
# mc: ssh mcmgt01, sbatch -p mc -C cpu_amd_epyc_9754, the same paths under
# the home):
#
#   ssh mcmgt01 'export PATH=/opt/slurm/current/bin:$PATH; cd code/flinner/shelf-runs &&
#     sbatch -p mc -C cpu_amd_epyc_9754 -c 48 --mem=128G -t 04:00:00 \
#       ~/code/flinner/shelf-scripts/accl-trace.sbatch COMMIT [check [NAMES]|full]'
#
# (the script itself copied there first: scp shelf/scripts/*.sbatch mcmgt01:code/flinner/shelf-scripts/)
#
# COMMIT: a PhiTeX commit in ~/code/flinner/phitex.git, or a snapshot of a
# working tree PhiTeX's scripts/accl pushed there as refs/wip/COMMIT: Shelf
# is its shelf/, the tracer (shelf/trace) built on that commit's engine.
# `check` traces NAMES only (a.sty,b.cls; default: a dozen common ones;
# TRACE_NAMES: cache/trace-<engine>-check.json) and builds nothing;
# `full` traces every name and builds (build/<label>, MANIFEST.tsv).
# TLPDB=path (sbatch --export=ALL,TLPDB=...): build from that package
# database, not tlnet's current one.
#
# Steps: (1) with the network, build.py FETCH_ONLY=1 (the database and
# the archives not yet in ~/shared-cache/shelf/archives, kept there for
# the next job) with FONT_TREE (Shelf's OpenType/TrueType fonts at their
# texmf paths); (2) without it, the tracer (shelf/trace, cargo --offline on
# PhiTeX's crate cache for its Cargo.lock, ~/shared-cache/phitex) and the
# font index over that tree (phitex-otf's otf-index, its fonts.conf in the
# job's clone pointed at the tree; once with absolute paths for the
# tracer, once relative to the tree, served as the release's FONT_INDEX);
# (3) without it, build.py with TRACE (both engines) and FONT_INDEX. Everything runs in /tmp/$USER-$SLURM_JOB_ID; the
# results go to ~/code/flinner/shelf-runs/<mode>-<job>/: trace-*.json,
# trace-*.log, logs/ (the TeX log of each job that made no page), texlive.tlpdb.xz, fontindex.pxfi, the tracer's Cargo.lock,
# build.log and (full) build/ and MANIFEST.tsv.
#SBATCH -J shelf-trace -o shelf-trace-%j.out
set -euo pipefail
commit=$1 mode=${2:-full}
sif=$HOME/shared-cache/phitex/${ACCL_IMAGE:-phitex-2026-09-25-tools}.sif
w=/tmp/$USER-$SLURM_JOB_ID
out=$HOME/code/flinner/shelf-runs/$mode-$SLURM_JOB_ID
arch=$HOME/shared-cache/shelf/archives
jobs=$((${SLURM_CPUS_ON_NODE:-8} - 2))
mkdir -p "$out" "$arch" "$w/noresolve" "$w/ftree"
trap 'cp -a "$w/results/." "$out/" 2>/dev/null; rm -rf "$w"' EXIT
exec > >(tee "$out/job.log") 2>&1
echo "node $(hostname), $jobs jobs, mode $mode, start $(date +%T)"

git clone -q "$HOME/code/flinner/phitex.git" "$w/phitex"
git -C "$w/phitex" fetch -q origin "+refs/wip/$commit:refs/wip/$commit" 2>/dev/null || true
git -C "$w/phitex" -c advice.detachedHead=false checkout -q "$commit"
key=$(sha256sum "$w/phitex/Cargo.lock" | cut -c1-16)
# (the engine's crates: PhiTeX's cache for its Cargo.lock, else fetched
# below, with the network, and kept there)
[ -d "$HOME/shared-cache/phitex/cargo-$key" ] && cp -a "$HOME/shared-cache/phitex/cargo-$key" "$w/cargo"
[ -f "$w/phitex/shelf/trace/Cargo.lock" ] || cp "$w/phitex/Cargo.lock" "$w/phitex/shelf/trace/Cargo.lock"
mkdir -p "$w/phitex/shelf/cache"
cp -a "$arch/." "$w/phitex/shelf/cache/"
mkdir -p "$w/results"

# (mc's nodes have no Apptainer: the user-space one in the shared cache)
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
# (1) the database and the archives, the fonts' tree
net env FETCH_ONLY=1 FONT_TREE="$w/ftree" ${TLPDB:+TLPDB="$TLPDB"} python3 build.py
cp -n "$w/phitex/shelf/cache/"*.tar.xz "$arch/" 2>/dev/null || true
cp "$w/phitex/shelf/cache/texlive.tlpdb.xz" "$w/results/"
echo "fetched $(date +%T)"

# (2) the tracer and the font index
box cargo build --release --offline --manifest-path trace/Cargo.toml -q
box trace/target/release/shelf-trace --version | tee "$w/results/traced_by.txt"
cp "$w/phitex/shelf/trace/Cargo.lock" "$w/results/trace-Cargo.lock"
cat > "$w/phitex/scripts/xetex/fonts.conf" <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$w/ftree/fonts/opentype</dir>
  <dir>$w/ftree/fonts/truetype</dir>
  <cachedir>$w/fc-cache</cachedir>
</fontconfig>
EOF
box sh -c "cd $w/phitex && cargo run -q --offline -p phitex-otf --profile fastdev --example otf-index -- $w/fontindex.pxfi"
# (the release's own, served as FONT_INDEX: its paths relative to the tree)
box sh -c "cd $w/phitex && cargo run -q --offline -p phitex-otf --profile fastdev --example otf-index -- $w/fontindex-rel.pxfi --relative $w/ftree"
cp "$w/fontindex.pxfi" "$w/fontindex-rel.pxfi" "$w/results/"
echo "tracer and font index $(date +%T)"

# (3) the traces and the build
names=${3:-${CHECK_NAMES:-amsmath.sty,hyperref.sty,tikz.sty,geometry.sty,graphicx.sty,biblatex.sty,beamer.cls,fontspec.sty,unicode-math.sty,polyglossia.sty,xeCJK.sty,ctex.sty}}
status=0
box env SHELF_TRACE_LOGS="$w/results/logs" TLPDB=cache/texlive.tlpdb.xz TRACE=trace/target/release/shelf-trace TRACE_FONT_INDEX="$w/fontindex.pxfi" \
  FONT_INDEX="$w/fontindex-rel.pxfi" \
  TRACE_JOBS="$((jobs - 1))" PACK_JOBS="$jobs" $([ "$mode" = check ] && echo TRACE_NAMES="$names") \
  python3 build.py > "$w/results/build.log" 2>&1 || status=$?
tail -40 "$w/results/build.log"
cp "$w/phitex/shelf/cache/"trace-*.json "$w/phitex/shelf/cache/"trace-*.log "$w/results/" 2>/dev/null || true
if [ "$mode" = full ] && [ "$status" = 0 ]; then
  cp -a "$w/phitex/shelf/build" "$w/phitex/shelf/MANIFEST.tsv" "$w/results/"
fi
echo "end $(date +%T) status $status"
exit $status
