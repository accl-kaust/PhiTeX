#!/bin/bash
# storm-tmp.sh [J]: in an accl `cmd` job with ACCL_COURSE the course3
# package ($w/course), the fastdev CLI with profile built, course.tex
# built once to make the format (no log), then again at -j J with
# PHITEX_WAKE_LOG: the wakes (op, node, field, cause kind, site op,
# refuted run's ns) in $r/wakes.tsv.gz, the acct lines in $r/acct.txt.
set -u
j=${1:-8}
s0=$(date +%s)
cargo build --profile fastdev -p phitex-cli --features phitex-tex/profile > "$r/build.txt" 2>&1 || { echo "BUILD FAILED"; grep -m5 -A5 '^error' "$r/build.txt"; exit 1; }
echo "built in $(( $(date +%s) - s0 )) s at $(git rev-parse --short=9 HEAD)"
bin=$PWD/target/fastdev/phitex
echo "course: $(ls "$w/course" | tr "\n" " " | cut -c1-200)"; grep -a "^ *\\\\include{\\|^ *\\\\input{ch" "$w/course/course.tex" | head -5
envs=(PHITEX_TEX_TIME=1 SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC PHITEX_CACHE_DIR=$w/pcache PHITEX_TIME_LIMIT_SECS=3600)
rm -rf "$w/c" && cp -a "$w/course" "$w/c" && cd "$w/c"
s0=$(date +%s.%N)
env "${envs[@]}" timeout 3000 "$bin" build --exact-pdf -j "$j" course.tex > "$r/warm.log" 2>&1
echo "warm: exit $? wall $(awk -v a="$s0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.1f", b-a}') s md5 $(md5sum < course.pdf | cut -c1-8)"
cd "$w" && rm -rf "$w/c" && cp -a "$w/course" "$w/c" && cd "$w/c"
s0=$(date +%s.%N)
env "${envs[@]}" PHITEX_WAKE_LOG=1 timeout 3000 "$bin" build --exact-pdf -j "$j" course.tex > "$r/run.log" 2>&1
e=$?
echo "storm: exit $e wall $(awk -v a="$s0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.1f", b-a}') s md5 $(md5sum < course.pdf | cut -c1-8)"
grep -a "	wake	" "$r/run.log" | sed 's/^.*	wake	/wake	/' | gzip > "$r/wakes.tsv.gz"
echo "wakes $(zcat "$r/wakes.tsv.gz" | wc -l)"
grep -a "^acct:" "$r/run.log" > "$r/acct.txt"
grep -a "^acct:" "$r/warm.log" > "$r/acct-warm.txt"
grep -a "^acct: \(wall\|[0-9]* runs\)" "$r/acct.txt" | cut -c1-300
grep -v "	wake	" "$r/run.log" | grep -a "^acct:" -v | tail -5 | cut -c1-200
gzip "$r/run.log"
