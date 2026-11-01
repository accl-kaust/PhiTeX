#!/bin/bash
# pages-c3-tmp.sh [RUNS] [ENV...]: in an accl `cmd` job (kept workspace,
# $w/course the course3 subset): the fastdev CLI with profile+observe built
# incrementally at the checkout's commit, then course3 --exact-pdf -j 8
# RUNS times (default 1) with PHITEX_TEX_TIME=1 and ENV (e.g.
# PHITEX_WAKE_LOG=1); the acct lines and md5 printed, logs in $r.
# RUNS `ams`: amsldoc --exact-pdf at -j 1/8/16 instead.
set -u
runs=${1:-1}; shift || true
cd "$w/phitex"
s0=$(date +%s)
cargo build --profile fastdev -p phitex-cli --features phitex-tex/observe,phitex-tex/profile > "$r/build.txt" 2>&1 || { echo "BUILD FAILED"; grep -m5 -A5 '^error' "$r/build.txt"; exit 1; }
echo "built in $(( $(date +%s) - s0 )) s at $(git rev-parse --short=9 HEAD)"
bin=$PWD/target/fastdev/phitex
if [ "$runs" = ams ]; then
  runs=0
  for j in 1 8 16; do
    rm -rf "$w/ams" && cp -a bench/amsldoc "$w/ams" && cd "$w/ams"
    SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC PHITEX_CACHE_DIR=$w/pcache timeout 1800 "$bin" build --exact-pdf -j $j amsldoc.tex > "$r/ams-j$j.log" 2>&1
    echo "amsldoc -j $j: exit $? md5 $(md5sum < amsldoc.pdf 2>/dev/null | cut -c1-8)"
    cd "$w/phitex"
  done
fi
for i in $(seq 1 "$runs"); do
  rm -rf "$w/c3" && cp -a "$w/course" "$w/c3" && cd "$w/c3"
  s0=$(date +%s.%N)
  env "$@" PHITEX_TEX_TIME=1 SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC PHITEX_CACHE_DIR=$w/pcache \
    PHITEX_TIME_LIMIT_SECS=3600 timeout 3600 "$bin" build --exact-pdf -j 8 course.tex > "$r/c3-$i.log" 2>&1
  e=$?
  s1=$(date +%s.%N)
  echo "== run $i: exit $e wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.1f", b-a}') s md5 $(md5sum < course.pdf 2>/dev/null | cut -c1-8)"
  grep '^acct:' "$r/c3-$i.log"
  grep -E '^!|panicked' "$r/c3-$i.log" | head -5
  cd "$w/phitex"
done
