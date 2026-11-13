#!/bin/bash
# (temporary) a Type 1 font's streamed program seeded with its codes below 128 (head) or not
# (noseed: only the glyphs the pages streamed so far used): the pages' files' bytes and the time
# their making took (PHITEX_STREAM_TRACE, `observe`), the clean build and EDITS edits at -j1
# with the viewer, small, amsldoc and DOCS; two runs each, interleaved.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
rm -rf $w/noseed && mkdir -p $w/noseed && git archive HEAD | tar -x -C $w/noseed && cp -a crates $w/noseed/; [ -d upstream ] && ln -s $PWD/upstream $w/noseed/upstream
sed -i 's/let seed = (0\.\.128)\.filter/let seed = (0..0).filter/' $w/noseed/crates/output/pdf/src/font.rs
grep -c "(0..0).filter" $w/noseed/crates/output/pdf/src/font.rs
build() { ( cd "$2" && cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; tail "$r/build-$1.txt"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; }
build head $PWD & build noseed $w/noseed || exit 1; wait
[ -x $w/bins/head/phitex ] || exit 1
mkdir -p $w/docs/amsldoc && cp bench/amsldoc/amsldoc.tex $w/docs/amsldoc/
list="amsldoc/amsldoc.tex"
for d in ${DOCS:-}; do
  n=$(basename $(dirname $d)); mkdir -p $w/docs/$n && cp -a /usr/share/doc/texlive/$(dirname $d)/. $w/docs/$n/ && rm -f $w/docs/$n/$(basename ${d%.tex}).pdf
  list="$list $n/$(basename $d)"
done
for spec in $list; do
  dn=$(dirname $spec) main=$(basename $spec)
  for i in 1 2; do for b in head noseed; do
    rm -rf $w/e && cp -a $w/docs/$dn $w/e
    python3 -I bench/cut1-paraedit.py $w/e/$main ${EDITS:-4} > $w/edits.json
    PHITEX_STREAM_TRACE=1 EDITS_STDERR=$r/e-$b-$dn-$i.err PHITEX_CACHE_DIR=$w/cache-$b timeout 3000 \
      python3 -I bench/stream-edits-tmp.py $w/bins/$b/phitex $w/e $main 1 $w/edits.json $r/e-$b-$dn-$i.json --view --write > $r/e-$b-$dn-$i.txt 2>&1
    python3 -I - $r/e-$b-$dn-$i.json $r/e-$b-$dn-$i.err "$dn $b run $i" <<'PY'
import json, re, sys
try:
    r = json.load(open(sys.argv[1]))
except Exception as e:
    print(sys.argv[3], "FAILED", e); sys.exit()
err = open(sys.argv[2], errors="replace").read().split("\n")
made = [l for l in err if re.match(r"stream: page \d+ exact", l)]
ev = [int(m.group(1)) for l in made for m in [re.search(r"evaluation (\d+)", l)] if m]
first = min(ev) if ev else 0
def tot(ls):
    ms = [float(m.group(1)) for l in ls for m in [re.search(r", ([0-9.]+) ms$", l)] if m]
    by = [int(m.group(1)) for l in ls for m in [re.search(r"(\d+) bytes", l)] if m]
    return len(ls), sum(by) // 1024, sum(ms)
c = [l for l in made if (m := re.search(r"evaluation (\d+)", l)) and int(m.group(1)) == first]
e = [l for l in made if (m := re.search(r"evaluation (\d+)", l)) and int(m.group(1)) > first]
memo = [l for l in err if l.startswith("stream: memo")]
print(f"{sys.argv[3]}: clean {r['cold_s']} s, its pages' files {tot(c)[0]}: {tot(c)[1]} KB in {tot(c)[2]:.0f} ms; "
      f"edits median {r['median_ms']} ms, their files {tot(e)[0]}: {tot(e)[1]} KB in {tot(e)[2]:.0f} ms; {r['verdict']}; {memo[-1] if memo else ''}")
PY
  done; done
done
