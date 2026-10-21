#!/bin/bash
# Font keys check (on accl, in a job; ACCL_WS=r15f): fastdev build, clippy, phitex-tex's
# unit tests for fonts, the plain/pdftex suites, amsldoc -j 8 md5.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1 | tee -a "$r/summary.txt"
cargo build -q --profile fastdev -p phitex-cli > "$r/build.txt" 2>&1; echo "build exit $?" | tee -a "$r/summary.txt"; grep -E "^error|-->" -A7 "$r/build.txt" | head -80 | tee -a "$r/summary.txt"
cargo clippy -q -p phitex-tex -p phitex-dvi --no-deps --all-targets -- -D warnings > "$r/clippy.txt" 2>&1; echo "clippy exit $?" | tee -a "$r/summary.txt"; grep -E -- "^(error|warning)|-->" -A5 "$r/clippy.txt" | head -60 | tee -a "$r/summary.txt"
cargo test -q --profile fastdev -p phitex-tex --lib fonts > "$r/unit.txt" 2>&1; echo "unit exit $?: $(grep -E '^test result' "$r/unit.txt" | tail -1)" | tee -a "$r/summary.txt"
bin=$PWD/target/fastdev/phitex
for d in amsldoc para4000; do
  dir=$w/run-$d; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 900 $bin build --exact-pdf -j 8 $P $d.tex > "$r/$d.log" 2>&1 )
  echo "$d j8 $(md5sum < "$dir/$d.pdf" | cut -d' ' -f1) $(grep -E '^(error|failed)' "$r/$d.log" | head -3)" | tee -a "$r/summary.txt"
done
for t in plain pdftex pages; do
  cargo test -q --profile fastdev -p phitex-tex --test $t > "$r/test-$t.txt" 2>&1
  echo "suite $t exit $?: $(grep -E '^test result' "$r/test-$t.txt" | tail -1)" | tee -a "$r/summary.txt"
  grep -E "^    [a-z_:0-9]+$|panicked" "$r/test-$t.txt" | head -10 | tee -a "$r/summary.txt"
done
