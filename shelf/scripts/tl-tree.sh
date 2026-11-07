#!/bin/bash
# TeX Live's own engines over a Shelf release's files alone, sourced by
# shelf/scripts/check.sh (Shelf's release check) and by the research
# corpus's Shelf lane (scripts/accl/gate-shard.sh research K N shelf):
#
#   tree_unpack REL/LABEL NEW TREE   the release NEW's files (its index and
#                                    packs under REL/LABEL) unpacked into the
#                                    texmf tree TREE (texmf-dist), with ls-R;
#                                    prints the release's numbers
#   tl_vars W TREE FONTS             the environment, one VAR=VALUE a line,
#                                    under which TeX Live's own kpsewhich,
#                                    fmtutil-sys, pdftex and xetex see TREE
#                                    alone (W/empty for the other trees, W/tlvar
#                                    and W/fmt theirs, web2c/ for the tcx;
#                                    W/fonts.conf, written, on FONTS's
#                                    OpenType and TrueType fonts)
#   tl_formats W                     (the caller's tl: env with tl_vars) TeX Live's formats (pdftex, pdflatex,
#                                    xetex, xelatex) made by fmtutil from that
#                                    tree into W/fmt; their log W/fmt.log
#
# Before they run, TeX Live's own root is read from the environment's
# kpathsea (TEXMFROOT), so source this before changing TEXMF*.
tlroot=$(kpsewhich -var-value=TEXMFROOT)
tree_unpack() {
  python3 - "$1" "$2" "$3" <<'PY'
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
}
tl_vars() {
  local w=$1 tree=$2 fonts=$3
  mkdir -p "$w/empty" "$w/tlvar" "$w/fmt"
  cat > "$w/fonts.conf" <<CONF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$fonts/fonts/opentype</dir>
  <dir>$fonts/fonts/truetype</dir>
  <cachedir>$w/fc-cache</cachedir>
</fontconfig>
CONF
  printf '%s\n' "TEXMFDIST=$tree" "TEXMFLOCAL=$w/empty" "TEXMFSYSVAR=$w/tlvar" "TEXMFSYSCONFIG=$w/empty" \
    "TEXMFVAR=$w/tlvar" "TEXMFCONFIG=$w/empty" "TEXMFHOME=$w/empty" "WEB2C=$tlroot/texmf-dist/web2c" \
    "TEXFORMATS=$w/fmt" "FONTCONFIG_FILE=$w/fonts.conf"
}
tl_formats() {
  local w=$1 f
  sh -c 'cat "$(kpsewhich fmtutil.cnf)"' > "$w/fmtutil.cnf"
  for f in pdftex pdflatex xetex xelatex; do
    tl fmtutil-sys --cnffile "$w/fmtutil.cnf" --fmtdir "$w/fmt" --byfmt $f >> "$w/fmt.log" 2>&1 || true
  done
  find "$w/fmt" -mindepth 2 -name '*.fmt' -exec mv {} "$w/fmt/" \;
}
