#!/bin/bash
# r3-cuts (accl cmd task): amsldoc cold --exact-pdf wall at -j 1 and -j 8,
# commit $A against this checkout, interleaved in one job (release CLIs
# built side by side). Env: A=<rev>, RUNS (default 5).
set -u
B=$(git rev-parse --short=8 HEAD)
export CARGO_BUILD_JOBS=16
# (in this checkout one after the other: the tree has files a worktree lacks)
CARGO_TARGET_DIR=$w/t-b cargo build -q --release -p phitex-cli > "$r/build-b.txt" 2>&1; echo "EXIT $?" >> "$r/build-b.txt"
git -c advice.detachedHead=false checkout -q "$A"
CARGO_TARGET_DIR=$w/t-a cargo build -q --release -p phitex-cli > "$r/build-a.txt" 2>&1; echo "EXIT $?" >> "$r/build-a.txt"
git -c advice.detachedHead=false checkout -q "$B"
for x in a b; do grep -q '^EXIT 0$' "$r/build-$x.txt" || { echo "build $x failed" | tee "$r/summary.txt"; exit 1; }; done
run() { # bin cache j tag
  local D=$w/run-$4; rm -rf "$D"; cp -a bench/amsldoc "$D"
  local t0=$(date +%s%N)
  ( cd "$D" && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$2 "$1" build amsldoc.tex --exact-pdf -j "$3" > "$r/$4.err" 2>&1 )
  local t1=$(date +%s%N)
  echo "$4 $(( (t1 - t0) / 1000000 )) $(md5sum < "$D/amsldoc.pdf" | cut -c1-8) $(grep -aoE '[0-9]+ nodes run of [0-9]+ live' "$r/$4.err" | tail -1)" >> "$r/raw.txt"
}
for x in a b; do run "$w/t-$x/release/phitex" "$w/cache-$x" 8 warm-$x; done
for i in $(seq 1 ${RUNS:-5}); do
  for j in 1 8; do for x in a b; do run "$w/t-$x/release/phitex" "$w/cache-$x" $j $x-j$j-$i; done; done
done
python3 -I - "$r/raw.txt" "$A" "$B" <<'PY' | tee "$r/summary.txt"
import sys, statistics as S
rows=[l.split() for l in open(sys.argv[1]) if not l.startswith("warm")]
for j in ("1","8"):
    for x,c in (("a",sys.argv[2]),("b",sys.argv[3])):
        r=[l for l in rows if l[0].startswith(f"{x}-j{j}-")]
        w=[float(l[1])/1000 for l in r]
        print(f"-j {j} {c}: min {min(w):.2f} s median {S.median(w):.2f} s md5 {sorted(set(l[2] for l in r))} {' '.join(r[0][3:])}")
PY
