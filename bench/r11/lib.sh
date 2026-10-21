# r11 helpers (sourced; accl cmd task: $w, $r from job.sbatch): node facts,
# the profile CLI of this tree, run/ams/para.
set -u
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
root=$PWD
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-32}
S=$r/summary.txt
{
  echo "node $(hostname) $(date +%T) commit $(git rev-parse --short HEAD)"
  echo "cpus allowed: $(grep Cpus_allowed_list /proc/self/status)"
} | tee -a "$S"
lscpu -e > "$r/lscpu-e.txt" 2>&1
nice cargo build -q --release -p phitex-cli --features phitex-tex/profile --target-dir $w/tp > "$r/build.txt" 2>&1 || { echo "build FAILED: $(grep -m2 -A12 '^error' $r/build.txt)" | tee -a "$S"; exit 1; }
B=$w/tp/release/phitex
E="SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1"
P="PHITEX_TEX_TIME=1 CLEAN_PROF=1"
PS="python3 -I $root/bench/r11/pstat.py"
run() { # tag dir pdf extra-env cmd...
  local tag=$1 d=$2 pdf=$3 x=$4; shift 4
  rm -f "$d/$pdf"
  local s0=$(date +%s.%N)
  (cd "$d" && env $E PHITEX_CACHE_DIR=${CACHE:-$w/cache} $x timeout 1200 "$@" > "$r/$tag.log" 2> "$r/$tag.err"); local e=$?
  local wall=$(awk -v a=$s0 -v b=$(date +%s.%N) 'BEGIN{printf "%.2f", b-a}')
  local rw=$(grep -m1 '^acct: round 0' "$r/$tag.err" | sed -E 's/.*wall ([0-9.]+) ms, op ([0-9.]+) ms, commit ([0-9.]+) ms.*runs ([0-9]+).*/rwall \1 op \2 commit \3 runs \4/')
  local k=$(grep -m1 '^acct: op time by kind' "$r/$tag.err" | sed 's/^acct: op time by kind: //')
  local ps=$(grep -m1 '^pstat:' "$r/$tag.err" | sed 's/^pstat: //')
  echo "$tag: exit $e wall $wall $( [ -f "$d/$pdf" ] && md5sum < "$d/$pdf" | cut -c1-8) | $rw | $k | $ps" | tee -a "$S"
}
A=$w/a; rm -rf $A && cp -a bench/amsldoc $A
Q=$w/p; mkdir -p $Q && cp bench/para4000.tex $Q/
ams() { run "$1" $A amsldoc.pdf "$2" "${@:3}" $B build amsldoc.tex --exact-pdf -j "$J"; }
para() { run "$1" $Q o-$1/para4000.pdf "$2" "${@:3}" $B build -j "$J" --output-dir=o-$1 --program pdftex para4000.tex; }
warm() {
  J=8 run warm $A amsldoc.pdf "" $B build amsldoc.tex --exact-pdf -j 8
  J=1 run warm-para $Q o-warm/para4000.pdf "" $B build -j 1 --output-dir=o-warm --program pdftex para4000.tex
}
