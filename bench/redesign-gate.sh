#!/bin/bash
# accl gate shard for the redesign (scripts/accl/accl submit cmd bash
# bench/redesign-gate.sh SHARD): suites | latex | sweep | docs
# Results in $r (summary.txt first).
set -u
export PHITEX_CACHE_DIR=$w/cache PHITEX_TEST_LIMIT=3000 CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-12}
mkdir -p "$PHITEX_CACHE_DIR"
S=$r/summary.txt
git log --oneline -1 | cut -c1-100 > "$S"
case $1 in
suites)
  PHITEX_CHECK_READS=fail cargo test -q --release -p phitex-tex --no-fail-fast --test pdftex --test edits --test pages \
    --test plain --test etex --test hints --test relaxed --test inputs --test defuse_gaps > "$r/suites.log" 2>&1
  echo "suites exit $?" >> "$S"; grep -E "^test result|FAILED|panicked at" "$r/suites.log" >> "$S" ;;
latex)
  PHITEX_CHECK_READS=fail cargo test -q --release -p phitex-tex --no-fail-fast --test latex -- --test-threads=4 > "$r/latex.log" 2>&1
  echo "latex exit $?" >> "$S"; grep -E "^test result|FAILED|panicked at" "$r/latex.log" >> "$S"
  PHITEX_CHECK_READS=fail cargo test -q --release -p phitex-tex --test latex -- --ignored --exact latex_hello pdflatex_hello > "$r/hello.log" 2>&1
  echo "hello exit $?" >> "$S"; grep -E "^test result|FAILED|panicked at" "$r/hello.log" >> "$S" ;;
sweep)
  PHITEX_SWEEP_N=5 cargo test -q --release -p phitex-tex --test latex -- --ignored --exact sweep::sweep_amsldoc --nocapture > "$r/sweep.log" 2>&1
  echo "sweep exit $?" >> "$S"; grep -E "edits,|per text|^sweep|violat|test result|panicked at" "$r/sweep.log" | head -60 >> "$S" ;;
docs)
  cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A5
  B=$PWD/target/release/phitex
  mkdir -p "$w/par" && cp bench/para4000.tex "$w/par/"
  (cd "$w/par" && "$B" build -j 8 --output-dir=warm --program pdftex para4000.tex > /dev/null 2>&1)
  for j in 1 8 16; do
    (cd "$w/par" && rm -rf o$j && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_PAR_PROF=$([ $j = 1 ] && echo 1 || echo 0) \
      /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$B" build -j $j --output-dir=o$j --program pdftex para4000.tex > "$r/para-j$j.log" 2>&1)
    echo "para4000 j$j: $(grep -oE 'WALL.*' "$r/para-j$j.log") md5 $(md5sum < "$w/par/o$j/para4000.pdf" | cut -c1-32) (bar 4146793290dc0ea887d79a1d2d1b4c89)" >> "$S"
    grep -E "^round: |round rerun causes|guess" "$r/para-j$j.log" | tail -3 | cut -c1-400 >> "$S"
  done
  grep parprof "$r/para-j1.log" >> "$S"
  # (the LaTeX format once, untimed)
  rm -rf "$w/amsw" && cp -a bench/amsldoc "$w/amsw" && (cd "$w/amsw" && "$B" build -j 8 amsldoc.tex > /dev/null 2>&1)
  for j in 1 8 16; do
    rm -rf "$w/ams$j" && cp -a bench/amsldoc "$w/ams$j"
    (cd "$w/ams$j" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 timeout 900 \
      /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$B" build amsldoc.tex --exact-pdf -j $j > "$r/ams-j$j.log" 2>&1)
    cp "$w/ams$j/amsldoc.pdf" "$r/ams-j$j.pdf" 2>/dev/null
    echo "amsldoc j$j: $(grep -oE 'WALL.*' "$r/ams-j$j.log") md5 $(md5sum < "$w/ams$j/amsldoc.pdf" | cut -c1-32) (bar 98c96e004c1c82db1e3d562cf0181a3e)" >> "$S"
    grep -E "^round: |round rerun causes|guess" "$r/ams-j$j.log" | tail -3 | cut -c1-400 >> "$S"
    grep -E "^check_reads: [0-9]+ stale" "$r/ams-j$j.log" | tr "\n" " " >> "$S"; echo >> "$S"; grep "^check_reads:" "$r/ams-j$j.log" | grep -v "stale$\|counters" | head -20 >> "$S"
  done ;;
esac
cat "$S"
