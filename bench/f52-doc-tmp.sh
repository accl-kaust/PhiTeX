#!/bin/bash
# (temporary) one TeX Live manual with boxscope off and on (the CI image's
# doc tree, ACCL_IMAGE=phitex-ci-2026-09-25): md5s, the first difference,
# where lists are left.   bench/f52-doc-tmp.sh latex/tikz2d-fr tikz2d-fr-doc.tex
set -u
dir=$1 main=$2
cd "$w/phitex"
mkdir -p "$w/b" "$r"; bin=$w/b/phitex
CARGO_TARGET_DIR=$w/target-prof cargo build -q --release -p phitex-cli \
  --features phitex-tex/profile > "$r/build.txt" 2>&1 || { tail -30 "$r/build.txt"; exit 1; }
cp "$w/target-prof/release/phitex" "$bin"
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
for m in off box; do
  e=(PHITEX_BOXSCOPE=0); [ $m = off ] || e=(PHITEX_BOXSCOPE=1 PHITEX_BOXSCOPE_TRACE=1)
  rm -rf "$w/d-$m"; cp -a "/usr/share/doc/texlive/$dir" "$w/d-$m" || exit 1
  ( cd "$w/d-$m" && env PHITEX_CACHE_DIR=$w/cache-$m "${e[@]}" "$bin" build "$main" --exact-pdf -j 1 > "$r/$m.txt" 2>&1; echo "$m exit $? $(md5sum < "${main%.tex}.pdf")"; cp "${main%.tex}.pdf" "$r/$m.pdf" )
done
python3 -I - "$r/off.pdf" "$r/box.pdf" <<'PY'
import sys, zlib, re
a, b = (open(p, 'rb').read() for p in sys.argv[1:3])
def objs(x):
    out = {}
    for m in re.finditer(rb'(\d+) 0 obj', x):
        s = x.find(b'stream', m.end()); e = x.find(b'endobj', m.end())
        if 0 <= s < e:
            s += 6
            while x[s:s+1] in b'\r\n': s += 1
            t = x.find(b'endstream', s)
            try: out[int(m.group(1))] = zlib.decompressobj().decompress(x[s:t])
            except Exception: out[int(m.group(1))] = x[s:t]
    return out
oa, ob = objs(a), objs(b)
n = 0
for k in sorted(oa):
    if oa.get(k) != ob.get(k):
        la, lb = oa[k].split(b'\n'), (ob.get(k) or b'').split(b'\n')
        for i, (p, q) in enumerate(zip(la, lb)):
            if p != q:
                print(f"obj {k} line {i}:\n  off {p[:240]!r}\n  box {q[:240]!r}")
                for j in range(max(0, i - 6), i):
                    print(f"  ctx {la[j][:200]!r}")
                break
        n += 1
        if n >= 3: break
PY
echo "== box: leaves, closes, paragraph ends"
grep "^boxscope: node .* \(leaves\|closes\|ends the paragraph\)" "$r/box.txt" | cut -c1-260 | head -40
