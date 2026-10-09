#!/bin/bash
# edit.sh DIR ARGS...: DIR/src holds the job; DIR/edits/1, 2, ... hold files
# copied over it in turn. Pure: one process, a rebuild after each edit
# (outputs snapshot after each); plain: a cold run of each stage. Files
# are stamped as e2e stamps them: the inputs at T0, edit k's at T0+1000k.
R=$(cd "$(dirname "$0")/../.." && pwd)
B=${BIN:-$R/target/fastdev/phitex}
S=${SANDBOX-$R/scripts/sandbox}
T0=1758800000
d=$(realpath "$1"); shift
rm -rf "$d/w"; mkdir -p "$d/w/p"
cp -r "$d/src/." "$d/w/p/"
(cd "$d/w/p" && find . -type f -exec touch -d @$T0 {} +)
n=$(ls "$d/edits" 2>/dev/null | wc -l)
cmds=""; NL=$'\n'
for k in $(seq 1 $n); do
  files=$(cd "$d/edits/$k" && ls)
  cmds="$cmds$NL""mkdir -p ../snap$((k-1)) && cp -r . ../snap$((k-1))/ && cp -r ../../edits/$k/. . && touch -d @$((T0+1000*k)) $files"
done
mkdir -p "$d/w/snap$n"
cmds="${cmds#$NL}"
(cd "$d/w/p" && env PARTEX_SSA=1 PHITEX_SSA_PURE=1 PARTEX_SSA_REBUILD="$cmds" $PUREENV timeout 600 $S env SOURCE_DATE_EPOCH=$T0 FORCE_SOURCE_DATE=1 $B "$@" > term.txt 2> err.txt; echo "pure exit $?")
cp -r "$d/w/p/." "$d/w/snap$n/"
grep -v "^phitex: pure ssa" "$d/w/p/err.txt" | head -20
grep "^phitex: pure ssa" "$d/w/p/err.txt" | cut -c1-300
for k in $(seq 0 $n); do
  o="$d/w/o$k"; mkdir -p "$o"; cp -r "$d/src/." "$o/"
  (cd "$o" && find . -type f -exec touch -d @$T0 {} +)
  for j in $(seq 1 $k); do cp -r "$d/edits/$j/." "$o/"; (cd "$d/edits/$j" && for f in *; do touch -d @$((T0+1000*j)) "$o/$f"; done); done
  (cd "$o" && timeout 600 $S env SOURCE_DATE_EPOCH=$T0 FORCE_SOURCE_DATE=1 $B "$@" > term.txt 2> err.txt)
  for f in $(cd "$o" && ls); do
    case $f in err.txt|term.txt) continue;; esac
    s="$d/w/snap$k/$f"
    if [ ! -e "$s" ]; then echo "stage $k MISSING $f"; continue; fi
    case $f in
      *.log) diff -q <(tail -n +2 "$o/$f") <(tail -n +2 "$s") >/dev/null && echo "stage $k same $f" || echo "stage $k DIFF $f";;
      *) cmp -s "$o/$f" "$s" && echo "stage $k same $f" || echo "stage $k DIFF $f";;
    esac
  done
done
