#!/bin/bash
# SyncTeX's cost (DESIGN 4.5), in the cluster container (job.sbatch `cmd`
# task, run from the checkout; $w is the workspace, $r the results):
#
#   synctex-ab.sh OLD [ROUNDS]
#
# OLD built in a worktree, the checkout built; the PGF subset
# (bench/inputs/pgfsub.tex, its aux files settled by TeX Live's pdflatex)
# run ROUNDS times (5) alternating: OLD; the checkout without -synctex
# (its cost off); the checkout with -synctex=1 (on). Each run's wall and
# user seconds, peak RSS and (with perf) user instructions to
# $r/summary.txt; the PDFs compared (/ID aside), OLD's against the
# checkout's off and on.
set -uo pipefail
old=${1:?old commit} n=${2:-5}
P=$PWD
S=$r/summary.txt
say() { echo "$*" | tee -a "$S"; }
git worktree add -q --detach "$w/old" "$old" || exit 1
(cd "$w/old" && cargo build --release -p partex-cli -q) || exit 1
cargo build --release -p partex-cli -q || exit 1
bin() { if [ "$1" = old ]; then echo "$w/old/target/release/partex"; else echo "$P/target/release/partex"; fi; }
pgf=$P/upstream/pgf/doc/generic/pgf
env0=(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC)
for which in old new; do
  f=$w/fmt-$which; mkdir -p "$f"
  (cd "$f" && ln -sf "$(bin $which)" pdftex && env "${env0[@]}" ./pdftex -ini -interaction=batchmode \
    -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1; rm -f pdftex)
  [ -f "$f/pdflatex.fmt" ] || { say "format $which FAILED"; exit 1; }
done
a=$w/aux; mkdir -p "$a"
for i in 1 2 3; do
  (cd "$pgf" && env "${env0[@]}" TEXINPUTS="$a:.:" pdflatex -interaction=batchmode \
    -output-directory="$a" "$P/bench/inputs/pgfsub.tex" >/dev/null 2>&1)
done
perf=$(command -v perf || true)
say "node $(hostname), old $(git -C "$w/old" rev-parse --short HEAD), new $(git rev-parse --short HEAD), perf ${perf:-none}"
# run CONFIG: old | off | on
run() {
  local c=$1 b o=$w/out-$1 opt=()
  b=$(bin "$( [ "$c" = old ] && echo old || echo new)")
  [ "$c" = on ] && opt=(-synctex=1)
  rm -rf "$o"; mkdir -p "$o"; cp "$a"/*.aux "$a"/*.out "$a"/*.toc "$o"/ 2>/dev/null
  local fmt=$w/fmt-$( [ "$c" = old ] && echo old || echo new)
  local pre=()
  [ -n "$perf" ] && pre=("$perf" stat -x, -o "$o/perf.txt" -e instructions:u)
  (cd "$pgf" && env "${env0[@]}" TEXFORMATS="$fmt:" TEXINPUTS="$o:.:" PARTEX_CACHE_DIR="$w/cache-$c" \
    /usr/bin/time -f "%e %U %M" -o "$o/time.txt" "${pre[@]}" "$b" --compat=pdftex -fmt=pdflatex \
    "${opt[@]}" -interaction=batchmode -output-directory="$o" "$P/bench/inputs/pgfsub.tex" \
    >/dev/null 2>&1 </dev/null)
  local ins=-
  [ -f "$o/perf.txt" ] && ins=$(grep instructions "$o/perf.txt" | cut -d, -f1)
  echo "$(cat "$o/time.txt") $ins"
}
for c in old off on; do run $c >/dev/null; done
for i in $(seq "$n"); do
  for c in old off on; do say "$c $i: $(run $c)"; done
done
strip() { sed 's|/ID \[<[0-9A-F]*> <[0-9A-F]*>\]||' "$1"; }
for c in off on; do
  if cmp -s <(strip "$w/out-old/pgfsub.pdf") <(strip "$w/out-$c/pgfsub.pdf"); then
    say "pdf $c: same as old"
  else
    say "pdf $c: DIFFERS from old"
  fi
done
ls -l "$w/out-on/"pgfsub.synctex* 2>&1 | tee -a "$S"
[ -f "$w/out-off/pgfsub.synctex.gz" ] && say "off wrote a synctex file"
cp "$w/out-on/pgfsub.synctex.gz" "$r/" 2>/dev/null
true
