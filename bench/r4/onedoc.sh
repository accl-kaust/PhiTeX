#!/bin/bash
# r4-latexmk (accl cmd task): `phitex build` of fixture FIX
# (oracle/fixtures/latexmk/FIX) with the CLI of each commit in REVS
# (built from this checkout's history), md5 against the fixture's
# oracle PDF; the rounds and the build's stderr kept.
set -u
export CARGO_BUILD_JOBS=16 SOURCE_DATE_EPOCH=${EPOCH:-1700000000} FORCE_SOURCE_DATE=1 NO_COLOR=1
want=$(md5sum < oracle/fixtures/latexmk/$FIX.pdf | cut -c1-8)
for rev in $REVS; do
  git worktree add -q $w/wt-$rev $rev 2>/dev/null || { echo "$rev: no worktree" | tee -a $r/summary.txt; continue; }
  (cd $w/wt-$rev && cp -a ../phitex/upstream . 2>/dev/null; CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli > $r/build-$rev.txt 2>&1)
  D=$w/d-$rev; rm -rf $D; mkdir -p $D; cp -a oracle/fixtures/latexmk/$FIX/. $D/
  (cd $D && PHITEX_CACHE_DIR=$w/cache-$rev timeout 900 $w/t/release/phitex build $FIX.tex --json > $r/$rev.json 2> $r/$rev.err)
  echo "$rev: $FIX pdf $(md5sum < $D/$FIX.pdf | cut -c1-8) (oracle $want) $(grep -o '"rounds":[0-9]*' $r/$rev.json)" | tee -a $r/summary.txt
done
