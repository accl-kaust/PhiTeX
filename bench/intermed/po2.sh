#!/bin/bash
cd /tmp/intermed-local
until grep -q ALLDONE out-knobs.txt; do sleep 20; done
mkdir -p out2
for d in amsldoc acm grail; do
  TAGX=-holds1 ./run.sh one po $d 8 clean keep PHITEX_RUN_LOG=1 PHITEX_INTERMED_LOG=1 PHITEX_CLEAN_HOLDS=1
  w=/tmp/intermed-local/w-$d
done
echo ALLDONE
