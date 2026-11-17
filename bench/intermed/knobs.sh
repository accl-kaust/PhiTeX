#!/bin/bash
cd /tmp/intermed-local
until grep -q ALLDONE out-po.txt; do sleep 20; done
for i in 1 2 3; do for d in amsldoc acm grail; do for mode in clean second; do
  ./run2.sh cnt rel $d 1 $mode
  ./run2.sh cnt rel $d 8 $mode
  for k in 1 2 3; do ./run2.sh cnt rel $d 8 $mode PHITEX_CLEAN_HOLDS=$k; done
  ./run2.sh cnt rel $d 8 $mode PHITEX_SPEC_WINDOW=1
done; done; done
echo ALLDONE
