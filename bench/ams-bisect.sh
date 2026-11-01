#!/bin/bash
# accl: amsldoc --exact-pdf -j 8 md5 per commit (COMMITS), each built from
# its archive (as gate-shard.sh's control)
here=$PWD; S=$r/summary.txt; : > $S
for c in $COMMITS; do
  wt=$w/bis-$c; rm -rf $wt; mkdir -p $wt && git archive $c | tar -x -C $wt && ln -s $here/upstream $wt/upstream
  (cd $wt && cargo build -q --release -p phitex-cli > $r/build-$c.txt 2>&1; echo "EXIT $?" >> $r/build-$c.txt)
  d=$w/ams-$c; rm -rf $d; cp -a bench/amsldoc $d
  (cd $d && PHITEX_CACHE_DIR=$w/cache-$c SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 $wt/target/release/phitex build amsldoc.tex --exact-pdf -j 8 > $r/ams-$c.log 2>&1)
  echo "$c $(md5sum < $d/amsldoc.pdf 2>/dev/null | cut -c1-32) build $(tail -1 $r/build-$c.txt) $(git log --oneline -1 $c | cut -c1-70)" | tee -a $S
done
