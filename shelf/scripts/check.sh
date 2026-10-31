#!/bin/bash
# Shelf's release check: a release against TeX Live. The release's files
# unpacked into a texmf tree of their own (with its ls-R); every font of
# TeX Live's own pdftex.map must be in the release's; TeX Live's own
# engines (this environment's: kpsewhich, fmtutil-sys, pdftex, xetex)
# make their formats from that tree alone and build each document from
# it; PhiTeX (BIN) builds each with --shelf from the release, served on
# the loopback by serve.py; each PDF must be TeX Live's byte for byte. A
# file the release lacks fails TeX Live's build, so the release is never
# checked against itself. Exits non-zero on any difference. Run in the CI
# image (TeX Live, Python): by .github/workflows/shelf.yml
# (scripts/release.sh) and on mc (scripts/accl-release-check.sbatch).
#
#   ROOT=<PhiTeX checkout> REL=<releases dir> LABEL=tl2026 NEW=tl2026.N \
#   BIN=<phitex> FTREE=<the release's fonts tree> W=<scratch> OUT=<results> \
#     shelf/scripts/check.sh
set -u
root=$ROOT rel=$REL label=$LABEL new=$NEW bin=$BIN w=$W out=$OUT
mkdir -p "$w" "$out"
tl() { # TeX Live's own, over the release's tree alone (and its web2c/: tcx)
  env TEXMFDIST="$w/tree/texmf-dist" TEXMFLOCAL="$w/empty" TEXMFSYSVAR="$w/tlvar" TEXMFSYSCONFIG="$w/empty" \
    TEXMFVAR="$w/tlvar" TEXMFCONFIG="$w/empty" TEXMFHOME="$w/empty" WEB2C="$tlroot/texmf-dist/web2c" \
    TEXFORMATS="$w/fmt" FONTCONFIG_FILE="$w/fonts.conf" SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 "$@"
}
cat > "$w/fonts.conf" <<CONF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$FTREE/fonts/opentype</dir>
  <dir>$FTREE/fonts/truetype</dir>
  <cachedir>$w/fc-cache</cachedir>
</fontconfig>
CONF

# (5) a texmf tree of the release's files, with its ls-R
python3 - "$rel/$label" "$new" "$w/tree/texmf-dist" <<'PY' | tee -a "$out/summary.txt"
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

tlroot=$(kpsewhich -var-value=TEXMFROOT)
# (5b) every font of TeX Live's own pdftex.map (the CI image's install) in the release's
fails=0
sh -c 'cat "$(kpsewhich pdftex.map)"' > "$w/tl-pdftex.map"
python3 - "$w/tl-pdftex.map" "$w/tree/texmf-dist/fonts/map/pdftex/updmap/pdftex.map" > "$out/map-missing.txt" <<'NAMES'
import sys
names = lambda f: {l.split()[0] for l in open(f, encoding='latin-1') if l.strip() and l[0] not in '%#'}
tl, rel = names(sys.argv[1]), names(sys.argv[2])
for n in sorted(tl - rel): print(n)
NAMES
n=$(wc -l < "$out/map-missing.txt")
echo "pdftex.map: $n of TeX Live's fonts missing from the release$([ "$n" = 0 ] || echo ": $(head -8 "$out/map-missing.txt" | tr '\n' ' ')...")" | tee -a "$out/summary.txt"
[ "$n" = 0 ] || fails=$((fails + 1))

# (6) the documents: PhiTeX's fixtures, plain TeX for each engine, and
# scripts/release-docs/ (research templates); a document's engine is its
# line '% engine: X', else xelatex
mkdir -p "$w/docs" "$w/empty" "$w/runs" "$w/fmt" "$w/tlvar"
cp -a "$root/oracle/fixtures/xetex/docs/." "$w/docs/"
cp "$root/oracle/fixtures/xetex/cjk/"*.tex "$w/docs/" 2>/dev/null || true
cp -a "$root/shelf/scripts/release-docs/." "$w/docs/"
printf '%s\n' '% engine: xetex' '\font\lm="Latin Modern Roman" at 11pt \lm Hello, Ünïcödé — XeTeX. \font\hv="TeX Gyre Heros/B" at 11pt \hv Bold sans.' '\bye' > "$w/docs/plainxe.tex"
printf '%s\n' '% engine: pdftex' 'Hello {\bf plain} {\it \TeX} $x^2+\sqrt{y}$ and $$\sum_{k=1}^n k = {n(n+1)\over 2}.$$' '\bye' > "$w/docs/plainpdf.tex"
docs=$(cd "$w/docs" && for d in fontspec.tex lang.tex umath.tex pics.tex xecjk.tex ctex.tex plainxe.tex plainpdf.tex; do [ -f "$d" ] && echo "$d"; done
       ls "$root/shelf/scripts/release-docs/" 2>/dev/null | grep '\.tex$' | sort)
engine() { e=$(sed -n 's/^% engine: \([a-z]*\).*/\1/p' "$w/docs/$1" | head -1); echo "${e:-xelatex}"; }

# (6a) TeX Live's engines: their formats made by fmtutil (TeX Live's
# fmtutil.cnf) from the release's tree, then each document, run until its
# .aux settles
sh -c 'cat "$(kpsewhich fmtutil.cnf)"' > "$w/fmtutil.cnf"
for f in pdftex pdflatex xetex xelatex; do
  tl fmtutil-sys --cnffile "$w/fmtutil.cnf" --fmtdir "$w/fmt" --byfmt $f >> "$out/fmt.log" 2>&1 || true
done
find "$w/fmt" -mindepth 2 -name '*.fmt' -exec mv {} "$w/fmt/" \;
echo "TeX Live's formats from the release: $(cd "$w/fmt" && ls *.fmt 2>/dev/null | tr '\n' ' ')" | tee -a "$out/summary.txt"
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
python3 $root/shelf/serve.py --root $rel --port 8124 > $w/serve.log 2>&1 &
sleep 1
bin=$bin
bad=0
for d in $(echo $docs); do
  n=\${d%.tex} e=\$(sed -n 's/^% engine: \([a-z]*\).*/\1/p' "$w/docs/\$d" | head -1); e=\${e:-xelatex}
  mkdir -p "s-\$n"; cp -a $w/docs/. "s-\$n/"
  (cd "s-\$n" && PHITEX_CACHE_DIR=$w/c-shelf PHITEX_SHELF_RELEASE=$new PHITEX_SHELF_URL=http://127.0.0.1:8124/$label/ \
    timeout 900 \$bin build --shelf --engine \$e -j 8 "\$d" > run.log 2>&1); a=\$?
  s="s-\$n/\$n.pdf" t="r-\$n/\$n.pdf"
  if [ ! -s "\$t" ]; then r="TeX Live's \$e made no PDF from the release (\$n-texlive.log)"; cp "r-\$n/tl.log" $out/\$n-texlive.log 2>/dev/null
  elif [ ! -s "\$s" ]; then r="PhiTeX made no PDF (exit \$a)"; cp "s-\$n/run.log" $out/\$n-shelf.log
  elif cmp -s "\$s" "\$t"; then echo "\$e \$d: identical"; continue
  else r="differs from TeX Live's at \$(cmp "\$s" "\$t" | sed 's/.*differ: //')"; cp "s-\$n/run.log" $out/\$n-shelf.log
  fi
  echo "\$e \$d: \$r"; bad=\$((bad + 1))
done
exit \$bad
CHECK
bash "$w/check.sh" 2>&1 | tee -a "$out/summary.txt"; fails=$((fails + ${PIPESTATUS[0]}))
echo "packs fetched by --shelf: $(ls $w/c-shelf/shelf/$new/h 2>/dev/null | wc -l)" | tee -a "$out/summary.txt"
[ "$fails" = 0 ] || { echo "FAILED: $fails (summary.txt)" | tee -a "$out/summary.txt"; exit 1; }
