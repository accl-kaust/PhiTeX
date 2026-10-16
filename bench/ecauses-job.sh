# accl (scripts/accl/accl submit cmd bash bench/ecauses-job.sh): the amsldoc
# sweep with the census, amsldoc exact PDFs at j1/j8 (copied to $r, compared
# with the epoch oracle on the head node), the suites
export PHITEX_CACHE_DIR=$w/cache PHITEX_TEST_LIMIT=3000 CARGO_BUILD_JOBS=10
mkdir -p $PHITEX_CACHE_DIR
PHITEX_RUN_LOG=1 PHITEX_SWEEP_CENSUS=1 PHITEX_SWEEP_N=5 cargo test -q --release -p phitex-tex --test latex sweep_amsldoc -- --ignored --nocapture > $r/sweep.log 2>&1; s=$?
grep -E "edits,|per text|^sweep amsldoc|test result" $r/sweep.log > $r/summary.txt
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A5
for j in 1 8; do rm -rf $w/ams && cp -a bench/amsldoc $w/ams && (cd $w/ams && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 $w/phitex/target/release/phitex build amsldoc.tex --exact-pdf -j $j > $r/ams-j$j.log 2>&1; echo "ams j$j exit $?" >> $r/summary.txt; cp amsldoc.pdf $r/ams-j$j.pdf); done
cargo test -q --release -p phitex-tex --no-fail-fast --test pdftex --test edits --test pages --test plain --test etex --test hints --test relaxed > $r/suites.log 2>&1; echo suites $? >> $r/summary.txt; grep -E "test result|FAILED|panicked" $r/suites.log >> $r/summary.txt
exit $s
