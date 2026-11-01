#!/bin/bash
# Shelf's release check: a release against TeX Live. The release's files
# unpacked into a texmf tree of their own (with its ls-R); every map file
# the release's TeX Live enables (its package database, TLPDB) must be in
# the release, and every font of those maps in its pdftex.map; TeX Live's own
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
#   TLPDB=<the release's texlive.tlpdb.xz> \
#     shelf/scripts/check.sh
set -u
root=$ROOT rel=$REL label=$LABEL new=$NEW bin=$BIN w=$W out=$OUT tlpdb=$TLPDB
mkdir -p "$w" "$out"
# (TeX Live's own over the release's tree alone: tl-tree.sh)
. "$root/shelf/scripts/tl-tree.sh"
mapfile -t tlenv < <(tl_vars "$w" "$w/tree/texmf-dist" "$FTREE")
tl() { env "${tlenv[@]}" SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 "$@"; }

# (5) a texmf tree of the release's files, with its ls-R
tree_unpack "$rel/$label" "$new" "$w/tree/texmf-dist" | tee -a "$out/summary.txt"

# (5b) every font of the maps the release's TeX Live enables (each
# package's `execute addMap` / `addMixedMap` in its database, TLPDB) in
# the release's pdftex.map, each map read from its package's archive as
# the build fetched it (beside TLPDB: the release serves pdftex.map, not
# the maps). (Not the CI image's own pdftex.map: that install is older
# than the tlnet the release is made from, and named 910 fonts the newer
# one had renamed or dropped, SourceSansPro, SourceSerifPro,
# mfb-oldstyle's ts1, run 41296048903)
fails=0
python3 - "$tlpdb" "$w/tree/texmf-dist/fonts/map/pdftex/updmap/pdftex.map" > "$out/map-missing.txt" <<'NAMES'
import lzma, os, sys, tarfile
db, pdfmap = sys.argv[1:3]
cache = os.path.dirname(db)
names = lambda b: {l.split()[0] for l in b.decode('latin-1').splitlines() if l.strip() and l[0] not in '%#'}
rel = names(open(pdfmap, 'rb').read())
for block in lzma.decompress(open(db, 'rb').read()).decode().split('\n\n'):
    lines = block.splitlines()
    head = dict(l.split(' ', 1) for l in lines if l and not l.startswith(' ') and ' ' in l)
    maps = {l.split()[-1] for l in lines if l.startswith(('execute addMap ', 'execute addMixedMap '))}
    if not maps:
        continue
    name, rev = head.get('name'), head.get('revision')
    arch = os.path.join(cache, f'{name}.r{rev}.tar.xz')
    if not os.path.exists(arch):
        print(f'{name} r{rev} (its archive, not fetched by the build)')
        continue
    with tarfile.open(arch, 'r:xz') as t:
        found = {os.path.basename(m.name): m for m in t if m.isfile() and '/fonts/map/' in m.name}
        for m in sorted(maps):
            if m not in found:
                print(f'{m} (the map file, not in {name})')
                continue
            for n in sorted(names(t.extractfile(found[m]).read()) - rel):
                print(f'{n} ({m})')
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
tl_formats "$w"
cp "$w/fmt.log" "$out/fmt.log" 2>/dev/null
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
