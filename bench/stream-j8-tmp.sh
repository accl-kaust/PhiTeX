#!/bin/bash
# (temporary) the final PDF with streaming on at -j8: the watch with its viewer (every build
# streams its pages), a clean build and EDITS one-word edits, its final PDF against a cold
# `phitex build` of the final text (stream-edits-tmp.py's verdict), for small, amsldoc and DOCS.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
bin=$PWD/target/release/phitex
mkdir -p $w/docs/small $w/docs/amsldoc
cp bench/amsldoc/amsldoc.tex $w/docs/amsldoc/
python3 -I - "$w/docs/small/small.tex" <<'PY'
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
PY
list="small/small.tex amsldoc/amsldoc.tex"
for d in ${DOCS:-}; do
  if [ $d = grail ]; then mkdir -p $w/docs/grail && cp -a $w/bin/grail-src/. $w/docs/grail/ && rm -f $w/docs/grail/grail.pdf; list="$list grail/grail.tex"; continue; fi
  n=$(basename $(dirname $d)); mkdir -p $w/docs/$n && cp -a /usr/share/doc/texlive/$(dirname $d)/. $w/docs/$n/ && rm -f $w/docs/$n/*.pdf
  list="$list $n/$(basename $d)"
done
for spec in $list; do
  dn=$(dirname $spec) main=$(basename $spec)
  for j in 1 8; do
    rm -rf $w/e && cp -a $w/docs/$dn $w/e
    python3 -I bench/cut1-paraedit.py $w/e/$main ${EDITS:-4} > $w/edits.json
    PHITEX_CACHE_DIR=$w/cache timeout 3000 python3 -I bench/stream-edits-tmp.py $bin $w/e $main $j $w/edits.json $r/e-$dn-$j.json --view --write > $r/e-$dn-$j.txt 2>&1
    echo "$dn -j$j: $(tail -1 $r/e-$dn-$j.txt)"
  done
done
