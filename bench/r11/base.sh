#!/bin/bash
# r11 baseline and company study (accl cmd task; $w, $r from job.sbatch).
# One profile CLI of this tree. amsldoc and para4000 at -j1/-j8/-j16 and
# amsldoc CLEAN_WINDOW=1 -j8, interleaved, REPS reps, op time by kind
# (PHITEX_TEX_TIME=1, CLEAN_PROF=1); hardware counters by pstat.py; then
# amsldoc -j1 with company: 7 amsldoc -j1 processes, 7 para4000 -j1
# processes, 7 busy loops; and -j8 CLEAN_WINDOW=1 with every single job
# on the pool (PHITEX_POOL_SINGLE: a run on another thread, no company).
set -u
cd "$(dirname "$0")/../.."
root=$PWD
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-32}
REPS=${REPS:-3}
S=$r/summary.txt
{
  echo "node $(hostname) $(date +%T)"
  echo "cpus allowed: $(grep Cpus_allowed_list /proc/self/status)"
  lscpu | grep -E 'Model name|^CPU\(s\)|Thread|Core|Socket|NUMA node|MHz|L3'
  echo "perf_event_paranoid $(cat /proc/sys/kernel/perf_event_paranoid 2>&1)"
  echo "governor $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>&1)"
} | tee -a "$S"
lscpu -e > "$r/lscpu-e.txt" 2>&1
nice cargo build -q --release -p phitex-cli --features phitex-tex/profile --target-dir $w/tp > "$r/build.txt" 2>&1 || { echo "build FAILED: $(grep -m2 -A6 '^error' $r/build.txt)" | tee -a "$S"; exit 1; }
B=$w/tp/release/phitex
E="SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1"
P="PHITEX_TEX_TIME=1 CLEAN_PROF=1"
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
J=8 run warm $A amsldoc.pdf "" $B build amsldoc.tex --exact-pdf -j 8
J=1 run warm-para $Q o-warm/para4000.pdf "" $B build -j 1 --output-dir=o-warm --program pdftex para4000.tex
for k in $(seq $REPS); do
  for J in 1 8 16; do ams ams-j$J-$k "$P"; para para-j$J-$k "$P"; done
  J=8 ams ams-w1-j8-$k "$P CLEAN_WINDOW=1"
  J=8 ams ams-w1pool-j8-$k "$P CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1"
  J=8 para para-w1-j8-$k "$P CLEAN_WINDOW=1"
done
# hardware counters, whole process (user mode)
PS="python3 -I $root/bench/r11/pstat.py"
for k in 1 2; do
  for J in 1 8; do ams ams-pstat-j$J-$k "$P" $PS; para para-pstat-j$J-$k "$P" $PS; done
  J=8 ams ams-pstat-w1-j8-$k "$P CLEAN_WINDOW=1" $PS
  J=8 ams ams-pstat-w1pool-j8-$k "$P CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1" $PS
done
# company: 7 other processes for the whole of an instrumented amsldoc -j1
company() { # kind: ams | para | spin
  rm -f $w/stop
  for i in 1 2 3 4 5 6 7; do
    case $1 in
      ams) d=$w/ca$i; rm -rf $d && cp -a bench/amsldoc $d && cp -a $w/cache $w/cc$i
        (cd $d && while [ ! -f $w/stop ]; do env $E PHITEX_CACHE_DIR=$w/cc$i $B build amsldoc.tex --exact-pdf -j 1 > /dev/null 2>&1; done) & ;;
      para) d=$w/cp$i; mkdir -p $d && cp bench/para4000.tex $d/ && cp -a $w/cache $w/cq$i
        (cd $d && while [ ! -f $w/stop ]; do env $E PHITEX_CACHE_DIR=$w/cq$i $B build -j 1 --output-dir=o --program pdftex para4000.tex > /dev/null 2>&1; done) & ;;
      spin) (while [ ! -f $w/stop ]; do :; done) & ;;
    esac
  done
  sleep 3
}
endco() { touch $w/stop; wait; rm -rf $w/ca* $w/cc* $w/cp* $w/cq*; }
for co in ams para spin; do
  company $co
  for k in 1 2 3; do J=1 ams ams-j1-with-$co-$k "$P" $PS; done
  for k in 1 2; do J=1 para para-j1-with-$co-$k "$P" $PS; done
  endco
done
echo "end $(date +%T)" | tee -a "$S"
