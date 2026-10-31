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
tlpdb=${TLPDB:-$HOME/code/flinner/shelf-runs/full-75761/texlive.tlpdb.xz}
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
cp "$HOME"/code/flinner/shelf-runs/full-75761/trace-*.json "$w/phitex/shelf/cache/" 2>/dev/null || true

command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
net() {
  apptainer exec --containall --bind "$w" --home "$w" --pwd "$w/phitex/shelf" "$sif" "$@"
}
tl() { # TeX Live's own, over the release's tree alone (and its web2c/: tcx), no network
  apptainer exec --containall --net --network none --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w" \
    --home "$w" --pwd "$w" --env TEXMFDIST="$w/tree/texmf-dist",TEXMFLOCAL="$w/empty",TEXMFSYSVAR="$w/tlvar",TEXMFSYSCONFIG="$w/empty",TEXMFVAR="$w/tlvar",TEXMFCONFIG="$w/empty",TEXMFHOME="$w/empty",WEB2C="$tlroot/texmf-dist/web2c",TEXFORMATS="$w/fmt",FONTCONFIG_FILE="$w/phitex/scripts/xetex/fonts.conf",SOURCE_DATE_EPOCH=0,FORCE_SOURCE_DATE=1 \
    "$tlsif" "$@"
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

# (5) a texmf tree of the release's files, with its ls-R
box python3 - "$w/phitex/shelf/releases-v1/$label" "$new" "$w/tree/texmf-dist" <<'PY' | tee -a "$w/results/summary.txt"
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

tlroot=$(apptainer exec --containall "$tlsif" kpsewhich -var-value=TEXMFROOT)
# (5b) every font of TeX Live's own pdftex.map (the CI image's install) in the release's
fails=0
apptainer exec --containall "$tlsif" sh -c 'cat "$(kpsewhich pdftex.map)"' > "$w/tl-pdftex.map"
python3 - "$w/tl-pdftex.map" "$w/tree/texmf-dist/fonts/map/pdftex/updmap/pdftex.map" > "$w/results/map-missing.txt" <<'NAMES'
import sys
names = lambda f: {l.split()[0] for l in open(f, encoding='latin-1') if l.strip() and l[0] not in '%#'}
tl, rel = names(sys.argv[1]), names(sys.argv[2])
for n in sorted(tl - rel): print(n)
NAMES
n=$(wc -l < "$w/results/map-missing.txt")
echo "pdftex.map: $n of TeX Live's fonts missing from the release$([ "$n" = 0 ] || echo ": $(head -8 "$w/results/map-missing.txt" | tr '\n' ' ')...")" | tee -a "$w/results/summary.txt"
[ "$n" = 0 ] || fails=$((fails + 1))

# (6) the documents: PhiTeX's fixtures, plain TeX for each engine, and
# scripts/release-docs/ (research templates); a document's engine is its
# line '% engine: X', else xelatex
mkdir -p "$w/docs" "$w/empty" "$w/runs" "$w/fmt" "$w/tlvar"
cp -a "$w/phitex/oracle/fixtures/xetex/docs/." "$w/docs/"
cp "$w/phitex/oracle/fixtures/xetex/cjk/"*.tex "$w/docs/" 2>/dev/null || true
cp -a "$w/phitex/shelf/scripts/release-docs/." "$w/docs/"
printf '%s\n' '% engine: xetex' '\font\lm="Latin Modern Roman" at 11pt \lm Hello, Ünïcödé — XeTeX. \font\hv="TeX Gyre Heros/B" at 11pt \hv Bold sans.' '\bye' > "$w/docs/plainxe.tex"
printf '%s\n' '% engine: pdftex' 'Hello {\bf plain} {\it \TeX} $x^2+\sqrt{y}$ and $$\sum_{k=1}^n k = {n(n+1)\over 2}.$$' '\bye' > "$w/docs/plainpdf.tex"
docs=$(cd "$w/docs" && for d in fontspec.tex lang.tex umath.tex pics.tex xecjk.tex ctex.tex plainxe.tex plainpdf.tex; do [ -f "$d" ] && echo "$d"; done
       ls "$w/phitex/shelf/scripts/release-docs/" 2>/dev/null | grep '\.tex$' | sort)
engine() { e=$(sed -n 's/^% engine: \([a-z]*\).*/\1/p' "$w/docs/$1" | head -1); echo "${e:-xelatex}"; }

# (6a) TeX Live's engines: their formats made by fmtutil (TeX Live's
# fmtutil.cnf) from the release's tree, then each document, run until its
# .aux settles
apptainer exec --containall "$tlsif" sh -c 'cat "$(kpsewhich fmtutil.cnf)"' > "$w/fmtutil.cnf"
for f in pdftex pdflatex xetex xelatex; do
  tl fmtutil-sys --cnffile "$w/fmtutil.cnf" --fmtdir "$w/fmt" --byfmt $f >> "$w/results/fmt.log" 2>&1 || true
done
find "$w/fmt" -mindepth 2 -name '*.fmt' -exec mv {} "$w/fmt/" \;
echo "TeX Live's formats from the release: $(cd "$w/fmt" && ls *.fmt 2>/dev/null | tr '\n' ' ')" | tee -a "$w/results/summary.txt"
for d in $docs; do
  n=${d%.tex} e=$(engine "$d")
  mkdir -p "$w/runs/r-$n"; cp -a "$w/docs/." "$w/runs/r-$n/"
  tl sh -c "cd $w/runs/r-$n && for i in 1 2 3 4; do cp $n.aux prev.aux 2>/dev/null; $e -interaction=nonstopmode -halt-on-error $d > tl.log 2>&1 || exit 1; cmp -s $n.aux prev.aux && break; done" || true
done

# (6b) PhiTeX from Shelf, each PDF against TeX Live's
cat > "$w/check.sh" <<CHECK
set -u
cd $w/runs
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
python3 $w/phitex/shelf/serve.py --root $w/phitex/shelf/releases-v1 --port 8124 > $w/serve.log 2>&1 &
sleep 1
bin=$w/phitex/target/fastdev/phitex
bad=0
for d in $(echo $docs); do
  n=\${d%.tex} e=\$(sed -n 's/^% engine: \([a-z]*\).*/\1/p' "$w/docs/\$d" | head -1); e=\${e:-xelatex}
  mkdir -p "s-\$n"; cp -a $w/docs/. "s-\$n/"
  (cd "s-\$n" && PHITEX_CACHE_DIR=$w/c-shelf PHITEX_SHELF_RELEASE=$new PHITEX_SHELF_URL=http://127.0.0.1:8124/$label/ \
    timeout 900 \$bin build --shelf --engine \$e -j 8 "\$d" > run.log 2>&1); a=\$?
  s="s-\$n/\$n.pdf" t="r-\$n/\$n.pdf"
  if [ ! -s "\$t" ]; then r="TeX Live's \$e made no PDF from the release (\$n-texlive.log)"; cp "r-\$n/tl.log" $w/results/\$n-texlive.log 2>/dev/null
  elif [ ! -s "\$s" ]; then r="PhiTeX made no PDF (exit \$a)"; cp "s-\$n/run.log" $w/results/\$n-shelf.log
  elif cmp -s "\$s" "\$t"; then echo "\$e \$d: identical"; continue
  else r="differs from TeX Live's at \$(cmp "\$s" "\$t" | sed 's/.*differ: //')"; cp "s-\$n/run.log" $w/results/\$n-shelf.log
  fi
  echo "\$e \$d: \$r"; bad=\$((bad + 1))
done
exit \$bad
CHECK
box bash "$w/check.sh" 2>&1 | tee -a "$w/results/summary.txt"; fails=$((fails + ${PIPESTATUS[0]}))
echo "packs fetched by --shelf: $(ls $w/c-shelf/shelf/$new/h 2>/dev/null | wc -l)" | tee -a "$w/results/summary.txt"
echo "end $(date +%T)"
[ "$fails" = 0 ] || { echo "FAILED: $fails (summary.txt)" | tee -a "$w/results/summary.txt"; exit 1; }
