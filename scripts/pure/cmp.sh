#!/bin/sh
# cmp.sh DIR ARGS...: run the job in DIR/a plain and DIR/b pure (from DIR/src), compare.
# BIN env: the binary (default fastdev); PUREENV: extra env for the pure run.
R=$(cd "$(dirname "$0")/../.." && pwd)
B=${BIN:-$R/target/fastdev/phitex}
S=${SANDBOX-$R/scripts/sandbox}
d=$1; shift
rm -rf "$d/a" "$d/b"; mkdir -p "$d/a" "$d/b"
cp -r "$d/src/." "$d/a/"; cp -r "$d/src/." "$d/b/"
(cd "$d/a" && timeout 300 $S env SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 $B "$@" > term.txt 2> err.txt; echo "plain exit $?")
(cd "$d/b" && env PARTEX_SSA=1 PHITEX_SSA_PURE=1 $PUREENV timeout 300 $S env SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 $B "$@" > term.txt 2> err.txt; echo "pure exit $?")
grep -v "^phitex: pure ssa" "$d/b/err.txt" | head -20
grep "^phitex: pure ssa" "$d/b/err.txt" | cut -c1-400
for f in $(cd "$d/a" && ls); do
  case $f in err.txt) continue;; esac
  if [ ! -e "$d/b/$f" ]; then echo "MISSING $f"; continue; fi
  case $f in
    *.log) if diff -q <(tail -n +2 "$d/a/$f") <(tail -n +2 "$d/b/$f") >/dev/null; then echo "same $f"; else echo "DIFF $f"; fi;;
    *) if cmp -s "$d/a/$f" "$d/b/$f"; then echo "same $f"; else echo "DIFF $f"; fi;;
  esac
done
