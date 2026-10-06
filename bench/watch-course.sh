#!/usr/bin/env bash
# bench/watch-course.sh WHAT...: the course's cold build to its fixpoint,
# its peak memory and time, in one of several ways (run inside the accl
# container by `scripts/accl/accl submit cmd bash bench/watch-course.sh WHAT...`,
# where $w/course holds the sources, $w/aux the settled aux files and $r
# the results; locally only under a memory cap):
#
#   watch       `partex watch` (machine mode), from no aux files
#   watch-aux   the same, from the settled aux files
#   nomachine   `partex watch --no-machine`, from no aux files
#   plain       `partex --compat=pdftex` passes until the aux files settle
#   pdflatex    TeX Live's pdflatex, the same passes
#
# A WHAT given again runs again (its results in <what>.2, ...). Locally,
# COURSE, AUX and RESULTS name the directories, and BIN the binary
# (target/release/phitex, built if missing); PRELOAD is a library the
# watches run with (libjemalloc.so for a heap profile, with MALLOC_CONF):
#
#   systemd-run --user --scope -q -p MemoryMax=16G -p MemorySwapMax=0 \
#     scripts/sandbox env COURSE=... RESULTS=... bash bench/watch-course.sh watch
#
# Each run's resident set is sampled every second (<what>/rss.tsv: the
# seconds since its start, VmHWM and VmRSS in kB, the lines of its
# terminal output so far), so a pass's peak can be read off against its
# lines.
set -uo pipefail
P=$(pwd)
B=${BIN:-$P/target/release/phitex}
C=${COURSE:-${w:-}/course}
A=${AUX:-${w:-}/aux}
R=${RESULTS:-${r:-}}
S=$R/summary.txt
mkdir -p "$R"
say() { echo "$*" | tee -a "$S"; }
# (the dates fixed: the outputs of two runs compare byte for byte)
env0=(SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1)

if [ ! -x "$B" ]; then
  cargo build --release -p partex-cli -q || { say "BUILD FAILED"; exit 1; }
fi

# sample PID OUTFILE TSV: the resident set of PID each second
sample() {
  local pid=$1 out=$2 tsv=$3 t0
  t0=$(date +%s.%N)
  while kill -0 "$pid" 2>/dev/null; do
    local st
    st=$(awk '/^VmRSS|^VmHWM/ {printf "%s ", $2}' "/proc/$pid/status" 2>/dev/null)
    printf '%s\t%s\t%s\n' "$(awk -v a="$(date +%s.%N)" -v b="$t0" 'BEGIN {printf "%.1f", a - b}')" "$st" "$(wc -l <"$out" 2>/dev/null)" >>"$tsv"
    sleep 1
  done
}

# child PID: the command GNU time (PID) runs
child() {
  local c
  c=$(cat /proc/$1/task/*/children 2>/dev/null | awk '{print $1; exit}')
  echo "${c:-$1}"
}

# timed DIR CMD...: CMD under GNU time in the course's directory, sampled
timed() {
  local d=$1; shift
  (cd "$C" && exec env "${env0[@]}" /usr/bin/time -v -o "$d/time.txt" "$@" </dev/null >"$d/term.txt" 2>&1) &
  local sh=$!
  sleep 0.5
  local pid
  pid=$(child "$sh")
  sample "$pid" "$d/term.txt" "$d/rss.tsv" &
  wait "$sh"
  local e=$?
  echo "exit $e wall $(awk '/Elapsed/ {print $NF}' "$d/time.txt") maxrss_kb $(awk '/Maximum resident/ {print $NF}' "$d/time.txt")"
}

# watched DIR ARGS...: `partex watch ARGS` until it settles, then q
watched() {
  local d=$1; shift
  mkfifo "$d/stdin"
  (cd "$C" && exec env NO_COLOR=1 "${env0[@]}" PARTEX_STORE_DIR="$d/store" PARTEX_CACHE_DIR="$d/cache" \
    PARTEX_FORMATS="$R/formats" ${PRELOAD:+LD_PRELOAD=$PRELOAD} /usr/bin/time -v -o "$d/time.txt" "$B" watch "$@" <"$d/stdin" >"$d/term.txt" 2>&1) &
  local sh=$!
  # (q only once the build has settled: the watch reads its input then)
  echo q >"$d/stdin" &
  sleep 0.5
  local pid
  pid=$(child "$sh")
  sample "$pid" "$d/term.txt" "$d/rss.tsv" &
  wait "$sh"
  local e=$?
  echo "exit $e wall $(awk '/Elapsed/ {print $NF}' "$d/time.txt") maxrss_kb $(awk '/Maximum resident/ {print $NF}' "$d/time.txt")"
}

# (the format the watches use, made once, outside their time)
mkdir -p "$R/formats"
[ -f "$R/formats/pdflatex.fmt" ] || (cd "$R/formats" && "$B" --compat=pdftex -ini -interaction=batchmode \
  -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1)

for what in "$@"; do
  d=$R/$what
  n=1
  while [ -e "$d" ]; do n=$((n + 1)); d=$R/$what.$n; done
  mkdir -p "$d/out"
  case $what in
    watch) say "$(basename "$d"): $(watched "$d" -o "$d/out" course.tex)" ;;
    watch-aux)
      cp "$A"/course.* "$d/out/"
      say "$(basename "$d"): $(watched "$d" -o "$d/out" course.tex)"
      ;;
    nomachine) say "$(basename "$d"): $(watched "$d" --no-machine -o "$d/out" course.tex)" ;;
    plain | pdflatex)
      if [ $what = plain ]; then
        run=("$B" --compat=pdftex -fmt="$R/formats/pdflatex")
      else
        run=(pdflatex)
      fi
      for i in 1 2 3 4 5; do
        mkdir -p "$d/p$i"
        say "$what pass $i: $(timed "$d/p$i" "${run[@]}" -interaction=batchmode -output-directory="$d/out" course.tex)"
        for f in "$d"/out/course.*; do cp "$f" "$d/p$i/"; done
        if [ $i -gt 1 ] && cmp -s "$d/p$i/course.aux" "$d/p$((i - 1))/course.aux" &&
          cmp -s "$d/p$i/course.toc" "$d/p$((i - 1))/course.toc" &&
          cmp -s "$d/p$i/course.out" "$d/p$((i - 1))/course.out"; then
          break
        fi
      done
      ;;
    *) say "unknown: $what" ;;
  esac
  ls -la "$d/out" >"$d/out.ls" 2>&1
  # (the outputs, but not the store)
  rm -rf "$d/store" "$d/cache"
done
