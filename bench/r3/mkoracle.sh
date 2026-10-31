#!/bin/bash
# r3-cuts (accl cmd task): pdftex -ini oracles for the named pdf fixtures
# (oracle/fixtures/pdf/<name>.tex), copied to $r with their logs.
set -u
for n in $NAMES; do
  D=$w/ini-$n; mkdir -p $D; cp oracle/fixtures/pdf/$n.tex $D/
  (cd $D && SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 pdftex -ini -interaction=batchmode $n.tex > /dev/null 2>&1; echo "$n: exit $?")
  cp $D/$n.pdf $D/$n.log $r/ 2>/dev/null
done
pdftex --version | head -1
