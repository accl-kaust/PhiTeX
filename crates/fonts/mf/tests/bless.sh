#!/bin/bash
# The oracles of the phitex-mf tests, made by TeX Live's own mf, tftopl,
# gftype and gftopk (run on mc in the CI image:
#   CLUSTER=mc scripts/accl/accl submit cmd bash crates/fonts/mf/tests/bless.sh
# ), into $r/mf:
#   trap/   the trap test as mftraptest.test runs it (triptrap/texmf.cnf),
#           the date fixed: mftrapin.fot, mftrapin.log, mftrap.fot,
#           mftrap.log, trap.tfm, trap.72270gf, mftrap.pl, trap.typ
#   fonts/  each font of the list made as mktexpk makes it
#           (\mode:=ljfour; mag:=MAG; nonstopmode; input NAME): its GF, TFM
#           and gftopk's PK, list.txt (name mag dpi and the md5 and size of
#           each), and src/ every file mf read (mf -recorder), so that the
#           tests can run without TeX Live.
set -eu
root=$PWD
out=${r:?}/mf
T=$root/upstream/texlive-source/texk/web2c/triptrap
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
mkdir -p "$out/trap" "$out/fonts/src"
{ mf --version | head -1; gftopk --version | head -1; tftopl --version | head -1; } > "$out/versions.txt"

# --- trap
tmp=$(mktemp -d)
(
  cd "$tmp"
  export TEXMFCNF=$T
  cp "$T/trap.mf" .
  mf --progname=inimf < "$T/mftrap1.in" > mftrapin.fot 2>&1 || true
  mv trap.log mftrapin.log
  mf --progname=inimf < "$T/mftrap2.in" > mftrap.fot 2>&1 || true
  mv trap.log mftrap.log
  tftopl ./trap.tfm mftrap.pl || true
  gftype -m -i ./trap.72270gf > trap.typ || true
  cp mftrapin.fot mftrapin.log mftrap.fot mftrap.log trap.tfm trap.72270gf mftrap.pl trap.typ "$out/trap/"
  ls -la > "$out/trap/ls.txt"
)
rm -rf "$tmp"

# --- fonts
tmp=$(mktemp -d)
cd "$tmp"
list() {
  # every font of these source directories that has a TFM in TeX Live
  for d in public/cm public/logo public/misc ; do
    dir=$(dirname "$(kpsewhich cmr10.mf)")/../${d#public/}
    for f in "$dir"/*.mf; do
      n=$(basename "$f" .mf)
      kpsewhich "$n.tfm" > /dev/null && echo "$n 1+0/600"
    done
  done
  for n in ghab ecrm1000 ecbx1200 ecti1000 ectt1000 tcrm1000 larm1000 lati1000 manfnt cmr10 ; do echo "$n 1+0/600"; done
  echo "cmr10 1+120/600"; echo "cmr10 0.5"; echo "cmbx12 1+0/600"; echo "logo10 1.2"
  echo "cmr5 0.1"; echo "cmtt8 0.2"; echo "cminch 1+0/600"
}
list | sort -u > fonts.in
while read -r name mag; do
  rm -f ./*gf ./*.tfm ./*.log ./*.fls
  mf -recorder "\\mode:=ljfour; mag:=$mag; nonstopmode; input $name" > "$name.out" 2>&1 < /dev/null || true
  gf=$(ls ./*gf 2> /dev/null | head -1 || true)
  if [ -z "$gf" ]; then echo "$name $mag NOGF" >> "$out/fonts/list.txt"; continue; fi
  gf=${gf#./}
  dpi=${gf#"$name".}; dpi=${dpi%gf}
  gftopk "./$gf" "$name.${dpi}pk"
  cp "$gf" "$name.${dpi}pk" "$out/fonts/"
  cp "$name.tfm" "$out/fonts/$name.tfm" 2>/dev/null || true
  cp "$name.log" "$out/fonts/$name.$dpi.log"
  grep '^INPUT ' ./*.fls | cut -c7- | while read -r f; do cp -n "$f" "$out/fonts/src/" 2>/dev/null || true; done
  echo "$name $mag $dpi $(md5sum < "$gf" | cut -c1-32) $(stat -c %s "$gf") $(md5sum < "$name.tfm" | cut -c1-32) $(md5sum < "$name.${dpi}pk" | cut -c1-32)" >> "$out/fonts/list.txt"
done < fonts.in
wc -l "$out/fonts/list.txt"
grep -c NOGF "$out/fonts/list.txt" || true
