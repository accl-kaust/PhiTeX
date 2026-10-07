#!/bin/bash
# `phitex watch` (machine mode) against `phitex watch --ssa` on the course,
# inside the cluster container (job.sbatch `watch` task; $w is the
# workspace, the course in $w/course, $r the results directory), or
# locally with BIN, COURSE and RESULTS set:
#
#   watch.sh [runs] [modes]     runs: sessions of each mode, alternating (2);
#                               modes: "machine ssa" by default
#
# A session: the watch started cold (no store: a cache of its own), then
# the ch00 edit ("This course exists" deleted from line 3), then a
# \section added in ch15 before its second section, each waited for until
# its rebuild line (settled); then quit. Its times are from the save (the
# file renamed into place) to the line's stamp; the first pages: SSA mode's
# report, machine mode's "pass 1 written" note (none: one pass). Peak RSS
# is the process's VmHWM after each step. After the last session of a
# mode, the final source is built cold (`phitex build`) and the PDF and the
# files the job reads back are compared.
set -uo pipefail
runs=${1:-2}; modes=${2:-machine ssa}
P=${w:+$w/partex}; P=${P:-$(cd "$(dirname "$0")/../../.." && pwd)}
B=${BIN:-$P/target/release/phitex}
C=${COURSE:-$w/course}
R=${RESULTS:-$r}
S=$R/summary.txt
mkdir -p "$R"
say() { echo "$*" | tee -a "$S"; }
now() { date +%s.%N; }
sub() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.2f", a - b }'; }
export NO_COLOR=1 SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 PARTEX_FORMATS=$R/fmt

session() { # MODE OUT
  local mode=$1 out=$2 flag= pid t0 tw ts tr first
  rm -rf "$out"; mkdir -p "$out"
  cp -a "$C" "$out/course"
  [ "$mode" = ssa ] && flag=--ssa
  mkfifo "$out/in"
  exec 3<>"$out/in"
  t0=$(now)
  (cd "$out/course" && PARTEX_CACHE_DIR=$out/cache exec "$B" watch $flag -v --no-view course.tex) \
    <"$out/in" 2> >(while IFS= read -r l; do printf '%s %s\n' "$(date +%s.%N)" "$l"; done >"$out/err.txt") &
  pid=$!
  # (the subshell execs phitex: $pid is the watch)
  hwm() { awk '/VmHWM/ {print $2}' /proc/$pid/status 2>/dev/null; }
  wait_for() { # PATTERN AFTER: the stamp of the first line matching after AFTER
    local t
    while kill -0 $pid 2>/dev/null; do
      t=$(awk -v a="$2" -v p="$1" '$1 > a && $0 ~ p {print $1; exit}' "$out/err.txt")
      [ -n "$t" ] && { echo "$t"; return 0; }
      sleep 0.1
    done
    echo DIED; return 1
  }
  edit() { sed "$2" "$1" >"$1.new" && mv "$1.new" "$1"; }
  tw=$(wait_for 'Watching' 0) || { say "$mode: the watch died"; return 1; }
  say "$mode cold: $(sub "$tw" "$t0") s; peak rss $(hwm) KB"
  grep -a 'built in' "$out/err.txt" | tail -1 | cut -c22- | tee -a "$S"
  sleep 3
  (cd "$out/course" && edit ch00.tex '3s/This course exists//')
  ts=$(now)
  tr=$(wait_for 'ch00.tex:3 .*(✓|✗)' "$ts") || { say "$mode: the watch died"; return 1; }
  first=$(awk -v a="$ts" '$1 > a && /pass 1 written/ {print $1; exit}' "$out/err.txt")
  say "$mode ch00 edit: settled $(sub "$tr" "$ts") s${first:+, pass 1 written $(sub "$first" "$ts") s}; peak rss $(hwm) KB"
  grep -a 'rebuilt in' "$out/err.txt" | tail -1 | cut -c22- | tee -a "$S"
  sleep 3
  (cd "$out/course" &&
    awk '/^\\section/ && ++n == 2 { print "\\section{An added section}\n\nA short paragraph of its own.\n" } { print }' \
      ch15.tex >ch15.tex.new && mv ch15.tex.new ch15.tex)
  ts=$(now)
  tr=$(wait_for 'ch15.tex:[0-9]+ .*(✓|✗)' "$ts") || { say "$mode: the watch died"; return 1; }
  first=$(awk -v a="$ts" '$1 > a && /pass 1 written/ {print $1; exit}' "$out/err.txt")
  say "$mode ch15 section: settled $(sub "$tr" "$ts") s${first:+, pass 1 written $(sub "$first" "$ts") s}; peak rss $(hwm) KB"
  grep -a 'rebuilt in' "$out/err.txt" | tail -1 | cut -c22- | tee -a "$S"
  echo q >&3
  wait $pid
  exec 3>&-
}

compare() { # OUT: the session's files against a cold build of its final source
  local out=$1 ref=$1/ref f
  mkdir -p "$ref"; cp -a "$C/." "$ref/"
  cp "$out/course/ch00.tex" "$out/course/ch15.tex" "$ref/"
  (cd "$ref" && PARTEX_CACHE_DIR=$out/cache-ref "$B" build course.tex >"$out/ref.txt" 2>&1)
  for f in course.pdf course.aux course.toc course.out; do
    if cmp -s "$out/course/$f" "$ref/$f"; then echo "$f same"; else echo "$f DIFFERS"; fi
  done | tr '\n' ' ' | sed 's/^/  vs a cold build: /' | tee -a "$S"
  echo | tee -a "$S"
}

for i in $(seq "$runs"); do
  for m in $modes; do
    session "$m" "$R/$m-$i"
    [ "$i" = "$runs" ] && compare "$R/$m-$i"
  done
done
# (the big outputs stay on the node)
find "$R" -name '*.pdf' -delete 2>/dev/null
find "$R" -maxdepth 2 -name course -type d -exec rm -rf {} + 2>/dev/null
true
