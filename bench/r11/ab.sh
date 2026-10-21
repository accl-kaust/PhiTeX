#!/bin/bash
# r11 A/B: busy-waiting workers (this tree) against sleeping ones.
# Profile CLI of this tree, PHITEX_NO_KEEP=1 (the pool as before) against
# default: op time by kind, clock probe; release CLIs of BASE (default
# e7aa893f) and this tree: wall and md5 (amsldoc j1 + 4 x j8, para4000).
source "$(dirname "$0")/lib.sh"
BASE=${BASE:-e7aa893f}
s=$w/src-base; rm -rf $s; mkdir -p $s; git archive $BASE | tar -x -C $s; ln -s $root/upstream $s/upstream
(cd $s && nice cargo build -q --release -p phitex-cli --target-dir $w/tb > $r/build-base.txt 2>&1) || echo "base build FAILED" | tee -a $S
nice cargo build -q --release -p phitex-cli --target-dir $w/tn > $r/build-new.txt 2>&1 || echo "new build FAILED: $(grep -m2 -A12 '^error' $r/build-new.txt)" | tee -a $S
warm
C="$P PHITEX_CLOCK_PROBE=1"
for k in 1 2 3; do
  for J in 1 8 16; do
    ams ams-j$J-old-$k "$C PHITEX_NO_KEEP=1"; ams ams-j$J-keep-$k "$C"
    para para-j$J-old-$k "$C PHITEX_NO_KEEP=1"; para para-j$J-keep-$k "$C"
  done
done
for k in 1 2 3 4; do
  for J in 1 8 16; do
    for v in base new; do
      [ $v = base ] && b=$w/tb/release/phitex || b=$w/tn/release/phitex
      [ $J = 1 ] && [ $k -gt 1 ] && continue
      run rel-ams-j$J-$v-$k $A amsldoc.pdf "" $b build amsldoc.tex --exact-pdf -j $J
      run rel-para-j$J-$v-$k $Q o-rel/para4000.pdf "" $b build -j $J --output-dir=o-rel --program pdftex para4000.tex
    done
  done
done
for f in $r/*.err; do t=$(basename $f .err); echo "$t: $(grep -h '^acct: clock at run' $f | tail -1 | sed 's/^acct: clock at run start.end by kind: //')"; done >> $S
echo "end $(date +%T)" | tee -a "$S"
