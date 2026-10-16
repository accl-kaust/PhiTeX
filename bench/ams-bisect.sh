#!/bin/bash
# accl: amsldoc --exact-pdf -j 8 md5 per commit (COMMITS), each built in its own worktree
here=$PWD; S=$r/summary.txt; : > $S
for c in $COMMITS; do
  wt=$w/bis-$c; git worktree add -q --detach $wt $c 2>&1 | tail -1; [ -e $wt/upstream ] || ln -s $here/upstream $wt/upstream
  (cd $wt && CARGO_TARGET_DIR=$here/target cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A5 | head -10)
  cp $here/target/release/phitex $w/phitex-$c
  d=$w/ams-$c; rm -rf $d; cp -a bench/amsldoc $d
  (cd $d && PHITEX_CACHE_DIR=$w/cache SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 $w/phitex-$c build -j 8 --exact-pdf amsldoc.tex > $r/ams-$c.log 2>&1)
  echo "$c $(md5sum < $d/amsldoc.pdf | cut -c1-32) $(git log --oneline -1 $c | cut -c1-70)" | tee -a $S
  cp $d/amsldoc.pdf $r/ams-$c.pdf
done
