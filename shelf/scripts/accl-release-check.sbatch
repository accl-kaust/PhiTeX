#!/bin/bash
# A release made and checked on mc, published nowhere (no Pages project,
# nothing deployed): build.py from a package database (default
# tl2026.6's), with the release's font index (FONT_INDEX, phitex-otf's
# otf-index over the release's own OpenType/TrueType fonts, relative to
# the tree), release.py on top of the releases made before (PRIOR, a copy
# of releases-v4/: the new release is numbered after them), then
# PhiTeX at COMMIT building documents twice: with --shelf from the new
# release, served by serve.py on the node's loopback, and with kpathsea
# over a texmf tree made of the same release's files: each pair's PDFs
# compared byte for byte.
#
#   rsync -a --delete --exclude-from=scripts/accl-exclude ./ mcmgt01:code/flinner/shelf-accl-check/
#   ssh mcmgt01 'export PATH=/opt/slurm/current/bin:$PATH; cd code/flinner/shelf-runs &&
#     sbatch -p mc -C cpu_amd_epyc_9754 -c 32 --mem=96G -t 04:00:00 \
#       ~/code/flinner/shelf-accl-check/scripts/accl-release-check.sbatch ~/code/flinner/shelf-accl-check COMMIT'
#
# COMMIT: a PhiTeX commit in ~/code/flinner/phitex.git (a branch's, or a
# snapshot PhiTeX's scripts/accl pushed as refs/wip/COMMIT). Results in
# ~/code/flinner/shelf-runs/check-release-<job>/: summary.txt (the
# release's numbers, each document's comparison), build.log, the new
# release's release.json, the logs of the documents that differ.
#SBATCH -J shelf-check -o shelf-check-%j.out
set -euo pipefail
src=$1 commit=$2 label=${3:-tl2026}
sif=$HOME/shared-cache/phitex/${ACCL_IMAGE:-phitex-2026-09-25-tools}.sif
w=/tmp/$USER-$SLURM_JOB_ID
out=$HOME/code/flinner/shelf-runs/check-release-$SLURM_JOB_ID
arch=$HOME/shared-cache/shelf/archives
prior=${PRIOR:-$HOME/code/flinner/shelf-releases-v4}
tlpdb=${TLPDB:-$HOME/code/flinner/shelf-runs/full-75761/texlive.tlpdb.xz}
jobs=$((${SLURM_CPUS_ON_NODE:-8} - 2))
mkdir -p "$w/shelf/cache" "$out" "$arch" "$w/noresolve" "$w/ftree" "$w/results"
trap 'cp -a "$w/results/." "$out/" 2>/dev/null; rm -rf "$w"' EXIT
exec > >(tee "$out/job.log") 2>&1
echo "node $(hostname), $jobs jobs, PhiTeX $commit, start $(date +%T)"

cp -a "$src/." "$w/shelf/"
git clone -q "$HOME/code/flinner/phitex.git" "$w/phitex"
git -C "$w/phitex" fetch -q origin "+refs/wip/$commit:refs/wip/$commit" 2>/dev/null || true
git -C "$w/phitex" -c advice.detachedHead=false checkout -q "$commit"
key=$(sha256sum "$w/phitex/Cargo.lock" | cut -c1-16)
[ -d "$HOME/shared-cache/phitex/cargo-$key" ] && cp -a "$HOME/shared-cache/phitex/cargo-$key" "$w/cargo"
cp "$tlpdb" "$w/shelf/cache/texlive.tlpdb.xz"
cp -a "$arch/." "$w/shelf/cache/"
# (the kept traces: a column whose inputs match is reused, else empty)
cp "$HOME"/code/flinner/shelf-runs/full-75761/trace-*.json "$w/shelf/cache/" 2>/dev/null || true

command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
net() {
  apptainer exec --containall --bind "$w" --home "$w" --pwd "$w/shelf" "$sif" "$@"
}
box() {
  apptainer exec --containall --net --network none --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w" \
    --home "$w" --pwd "$w/shelf" --env CARGO_HOME="$w/cargo",CARGO_NET_OFFLINE=true "$sif" "$@"
}
if box getent hosts mirror.ctan.org >/dev/null 2>&1; then echo "FAILED: the box resolves names"; exit 76; fi
if [ ! -d "$w/cargo" ]; then
  net env CARGO_HOME="$w/cargo" sh -c "cd $w/phitex && cargo fetch -q"
  p=$HOME/shared-cache/phitex/cargo-$key.part-$SLURM_JOB_ID
  cp -a "$w/cargo" "$p" && { mv -T "$p" "$HOME/shared-cache/phitex/cargo-$key" 2>/dev/null || rm -rf "$p"; }
fi

# (1) the archives (the network: those of packages taken now and not before)
net env FETCH_ONLY=1 FONT_TREE="$w/ftree" TLPDB=cache/texlive.tlpdb.xz python3 build.py "$label"
cp -n "$w/shelf/cache/"*.tar.xz "$arch/" 2>/dev/null || true
echo "fetched $(date +%T)"

# (2) PhiTeX and the release's font index (fontconfig's order, in the box)
box sh -c "cd $w/phitex && cargo build -q --offline --profile fastdev -p phitex-cli -p phitex-otf --example otf-index"
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
cp -a "$prior/." "$w/shelf/releases-v4/"
before=$(ls "$w/shelf/releases-v4/$label/h" | wc -l)
old=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['release'])" "$w/shelf/releases-v4/$label/release.json")
box python3 release.py "$label" --releases releases-v4 2>&1 | tail -5
new=$(python3 -c "import json,sys;print(json.load(open(sys.argv[1]))['release'])" "$w/shelf/releases-v4/$label/release.json")
cp "$w/shelf/releases-v4/$label/release.json" "$w/results/"
after=$(ls "$w/shelf/releases-v4/$label/h" | wc -l)
echo "release $new (after $old): $after pack files kept, $((after - before)) new" | tee "$w/results/summary.txt"

# (5) a texmf tree of the release's files, with its ls-R
box python3 - "$w/shelf/releases-v4/$label" "$new" "$w/tree/texmf-dist" <<'PY' | tee -a "$w/results/summary.txt"
import sys, os, gzip, struct
rel, name, tree = sys.argv[1:]
meta = __import__('json').load(open(f'{rel}/release-{name}.json'))
rows = gzip.decompress(open(f"{rel}/{meta['index']}", 'rb').read()).decode().splitlines()
table = gzip.decompress(open(f"{rel}/{meta['pack_table']}", 'rb').read()).decode().splitlines()
packs = {r.split('\t')[1] for r in rows if '\t' in r}
size = sum(int(t.split('\t')[2]) for t in table if t)
dirs = {}
for p in sorted(packs):
    b = gzip.decompress(open(f'{rel}/h/{p}.pack', 'rb').read())
    n = struct.unpack('<I', b[:4])[0]; at = 4
    for _ in range(n):
        k = struct.unpack('<I', b[at:at+4])[0]; path = b[at+4:at+4+k].decode(); at += 4 + k
        l = struct.unpack('<I', b[at:at+4])[0]; data = b[at+4:at+4+l]; at += 4 + l
        f = os.path.join(tree, path); os.makedirs(os.path.dirname(f), exist_ok=True)
        open(f, 'wb').write(data)
        d, base = os.path.split(path); dirs.setdefault(d, []).append(base)
kids = {}
for d in list(dirs):
    parts = d.split('/')
    for i in range(len(parts)):
        kids.setdefault('/'.join(parts[:i]), set()).add(parts[i])
with open(os.path.join(tree, 'ls-R'), 'w') as o:
    o.write('% ls-R -- filename database for kpathsea; do not change this line.\n')
    for d in sorted(set(dirs) | set(kids)):
        o.write(f"\n./{d}:\n" if d else "\n./:\n")
        for e in sorted(set(dirs.get(d, [])) | kids.get(d, set())):
            o.write(e + '\n')
print(f"{name}: {len(rows)} paths in {len(packs)} packs, {size / 1e6:.0f} MB as served")
PY

# (6) the documents, each from Shelf and from the tree
mkdir -p "$w/docs" "$w/empty"
cp -a "$w/phitex/oracle/fixtures/xetex/docs/." "$w/docs/"
cp "$w/phitex/oracle/fixtures/xetex/cjk/"*.tex "$w/docs/" 2>/dev/null || true
printf '%s\n' '\font\lm="Latin Modern Roman" at 11pt \lm Hello, Ünïcödé — XeTeX. \font\hv="TeX Gyre Heros/B" at 11pt \hv Bold sans.' '\bye' > "$w/docs/plainxe.tex"
printf '%s\n' 'Hello {\bf plain} {\it \TeX} $x^2+\sqrt{y}$ and $$\sum_{k=1}^n k = {n(n+1)\over 2}.$$' '\bye' > "$w/docs/plainpdf.tex"
cat > "$w/check.sh" <<CHECK
set -u
cd $w/docs
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
python3 $w/shelf/serve.py --root $w/shelf/releases-v4 --port 8124 > $w/serve.log 2>&1 &
sleep 1
bin=$w/phitex/target/fastdev/phitex
one() { # ENGINE DOC
  local e=\$1 d=\$2 n=\${2%.tex}
  mkdir -p "s-\$n" "t-\$n"; cp -a $w/docs/. "s-\$n/"; cp -a $w/docs/. "t-\$n/"
  (cd "s-\$n" && PHITEX_CACHE_DIR=$w/c-shelf PHITEX_SHELF_RELEASE=$new PHITEX_SHELF_URL=http://127.0.0.1:8124/$label/ \
    timeout 900 \$bin build --shelf --engine \$e -j 8 "\$d" > run.log 2>&1); local a=\$?
  (cd "t-\$n" && PHITEX_CACHE_DIR=$w/c-tree TEXMFDIST=$w/tree/texmf-dist TEXMFLOCAL=$w/empty TEXMFSYSVAR=$w/empty \
    TEXMFSYSCONFIG=$w/empty TEXMFVAR=$w/empty TEXMFCONFIG=$w/empty TEXMFHOME=$w/empty \
    timeout 900 \$bin build --engine \$e -j 8 "\$d" > run.log 2>&1); local b=\$?
  if [ -s "s-\$n/\$n.pdf" ] && cmp -s "s-\$n/\$n.pdf" "t-\$n/\$n.pdf"; then r=identical
  elif [ -s "s-\$n/\$n.pdf" ] && [ -s "t-\$n/\$n.pdf" ]; then r="differ at \$(cmp "s-\$n/\$n.pdf" "t-\$n/\$n.pdf" | sed 's/.*differ: //')"
    cp "s-\$n/run.log" $w/results/\$n-shelf.log; cp "t-\$n/run.log" $w/results/\$n-tree.log
  else r="no PDF (shelf exit \$a, tree exit \$b)"; cp "s-\$n/run.log" $w/results/\$n-shelf.log 2>/dev/null; cp "t-\$n/run.log" $w/results/\$n-tree.log 2>/dev/null
  fi
  echo "\$e \$d: \$r"
}
for d in fontspec.tex lang.tex umath.tex pics.tex xecjk.tex ctex.tex; do [ -f "\$d" ] && one xelatex "\$d"; done
one xetex plainxe.tex
one pdftex plainpdf.tex
CHECK
box bash "$w/check.sh" 2>&1 | tee -a "$w/results/summary.txt"
echo "packs fetched by --shelf: $(ls $w/c-shelf/shelf/$new/h 2>/dev/null | wc -l)" | tee -a "$w/results/summary.txt"
echo "end $(date +%T)"
