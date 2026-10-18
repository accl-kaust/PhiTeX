#!/bin/bash
# One shard of the fast gate (scripts/accl/gate-fast), as job.sbatch's
# `cmd` task runs it: inside the container, in the commit's checkout,
# $r the results directory, $w the workspace. gate-fast passes this
# file's text to `bash -c` (the commit under test need not have it).
#
#   gate-shard.sh build then one line an edit)             the commit's binaries, once: phitex-tex's
# then one line an edit)                                   test binaries (fastdev) and the release
# then one line an edit)                                   CLI, built side by side, left in
# then one line an edit)                                   $r/store for the other shards
# then one line an edit)                                   (job.sbatch ACCL_STORE_OUT/IN)
#   gate-shard.sh lint then one line an edit)              clippy -D warnings (no fmt: version skew)
#   gate-shard.sh suites then one line an edit)            the local suites of phitex-tex
#   gate-shard.sh latex K N then one line an edit)         the latex target's tests, shard K of N
#   gate-shard.sh ams ORACLE_MD5 CTL then one line an edit)amsldoc --exact-pdf at -j 1, 8 and 16;
# then one line an edit)                                   cold watch timing at -j 1/8/16, the
# then one line an edit)                                   head beside control CTL's recorded
# then one line an edit)                                   timings ($w/gate-control/CTL)
#   gate-shard.sh amsedit J then one line an edit)         amsldoc's edit history at -j J
# then one line an edit)                                   (GATE_AMS_EDITS one-word edits, 10)
#   gate-shard.sh control CTL then one line an edit)       control CTL's CLI built, its cold
# then one line an edit)                                   watch timings at -j 1/8/16 (recorded
# then one line an edit)                                   by `gate-fast control`)
#   gate-shard.sh cg then one line an edit)                para4000 at one thread under callgrind:
# then one line an edit)                                   startup Ir, node bodies' Ir per
# then one line an edit)                                   command, top 15 functions
#   gate-shard.sh para BAR_MD5 then one line an edit)      bench/para4000.tex (plain pdfTeX) cold
# then one line an edit)                                   at -j 1/8/16, three runs each (the
# then one line an edit)                                   node is shared: the min quoted, the
# then one line an edit)                                   median beside it; the node's name
# then one line an edit)                                   and load first): md5 of every run
# then one line an edit)                                   against BAR_MD5, wall, us per
# then one line an edit)                                   paragraph, realised critical path,
# then one line an edit)                                   guess counters
#
# The shards but build and lint build nothing: they run the build
# shard's binaries from $w/bin (built at the same path, ACCL_ROOT). A test
# runs in its own process, GATE_TEST_PAR at a time, killed after
# GATE_TEST_TIMEOUT s (120): a hang is a red line.
#
# Writes $r/shard.txt, $r/status (green|red), $r/red.txt (one red name a
# line), $r/times.txt (build_s, wall_s) and the shard's logs.
set -u
shard=$1
shift
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-8} PHITEX_CACHE_DIR=$w/cache
export PHITEX_TEST_LIMIT=${PHITEX_TEST_LIMIT:-300}
mkdir -p "$PHITEX_CACHE_DIR"
t0=$(date +%s)
echo "$shard $*" > "$r/shard.txt"
: > "$r/red.txt"
status=green
red() { echo "$1" >> "$r/red.txt"; status=red; }
built() { echo "build_s $(( $(date +%s) - t0 ))" >> "$r/times.txt"; }
T="cargo test --profile fastdev -p phitex-tex --features profile,observe"
# the test targets the shards run (the build shard builds them)
SUITES="pdftex edits pages plain etex hints inputs defuse_gaps"
# The compiler cache (job.sbatch's PHITEX_SCCACHE, on the shared home):
# the compiler's wrapper once its server starts, else none (the build as
# before). Incremental builds are not cacheable, and a fresh target
# directory gains nothing from them: off with the cache.
cache_on() {
  [ -n "${PHITEX_SCCACHE:-}" ] || return 0
  if "$PHITEX_SCCACHE" --start-server > "$r/sccache-start.txt" 2>&1; then
 then one line an edit)export RUSTC_WRAPPER=$PHITEX_SCCACHE CARGO_INCREMENTAL=0
  else
 then one line an edit)echo "sccache: no server, building without it: $(tail -1 "$r/sccache-start.txt")"
  fi
}
# its counters in $r/sccache.txt, and the server stopped (its writes done)
cache_off() {
  [ -n "${RUSTC_WRAPPER:-}" ] || return 0
  "$RUSTC_WRAPPER" --show-stats > "$r/sccache.txt" 2>&1
  "$RUSTC_WRAPPER" --stop-server > /dev/null 2>&1
  echo "sccache: $(grep -E '^(Compile requests|Cache hits|Cache misses|Non-cacheable calls) ' "$r/sccache.txt" | sed -E 's/  +/ /g' | tr '\n' ',' | sed 's/,$//')"
}
# the build's compile time, as the shard's build_s
fetched() { echo "build_s $(cat "$w/bin/build_s" "$w/bin/tests/build_s" "$w/bin/cli/build_s" 2>/dev/null | head -1 | grep . || echo 0)" >> "$r/times.txt"; }
# runtests BIN PREFIX LIST DIR: each test of LIST (names, one a line) its
# own process from the crate's directory as cargo runs it, GATE_TEST_PAR at
# a time, killed after GATE_TEST_TIMEOUT s; DIR/<test>.txt its log,
# DIR/walls.txt `name wall_s exit`; the red ones named (a timeout as such)
runtests() {
  local bin=$1 pre=$2 list=$3 d=$4 t e
  mkdir -p "$d"
  : > "$d/walls.txt"
  [ -s "$list" ] || return 0
  ( cd crates/engines/tex && export bin d CARGO_MANIFEST_DIR=$PWD TO=${GATE_TEST_TIMEOUT:-120} RT_EXTRA=${RT_EXTRA:-} &&
 then one line an edit)xargs -a "$list" -d '\n' -P "${GATE_TEST_PAR:-4}" -I{} bash -c '
 then one line an edit)  t=$1; f=$d/$(tr ":/" "__" <<< "$t").txt
 then one line an edit)  s0=$(date +%s.%N)
 then one line an edit)  timeout -k 10 "$TO" "$bin" $RT_EXTRA --exact "$t" > "$f" 2>&1
 then one line an edit)  e=$?
 then one line an edit)  s1=$(date +%s.%N)
 then one line an edit)  echo "EXIT $e" >> "$f"
 then one line an edit)  echo "$t $(awk -v a="$s0" -v b="$s1" "BEGIN{printf \"%.3f\", b-a}") $e" >> "$d/walls.txt"
 then one line an edit)  # (a timeout: the test again under gdb, interrupted at the same
 then one line an edit)  # limit, every thread'"'"'s backtrace kept beside its log: gdb as the
 then one line an edit)  # parent may trace it, an attach may not)
 then one line an edit)  if [ "$e" = 124 ] || [ "$e" = 137 ]; then
 then one line an edit)    timeout -s INT -k 30 "$TO" gdb -batch -nx -ex "set pagination off" -ex run \
 then one line an edit)      -ex "thread apply all bt 40" -ex kill --args "$bin" $RT_EXTRA --exact "$t" \
 then one line an edit)      > "${f%.txt}.gdb" 2>&1
 then one line an edit)    echo "backtraces after ${TO} s under gdb: ${f%.txt}.gdb ($(grep -c "^Thread " "${f%.txt}.gdb") threads)" >> "$f"
 then one line an edit)  fi' _ {} )
  while read -r t _ e; do
 then one line an edit)case $e in
 then one line an edit)  0) ;;
 then one line an edit)  124 | 137) red "$pre$t (timed out after ${GATE_TEST_TIMEOUT:-120} s; every thread's backtrace under gdb: $(basename "$d")/$(tr ":/" "__" <<< "$t").gdb)" ;;
 then one line an edit)  *) red "$pre$t$(grep -q "^test $t \.\.\. FAILED" "$d/$(tr ":/" "__" <<< "$t").txt" || echo " (exit $e: $(grep -m1 -E 'panicked at|error(\[|:)|signal' "$d/$(tr ":/" "__" <<< "$t").txt" | cut -c1-140))")" ;;
 then one line an edit)esac
  done < "$d/walls.txt"
}
# the walls of runtests' DIR as test-walls.txt has them: `name wall ok|FAILED`
walls() { awk -v p="$2" '{print p $1, $2, ($3 == 0 ? "ok" : "FAILED")}' "$1/walls.txt"; }
# the red tests of a libtest log, else why the binary gave none
reds() {
  local log=$1 pre=$2 n
  n=$(grep -E '^test .* \.\.\. FAILED( <.*)?$' "$log" | sed -E 's/^test (.*) \.\.\. FAILED( <.*)?$/\1/')
  if [ -n "$n" ]; then
 then one line an edit)for x in $n; do red "$pre$x"; done
  elif ! grep -q '^EXIT 0$' "$log"; then
 then one line an edit)red "$pre(exit $(sed -n 's/^EXIT //p' "$log" | tail -1), no test named: $(grep -m1 -E 'panicked at|error(\[|:)|timed out|signal' "$log" | cut -c1-160))"
  fi
}
# One logical CPU per physical core of the allowed set (the lowest of
# each core's siblings), comma-separated: the -j runs up to the core
# count pin to it; the SMT run takes the whole set.
one_per_core() {
  python3 -I -c '
import os
def expand(s):
 then one line an edit)out = []
 then one line an edit)for part in s.split(","):
 then one line an edit)    a, _, b = part.partition("-")
 then one line an edit)    out.extend(range(int(a), int(b or a) + 1))
 then one line an edit)return out
cpus = sorted(os.sched_getaffinity(0))
seen, pick = set(), []
for c in cpus:
 then one line an edit)try:
 then one line an edit)    sib = expand(open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip())
 then one line an edit)except OSError:
 then one line an edit)    sib = [c]
 then one line an edit)key = min(sib)
 then one line an edit)if key not in seen:
 then one line an edit)    seen.add(key)
 then one line an edit)    pick.append(c)
print(",".join(map(str, pick)))'
}

case $shard in
  lint)
 then one line an edit)# (no fmt: the image's rustfmt is another version than the pinned
 then one line an edit)# nightly's and reflows what the local one keeps, as job.sbatch's
 then one line an edit)# gate since b2fbf61a; clippy skips the vendored engine)
 then one line an edit)built
 then one line an edit)cache_on
 then one line an edit)if ! cargo clippy --workspace --exclude defuse --all-targets --keep-going -q --message-format short -- -D warnings > "$r/clippy.txt" 2>&1; then
 then one line an edit)  grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)' "$r/clippy.txt" | sort -u | head -60 | while IFS= read -r l; do echo "clippy: $l"; done >> "$r/red.txt"
 then one line an edit)  grep -q . "$r/red.txt" || echo "clippy: $(grep -m1 '^error' "$r/clippy.txt")" >> "$r/red.txt"
 then one line an edit)  status=red
 then one line an edit)fi
 then one line an edit)cache_off
 then one line an edit);;
  build)
 then one line an edit)# (the test binaries and the CLI in their own target directories,
 then one line an edit)# side by side: one cargo would hold the other's lock)
 then one line an edit)cache_on
 then one line an edit)a=()
 then one line an edit)for s in latex $SUITES; do a+=(--test "$s"); done
 then one line an edit)( $T "${a[@]}" --no-run --message-format=json-render-diagnostics > "$r/build-tests.json" 2> "$r/build-tests.txt"; echo "EXIT $?" >> "$r/build-tests.txt" ) &
 then one line an edit)# (three CLIs: the checks' (profile and observe: CHECK_READS, the
 then one line an edit)# recordings, amsldoc), the timings' (profile: the acct lines without
 then one line an edit)# the engine's recorder) and the users' (no feature))
 then one line an edit)( CARGO_TARGET_DIR=$PWD/target-cli cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt" ) &
 then one line an edit)( CARGO_TARGET_DIR=$PWD/target-cli-prof cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build-prof.txt" 2>&1; echo "EXIT $?" >> "$r/build-prof.txt" ) &
 then one line an edit)( CARGO_TARGET_DIR=$PWD/target-cli-rel cargo build -q --release -p phitex-cli > "$r/build-rel.txt" 2>&1; echo "EXIT $?" >> "$r/build-rel.txt" ) &
 then one line an edit)wait
 then one line an edit)built
 then one line an edit)cache_off
 then one line an edit)grep -q '^EXIT 0$' "$r/build-tests.txt" || red "build (tests): $(grep -m1 '^error' "$r/build-tests.txt")"
 then one line an edit)grep -q '^EXIT 0$' "$r/build.txt" || red "build (cli): $(grep -m1 '^error' "$r/build.txt")"
 then one line an edit)grep -q '^EXIT 0$' "$r/build-prof.txt" || red "build (cli, profile): $(grep -m1 '^error' "$r/build-prof.txt")"
 then one line an edit)grep -q '^EXIT 0$' "$r/build-rel.txt" || red "build (cli, release): $(grep -m1 '^error' "$r/build-rel.txt")"
 then one line an edit)if [ $status = green ]; then
 then one line an edit)  mkdir -p "$r/store.tmp/tests" "$r/store.tmp/cli"
 then one line an edit)  python3 -I -c '
import json, os, shutil, sys
for l in open(sys.argv[1]):
 then one line an edit)try:
 then one line an edit)    m = json.loads(l)
 then one line an edit)except ValueError:
 then one line an edit)    continue
 then one line an edit)if m.get("reason") == "compiler-artifact" and m.get("executable") and "test" in m["target"]["kind"]:
 then one line an edit)    shutil.copy2(m["executable"], os.path.join(sys.argv[2], m["target"]["name"]))' "$r/build-tests.json" "$r/store.tmp/tests"
 then one line an edit)  cp target-cli/release/phitex "$r/store.tmp/cli/phitex"
 then one line an edit)  # (each under the name `phitex`: the CLI is a TeX-compatible
 then one line an edit)  # command line under any other name)
 then one line an edit)  mkdir -p "$r/store.tmp/cli/prof" "$r/store.tmp/cli/rel"
 then one line an edit)  cp target-cli-prof/release/phitex "$r/store.tmp/cli/prof/phitex"
 then one line an edit)  cp target-cli-rel/release/phitex "$r/store.tmp/cli/rel/phitex"
 then one line an edit)  sed -n 's/^build_s //p' "$r/times.txt" | tee "$r/store.tmp/tests/build_s" > "$r/store.tmp/cli/build_s"
 then one line an edit)  ls -la "$r/store.tmp/tests" "$r/store.tmp/cli" > "$r/store.txt"
 then one line an edit)  mv "$r/store.tmp" "$r/store"
 then one line an edit)fi
 then one line an edit)rm -f "$r/build-tests.json"
 then one line an edit);;
  suites)
 then one line an edit)fetched
 then one line an edit)for s in $SUITES; do
 then one line an edit)  "$w/bin/tests/$s" --list 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/all-$s.txt"
 then one line an edit)  "$w/bin/tests/$s" --list --ignored 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/ign-$s.txt"
 then one line an edit)  comm -23 "$w/all-$s.txt" "$w/ign-$s.txt" > "$w/run-$s.txt"
 then one line an edit)done
 then one line an edit)# (every suite at once: their tests share the GATE_TEST_PAR slots
 then one line an edit)# each suite has; a suite's wall is its first start to its last end)
 then one line an edit)for s in $SUITES; do
 then one line an edit)  ( s0=$(date +%s); GATE_TEST_PAR=${GATE_SUITE_PAR:-2} runtests "$w/bin/tests/$s" "$s::" "$w/run-$s.txt" "$r/t-$s"; echo "$s $(( $(date +%s) - s0 ))" >> "$r/suite-walls.txt" ) &
 then one line an edit)done
 then one line an edit)wait
 then one line an edit)# (the reds are written by the subshells' runtests into red.txt; the
 then one line an edit)# status from it)
 then one line an edit)grep -q . "$r/red.txt" && status=red
 then one line an edit)for s in $SUITES; do walls "$r/t-$s" "$s::"; done > "$r/suite-test-walls.txt"
 then one line an edit);;
  latex)
 then one line an edit)k=$1 n=$2
 then one line an edit)fetched
 then one line an edit)"$w/bin/latex" --list 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/all.txt"
 then one line an edit)"$w/bin/latex" --list --ignored 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/ign.txt"
 then one line an edit)# (the order: GATE_LATEX_ORDER, names heaviest first, else by name;
 then one line an edit)# the shards deal them out in turn)
 then one line an edit)comm -23 "$w/all.txt" "$w/ign.txt" > "$w/run.txt"
 then one line an edit)if [ -n "${GATE_LATEX_ORDER:-}" ]; then
 then one line an edit)  { tr ' ' '\n' <<< "$GATE_LATEX_ORDER" | grep -Fxf "$w/run.txt"; grep -Fxvf <(tr ' ' '\n' <<< "$GATE_LATEX_ORDER") "$w/run.txt"; } > "$w/ord.txt"
 then one line an edit)else
 then one line an edit)  cp "$w/run.txt" "$w/ord.txt"
 then one line an edit)fi
 then one line an edit)awk -v k="$k" -v n="$n" '(NR-1)%n==k' "$w/ord.txt" > "$r/tests.txt"
 then one line an edit)# (each test its own process: a hang or a crash is that test's red
 then one line an edit)# line; the walls for the resharding)
 then one line an edit)runtests "$w/bin/latex" "latex::" "$r/tests.txt" "$r/t-latex"
 then one line an edit)walls "$r/t-latex" "" > "$r/test-walls.txt"
 then one line an edit)# the corpus fixtures that are red (ignored "red: corpus ..."): run,
 then one line an edit)# reported apart (known-red.txt; one that passes is news), never red
 then one line an edit)grep '^corpus_' "$w/ign.txt" | awk -v k="$k" -v n="$n" '(NR-1)%n==k' > "$r/known.txt"
 then one line an edit)if [ -s "$r/known.txt" ]; then
 then one line an edit)  s=$status
 then one line an edit)  cp "$r/red.txt" "$w/red.keep"
 then one line an edit)  RT_EXTRA=--ignored runtests "$w/bin/latex" "" "$r/known.txt" "$r/t-known"
 then one line an edit)  awk '$3 == 0 {print $1 " (passes now)"} $3 == 124 || $3 == 137 {print $1 " (timed out)"} $3 != 0 && $3 != 124 && $3 != 137 {print $1}' "$r/t-known/walls.txt" > "$r/known-red.txt"
 then one line an edit)  cp "$w/red.keep" "$r/red.txt"
 then one line an edit)  status=$s
 then one line an edit)fi
 then one line an edit);;
  ams)
 then one line an edit)# ams ORACLE_MD5 CONTROL: the build shard's release CLI; amsldoc
 then one line an edit)# --exact-pdf at -j 1, 8 and 16 (md5 against the oracle, the round
 then one line an edit)# counters); then the cold watch build (bench/ams-cold-tmp.sh's
 then one line an edit)# command) at -j 1, 8, 16, timed by op class (gate-time.py, passed as
 then one line an edit)# $GATE_TIME_PY) beside the control's timings as `gate-fast control
 then one line an edit)# CTL` recorded them (never built here). The edit histories are the
 then one line an edit)# amsedit shards'.
 then one line an edit)want=$1 ctl=${2:-}
 then one line an edit)printf '%s\n' "$GATE_TIME_PY" > "$w/gate-time.py"
 then one line an edit)fetched
 then one line an edit)bin=$w/bin/cli/phitex
 then one line an edit)for j in 1 8 16; do
 then one line an edit)  rm -rf "$w/ams" && cp -a bench/amsldoc "$w/ams"
 then one line an edit)  s0=$(date +%s.%N)
 then one line an edit)  (cd "$w/ams" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/acache \
 then one line an edit)    timeout 900 "$bin" build amsldoc.tex --exact-pdf -j $j > "$r/ams-j$j.log" 2>&1)
 then one line an edit)  e=$?
 then one line an edit)  s1=$(date +%s.%N)
 then one line an edit)  cp "$w/ams/amsldoc.pdf" "$r/ams-j$j.pdf" 2>/dev/null
 then one line an edit)  m=$(md5sum < "$r/ams-j$j.pdf" 2>/dev/null | cut -c1-32)
 then one line an edit)  [ -n "$m" ] || m=none
 then one line an edit)  L=$r/ams-j$j.log
 then one line an edit)  sum() { grep -E 'round guesses: ' "$L" | grep -oE "[0-9]+ $1" | awk '{s+=$1} END {print s+0}'; }
 then one line an edit)  cp_n=$(grep -oE 'realised cp [0-9]+ nodes [^,]+' "$L" | awk '{if ($3+0 >= m) {m=$3+0; t=$5}} END {print m+0, t}')
 then one line an edit)  nodes=$(grep -oE '[0-9]+ nodes run of [0-9]+ live' "$L" | tail -1 | awk '{print $1"/"$5}')
 then one line an edit)  pages=$(grep -oE '[0-9]+ pages' "$L" | tail -1 | awk '{print $1}')
 then one line an edit)  echo "build --exact-pdf -j $j: exit $e md5 $m oracle $want $([ "$m" = "$want" ] && echo same || echo DIFFERS) wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}') pages ${pages:-?} nodes ${nodes:-?} claims_refuted $(sum 'claims refuted') effects_refuted $(sum 'effects refuted') claim_siblings $(sum 'claim siblings') defs_refuted $(sum 'definitions refuted') stale_reads $(grep -oE 'check_reads: [0-9]+ stale' "$L" | awk '{s+=$2} END {print s+0}') cp $cp_n" >> "$r/ams.txt"
 then one line an edit)  [ "$e" = 0 ] || red "amsldoc -j $j: exit $e $(grep -m1 -E 'panicked at|error' "$L" | cut -c1-160)"
 then one line an edit)  [ "$m" = "$want" ] || red "amsldoc -j $j: md5 $m, oracle $want"
 then one line an edit)  python3 -I scripts/accl/gate-acct.py acct "$L" "amsldoc -j $j" >> "$r/acct.txt" 2>&1
 then one line an edit)done
 then one line an edit)python3 -I scripts/accl/gate-acct.py table ams "$r" > "$r/table.txt" 2>&1
 then one line an edit)# the cold watch builds of the head (same node, same command, a fresh
 then one line an edit)# cache each), beside the recorded control
 then one line an edit)for j in 1 8 16; do
 then one line an edit)  rm -rf "$w/ams" "$w/wcache" && cp -a bench/amsldoc "$w/ams"
 then one line an edit)  SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/wcache NO_COLOR=1 \
 then one line an edit)    timeout 900 python3 -I "$w/gate-time.py" run "$bin" "$w/ams" amsldoc.tex $j "$r/cold-head-j$j.json" "$r/cold-head-j$j.err" \
 then one line an edit)    || echo "cold head -j $j: exit $?" >> "$r/ams.txt"
 then one line an edit)done
 then one line an edit)if [ -n "$ctl" ]; then
 then one line an edit)  if ls "$w/gate-control/$ctl"/cold-control-j*.json > /dev/null 2>&1; then
 then one line an edit)    cp "$w/gate-control/$ctl"/cold-control-j*.json "$r/"
 then one line an edit)    echo "control $ctl: timings as recorded ($(cat "$w/gate-control/$ctl/recorded.txt" 2>/dev/null || echo 'source unknown'))" >> "$r/ams.txt"
 then one line an edit)  else
 then one line an edit)    echo "control $ctl: no recorded timings (scripts/accl/gate-fast control $ctl records them)" >> "$r/ams.txt"
 then one line an edit)  fi
 then one line an edit)fi
 then one line an edit)ls "$r"/cold-*.json > /dev/null 2>&1 && python3 -I "$w/gate-time.py" show $(ls "$r"/cold-*-j1.json "$r"/cold-*-j8.json "$r"/cold-*-j16.json 2>/dev/null) > "$r/cold.txt" 2>&1
 then one line an edit);;
  amsedit)
 then one line an edit)# amsedit J: the edit history (spec-final §0 bytes) at -j J:
 then one line an edit)# GATE_AMS_EDITS one-word edits at random places (10; `gate-fast
 then one line an edit)# edits` runs 50), then the PDF equal to a cold build of the final text
 then one line an edit)j=$1 n=${GATE_AMS_EDITS:-10}
 then one line an edit)fetched
 then one line an edit)bin=$w/bin/cli/phitex
 then one line an edit)rm -rf "$w/amsh" "$w/amsh-cold" && cp -a bench/amsldoc "$w/amsh"
 then one line an edit)# (each evaluation's runs and per-round memo hits by round,
 then one line an edit)# `acct-edit:` lines, PHITEX_ROUND_ACCT: the watch's cold build first,
 then one line an edit)# then one line an edit)
 then one line an edit)(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/acache \
 then one line an edit)  PHITEX_ROUND_ACCT=1 PHITEX_NO_HEIGHTS=1 GATE_EDIT_STDERR="$r/edits-j$j.err" \
 then one line an edit)  timeout 2400 python3 -I scripts/accl/gate-edits.py "$bin" "$w/amsh" amsldoc.tex $j $n $j "$r/edits-j$j.json" --exact-pdf) >> "$r/ams.txt" 2>&1
 then one line an edit)grep -E '^acct-edit:' "$r/edits-j$j.err" 2>/dev/null | cut -c1-400 >> "$r/ams.txt"
 then one line an edit)grep -q '"verdict": "same"' "$r/edits-j$j.json" 2>/dev/null || red "amsldoc edit history -j $j: $(python3 -c "import json;print(json.load(open('$r/edits-j$j.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
 then one line an edit);;
  control)
 then one line an edit)# control CTL: the control commit's release CLI built, its cold watch
 then one line an edit)# builds at -j 1, 8, 16 timed as the ams shard times the head's
 then one line an edit)ctl=$1
 then one line an edit)printf '%s\n' "$GATE_TIME_PY" > "$w/gate-time.py"
 then one line an edit)rm -rf "$w/control" && mkdir -p "$w/control" && git archive "$ctl" | tar -x -C "$w/control"
 then one line an edit)ln -s "$PWD/upstream" "$w/control/upstream"
 then one line an edit)( cd "$w/control" && cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > "$r/build-control.txt" 2>&1 ) || red "control $ctl did not build: $(grep -m1 '^error' "$r/build-control.txt")"
 then one line an edit)built
 then one line an edit)b=$w/control/target/release/phitex
 then one line an edit)for j in 1 8 16; do
 then one line an edit)  rm -rf "$w/ams" "$w/wcache" && cp -a bench/amsldoc "$w/ams"
 then one line an edit)  SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/wcache NO_COLOR=1 \
 then one line an edit)    timeout 900 python3 -I "$w/gate-time.py" run "$b" "$w/ams" amsldoc.tex $j "$r/cold-control-j$j.json" "$r/cold-control-j$j.err" \
 then one line an edit)    || red "cold control -j $j: exit $?"
 then one line an edit)done
 then one line an edit);;
  big)
 then one line an edit)# big: bench/para40000.tex cold at -j 1, 8 and 16 (wall, the round's
 then one line an edit)# op, commit, check and idle, the link's phases), then three
 then one line an edit)# one-word edits at -j 8 timed from the save to the build's event
 then one line an edit)fetched
 then one line an edit)bin=$w/bin/cli/prof/phitex
 then one line an edit)echo "big node ${SLURMD_NODENAME:-$(cat /proc/sys/kernel/hostname 2>/dev/null)} cpus $(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)" >> "$r/big.txt"
 then one line an edit)mkdir -p "$w/big" && cp bench/para40000.tex "$w/big/"
 then one line an edit)(cd "$w/big" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para40000.tex > "$r/big-warm.log" 2>&1)
 then one line an edit)cores=$(one_per_core); all=$(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)
 then one line an edit)echo "big cores (one sibling each) $cores" >> "$r/big.txt"
 then one line an edit)for j in 1 8 16 32; do
 then one line an edit)  L=$r/big-j$j.log
 then one line an edit)  pin=$cores; [ "$j" -gt 16 ] && pin=$all
 then one line an edit)  s0=$(date +%s.%N)
 then one line an edit)  (cd "$w/big" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 NO_COLOR=1 \
 then one line an edit)    timeout 900 taskset -c "$pin" "$bin" build -j $j --output-dir=o$j --program pdftex para40000.tex > "$L" 2>&1)
 then one line an edit)  e=$?
 then one line an edit)  s1=$(date +%s.%N)
 then one line an edit)  echo "shell: the process ran from $s0 to $s1" >> "$L"
 then one line an edit)  echo "para40000 -j $j: exit $e wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}') md5 $(md5sum < "$w/big/o$j/para40000.pdf" 2>/dev/null | cut -c1-32)" >> "$r/big.txt"
 then one line an edit)  grep -E '^acct: (round|1 rounds)|^pdf exact|^link:' "$L" | sed 's/^/  /' >> "$r/big.txt"
 then one line an edit)  [ "$e" = 0 ] || red "para40000 -j $j: exit $e"
 then one line an edit)done
 then one line an edit)rm -rf "$w/bige" && mkdir -p "$w/bige" && cp bench/para40000.tex "$w/bige/"
 then one line an edit)(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 timeout 1800 python3 -I scripts/accl/edit-one.py "$bin" "$w/bige" para40000.tex 8 3 "$r/big-watch.err" --program pdftex) >> "$r/big.txt" 2>&1
 then one line an edit)grep -E '^(edit phases|session phases|pdf exact|link:)' "$r/big-watch.err" | tail -8 | sed 's/^/  /' >> "$r/big.txt"
 then one line an edit);;
  para)
 then one line an edit)# para BAR_MD5: the head's release CLI; the format made once untimed,
 then one line an edit)# then each -j cold in its own output directory (same cache: the
 then one line an edit)# format's, not the document's)
 then one line an edit)want=$1
 then one line an edit)fetched
 then one line an edit)bin=$w/bin/cli/prof/phitex
 then one line an edit)mkdir -p "$w/par" && cp bench/para4000.tex "$w/par/"
 then one line an edit)(cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para4000.tex > "$r/para-warm.log" 2>&1)
 then one line an edit)# (the node is shared: each -j runs three times, the min quoted,
 then one line an edit)# the median beside it; the node and its load first)
 then one line an edit)echo "para node ${SLURMD_NODENAME:-$(cat /proc/sys/kernel/hostname 2>/dev/null)} load $(cut -d' ' -f1-3 /proc/loadavg) cpus $(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)" >> "$r/para.txt"
 then one line an edit)cores=$(one_per_core); all=$(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)
 then one line an edit)echo "para cores (one sibling each) $cores" >> "$r/para.txt"
 then one line an edit)for j in 1 8 16 32; do
 then one line an edit)  pin=$cores; [ "$j" -gt 16 ] && pin=$all
 then one line an edit)  walls=""
 then one line an edit)  best=""
 then one line an edit)  for k in 1 2 3; do
 then one line an edit)    Lk=$r/para-j$j-r$k.log
 then one line an edit)    rm -rf "$w/par/o$j"
 then one line an edit)    s0=$(date +%s.%N)
 then one line an edit)    (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 \
 then one line an edit)      timeout 900 taskset -c "$pin" "$bin" build -j $j --output-dir=o$j --program pdftex para4000.tex > "$Lk" 2>&1)
 then one line an edit)    ek=$?
 then one line an edit)    s1=$(date +%s.%N)
 then one line an edit)    echo "shell: the process ran from $s0 to $s1" >> "$Lk"
 then one line an edit)    mk=$(md5sum < "$w/par/o$j/para4000.pdf" 2>/dev/null | cut -c1-32)
 then one line an edit)    [ -n "$mk" ] || mk=none
 then one line an edit)    wk=$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}')
 then one line an edit)    walls="$walls $wk"
 then one line an edit)    [ "$ek" = 0 ] || red "para4000 -j $j run $k: exit $ek $(grep -m1 -E 'panicked at|error' "$Lk" | cut -c1-160)"
 then one line an edit)    [ "$mk" = "$want" ] || red "para4000 -j $j run $k: md5 $mk, bar $want"
 then one line an edit)    # (the fastest run's log and result stand for the -j)
 then one line an edit)    if [ -z "$best" ] || awk -v x="$wk" -v y="$best" 'BEGIN{exit !(x < y)}'; then
 then one line an edit)      best=$wk e=$ek m=$mk
 then one line an edit)      cp "$Lk" "$r/para-j$j.log"
 then one line an edit)    fi
 then one line an edit)  done
 then one line an edit)  L=$r/para-j$j.log
 then one line an edit)  wall=$best
 then one line an edit)  med=$(echo $walls | tr ' ' '\n' | sort -n | sed -n 2p)
 then one line an edit)  sum() { grep -E 'round guesses: ' "$L" | grep -oE "[0-9]+ $1" | awk '{s+=$1} END {print s+0}'; }
 then one line an edit)  cp_n=$(grep -oE 'realised cp [0-9]+ nodes [^,]+' "$L" | awk '{if ($3+0 >= m) {m=$3+0; t=$5}} END {print m+0, t}')
 then one line an edit)  nodes=$(grep -oE '[0-9]+ nodes run of [0-9]+ live' "$L" | tail -1 | awk '{print $1"/"$5}')
 then one line an edit)  echo "para4000 -j $j: exit $e md5 $m bar $want $([ "$m" = "$want" ] && echo same || echo DIFFERS) wall $wall us/paragraph $(awk -v w="$wall" 'BEGIN{printf "%.0f", w*1e6/4000}') nodes ${nodes:-?} cp $cp_n claims_refuted $(sum 'claims refuted') effects_refuted $(sum 'effects refuted') defs_refuted $(sum 'definitions refuted') stale_reads $(grep -oE 'check_reads: [0-9]+ stale' "$L" | awk '{s+=$2} END {print s+0}') wall_min $wall wall_median $med walls$walls" >> "$r/para.txt"
 then one line an edit)  python3 -I scripts/accl/gate-acct.py acct "$L" "para4000 -j $j" >> "$r/acct.txt" 2>&1
 then one line an edit)done
 then one line an edit)# the users' build (no feature): j1 and j8, three runs each, the min
 then one line an edit)for j in 1 8; do
 then one line an edit)  rw=""
 then one line an edit)  for k in 1 2 3; do
 then one line an edit)    rm -rf "$w/par/rel$j"
 then one line an edit)    s0=$(date +%s.%N)
 then one line an edit)    (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
 then one line an edit)      timeout 900 taskset -c "$cores" "$w/bin/cli/rel/phitex" build -j $j --output-dir=rel$j --program pdftex para4000.tex > "$r/para-rel-j$j.log" 2>&1)
 then one line an edit)    s1=$(date +%s.%N)
 then one line an edit)    rw="$rw $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}')"
 then one line an edit)  done
 then one line an edit)  rm_=$(md5sum < "$w/par/rel$j/para4000.pdf" 2>/dev/null | cut -c1-32)
 then one line an edit)  echo "para4000 release -j $j: md5 ${rm_:-none} $([ "$rm_" = "$want" ] && echo same || echo DIFFERS) wall_min $(echo $rw | tr ' ' '\n' | sort -n | head -1) walls$rw" >> "$r/para.txt"
 then one line an edit)  [ "$rm_" = "$want" ] || red "para4000 release -j $j: md5 ${rm_:-none}, bar $want"
 then one line an edit)done
 then one line an edit)# the committed reads checked again after a j8 build, on the checks'
 then one line an edit)# build (the profile CLI above has the check compiled out): a read
 then one line an edit)# that no longer holds is a missed invalidation, the gate fails
 then one line an edit)bin=$w/bin/cli/phitex
 then one line an edit)rm -rf "$w/par/chk"
 then one line an edit)(cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 \
 then one line an edit)  timeout 900 taskset -c "$cores" "$bin" build -j 8 --output-dir=chk --program pdftex para4000.tex > "$r/para-check-reads.log" 2>&1)
 then one line an edit)stale=$(sed -n 's/^check_reads: \([0-9]*\) stale.*/\1/p' "$r/para-check-reads.log" | tail -1)
 then one line an edit)cm=$(md5sum < "$w/par/chk/para4000.pdf" 2>/dev/null | cut -c1-32)
 then one line an edit)echo "para4000 checked reads -j 8: ${stale:-none} stale, md5 ${cm:-none} $([ "$cm" = "$want" ] && echo same || echo DIFFERS)" >> "$r/para.txt"
 then one line an edit)grep '^check_reads: NodeId' "$r/para-check-reads.log" | head -5 >> "$r/para.txt"
 then one line an edit)[ "${stale:-x}" = 0 ] || red "para4000 -j 8 on the checks' build: ${stale:-no} stale reads (PHITEX_CHECK_READS)"
 then one line an edit)[ "$cm" = "$want" ] || red "para4000 -j 8 on the checks' build: md5 ${cm:-none}, bar $want"
 then one line an edit)# the edit histories (spec-final §0 bytes), on the checks' build
 then one line an edit)for j in 8 16; do
 then one line an edit)  rm -rf "$w/parh" "$w/parh-cold" && mkdir -p "$w/parh" && cp bench/para4000.tex "$w/parh/"
 then one line an edit)  (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
 then one line an edit)    timeout 2400 python3 -I scripts/accl/gate-edits.py "$bin" "$w/parh" para4000.tex $j 50 $j "$r/edits-j$j.json" --program pdftex) >> "$r/para.txt" 2>&1
 then one line an edit)  grep -q '"verdict": "same"' "$r/edits-j$j.json" 2>/dev/null || red "para4000 edit history -j $j: $(python3 -c "import json;print(json.load(open('$r/edits-j$j.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
 then one line an edit)done
 then one line an edit)# one edit's phases at -j 8: 10 edits with the watch's timelines
 then one line an edit)# (PHITEX_TEX_TIME), the median edit's table quoted (the engine's
 then one line an edit)# evaluation against the phases around it)
 then one line an edit)rm -rf "$w/parp" "$w/parp-cold" && mkdir -p "$w/parp" && cp bench/para4000.tex "$w/parp/"
 then one line an edit)# (twice: the engine's heights on, as profile builds run, and off,
 then one line an edit)# as release builds run, PHITEX_NO_HEIGHTS)
 then one line an edit)for hv in on off; do
 then one line an edit)rm -rf "$w/parp" "$w/parp-cold" && mkdir -p "$w/parp" && cp bench/para4000.tex "$w/parp/"
 then one line an edit)(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_TEX_TIME=1 GATE_EDIT_STDERR="$r/edit-phases-j8-$hv.log" \
 then one line an edit)  PHITEX_NO_HEIGHTS=$([ $hv = off ] && echo 1) \
 then one line an edit)  timeout 900 python3 -I scripts/accl/gate-edits.py "$bin" "$w/parp" para4000.tex 8 10 7 "$r/edits-phases-j8-$hv.json" --program pdftex) > /dev/null 2>&1
 then one line an edit)echo "(heights $hv)" >> "$r/para.txt"
 then one line an edit)python3 -I - "$r/edit-phases-j8-$hv.log" >> "$r/para.txt" <<'PY'
import re, sys
blocks, cur = [], None
for line in open(sys.argv[1], errors="replace"):
 then one line an edit)if line.startswith("timeline ("):
 then one line an edit)    cur = [line.rstrip()]
 then one line an edit)    blocks.append(cur)
 then one line an edit)elif cur is not None and line.startswith("  "):
 then one line an edit)    cur.append(line.rstrip())
 then one line an edit)    if line.startswith("  clock:"):
 then one line an edit)        cur = None
 then one line an edit)else:
 then one line an edit)    cur = None
edits = [b for b in blocks[1:] if b]
if not edits:
 then one line an edit)print("para4000 edit phases -j 8: no timeline")
else:
 then one line an edit)total = lambda b: float(re.search(r"([0-9.]+) ms from", b[0]).group(1))
 then one line an edit)edits.sort(key=total)
 then one line an edit)b = edits[len(edits) // 2]
 then one line an edit)print("para4000 edit phases -j 8 (median of %d edits; totals %s ms):" % (len(edits), " ".join("%.1f" % total(x) for x in edits)))
 then one line an edit)print("\n".join(b[:40]))
PY
 then one line an edit)done
 then one line an edit)python3 -I scripts/accl/gate-acct.py table para "$r" > "$r/table.txt" 2>&1
 then one line an edit);;
  cg)
 then one line an edit)# cg: para4000 at one thread under callgrind (the build shard's
 then one line an edit)# release CLI: the profiling build's code, its symbols and line tables;
 then one line an edit)# callgrind attributes by symbol, not by inlined frame), the format
 then one line an edit)# made once unprofiled. Prints the
 then one line an edit)# Ir in all, the startup's (all but the evaluation, `Graph::settle`),
 then one line an edit)# the node bodies' (the operations' runs, inclusive) over the
 then one line an edit)# commands they did (`acct: commands`), and the top 15 functions by
 then one line an edit)# exclusive Ir
 then one line an edit)fetched
 then one line an edit)bin=$w/bin/cli/prof/phitex
 then one line an edit)doc=${GATE_CG_DOC:-para4000}
 then one line an edit)mkdir -p "$w/cg" && cp "bench/$doc.tex" "$w/cg/"
 then one line an edit)(cd "$w/cg" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 1 --output-dir=warm --program pdftex "$doc.tex" > "$r/cg-warm.log" 2>&1)
 then one line an edit)(cd "$w/cg" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_THREADS=1 \
 then one line an edit)  timeout 3000 valgrind --tool=callgrind --callgrind-out-file="$r/cg.out" "$bin" build -j 1 --output-dir=o1 --program pdftex "$doc.tex" > "$r/cg-run.log" 2>&1) \
 then one line an edit)  || red "cg: callgrind run exit $?"
 then one line an edit)callgrind_annotate --threshold=100 "$r/cg.out" > "$r/cg.excl.txt" 2>&1
 then one line an edit)callgrind_annotate --inclusive=yes --threshold=100 "$r/cg.out" > "$r/cg.incl.txt" 2>&1
 then one line an edit)num() { grep -m1 -F "$1" "$r/cg.incl.txt" | awk '{print $1}' | tr -d ,; }
 then one line an edit)total=$(grep -m1 'PROGRAM TOTALS' "$r/cg.excl.txt" | awk '{print $1}' | tr -d ,)
 then one line an edit)settle=$(num 'phitex_tex::api::Graph>::settle')
 then one line an edit)bodies=0
 then one line an edit)for op in 'ops::Exec as defuse::api::Operation>::run' 'pages::Resume as defuse::api::Operation>::run' \
 then one line an edit)  'pages::Scan as defuse::api::Operation>::run' 'pages::Finish as defuse::api::Operation>::run' \
 then one line an edit)  'ops::Link as defuse::api::Operation>::run' 'ops::Dvi as defuse::api::Operation>::run'; do
 then one line an edit)  x=$(num "$op"); bodies=$((bodies + ${x:-0}))
 then one line an edit)done
 then one line an edit)cmds=$(grep -oE 'acct: commands [0-9]+' "$r/cg-run.log" | tail -1 | awk '{print $3}')
 then one line an edit){
 then one line an edit)  echo "$doc -j 1 under callgrind: Ir ${total:-?}; startup (all but the evaluation) $(( ${total:-0} - ${settle:-0} )); evaluation ${settle:-?}, of which node bodies $bodies; commands ${cmds:-?}; node bodies per command $(awk -v a="$bodies" -v b="${cmds:-0}" 'BEGIN { if (b > 0) printf "%.0f", a / b; else print "?" }') Ir (pdfTeX about 2,900)"
 then one line an edit)  echo "top 15 by exclusive Ir:"
 then one line an edit)  grep -E '^ *[0-9,]+ +\(' "$r/cg.excl.txt" | grep -v 'PROGRAM TOTALS' | head -15 | cut -c1-200
 then one line an edit)} > "$r/cg.txt"
 then one line an edit);;
  *) red "unknown shard $shard" ;;
esac
echo "wall_s $(( $(date +%s) - t0 ))" >> "$r/times.txt"
echo "$status" > "$r/status"
echo "$shard $*: $status ($(grep -c . "$r/red.txt") red), $(tr '\n' ' ' < "$r/times.txt")" > "$r/summary.txt"
# (a failed build fails its job: the shards waiting on it are cancelled)
[ "$shard" = build ] && [ "$status" != green ] && exit 1
exit 0
