#!/bin/bash
# Recordings for the engine's player (on accl, in a job): the clean core records nothing, so
# this builds on the round core (the `clean` feature dropped from phitex-defuse in the job's
# checkout only), observe; para4000 and amsldoc at -j 8 with DEFUSE_RECORD, gzipped into $r.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1
sed -i 's/features = \["clean"\]/features = []/' crates/core/defuse/Cargo.toml
grep -n "defuse = " crates/core/defuse/Cargo.toml
cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build.txt" | head -60 | tee -a "$r/summary.txt"; exit 1; }
mkdir -p "$w/bin/o"; cp target/release/phitex "$w/bin/o/phitex"
E="SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache"
doc() { # tag doc j [env...]
  local t=$1 d=$2 j=$3; shift 3
  rm -rf "$w/d-$t"; mkdir -p "$w/d-$t"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$w/d-$t/"; P=""; else cp bench/$d.tex "$w/d-$t/"; P="--program pdftex"; fi
  ( cd "$w/d-$t" && env $E "$@" timeout 1800 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$w/bin/o/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | pdf $(md5sum < "$w/d-$t/$d.pdf" 2>/dev/null | cut -c1-8)" | tee -a "$r/summary.txt"
}
doc warm amsldoc 8
for d in para4000 amsldoc; do
  doc $d-j8-rec $d 8 DEFUSE_RECORD=$r/$d-j8.rec
  gzip -1 "$r/$d-j8.rec"; ls -la "$r/$d-j8.rec.gz" | tee -a "$r/summary.txt"
done
echo done
