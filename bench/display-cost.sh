#!/usr/bin/env bash
# Display lists' cost (DESIGN 4.6), in an accl `cmd` job (the cluster's
# container; from the checkout, $w its workspace, $r its results):
#
#   scripts/accl/accl run cmd bench/display-cost.sh BASE [RUNS]
#
# The tree's release binary against BASE's, built the same way, on the
# PGF subset (bench/inputs/pgfsub.tex: four chapters of the manual) and
# the course ($w/course, its settled auxiliary files in $w/aux): plain
# runs on settled auxiliary files, RUNS (3) of each, interleaved, with
# display lists off (BASE, and the tree), kept (`PARTEX_DISPLAY=keep`:
# what keeping them costs a build), and once written
# (`PARTEX_DISPLAY=1`: every page's list made, the time on stderr); then
# the PGF subset in SSA mode, cold with a word edit rebuilt in the same
# process (off and written). Each run's wall time and peak RSS (GNU
# time), and its user instructions where `perf stat -e instructions:u`
# counts; every PDF compared with BASE's, byte for byte.
set -uo pipefail
base=${1:?base commit}
runs=${2:-3}
P=$PWD
S=$r/summary.txt
say() { echo "$*" | tee -a "$S"; }
env0=(SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC)

# (the binary: phitex, or partex in a commit from before the rename)
rel() { if [ -x "$1/phitex" ]; then echo "$1/phitex"; else echo "$1/partex"; fi; }
# the two binaries, built as the tree's profile says (fat LTO)
mkdir -p "$w/bin"
cargo build --release -p partex-cli -q || { say "BUILD FAILED"; exit 1; }
cp target/release/phitex "$w/bin/new"
git -c advice.detachedHead=false worktree add -q -f "$w/base" "$base" || { say "NO BASE $base"; exit 1; }
(cd "$w/base" && CARGO_TARGET_DIR="$w/base-target" cargo build --release -p partex-cli -q) ||
  { say "BASE BUILD FAILED"; exit 1; }
cp "$(rel "$w/base-target/release")" "$w/bin/base"
say "binaries: new $(git rev-parse --short=12 HEAD), base $(git -C "$w/base" rev-parse --short=12 HEAD)"

# each binary's pdflatex format
for b in new base; do
  d=$w/fmt-$b
  mkdir -p "$d"
  (cd "$d" && ln -sf "$w/bin/$b" pdftex && timeout -s KILL 600 ./pdftex -ini -interaction=batchmode \
    -jobname=pdflatex -translate-file=cp227.tcx '*pdflatex.ini' >/dev/null 2>&1; rm -f pdftex)
  [ -f "$d/pdflatex.fmt" ] || { say "FORMAT FAILED ($b)"; exit 1; }
done

insn=1
perf stat -e instructions:u -x, -o /dev/null -- true >/dev/null 2>&1 || insn=0
say "perf instructions:u: $([ $insn = 1 ] && echo yes || echo no)"

# run <label> <bin> <srcdir> <outdir> <input> [NAME=VALUE...]: one build
# of <input> run from <srcdir> into <outdir>; prints its numbers
run() {
  local label=$1 b=$2 src=$3 o=$4 input=$5
  shift 5
  local pre=()
  [ $insn = 1 ] && pre=(perf stat -e instructions:u -x, -o "$o/perf.csv" --)
  (cd "$src" && /usr/bin/time -f "%e %M" -o "$o/time.txt" "${pre[@]}" timeout -s KILL 7200 \
    env "${env0[@]}" TEXFORMATS="$w/fmt-$b:" PARTEX_CACHE_DIR="$w/cache-$b" "$@" \
    "$w/bin/$b" --compat=pdftex -fmt=pdflatex -interaction=batchmode -output-directory="$o" "$input" \
    </dev/null >"$o/term.txt" 2>"$o/err.txt")
  local e=$?
  local t m i
  read -r t m < <(tail -1 "$o/time.txt")
  i=$([ -f "$o/perf.csv" ] && awk -F, '/instructions/ {print $1}' "$o/perf.csv" | tail -1)
  say "$label: exit $e wall ${t}s maxrss ${m}kB instructions ${i:-?}"
}

# a document's runs: <name> <srcdir> <input> <auxdir> <job> [NAME=VALUE...]
doc() {
  local name=$1 src=$2 input=$3 aux=$4 job=$5
  shift 5
  local o=$w/out
  fresh() {
    rm -rf "$o"
    mkdir -p "$o"
    for x in aux toc out; do cp "$aux"/*."$x" "$o"/ 2>/dev/null; done
    true
  }
  for i in $(seq "$runs"); do
    for c in base:off new:off new:keep; do
      b=${c%%:*} k=${c#*:}
      fresh
      if [ "$k" = keep ]; then run "$name $c $i" "$b" "$src" "$o" "$input" "$@" PARTEX_DISPLAY=keep
      else run "$name $c $i" "$b" "$src" "$o" "$input" "$@"; fi
      cp "$o/$job.pdf" "$w/$name-$b-$k.pdf"
    done
  done
  fresh
  run "$name new:written" new "$src" "$o" "$input" "$@" PARTEX_DISPLAY=1
  grep -a "display lists:" "$o/err.txt" | tee -a "$S"
  cp "$o/$job.pdf" "$w/$name-new-written.pdf"
  for k in new-off new-keep new-written; do
    cmp -s "$w/$name-base-off.pdf" "$w/$name-$k.pdf" && say "$name $k: PDF same as base" ||
      say "$name $k: PDF DIFFERS from base"
  done
}

# the PGF subset, its auxiliary files settled by pdflatex (three runs)
pgf=$P/upstream/pgf/doc/generic/pgf
sub=$P/bench/inputs/pgfsub.tex
ref=$w/pgf-ref
mkdir -p "$ref"
for i in 1 2 3; do
  (cd "$pgf" && env "${env0[@]}" TEXINPUTS="$ref:.:" pdflatex -interaction=batchmode \
    -output-directory="$ref" "$sub" </dev/null >/dev/null 2>&1)
done
doc pgfsub "$pgf" "$sub" "$ref" pgfsub TEXINPUTS="$w/out:.:"

# the course
doc course "$w/course" course.tex "$w/aux" course

# the PGF subset in SSA mode: cold, then a word edit rebuilt
cp -a "$pgf" "$w/pgf-ssa"
tut=$w/pgf-ssa/pgfmanual-en-tutorial.tex
cp "$tut" "$w/tutorial.orig"
for c in base:off new:off new:written; do
  b=${c%%:*} k=${c#*:}
  cp "$w/tutorial.orig" "$tut"
  o=$w/out
  rm -rf "$o"
  mkdir -p "$o"
  for x in aux toc out; do cp "$ref"/*."$x" "$o"/ 2>/dev/null; done
  edit="sed -i 's/a math and chemistry/a maths and chemistry/' $tut"
  if [ "$k" = written ]; then
    run "pgfsub ssa $c" "$b" "$w/pgf-ssa" "$o" "$sub" TEXINPUTS="$o:.:" PARTEX_SSA=1 \
      PARTEX_SSA_TRIPS=1 PARTEX_DISPLAY=1 "PARTEX_SSA_REBUILD=$edit"
  else
    run "pgfsub ssa $c" "$b" "$w/pgf-ssa" "$o" "$sub" TEXINPUTS="$o:.:" PARTEX_SSA=1 \
      PARTEX_SSA_TRIPS=1 "PARTEX_SSA_REBUILD=$edit"
  fi
  grep -a "ssa build 0: [0-9.]* ms\|ssa rebuild 1: [0-9.]* ms\|display lists:\|panicked" "$o/err.txt" |
    cut -c1-400 | tee -a "$S"
  cp "$o/pgfsub.pdf" "$w/ssa-$b-$k.pdf"
done
for k in new-off new-written; do
  cmp -s "$w/ssa-base-off.pdf" "$w/ssa-$k.pdf" && say "pgfsub ssa $k: PDF same as base" ||
    say "pgfsub ssa $k: PDF DIFFERS from base"
done
exit 0
