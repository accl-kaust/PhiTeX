#!/bin/bash
# (temporary) page streaming, CONTROL against this commit, on one node, interleaved: the
# clean build at -j1 (`build`, nothing streamed) and the watch with its viewer (every build
# streams its pages): its clean build and N one-word edits at -j1 (bench/stream-edits-tmp.py),
# the streamed pages counted (PHITEX_STREAM_TRACE), the final PDF against a clean build's.
# Documents: a small article (4 pages), amsldoc (44), and DOCS (TeX Live manuals by their
# directory/main under /usr/share/doc/texlive: the CI image, ACCL_IMAGE=phitex-ci-2026-09-25).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
build() { ( cd "$2" && cargo build -q --release -p phitex-cli > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; tail "$r/build-$1.txt"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
build control $w/control & build head $PWD || exit 1; wait
# (this commit's with the engine's traces, `observe`: the pages streamed counted, each one's
# making timed; never timed itself)
( cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build-obs.txt" 2>&1 ) && mkdir -p $w/bins/obs && cp target/release/phitex $w/bins/obs/
[ -x $w/bins/control/phitex ] || exit 1
mkdir -p $w/docs/small $w/docs/amsldoc
cp bench/amsldoc/amsldoc.tex $w/docs/amsldoc/
python3 -I - "$w/docs/small/small.tex" <<'EOF'
import sys
words = open("bench/para400.tex").read().split("\n")
paras = [l for l in words if l and not l.startswith("\\")][:24]
out = ["\\documentclass{article}", "\\begin{document}"]
for i, p in enumerate(paras):
    if i % 6 == 0:
        out.append("\\section{Part %d}" % (i // 6 + 1))
    out.append(p[:900] + "\n")
out.append("\\end{document}")
open(sys.argv[1], "w").write("\n".join(out) + "\n")
EOF
list="small/small.tex amsldoc/amsldoc.tex"
for d in ${DOCS:-}; do
  n=$(basename $(dirname $d)); mkdir -p $w/docs/$n && cp -a /usr/share/doc/texlive/$(dirname $d)/. $w/docs/$n/ && rm -f $w/docs/$n/*.pdf
  list="$list $n/$(basename $d)"
done
N=${N:-6}
for spec in $list; do
  dn=$(dirname $spec) main=$(basename $spec)
  # (each binary's format made first, out of the timings)
  for b in control head obs; do
    rm -rf $w/warm && cp -a $w/docs/$dn $w/warm
    (cd $w/warm && PHITEX_CACHE_DIR=$w/cache-$b timeout 1800 $w/bins/$b/phitex build -j1 $main > $r/warm-$b-$dn.txt 2>&1)
  done
  pages=$(qpdf --show-npages $w/warm/${main%.tex}.pdf 2>/dev/null)
  echo "== $dn: $pages pages"
  for i in 1 2; do for b in control head; do
    rm -rf $w/c && cp -a $w/docs/$dn $w/c
    t0=$(date +%s.%N); (cd $w/c && PHITEX_CACHE_DIR=$w/cache-$b timeout 1800 $w/bins/$b/phitex build -j1 $main > /dev/null 2>&1); t1=$(date +%s.%N)
    echo "$dn $b build -j1 run $i: $(python3 -I -c "print(round($t1-$t0,3))") s md5 $(md5sum < $w/c/${main%.tex}.pdf | cut -c1-12)"
  done; done
  for i in 1 2 obs; do for b in control head; do
    [ $i = obs ] && { [ $b = head ] && b=obs || continue; }
    rm -rf $w/e $w/e-cold && cp -a $w/docs/$dn $w/e
    python3 -I bench/cut1-paraedit.py $w/e/$main $N > $w/edits.json
    PHITEX_STREAM_TRACE=1 EDITS_STDERR=$r/e-$b-$dn-$i.err PHITEX_CACHE_DIR=$w/cache-$b timeout 1800 \
      python3 -I bench/stream-edits-tmp.py $w/bins/$b/phitex $w/e $main 1 $w/edits.json $r/e-$b-$dn-$i.json --view --write > $r/e-$b-$dn-$i.txt 2>&1
    python3 -I - $r/e-$b-$dn-$i.json $r/e-$b-$dn-$i.err "$dn $b watch -j1 run $i" <<'EOF'
import json, re, sys
try:
    r = json.load(open(sys.argv[1]))
except Exception as e:
    print(sys.argv[3], "FAILED", e); sys.exit()
err = open(sys.argv[2], errors="replace").read().split("\n")
made = [l for l in err if re.match(r"stream: page \d+ exact", l)]
ms = [float(m.group(1)) for l in made for m in [re.search(r", ([0-9.]+) ms$", l)] if m]
ev = [int(m.group(1)) for l in made for m in [re.search(r"evaluation (\d+)", l)] if m]
first = min(ev) if ev else 0
edit_ms = [float(m.group(2)) for l in made for m in [re.search(r"evaluation (\d+), ([0-9.]+) ms$", l)] if m and int(m.group(1)) > first]
by = [int(m.group(1)) for l in made for m in [re.search(r"(\d+) bytes", l)] if m]
w = sorted(x["wall_ms"] for x in r["edits"] if x.get("wall_ms") is not None)
print(f'{sys.argv[3]}: clean {r["cold_s"]} s (wall_ms {r["cold_wall_ms"]}); edits median {r["median_ms"]} ms, wall_ms median {w[len(w)//2] if w else None} all {w}; '
      f'pages streamed {len(made)} ({sum(by)//1024} KB, made in {sum(ms):.0f} ms, max {max(ms) if ms else 0:.1f} ms; by edits {len(edit_ms)} in {sum(edit_ms):.0f} ms); {r["verdict"]} hwm {r["watch_hwm_mb"]} MB')
EOF
  done; done
done
