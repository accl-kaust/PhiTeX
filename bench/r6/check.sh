#!/bin/bash
# r6-scale's check of a commit (on accl, in a job): clippy on phitex-tex; the suites named
# (default hints etex plain pdftex edits); amsldoc -j 1 and 4x -j 8 md5, para4000 md5 (release);
# amsldoc -j 8 critical path (profile, PHITEX_TEX_TIME); amsldoc's piece->piece chain from a -j 8
# recording (observe, bench/r6/fin-reads.py).
#   scripts/accl/accl submit cmd bash bench/r6/check.sh [suite...]
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-16}
suites=${*:-hints etex plain pdftex edits}
git log --oneline -1 | tee -a "$r/summary.txt"
cargo clippy -q -p phitex-tex --no-deps --all-targets -- -D warnings > "$r/clippy.txt" 2>&1
echo "clippy exit $?" | tee -a "$r/summary.txt"; grep -E -- "^(error|warning)|-->" "$r/clippy.txt" | head -20 | tee -a "$r/summary.txt"
for t in $suites; do
  CARGO_TARGET_DIR=$w/t-test cargo test -q --release -p phitex-tex --test "$t" > "$r/test-$t.txt" 2>&1
  s=$?
  echo "suite $t exit $s: $(grep -E '^test result' "$r/test-$t.txt" | tail -1)" | tee -a "$r/summary.txt"
  [ $s = 0 ] || grep -E "panicked|^    [a-z_:0-9]+$" "$r/test-$t.txt" | head -20 | tee -a "$r/summary.txt"
done
mkdir -p "$w/bin/rel" "$w/bin/prof" "$w/bin/obs"
cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1 && cp target/release/phitex "$w/bin/rel/"
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build-prof.txt" 2>&1 && cp target/release/phitex "$w/bin/prof/"
cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build-obs.txt" 2>&1 && cp target/release/phitex "$w/bin/obs/"
run() { # bin doc j tag [env...]
  local b=$1 d=$2 j=$3 t=$4; shift 4
  local dir=$w/run-$t; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && env "$@" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 900 /usr/bin/time -f "WALL %e" "$w/bin/$b/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | $(md5sum < "$dir/$d.pdf" 2>/dev/null | cut -c1-8) $(qpdf --check "$dir/$d.pdf" > /dev/null 2>&1 && echo qpdf-ok || echo QPDF-FAIL)" | tee -a "$r/summary.txt"
}
run rel amsldoc 8 warm > /dev/null
run rel amsldoc 1 ams-j1
for i in 1 2 3 4; do run rel amsldoc 8 ams-j8-$i; done
run rel para4000 1 para-j1
run rel para4000 8 para-j8
run prof amsldoc 8 ams-prof PHITEX_TEX_TIME=1
grep -E "^acct: critical path" "$r/ams-prof.log" | cut -c1-300 | tee -a "$r/summary.txt"
run obs amsldoc 8 ams-rec DEFUSE_RECORD=$w/ams.rec
gzip -1 "$w/ams.rec" && python3 -I bench/r6/fin-reads.py "$w/ams.rec.gz" > "$r/chain.txt" 2>&1
grep -E "longest chain|its links by kind|without" "$r/chain.txt" | cut -c1-400 | tee -a "$r/summary.txt"
echo done
