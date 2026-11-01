#!/bin/bash
# r11 push check: clippy (as the gate), release amsldoc j1 + 4 x j8 and para4000 md5s, profile build runs with the probes.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
S=$r/summary.txt
cargo clippy -p phitex-tex --all-targets -q -- -D warnings > $r/clippy.txt 2>&1; echo "clippy tex exit $? $(grep -m3 -E '^(error|warning)' $r/clippy.txt | tr '\n' ' ')" | tee -a $S
cargo clippy -p phitex-tex --features profile --all-targets -q -- -D warnings > $r/clippy-p.txt 2>&1; echo "clippy tex profile exit $? $(grep -m3 -E '^(error|warning)' $r/clippy-p.txt | tr '\n' ' ')" | tee -a $S
nice cargo build -q --release -p phitex-cli --target-dir $w/tn > $r/build.txt 2>&1 || echo "build FAILED" | tee -a $S
B=$w/tn/release/phitex
E="SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/cache"
A=$w/a; cp -a bench/amsldoc $A; Q=$w/p; mkdir -p $Q; cp bench/para4000.tex $Q/
(cd $A && env $E $B build amsldoc.tex --exact-pdf -j 8 > /dev/null 2>&1)
for t in 1 8 8 8 8; do rm -f $A/amsldoc.pdf; (cd $A && env $E $B build amsldoc.tex --exact-pdf -j $t > $r/a$t.log 2>&1); echo "amsldoc j$t $(md5sum < $A/amsldoc.pdf | cut -c1-32) pages $(grep -ao '/Type /Page[^s]' $A/amsldoc.pdf | wc -l)" | tee -a $S; done
for t in 1 8; do (cd $Q && env $E $B build -j $t --output-dir=o$t --program pdftex para4000.tex > /dev/null 2>&1); echo "para4000 j$t $(md5sum < $Q/o$t/para4000.pdf | cut -c1-32)" | tee -a $S; done
