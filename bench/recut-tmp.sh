#!/bin/bash
# amsldoc -j1 clean build, base (CONTROL) vs head: wall (3 runs each, interleaved) and
# the commands, second passes (PHITEX_TEX_TIME's acct line).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
bins=()
build() { ( cd "$2" && cargo build -q --release -p phitex-cli > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; bins+=("$1"); }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream; build control $w/control
build head $PWD || exit 1
one() { rm -rf $w/a-$1 && cp -a bench/amsldoc $w/a-$1 && cd $w/a-$1 && /usr/bin/time -f "%e s %M KB" env PHITEX_CACHE_DIR=$w/cache-$1 "${@:2}" $w/bins/$1/phitex build -j1 --exact-pdf amsldoc.tex > /dev/null 2> $r/err-$1.txt; tail -1 $r/err-$1.txt; md5sum < amsldoc.pdf; cd - > /dev/null; }
for l in "${bins[@]}"; do one $l > /dev/null; done
for i in 1 2 3; do for l in "${bins[@]}"; do echo "$l run $i: $(one $l | tr '\n' ' ')"; done; done
for l in "${bins[@]}"; do one $l env PHITEX_TEX_TIME=1 > /dev/null; grep -E "acct: commands|exec run:.*second pass" $r/err-$l.txt | head -3 | sed "s|^|$l: |"; grep -c "second pass" $r/err-$l.txt | sed "s|^|$l slow exec runs cut again: |"; cp $r/err-$l.txt $r/time-$l.txt; done
