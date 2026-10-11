#!/bin/bash
# Builds the tests (release unless PROFILE=debug) and runs each one alone with
# a timeout, at low priority inside bubblewrap.
cd "$(dirname "$0")/.." || exit 1
P=${PROFILE:-release}
F=; [ "$P" = release ] && F=--release
scripts/nc.sh test $F --no-run -q 2>&1 | grep -E "^(error|warning: unused)" -A5
T=$(ls -t target/$P/build/defuse/*/out/engine-* target/$P/deps/engine-* 2>/dev/null | grep -v "\.d$" | head -1)
N="nice -n 19 ionice -c3 /home/lambda/.local/bin/bw"
for t in $($N $T --list 2>/dev/null | grep ': test' | sed 's/: test//' | grep -E "${1:-.}"); do
  s=$(date +%s.%N)
  o=$(timeout ${TMO:-60} $N $T --exact $t -q 2>&1); rc=$?
  r=$(echo "$o" | grep -E "panicked|left|right" | head -3 | tr '\n' ' ')
  [ $rc = 124 ] && r=TIMEOUT
  [ $rc != 0 ] && [ -z "$r" ] && r="FAILED rc=$rc"
  printf '%s => %s (%.1fs)\n' "$t" "${r:-ok}" "$(echo "$(date +%s.%N) - $s" | bc)"
done
