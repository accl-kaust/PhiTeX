#!/bin/bash
# (temporary) one manual's clean build, boxscope off and on, -j $3: node
# runs by op, the nodes run most, the slowest.  bench/f52-time-tmp.sh latex/huaz huaz.tex 1
set -u
dir=$1 main=$2 jn=${3:-1}
cd "$w/phitex"
mkdir -p "$w/b" "$r"; bin=$w/b/phitex
CARGO_TARGET_DIR=$w/target-prof cargo build -q --release -p phitex-cli \
  --features phitex-tex/profile > "$r/build.txt" 2>&1 || { tail -30 "$r/build.txt"; exit 1; }
cp "$w/target-prof/release/phitex" "$bin"
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
for m in off box; do
  e=(PHITEX_BOXSCOPE=0); [ $m = off ] || e=(PHITEX_BOXSCOPE=1)
  rm -rf "$w/d-$m"; cp -a "/usr/share/doc/texlive/$dir" "$w/d-$m" || exit 1
  ( cd "$w/d-$m" && /usr/bin/time -f "%e s %M KB" env PHITEX_CACHE_DIR=$w/cache-$m PHITEX_RUN_TIMES=1 "${e[@]}" "$bin" build "$main" --exact-pdf --engine "${ENG:-pdflatex}" -j "$jn" > "$r/$m.txt" 2>&1; echo "$m exit $? $(md5sum < "${main%.tex}.pdf")"; tail -1 "$r/$m.txt" )
  python3 -I - "$r/$m.txt" <<'PY'
import sys, collections
runs = collections.Counter(); ms = collections.Counter(); per = collections.Counter(); pert = collections.Counter()
for l in open(sys.argv[1], errors='replace'):
    if not l.startswith('runtime: '): continue
    p = l.split()
    if len(p) < 5: continue
    op, node = p[1], p[2]
    try: d = float(p[4]) - float(p[3])
    except ValueError: continue
    runs[op] += 1; ms[op] += d; per[(op, node)] += 1; pert[(op, node)] += d
print("  runs by op:", ", ".join(f"{o} {runs[o]} ({ms[o]/1000:.1f} s)" for o, _ in ms.most_common(8)))
print("  nodes run most:", ", ".join(f"{o}:{n} x{c}" for (o, n), c in per.most_common(8)))
print("  slowest nodes:", ", ".join(f"{o}:{n} {t/1000:.2f} s" for (o, n), t in pert.most_common(8)))
PY
done
