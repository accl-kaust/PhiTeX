#!/bin/bash
cd /tmp/intermed-local; chmod +x run.sh
for d in amsldoc acm grail; do ./run.sh prep $d; done
for d in amsldoc acm grail; do for mode in clean second; do
  for j in 1 8; do ./run.sh one po $d $j $mode keep PHITEX_RUN_LOG=1 PHITEX_INTERMED_LOG=1; done
  for i in 1 2 3; do for j in 1 8; do ./run.sh one rel $d $j $mode no; done; done
done; done
echo ALLDONE
