#!/bin/bash
# One shard of the fast gate (scripts/accl/gate-fast), as job.sbatch's
# `cmd` task runs it: inside the container, in the commit's checkout,
# $r the results directory, $w the workspace. gate-fast passes this
# file's text to `bash -c` (the commit under test need not have it).
#
#   gate-shard.sh build                 the commit's binaries, once: phitex-tex's
#                                       test binaries (fastdev) and the release
#                                       CLI, built side by side, left in
#                                       $r/store for the other shards
#                                       (job.sbatch ACCL_STORE_OUT/IN)
#   gate-shard.sh lint                  clippy -D warnings (no fmt: version skew)
#   gate-shard.sh suites                the local suites of phitex-tex
#   gate-shard.sh latex K N             the latex target's tests, shard K of N
#   gate-shard.sh ams ORACLE_MD5 CTL    amsldoc --exact-pdf at -j 1, 8 and 16;
#                                       cold watch timing at -j 1/8/16, the
#                                       head beside control CTL's recorded
#                                       timings ($w/gate-control/CTL)
#   gate-shard.sh amsedit J             amsldoc's edit history at -j J
#                                       (GATE_AMS_EDITS one-word edits, 10)
#   gate-shard.sh control CTL           control CTL's CLI built, its cold
#                                       watch timings at -j 1/8/16 (recorded
#                                       by `gate-fast control`)
#   gate-shard.sh cg                    para4000 at one thread under callgrind:
#                                       startup Ir, node bodies' Ir per
#                                       command, top 15 functions
#   gate-shard.sh para BAR_MD5          bench/para4000.tex (plain pdfTeX) cold
#                                       at -j 1/8/16, three runs each (the
#                                       node is shared: the min quoted, the
#                                       median beside it; the node's name
#                                       and load first): md5 of every run
#                                       against BAR_MD5, wall, us per
#                                       paragraph, realised critical path,
#                                       guess counters
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
T="cargo test --profile fastdev -p phitex-tex"
# the test targets the shards run (the build shard builds them)
SUITES="pdftex edits pages plain etex hints inputs defuse_gaps"
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
    xargs -a "$list" -d '\n' -P "${GATE_TEST_PAR:-4}" -I{} bash -c '
      t=$1; f=$d/$(tr ":/" "__" <<< "$t").txt
      s0=$(date +%s.%N)
      timeout -k 10 "$TO" "$bin" $RT_EXTRA --exact "$t" > "$f" 2>&1
      e=$?
      s1=$(date +%s.%N)
      echo "EXIT $e" >> "$f"
      echo "$t $(awk -v a="$s0" -v b="$s1" "BEGIN{printf \"%.3f\", b-a}") $e" >> "$d/walls.txt"
      # (a timeout: the test again under gdb, interrupted at the same
      # limit, every thread'"'"'s backtrace kept beside its log: gdb as the
      # parent may trace it, an attach may not)
      if [ "$e" = 124 ] || [ "$e" = 137 ]; then
        timeout -s INT -k 30 "$TO" gdb -batch -nx -ex "set pagination off" -ex run \
          -ex "thread apply all bt 40" -ex kill --args "$bin" $RT_EXTRA --exact "$t" \
          > "${f%.txt}.gdb" 2>&1
        echo "backtraces after ${TO} s under gdb: ${f%.txt}.gdb ($(grep -c "^Thread " "${f%.txt}.gdb") threads)" >> "$f"
      fi' _ {} )
  while read -r t _ e; do
    case $e in
      0) ;;
      124 | 137) red "$pre$t (timed out after ${GATE_TEST_TIMEOUT:-120} s; every thread's backtrace under gdb: $(basename "$d")/$(tr ":/" "__" <<< "$t").gdb)" ;;
      *) red "$pre$t$(grep -q "^test $t \.\.\. FAILED" "$d/$(tr ":/" "__" <<< "$t").txt" || echo " (exit $e: $(grep -m1 -E 'panicked at|error(\[|:)|signal' "$d/$(tr ":/" "__" <<< "$t").txt" | cut -c1-140))")" ;;
    esac
  done < "$d/walls.txt"
}
# the walls of runtests' DIR as test-walls.txt has them: `name wall ok|FAILED`
walls() { awk -v p="$2" '{print p $1, $2, ($3 == 0 ? "ok" : "FAILED")}' "$1/walls.txt"; }
# the red tests of a libtest log, else why the binary gave none
reds() {
  local log=$1 pre=$2 n
  n=$(grep -E '^test .* \.\.\. FAILED( <.*)?$' "$log" | sed -E 's/^test (.*) \.\.\. FAILED( <.*)?$/\1/')
  if [ -n "$n" ]; then
    for x in $n; do red "$pre$x"; done
  elif ! grep -q '^EXIT 0$' "$log"; then
    red "$pre(exit $(sed -n 's/^EXIT //p' "$log" | tail -1), no test named: $(grep -m1 -E 'panicked at|error(\[|:)|timed out|signal' "$log" | cut -c1-160))"
  fi
}
case $shard in
  lint)
    # (no fmt: the image's rustfmt is another version than the pinned
    # nightly's and reflows what the local one keeps, as job.sbatch's
    # gate since b2fbf61a; clippy skips the vendored engine)
    built
    if ! cargo clippy --workspace --exclude defuse --all-targets --keep-going -q --message-format short -- -D warnings > "$r/clippy.txt" 2>&1; then
      grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)' "$r/clippy.txt" | sort -u | head -60 | while IFS= read -r l; do echo "clippy: $l"; done >> "$r/red.txt"
      grep -q . "$r/red.txt" || echo "clippy: $(grep -m1 '^error' "$r/clippy.txt")" >> "$r/red.txt"
      status=red
    fi
    ;;
  build)
    # (the test binaries and the CLI in their own target directories,
    # side by side: one cargo would hold the other's lock)
    a=()
    for s in latex $SUITES; do a+=(--test "$s"); done
    ( $T "${a[@]}" --no-run --message-format=json-render-diagnostics > "$r/build-tests.json" 2> "$r/build-tests.txt"; echo "EXIT $?" >> "$r/build-tests.txt" ) &
    ( CARGO_TARGET_DIR=$PWD/target-cli cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt" ) &
    wait
    built
    grep -q '^EXIT 0$' "$r/build-tests.txt" || red "build (tests): $(grep -m1 '^error' "$r/build-tests.txt")"
    grep -q '^EXIT 0$' "$r/build.txt" || red "build (cli): $(grep -m1 '^error' "$r/build.txt")"
    if [ $status = green ]; then
      mkdir -p "$r/store.tmp/tests" "$r/store.tmp/cli"
      python3 -I -c '
import json, os, shutil, sys
for l in open(sys.argv[1]):
    try:
        m = json.loads(l)
    except ValueError:
        continue
    if m.get("reason") == "compiler-artifact" and m.get("executable") and "test" in m["target"]["kind"]:
        shutil.copy2(m["executable"], os.path.join(sys.argv[2], m["target"]["name"]))' "$r/build-tests.json" "$r/store.tmp/tests"
      cp target-cli/release/phitex "$r/store.tmp/cli/phitex"
      sed -n 's/^build_s //p' "$r/times.txt" | tee "$r/store.tmp/tests/build_s" > "$r/store.tmp/cli/build_s"
      ls -la "$r/store.tmp/tests" "$r/store.tmp/cli" > "$r/store.txt"
      mv "$r/store.tmp" "$r/store"
    fi
    rm -f "$r/build-tests.json"
    ;;
  suites)
    fetched
    for s in $SUITES; do
      "$w/bin/tests/$s" --list 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/all-$s.txt"
      "$w/bin/tests/$s" --list --ignored 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/ign-$s.txt"
      comm -23 "$w/all-$s.txt" "$w/ign-$s.txt" > "$w/run-$s.txt"
    done
    # (every suite at once: their tests share the GATE_TEST_PAR slots
    # each suite has; a suite's wall is its first start to its last end)
    for s in $SUITES; do
      ( s0=$(date +%s); GATE_TEST_PAR=${GATE_SUITE_PAR:-2} runtests "$w/bin/tests/$s" "$s::" "$w/run-$s.txt" "$r/t-$s"; echo "$s $(( $(date +%s) - s0 ))" >> "$r/suite-walls.txt" ) &
    done
    wait
    # (the reds are written by the subshells' runtests into red.txt; the
    # status from it)
    grep -q . "$r/red.txt" && status=red
    for s in $SUITES; do walls "$r/t-$s" "$s::"; done > "$r/suite-test-walls.txt"
    ;;
  latex)
    k=$1 n=$2
    fetched
    "$w/bin/latex" --list 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/all.txt"
    "$w/bin/latex" --list --ignored 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/ign.txt"
    # (the order: GATE_LATEX_ORDER, names heaviest first, else by name;
    # the shards deal them out in turn)
    comm -23 "$w/all.txt" "$w/ign.txt" > "$w/run.txt"
    if [ -n "${GATE_LATEX_ORDER:-}" ]; then
      { tr ' ' '\n' <<< "$GATE_LATEX_ORDER" | grep -Fxf "$w/run.txt"; grep -Fxvf <(tr ' ' '\n' <<< "$GATE_LATEX_ORDER") "$w/run.txt"; } > "$w/ord.txt"
    else
      cp "$w/run.txt" "$w/ord.txt"
    fi
    awk -v k="$k" -v n="$n" '(NR-1)%n==k' "$w/ord.txt" > "$r/tests.txt"
    # (each test its own process: a hang or a crash is that test's red
    # line; the walls for the resharding)
    runtests "$w/bin/latex" "latex::" "$r/tests.txt" "$r/t-latex"
    walls "$r/t-latex" "" > "$r/test-walls.txt"
    # the corpus fixtures that are red (ignored "red: corpus ..."): run,
    # reported apart (known-red.txt; one that passes is news), never red
    grep '^corpus_' "$w/ign.txt" | awk -v k="$k" -v n="$n" '(NR-1)%n==k' > "$r/known.txt"
    if [ -s "$r/known.txt" ]; then
      s=$status
      cp "$r/red.txt" "$w/red.keep"
      RT_EXTRA=--ignored runtests "$w/bin/latex" "" "$r/known.txt" "$r/t-known"
      awk '$3 == 0 {print $1 " (passes now)"} $3 == 124 || $3 == 137 {print $1 " (timed out)"} $3 != 0 && $3 != 124 && $3 != 137 {print $1}' "$r/t-known/walls.txt" > "$r/known-red.txt"
      cp "$w/red.keep" "$r/red.txt"
      status=$s
    fi
    ;;
  ams)
    # ams ORACLE_MD5 CONTROL: the build shard's release CLI; amsldoc
    # --exact-pdf at -j 1, 8 and 16 (md5 against the oracle, the round
    # counters); then the cold watch build (bench/ams-cold-tmp.sh's
    # command) at -j 1, 8, 16, timed by op class (gate-time.py, passed as
    # $GATE_TIME_PY) beside the control's timings as `gate-fast control
    # CTL` recorded them (never built here). The edit histories are the
    # amsedit shards'.
    want=$1 ctl=${2:-}
    printf '%s\n' "$GATE_TIME_PY" > "$w/gate-time.py"
    fetched
    bin=$w/bin/cli/phitex
    for j in 1 8 16; do
      rm -rf "$w/ams" && cp -a bench/amsldoc "$w/ams"
      s0=$(date +%s.%N)
      (cd "$w/ams" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/acache \
        timeout 900 "$bin" build amsldoc.tex --exact-pdf -j $j > "$r/ams-j$j.log" 2>&1)
      e=$?
      s1=$(date +%s.%N)
      cp "$w/ams/amsldoc.pdf" "$r/ams-j$j.pdf" 2>/dev/null
      m=$(md5sum < "$r/ams-j$j.pdf" 2>/dev/null | cut -c1-32)
      [ -n "$m" ] || m=none
      L=$r/ams-j$j.log
      sum() { grep -E 'round guesses: ' "$L" | grep -oE "[0-9]+ $1" | awk '{s+=$1} END {print s+0}'; }
      cp_n=$(grep -oE 'realised cp [0-9]+ nodes [^,]+' "$L" | awk '{if ($3+0 >= m) {m=$3+0; t=$5}} END {print m+0, t}')
      nodes=$(grep -oE '[0-9]+ nodes run of [0-9]+ live' "$L" | tail -1 | awk '{print $1"/"$5}')
      pages=$(grep -oE '[0-9]+ pages' "$L" | tail -1 | awk '{print $1}')
      echo "build --exact-pdf -j $j: exit $e md5 $m oracle $want $([ "$m" = "$want" ] && echo same || echo DIFFERS) wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}') pages ${pages:-?} nodes ${nodes:-?} claims_refuted $(sum 'claims refuted') effects_refuted $(sum 'effects refuted') claim_siblings $(sum 'claim siblings') defs_refuted $(sum 'definitions refuted') stale_reads $(grep -oE 'check_reads: [0-9]+ stale' "$L" | awk '{s+=$2} END {print s+0}') cp $cp_n" >> "$r/ams.txt"
      [ "$e" = 0 ] || red "amsldoc -j $j: exit $e $(grep -m1 -E 'panicked at|error' "$L" | cut -c1-160)"
      [ "$m" = "$want" ] || red "amsldoc -j $j: md5 $m, oracle $want"
      python3 -I scripts/accl/gate-acct.py acct "$L" "amsldoc -j $j" >> "$r/acct.txt" 2>&1
    done
    python3 -I scripts/accl/gate-acct.py table ams "$r" > "$r/table.txt" 2>&1
    # the cold watch builds of the head (same node, same command, a fresh
    # cache each), beside the recorded control
    for j in 1 8 16; do
      rm -rf "$w/ams" "$w/wcache" && cp -a bench/amsldoc "$w/ams"
      SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/wcache NO_COLOR=1 \
        timeout 900 python3 -I "$w/gate-time.py" run "$bin" "$w/ams" amsldoc.tex $j "$r/cold-head-j$j.json" "$r/cold-head-j$j.err" \
        || echo "cold head -j $j: exit $?" >> "$r/ams.txt"
    done
    if [ -n "$ctl" ]; then
      if ls "$w/gate-control/$ctl"/cold-control-j*.json > /dev/null 2>&1; then
        cp "$w/gate-control/$ctl"/cold-control-j*.json "$r/"
        echo "control $ctl: timings as recorded ($(cat "$w/gate-control/$ctl/recorded.txt" 2>/dev/null || echo 'source unknown'))" >> "$r/ams.txt"
      else
        echo "control $ctl: no recorded timings (scripts/accl/gate-fast control $ctl records them)" >> "$r/ams.txt"
      fi
    fi
    ls "$r"/cold-*.json > /dev/null 2>&1 && python3 -I "$w/gate-time.py" show $(ls "$r"/cold-*-j1.json "$r"/cold-*-j8.json "$r"/cold-*-j16.json 2>/dev/null) > "$r/cold.txt" 2>&1
    ;;
  amsedit)
    # amsedit J: the edit history (spec-final §0 bytes) at -j J:
    # GATE_AMS_EDITS one-word edits at random places (10; `gate-fast
    # edits` runs 50), then the PDF equal to a cold build of the final text
    j=$1 n=${GATE_AMS_EDITS:-10}
    fetched
    bin=$w/bin/cli/phitex
    rm -rf "$w/amsh" "$w/amsh-cold" && cp -a bench/amsldoc "$w/amsh"
    (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/acache \
      timeout 2400 python3 -I scripts/accl/gate-edits.py "$bin" "$w/amsh" amsldoc.tex $j $n $j "$r/edits-j$j.json" --exact-pdf) >> "$r/ams.txt" 2>&1
    grep -q '"verdict": "same"' "$r/edits-j$j.json" 2>/dev/null || red "amsldoc edit history -j $j: $(python3 -c "import json;print(json.load(open('$r/edits-j$j.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
    ;;
  control)
    # control CTL: the control commit's release CLI built, its cold watch
    # builds at -j 1, 8, 16 timed as the ams shard times the head's
    ctl=$1
    printf '%s\n' "$GATE_TIME_PY" > "$w/gate-time.py"
    rm -rf "$w/control" && mkdir -p "$w/control" && git archive "$ctl" | tar -x -C "$w/control"
    ln -s "$PWD/upstream" "$w/control/upstream"
    ( cd "$w/control" && cargo build -q --release -p phitex-cli > "$r/build-control.txt" 2>&1 ) || red "control $ctl did not build: $(grep -m1 '^error' "$r/build-control.txt")"
    built
    b=$w/control/target/release/phitex
    for j in 1 8 16; do
      rm -rf "$w/ams" "$w/wcache" && cp -a bench/amsldoc "$w/ams"
      SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/wcache NO_COLOR=1 \
        timeout 900 python3 -I "$w/gate-time.py" run "$b" "$w/ams" amsldoc.tex $j "$r/cold-control-j$j.json" "$r/cold-control-j$j.err" \
        || red "cold control -j $j: exit $?"
    done
    ;;
  big)
    # big: bench/para40000.tex cold at -j 1, 8 and 16 (wall, the round's
    # op, commit, check and idle, the link's phases), then three
    # one-word edits at -j 8 timed from the save to the build's event
    fetched
    bin=$w/bin/cli/phitex
    mkdir -p "$w/big" && cp bench/para40000.tex "$w/big/"
    (cd "$w/big" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para40000.tex > "$r/big-warm.log" 2>&1)
    for j in 1 8 16; do
      L=$r/big-j$j.log
      s0=$(date +%s.%N)
      (cd "$w/big" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 NO_COLOR=1 \
        timeout 900 "$bin" build -j $j --output-dir=o$j --program pdftex para40000.tex > "$L" 2>&1)
      e=$?
      s1=$(date +%s.%N)
      echo "shell: the process ran from $s0 to $s1" >> "$L"
      echo "para40000 -j $j: exit $e wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}') md5 $(md5sum < "$w/big/o$j/para40000.pdf" 2>/dev/null | cut -c1-32)" >> "$r/big.txt"
      grep -E '^acct: (round|1 rounds)|^pdf exact|^link:' "$L" | sed 's/^/  /' >> "$r/big.txt"
      [ "$e" = 0 ] || red "para40000 -j $j: exit $e"
    done
    rm -rf "$w/bige" && mkdir -p "$w/bige" && cp bench/para40000.tex "$w/bige/"
    (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 timeout 1800 python3 -I scripts/accl/edit-one.py "$bin" "$w/bige" para40000.tex 8 3 "$r/big-watch.err" --program pdftex) >> "$r/big.txt" 2>&1
    grep -E '^(edit phases|session phases|pdf exact|link:)' "$r/big-watch.err" | tail -8 | sed 's/^/  /' >> "$r/big.txt"
    ;;
  para)
    # para BAR_MD5: the head's release CLI; the format made once untimed,
    # then each -j cold in its own output directory (same cache: the
    # format's, not the document's)
    want=$1
    fetched
    bin=$w/bin/cli/phitex
    mkdir -p "$w/par" && cp bench/para4000.tex "$w/par/"
    (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para4000.tex > "$r/para-warm.log" 2>&1)
    # (the node is shared: each -j runs three times, the min quoted,
    # the median beside it; the node and its load first)
    echo "para node ${SLURMD_NODENAME:-$(cat /proc/sys/kernel/hostname 2>/dev/null)} load $(cut -d' ' -f1-3 /proc/loadavg)" >> "$r/para.txt"
    for j in 1 8 16; do
      walls=""
      best=""
      for k in 1 2 3; do
        Lk=$r/para-j$j-r$k.log
        rm -rf "$w/par/o$j"
        s0=$(date +%s.%N)
        (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 \
          timeout 900 "$bin" build -j $j --output-dir=o$j --program pdftex para4000.tex > "$Lk" 2>&1)
        ek=$?
        s1=$(date +%s.%N)
        echo "shell: the process ran from $s0 to $s1" >> "$Lk"
        mk=$(md5sum < "$w/par/o$j/para4000.pdf" 2>/dev/null | cut -c1-32)
        [ -n "$mk" ] || mk=none
        wk=$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}')
        walls="$walls $wk"
        [ "$ek" = 0 ] || red "para4000 -j $j run $k: exit $ek $(grep -m1 -E 'panicked at|error' "$Lk" | cut -c1-160)"
        [ "$mk" = "$want" ] || red "para4000 -j $j run $k: md5 $mk, bar $want"
        # (the fastest run's log and result stand for the -j)
        if [ -z "$best" ] || awk -v x="$wk" -v y="$best" 'BEGIN{exit !(x < y)}'; then
          best=$wk e=$ek m=$mk
          cp "$Lk" "$r/para-j$j.log"
        fi
      done
      L=$r/para-j$j.log
      wall=$best
      med=$(echo $walls | tr ' ' '\n' | sort -n | sed -n 2p)
      sum() { grep -E 'round guesses: ' "$L" | grep -oE "[0-9]+ $1" | awk '{s+=$1} END {print s+0}'; }
      cp_n=$(grep -oE 'realised cp [0-9]+ nodes [^,]+' "$L" | awk '{if ($3+0 >= m) {m=$3+0; t=$5}} END {print m+0, t}')
      nodes=$(grep -oE '[0-9]+ nodes run of [0-9]+ live' "$L" | tail -1 | awk '{print $1"/"$5}')
      echo "para4000 -j $j: exit $e md5 $m bar $want $([ "$m" = "$want" ] && echo same || echo DIFFERS) wall $wall us/paragraph $(awk -v w="$wall" 'BEGIN{printf "%.0f", w*1e6/4000}') nodes ${nodes:-?} cp $cp_n claims_refuted $(sum 'claims refuted') effects_refuted $(sum 'effects refuted') defs_refuted $(sum 'definitions refuted') stale_reads $(grep -oE 'check_reads: [0-9]+ stale' "$L" | awk '{s+=$2} END {print s+0}') wall_min $wall wall_median $med walls$walls" >> "$r/para.txt"
      python3 -I scripts/accl/gate-acct.py acct "$L" "para4000 -j $j" >> "$r/acct.txt" 2>&1
    done
    # the edit histories (spec-final §0 bytes)
    for j in 8 16; do
      rm -rf "$w/parh" "$w/parh-cold" && mkdir -p "$w/parh" && cp bench/para4000.tex "$w/parh/"
      (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
        timeout 2400 python3 -I scripts/accl/gate-edits.py "$bin" "$w/parh" para4000.tex $j 50 $j "$r/edits-j$j.json" --program pdftex) >> "$r/para.txt" 2>&1
      grep -q '"verdict": "same"' "$r/edits-j$j.json" 2>/dev/null || red "para4000 edit history -j $j: $(python3 -c "import json;print(json.load(open('$r/edits-j$j.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
    done
    python3 -I scripts/accl/gate-acct.py table para "$r" > "$r/table.txt" 2>&1
    ;;
  cg)
    # cg: para4000 at one thread under callgrind (the build shard's
    # release CLI: the profiling build's code, its symbols and line tables;
    # callgrind attributes by symbol, not by inlined frame), the format
    # made once unprofiled. Prints the
    # Ir in all, the startup's (all but the evaluation, `Graph::settle`),
    # the node bodies' (the operations' runs, inclusive) over the
    # commands they did (`acct: commands`), and the top 15 functions by
    # exclusive Ir
    fetched
    bin=$w/bin/cli/phitex
    doc=${GATE_CG_DOC:-para4000}
    mkdir -p "$w/cg" && cp "bench/$doc.tex" "$w/cg/"
    (cd "$w/cg" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 1 --output-dir=warm --program pdftex "$doc.tex" > "$r/cg-warm.log" 2>&1)
    (cd "$w/cg" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_THREADS=1 \
      timeout 3000 valgrind --tool=callgrind --callgrind-out-file="$r/cg.out" "$bin" build -j 1 --output-dir=o1 --program pdftex "$doc.tex" > "$r/cg-run.log" 2>&1) \
      || red "cg: callgrind run exit $?"
    callgrind_annotate --threshold=100 "$r/cg.out" > "$r/cg.excl.txt" 2>&1
    callgrind_annotate --inclusive=yes --threshold=100 "$r/cg.out" > "$r/cg.incl.txt" 2>&1
    num() { grep -m1 -F "$1" "$r/cg.incl.txt" | awk '{print $1}' | tr -d ,; }
    total=$(grep -m1 'PROGRAM TOTALS' "$r/cg.excl.txt" | awk '{print $1}' | tr -d ,)
    settle=$(num 'phitex_tex::api::Graph>::settle')
    bodies=0
    for op in 'ops::Exec as defuse::api::Operation>::run' 'pages::Resume as defuse::api::Operation>::run' \
      'pages::Scan as defuse::api::Operation>::run' 'pages::Finish as defuse::api::Operation>::run' \
      'ops::Link as defuse::api::Operation>::run' 'ops::Dvi as defuse::api::Operation>::run'; do
      x=$(num "$op"); bodies=$((bodies + ${x:-0}))
    done
    cmds=$(grep -oE 'acct: commands [0-9]+' "$r/cg-run.log" | tail -1 | awk '{print $3}')
    {
      echo "$doc -j 1 under callgrind: Ir ${total:-?}; startup (all but the evaluation) $(( ${total:-0} - ${settle:-0} )); evaluation ${settle:-?}, of which node bodies $bodies; commands ${cmds:-?}; node bodies per command $(awk -v a="$bodies" -v b="${cmds:-0}" 'BEGIN { if (b > 0) printf "%.0f", a / b; else print "?" }') Ir (pdfTeX about 2,900)"
      echo "top 15 by exclusive Ir:"
      grep -E '^ *[0-9,]+ +\(' "$r/cg.excl.txt" | grep -v 'PROGRAM TOTALS' | head -15 | cut -c1-200
    } > "$r/cg.txt"
    ;;
  *) red "unknown shard $shard" ;;
esac
echo "wall_s $(( $(date +%s) - t0 ))" >> "$r/times.txt"
echo "$status" > "$r/status"
echo "$shard $*: $status ($(grep -c . "$r/red.txt") red), $(tr '\n' ' ' < "$r/times.txt")" > "$r/summary.txt"
# (a failed build fails its job: the shards waiting on it are cancelled)
[ "$shard" = build ] && [ "$status" != green ] && exit 1
exit 0
