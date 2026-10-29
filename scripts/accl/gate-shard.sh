#!/bin/bash
# One shard of the fast gate (scripts/accl/gate-fast), as job.sbatch's
# `cmd` task runs it: inside the container, in the commit's checkout,
# $r the results directory, $w the workspace. gate-fast passes this
# file's text to `bash -c` (the commit under test need not have it).
#
#   gate-shard.sh all SPEC...           the whole gate in one job (below:
#                                       mc caps a user's submitted jobs)
#   gate-shard.sh build                 the commit's binaries, once: phitex-tex's
#                                       test binaries (fastdev) and the release
#                                       CLI, built side by side, left in
#                                       $r/store for the other shards
#                                       (job.sbatch ACCL_STORE_OUT/IN)
#   gate-shard.sh lint                  clippy -D warnings (no fmt: version skew),
#                                       phitex-live checked for wasm32-wasip1-threads
#   gate-shard.sh suites                the local suites of phitex-tex
#   gate-shard.sh latex K N             the latex target's tests, shard K of N
#   gate-shard.sh ams ORACLE_MD5 CTL    amsldoc --exact-pdf at -j 1, 8 and 16;
#                                       cold watch timing at -j 1/8/16, the
#                                       head beside control CTL's recorded
#                                       timings ($w/gate-control/CTL)
#   gate-shard.sh amsedit J             amsldoc's edit history at -j J
#                                       (GATE_AMS_EDITS one-word edits, 10)
#   gate-shard.sh editkinds             amsldoc in one watch session per
#                                       -j (8, then 1), every edit kind in
#                                       turn (gate-editkinds.py): each
#                                       edit's PDF against a cold build of
#                                       its text (red if not the same),
#                                       its save-to-build ms against its
#                                       kind's target, rounds, runs,
#                                       cutoffs, wasted reruns and their
#                                       top causes (recorded, not red)
#   gate-shard.sh control CTL           control CTL's CLI built, its cold
#                                       watch timings at -j 1/8/16 (recorded
#                                       by `gate-fast control`)
#   gate-shard.sh cg                    GATE_CG_DOC (para4000 and macros) at
#                                       one thread under callgrind: startup
#                                       Ir, Ir per command, the phases'
#                                       table, top 15 functions; each PDF's
#                                       md5 against pdfTeX's (red if not)
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
# all NAME:CPUS:SHARD[,ARG...] ...: the whole gate in this one job, as
# gate-fast submits it where the cluster caps a user's submitted jobs
# (mc: 16, a gate of separate shards 12 of them). The build first unless
# the binaries came in ($w/bin/cli/phitex), on every CPU of the job; its
# store into $w/bin and $r/store (job.sbatch publishes it, ACCL_STORE_OUT).
# Then every shard at once, each as its own job would run it: its own
# results ($r/s-NAME, its log there), workspace ($w/s-NAME, the job's
# entries linked in but the cache) and CPUs (the job's dealt out in the
# order named, CPUS each). Always exits 0: the shards' verdicts are in
# their directories (gate-fast's table reads them).
if [ "$shard" = all ]; then
  r0=$r w0=$w
  # (the job's CPUs, one a line)
  cpus=$(taskset -pc $$ 2>/dev/null | sed 's/.*: //' | tr ',' '\n' |
    awk -F- '{ if (NF == 2) { for (i = $1; i <= $2; i++) print i } else if ($1 != "") print $1 }')
  next=0
  # (take N: the next N of them, comma-separated, in $set_)
  take() { set_=$(sed -n "$((next + 1)),$((next + $1))p" <<< "$cpus" | paste -sd,); next=$((next + $1)); }
  # (one NAME CPUS SHARD ARGS...: a shard in its own results, workspace and CPUs)
  one() {
    local name=$1 n=$2 x
    shift 2
    take "$n"
    mkdir -p "$r0/s-$name" "$w0/s-$name"
    for x in "$w0"/*; do
      case ${x##*/} in s-* | cache) ;; *) ln -sfn "$x" "$w0/s-$name/" ;; esac
    done
    (
      export r=$r0/s-$name w=$w0/s-$name
      if [ -n "$set_" ] && command -v taskset > /dev/null; then taskset -c "$set_" bash "$0" "$@"; else bash "$0" "$@"; fi
    ) > "$r0/s-$name/log" 2>&1
  }
  if [ ! -x "$w0/bin/cli/phitex" ]; then
    one build "$(wc -l <<< "$cpus")" build
    next=0
    if [ "$(cat "$r0/s-build/status" 2>/dev/null)" != green ] || [ ! -d "$r0/s-build/store" ]; then
      echo red > "$r0/status"
      exit 0
    fi
    mkdir -p "$w0/bin"
    cp -a "$r0/s-build/store/." "$w0/bin/"
    cp -a "$r0/s-build/store" "$r0/store"
  fi
  for spec in "$@"; do
    IFS=: read -r name n rest <<< "$spec"
    IFS=, read -ra args <<< "$rest"
    one "$name" "$n" "${args[@]}" &
  done
  wait
  st=green
  for d in "$r0"/s-*/; do [ "$(cat "$d/status" 2>/dev/null)" = green ] || st=red; done
  echo $st > "$r0/status"
  exit 0
fi
T="cargo test --profile fastdev -p phitex-tex --features profile,observe"
# the test targets the shards run (the build shard builds them)
SUITES="pdftex edits pages page_steps plain etex hints inputs"
# The compiler cache (job.sbatch's PHITEX_SCCACHE, on the shared home):
# the compiler's wrapper once its server starts, else none (the build as
# before). Incremental builds are not cacheable, and a fresh target
# directory gains nothing from them: off with the cache.
cache_on() {
  [ -n "${PHITEX_SCCACHE:-}" ] || return 0
  if "$PHITEX_SCCACHE" --start-server > "$r/sccache-start.txt" 2>&1; then
    export RUSTC_WRAPPER=$PHITEX_SCCACHE CARGO_INCREMENTAL=0
  else
    echo "sccache: no server, building without it: $(tail -1 "$r/sccache-start.txt")"
  fi
}
# its counters in $r/sccache.txt, and the server stopped (its writes done)
cache_off() {
  [ -n "${RUSTC_WRAPPER:-}" ] || return 0
  "$RUSTC_WRAPPER" --show-stats > "$r/sccache.txt" 2>&1
  "$RUSTC_WRAPPER" --stop-server > /dev/null 2>&1
  echo "sccache: $(grep -E '^(Compile requests|Cache hits|Cache misses|Non-cacheable calls) ' "$r/sccache.txt" | sed -E 's/  +/ /g' | tr '\n' ',' | sed 's/,$//')"
}
# update FILE_TXT LABEL DIR MAIN J N ARGS...: a watch writing update
# sections (gate-update.py: N edits, each file kept, the idle write's
# md5 against a cold build of each text), its files read back by the PDF
# writer's update test (harness_pairs); red if either fails
update() {
  local out=$1 label=$2 dir=$3 main=$4 j=$5 n=$6
  shift 6
  rm -rf "$w/$dir" "$w/$dir-cold" "$w/$dir-pairs" && mkdir -p "$w/$dir" && cp "bench/$main" "$w/$dir/"
  (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
    timeout 3000 python3 -I scripts/accl/gate-update.py "$bin" "$w/$dir" "$main" "$j" "$n" "$j" "$r/$label.json" "$w/$dir-pairs" "$@" --program pdftex) >> "$out" 2>&1
  grep -q '"verdict": "same"' "$r/$label.json" 2>/dev/null || red "$label: $(python3 -c "import json;print(json.load(open('$r/$label.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
  if (cd crates/output/pdf && PHITEX_UPDATE_PAIRS="$w/$dir-pairs" timeout 1800 "$w/bin/update" --exact harness_pairs --nocapture > "$r/$label-read.txt" 2>&1); then
    grep '^harness_pairs' "$r/$label-read.txt" | sed "s/^/  $label: /" >> "$out"
  else
    red "$label: the update files read back ($(grep -m1 -E 'panicked|differs|error' "$r/$label-read.txt" | cut -c1-160))"
  fi
  rm -rf "$w/$dir-pairs"
}
# phases LOG LABEL: the median edit's timeline of a watch's stderr
# (PHITEX_TEX_TIME), quoted
phases() {
  python3 -I - "$1" "$2" <<'PY'
import re, sys
blocks, cur = [], None
for line in open(sys.argv[1], errors="replace"):
    if line.startswith("timeline ("):
        cur = [line.rstrip()]
        blocks.append(cur)
    elif cur is not None and line.startswith("  "):
        cur.append(line.rstrip())
        if line.startswith("  clock:"):
            cur = None
    else:
        cur = None
edits = [b for b in blocks[1:] if b]
if not edits:
    print(sys.argv[2] + ": no timeline")
else:
    total = lambda b: float(re.search(r"([0-9.]+) ms from", b[0]).group(1))
    edits.sort(key=total)
    b = edits[len(edits) // 2]
    print("%s (median of %d edits; totals %s ms):" % (sys.argv[2], len(edits), " ".join("%.1f" % total(x) for x in edits)))
    print("\n".join(b[:48]))
PY
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
# One logical CPU per physical core of the allowed set (the lowest of
# each core's siblings), comma-separated: the -j runs up to the core
# count pin to it; the SMT run takes the whole set.
one_per_core() {
  python3 -I -c '
import os
def expand(s):
    out = []
    for part in s.split(","):
        a, _, b = part.partition("-")
        out.extend(range(int(a), int(b or a) + 1))
    return out
cpus = sorted(os.sched_getaffinity(0))
seen, pick = set(), []
for c in cpus:
    try:
        sib = expand(open(f"/sys/devices/system/cpu/cpu{c}/topology/thread_siblings_list").read().strip())
    except OSError:
        sib = [c]
    key = min(sib)
    if key not in seen:
        seen.add(key)
        pick.append(c)
print(",".join(map(str, pick)))'
}

case $shard in
  lint)
    # (no fmt: the image's rustfmt is another version than the pinned
    # nightly's and reflows what the local one keeps, as job.sbatch's
    # gate since b2fbf61a; clippy skips the engine (crates/core/defuse): `--exclude`
    # drops it from the members linted, `--no-deps` from the path
    # dependencies cargo would lint with them)
    built
    cache_on
    if ! cargo clippy --workspace --exclude phitex-defuse --no-deps --all-targets --keep-going -q --message-format short -- -D warnings > "$r/clippy.txt" 2>&1; then
      grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)' "$r/clippy.txt" | sort -u | head -60 | while IFS= read -r l; do echo "clippy: $l"; done >> "$r/red.txt"
      grep -q . "$r/red.txt" || echo "clippy: $(grep -m1 '^error' "$r/clippy.txt")" >> "$r/red.txt"
      status=red
    fi
    # (phitex-live, the session and viewer protocol the frontends' wasm
    # core runs, builds for its target: nothing of the machine in it; an
    # image made before phitex.def added that target's std (7c63e95b)
    # checks wasm32-unknown-unknown, said in wasm-live.txt)
    wt=wasm32-wasip1-threads
    if [ ! -d "$(rustc --print sysroot)/lib/rustlib/$wt" ]; then
      wt=wasm32-unknown-unknown
      echo "the image has no wasm32-wasip1-threads std: checked for $wt" > "$r/wasm-live.txt"
    fi
    if ! cargo check -p phitex-live --target $wt -q --message-format short >> "$r/wasm-live.txt" 2>&1; then
      grep -E '^[^ ]+:[0-9]+:[0-9]+: error' "$r/wasm-live.txt" | sort -u | head -20 | while IFS= read -r l; do echo "$wt (phitex-live): $l"; done >> "$r/red.txt"
      grep -q "$wt (phitex-live)" "$r/red.txt" || echo "$wt (phitex-live): $(grep -m1 '^error' "$r/wasm-live.txt")" >> "$r/red.txt"
      status=red
    fi
    cache_off
    ;;
  build)
    # (the test binaries and the CLI in their own target directories,
    # side by side: one cargo would hold the other's lock)
    cache_on
    a=()
    for s in latex $SUITES; do a+=(--test "$s"); done
    # (and the PDF writer's update test, which reads back a watch's
    # update files: the para and big shards run it on theirs)
    ( { $T "${a[@]}" --no-run --message-format=json-render-diagnostics && { [ ! -f crates/output/pdf/tests/update.rs ] || cargo test --profile fastdev -p phitex-pdf --test update --no-run --message-format=json-render-diagnostics; }; } > "$r/build-tests.json" 2> "$r/build-tests.txt"; echo "EXIT $?" >> "$r/build-tests.txt" ) &
    # (three CLIs: the checks' (profile and observe: CHECK_READS, the
    # recordings, amsldoc), the timings' (profile: the acct lines without
    # the engine's recorder) and the users' (no feature))
    ( CARGO_TARGET_DIR=$PWD/target-cli cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt" ) &
    ( CARGO_TARGET_DIR=$PWD/target-cli-prof cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build-prof.txt" 2>&1; echo "EXIT $?" >> "$r/build-prof.txt" ) &
    ( CARGO_TARGET_DIR=$PWD/target-cli-rel cargo build -q --release -p phitex-cli > "$r/build-rel.txt" 2>&1; echo "EXIT $?" >> "$r/build-rel.txt" ) &
    wait
    built
    cache_off
    grep -q '^EXIT 0$' "$r/build-tests.txt" || red "build (tests): $(grep -m1 '^error' "$r/build-tests.txt")"
    grep -q '^EXIT 0$' "$r/build.txt" || red "build (cli): $(grep -m1 '^error' "$r/build.txt")"
    grep -q '^EXIT 0$' "$r/build-prof.txt" || red "build (cli, profile): $(grep -m1 '^error' "$r/build-prof.txt")"
    grep -q '^EXIT 0$' "$r/build-rel.txt" || red "build (cli, release): $(grep -m1 '^error' "$r/build-rel.txt")"
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
      # (each under the name `phitex`: the CLI is a TeX-compatible
      # command line under any other name)
      mkdir -p "$r/store.tmp/cli/prof" "$r/store.tmp/cli/rel"
      cp target-cli-prof/release/phitex "$r/store.tmp/cli/prof/phitex"
      cp target-cli-rel/release/phitex "$r/store.tmp/cli/rel/phitex"
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
    # (the format built into the cache first, by a one-line document: each
    # -j build then counts only its document's rounds, not the format's
    # evaluation as well)
    mkdir -p "$w/warm" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$w/warm/warm.tex"
    (cd "$w/warm" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/acache \
      timeout 900 "$bin" build warm.tex > "$r/ams-warm.log" 2>&1) || red "amsldoc: the format build (warm.tex) failed: $(tail -3 "$r/ams-warm.log" | tr '\n' ' ' | cut -c1-200)"
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
    # (each evaluation's runs and per-round memo hits by round,
    # `acct-edit:` lines, PHITEX_ROUND_ACCT: the watch's cold build first,
    # then one line an edit)
    (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/acache \
      PHITEX_ROUND_ACCT=1 PHITEX_NO_HEIGHTS=1 GATE_EDIT_STDERR="$r/edits-j$j.err" \
      timeout 2400 python3 -I scripts/accl/gate-edits.py "$bin" "$w/amsh" amsldoc.tex $j $n $j "$r/edits-j$j.json" --exact-pdf) >> "$r/ams.txt" 2>&1
    grep -E '^acct-edit:' "$r/edits-j$j.err" 2>/dev/null | cut -c1-400 >> "$r/ams.txt"
    grep -q '"verdict": "same"' "$r/edits-j$j.json" 2>/dev/null || red "amsldoc edit history -j $j: $(python3 -c "import json;print(json.load(open('$r/edits-j$j.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
    ;;
  editkinds)
    # editkinds: every edit kind on amsldoc, one watch session per -j,
    # on the timings' CLI (profile: the acct lines); the script staged
    # with this one (GATE_EDITKINDS), else the checkout's
    fetched
    bin=$w/bin/cli/prof/phitex
    py=${GATE_EDITKINDS:-scripts/accl/gate-editkinds.py}
    # (the format built first, by a one-line document: the first session's
    # cold watch build then times the document, as the second's does, not
    # the format's build)
    mkdir -p "$w/ekwarm" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$w/ekwarm/warm.tex"
    (cd "$w/ekwarm" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/ekcache timeout 900 "$bin" build warm.tex > /dev/null 2>&1)
    for j in 8 1; do
      rm -rf "$w/ek" "$w/ek-cold" && cp -a bench/amsldoc "$w/ek"
      (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/ekcache \
        PHITEX_ROUND_ACCT=1 PHITEX_RUN_AUDIT=1 GATE_EDIT_STDERR="$r/editkinds-j$j.err" \
        timeout 2400 python3 -I "$py" "$bin" "$w/ek" amsldoc.tex $j "$r/editkinds-j$j.json" --exact-pdf) >> "$r/editkinds.txt" 2>&1
      v=$(python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
bad = [e["kind"] for e in r["edits"] if not e["same"]]
print(r["verdict"] + (": " + ", ".join(bad) if bad else ""))' "$r/editkinds-j$j.json" 2>/dev/null || echo 'no result')
      [ "$v" = same ] || red "amsldoc edit kinds -j $j: $v"
    done
    ;;
  control)
    # control CTL: the control commit's release CLI built, its cold watch
    # builds at -j 1, 8, 16 timed as the ams shard times the head's
    ctl=$1
    printf '%s\n' "$GATE_TIME_PY" > "$w/gate-time.py"
    rm -rf "$w/control" && mkdir -p "$w/control" && git archive "$ctl" | tar -x -C "$w/control"
    ln -s "$PWD/upstream" "$w/control/upstream"
    ( cd "$w/control" && cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > "$r/build-control.txt" 2>&1 ) || red "control $ctl did not build: $(grep -m1 '^error' "$r/build-control.txt")"
    built
    b=$w/control/target/release/phitex
    for j in 1 8 16; do
      rm -rf "$w/ams" "$w/wcache" && cp -a bench/amsldoc "$w/ams"
      SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/wcache NO_COLOR=1 \
        timeout 900 python3 -I "$w/gate-time.py" run "$b" "$w/ams" amsldoc.tex $j "$r/cold-control-j$j.json" "$r/cold-control-j$j.err" \
        || red "cold control -j $j: exit $?"
    done
    ;;
  exp)
    # exp: para4000 runs to tell the machine from the engine: one -j 1
    # alone, eight -j 1 processes at once (one core each), -j 8 on eight
    # cores and -j 32 on all, each with every run's time
    # (PHITEX_RUN_TIMES); the average run by operation of each
    fetched
    bin=$w/bin/cli/prof/phitex
    mkdir -p "$w/exp" && cp bench/para4000.tex "$w/exp/"
    (cd "$w/exp" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 1 --output-dir=warm --program pdftex para4000.tex > /dev/null 2>&1)
    cores=$(one_per_core); all=$(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)
    IFS=, read -r -a cs <<< "$cores"
    avg() { awk '/^runtime/{d=($5-$4)*1000; s[$2]+=d; c[$2]++} END{for(k in c) printf " %s %d runs %.1f us", k, c[k], s[k]/c[k]}' "$1"; }
    run1() { (cd "$w/exp" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_RUN_TIMES=1 PHITEX_TEX_TIME=1 NO_COLOR=1 \
      timeout 900 taskset -c "$2" "$bin" build -j "$3" --output-dir="o-$1" --program pdftex para4000.tex > "$r/exp-$1.log" 2>&1); }
    run1 alone "${cs[0]}" 1
    echo "exp alone -j 1:$(avg "$r/exp-alone.log")" >> "$r/exp.txt"
    for i in 0 1 2 3 4 5 6 7; do run1 "c$i" "${cs[$i]}" 1 & done; wait
    for i in 0 1 2 3 4 5 6 7; do echo "exp concurrent $i -j 1:$(avg "$r/exp-c$i.log")" >> "$r/exp.txt"; done
    run1 j8 "$(IFS=,; echo "${cs[*]:0:8}")" 8
    echo "exp -j 8:$(avg "$r/exp-j8.log")" >> "$r/exp.txt"
    run1 j32 "$all" 32
    echo "exp -j 32:$(avg "$r/exp-j32.log")" >> "$r/exp.txt"
    grep -h '^acct: wall\|^acct: op time\|^acct: .*refuted by field' "$r/exp-j8.log" "$r/exp-j32.log" >> "$r/exp.txt"
    ;;
  big)
    # big: bench/para40000.tex cold at -j 1, 8 and 16 (wall, the round's
    # op, commit, check and idle, the link's phases), then three
    # one-word edits at -j 8 timed from the save to the build's event
    fetched
    bin=$w/bin/cli/prof/phitex
    echo "big node ${SLURMD_NODENAME:-$(cat /proc/sys/kernel/hostname 2>/dev/null)} cpus $(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)" >> "$r/big.txt"
    mkdir -p "$w/big" && cp bench/para40000.tex "$w/big/"
    (cd "$w/big" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para40000.tex > "$r/big-warm.log" 2>&1)
    cores=$(one_per_core); all=$(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)
    echo "big cores (one sibling each) $cores" >> "$r/big.txt"
    for j in 1 8 16 32; do
      L=$r/big-j$j.log
      pin=$cores; [ "$j" -gt 16 ] && pin=$all
      s0=$(date +%s.%N)
      (cd "$w/big" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 NO_COLOR=1 \
        timeout 900 taskset -c "$pin" "$bin" build -j $j --output-dir=o$j --program pdftex para40000.tex > "$L" 2>&1)
      e=$?
      s1=$(date +%s.%N)
      echo "shell: the process ran from $s0 to $s1" >> "$L"
      echo "para40000 -j $j: exit $e wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}') md5 $(md5sum < "$w/big/o$j/para40000.pdf" 2>/dev/null | cut -c1-32)" >> "$r/big.txt"
      grep -E '^acct: (round|1 rounds)|^pdf exact|^link:' "$L" | sed 's/^/  /' >> "$r/big.txt"
      [ "$e" = 0 ] || red "para40000 -j $j: exit $e"
    done
    rm -rf "$w/bige" && mkdir -p "$w/bige" && cp bench/para40000.tex "$w/bige/"
    # (the exact file after every edit, --exact, as before; then the
    # watch's default, update sections)
    echo "para40000 watch --exact:" >> "$r/big.txt"
    (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 timeout 1800 python3 -I scripts/accl/edit-one.py "$bin" "$w/bige" para40000.tex 8 3 "$r/big-watch-exact.err" --program pdftex --exact) >> "$r/big.txt" 2>&1
    grep -E '^(edit phases|session phases|pdf exact|link:)' "$r/big-watch-exact.err" | tail -8 | sed 's/^/  /' >> "$r/big.txt"
    phases "$r/big-watch-exact.err" "para40000 --exact edit phases -j 8" >> "$r/big.txt"
    rm -rf "$w/bige" && mkdir -p "$w/bige" && cp bench/para40000.tex "$w/bige/"
    echo "para40000 watch (update sections):" >> "$r/big.txt"
    (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 timeout 1800 python3 -I scripts/accl/edit-one.py "$bin" "$w/bige" para40000.tex 8 5 "$r/big-watch.err" --program pdftex) >> "$r/big.txt" 2>&1
    grep -E '^(edit phases|session phases|pdf exact|link:)' "$r/big-watch.err" | tail -8 | sed 's/^/  /' >> "$r/big.txt"
    phases "$r/big-watch.err" "para40000 edit phases -j 8" >> "$r/big.txt"
    # the update file: word edits each followed by the idle write (the
    # file kept, read back, the idle write's md5 against a cold build),
    # then a burst of edits without the idle wait, at -j 8 and 16
    for j in 8 16; do
      update "$r/big.txt" "big-update-j$j" bigu para40000.tex $j 3 --burst 6
    done
    ;;
  para)
    # para BAR_MD5: the head's release CLI; the format made once untimed,
    # then each -j cold in its own output directory (same cache: the
    # format's, not the document's)
    want=$1
    fetched
    bin=$w/bin/cli/prof/phitex
    mkdir -p "$w/par" && cp bench/para4000.tex "$w/par/"
    (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para4000.tex > "$r/para-warm.log" 2>&1)
    # (the node is shared: each -j runs three times, the min quoted,
    # the median beside it; the node and its load first)
    echo "para node ${SLURMD_NODENAME:-$(cat /proc/sys/kernel/hostname 2>/dev/null)} load $(cut -d' ' -f1-3 /proc/loadavg) cpus $(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)" >> "$r/para.txt"
    cores=$(one_per_core); all=$(awk '/Cpus_allowed_list/{print $2}' /proc/self/status)
    echo "para cores (one sibling each) $cores" >> "$r/para.txt"
    for j in 1 8 16 32; do
      pin=$cores; [ "$j" -gt 16 ] && pin=$all
      walls=""
      best=""
      for k in 1 2 3; do
        Lk=$r/para-j$j-r$k.log
        rm -rf "$w/par/o$j"
        s0=$(date +%s.%N)
        (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 \
          timeout 900 taskset -c "$pin" "$bin" build -j $j --output-dir=o$j --program pdftex para4000.tex > "$Lk" 2>&1)
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
    # the users' build (no feature): j1 and j8, three runs each, the min
    for j in 1 8; do
      rw=""
      for k in 1 2 3; do
        rm -rf "$w/par/rel$j"
        s0=$(date +%s.%N)
        (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
          timeout 900 taskset -c "$cores" "$w/bin/cli/rel/phitex" build -j $j --output-dir=rel$j --program pdftex para4000.tex > "$r/para-rel-j$j.log" 2>&1)
        s1=$(date +%s.%N)
        rw="$rw $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}')"
      done
      rm_=$(md5sum < "$w/par/rel$j/para4000.pdf" 2>/dev/null | cut -c1-32)
      echo "para4000 release -j $j: md5 ${rm_:-none} $([ "$rm_" = "$want" ] && echo same || echo DIFFERS) wall_min $(echo $rw | tr ' ' '\n' | sort -n | head -1) walls$rw" >> "$r/para.txt"
      [ "$rm_" = "$want" ] || red "para4000 release -j $j: md5 ${rm_:-none}, bar $want"
    done
    # the committed reads checked again after a j8 build, on the checks'
    # build (the profile CLI above has the check compiled out): a read
    # that no longer holds is a missed invalidation, the gate fails
    bin=$w/bin/cli/phitex
    rm -rf "$w/par/chk"
    (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 \
      timeout 900 taskset -c "$cores" "$bin" build -j 8 --output-dir=chk --program pdftex para4000.tex > "$r/para-check-reads.log" 2>&1)
    stale=$(sed -n 's/^check_reads: \([0-9]*\) stale.*/\1/p' "$r/para-check-reads.log" | tail -1)
    cm=$(md5sum < "$w/par/chk/para4000.pdf" 2>/dev/null | cut -c1-32)
    echo "para4000 checked reads -j 8: ${stale:-none} stale, md5 ${cm:-none} $([ "$cm" = "$want" ] && echo same || echo DIFFERS)" >> "$r/para.txt"
    grep '^check_reads: NodeId' "$r/para-check-reads.log" | head -5 >> "$r/para.txt"
    [ "${stale:-x}" = 0 ] || red "para4000 -j 8 on the checks' build: ${stale:-no} stale reads (PHITEX_CHECK_READS)"
    [ "$cm" = "$want" ] || red "para4000 -j 8 on the checks' build: md5 ${cm:-none}, bar $want"
    # the edit histories (spec-final §0 bytes), on the checks' build
    for j in 8 16; do
      rm -rf "$w/parh" "$w/parh-cold" && mkdir -p "$w/parh" && cp bench/para4000.tex "$w/parh/"
      (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 \
        timeout 2400 python3 -I scripts/accl/gate-edits.py "$bin" "$w/parh" para4000.tex $j 50 $j "$r/edits-j$j.json" --program pdftex --exact) >> "$r/para.txt" 2>&1
      grep -q '"verdict": "same"' "$r/edits-j$j.json" 2>/dev/null || red "para4000 edit history -j $j: $(python3 -c "import json;print(json.load(open('$r/edits-j$j.json')).get('verdict'))" 2>/dev/null || echo 'no result')"
    done
    # one edit's phases at -j 8: 10 edits with the watch's timelines
    # (PHITEX_TEX_TIME), the median edit's table quoted (the engine's
    # evaluation against the phases around it)
    rm -rf "$w/parp" "$w/parp-cold" && mkdir -p "$w/parp" && cp bench/para4000.tex "$w/parp/"
    # (twice: the engine's heights on, as profile builds run, and off,
    # as release builds run, PHITEX_NO_HEIGHTS)
    for hv in on off; do
    rm -rf "$w/parp" "$w/parp-cold" && mkdir -p "$w/parp" && cp bench/para4000.tex "$w/parp/"
    (SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_TEX_TIME=1 GATE_EDIT_STDERR="$r/edit-phases-j8-$hv.log" \
      PHITEX_NO_HEIGHTS=$([ $hv = off ] && echo 1) \
      timeout 900 python3 -I scripts/accl/gate-edits.py "$bin" "$w/parp" para4000.tex 8 10 7 "$r/edits-phases-j8-$hv.json" --program pdftex --exact) > /dev/null 2>&1
    echo "(heights $hv)" >> "$r/para.txt"
    python3 -I - "$r/edit-phases-j8-$hv.log" >> "$r/para.txt" <<'PY'
import re, sys
blocks, cur = [], None
for line in open(sys.argv[1], errors="replace"):
    if line.startswith("timeline ("):
        cur = [line.rstrip()]
        blocks.append(cur)
    elif cur is not None and line.startswith("  "):
        cur.append(line.rstrip())
        if line.startswith("  clock:"):
            cur = None
    else:
        cur = None
edits = [b for b in blocks[1:] if b]
if not edits:
    print("para4000 edit phases -j 8: no timeline")
else:
    total = lambda b: float(re.search(r"([0-9.]+) ms from", b[0]).group(1))
    edits.sort(key=total)
    b = edits[len(edits) // 2]
    print("para4000 edit phases -j 8 (median of %d edits; totals %s ms):" % (len(edits), " ".join("%.1f" % total(x) for x in edits)))
    print("\n".join(b[:40]))
PY
    done
    # the update file (watch's default): word edits each followed by
    # the idle write, then a burst without the idle wait, at -j 8 and 16
    # (the walls without the timelines); an insert every other edit at
    # -j 8; the update run's phases at -j 8 (PHITEX_TEX_TIME)
    bin=$w/bin/cli/prof/phitex
    for j in 8 16; do
      update "$r/para.txt" "para-update-j$j" paru para4000.tex $j 10 --burst 10
    done
    update "$r/para.txt" "para-update-insert-j8" paru para4000.tex 8 6 --insert
    GATE_EDIT_STDERR="$r/update-phases-j8.log" PHITEX_TEX_TIME=1 update "$r/para.txt" "para-update-phases-j8" paru para4000.tex 8 6 --burst 6
    phases "$r/update-phases-j8.log" "para4000 update edit phases -j 8" >> "$r/para.txt"
    python3 -I scripts/accl/gate-acct.py table para "$r" > "$r/table.txt" 2>&1
    ;;
  cg)
    # cg: each document of GATE_CG_DOC (default para4000 and macros) at
    # one thread under callgrind (the build shard's release CLI: the
    # profiling build's code, its symbols and line tables; callgrind
    # attributes by symbol, not by inlined frame), the format made once
    # unprofiled. Prints the Ir in all, the startup's (all but the
    # evaluation, `Graph::settle`), the node bodies' (the operations'
    # runs, inclusive) over the commands they did (`acct: commands`),
    # the phases' table (cgtable) and the top 15 functions by exclusive
    # Ir; para4000's files are cg.*, another document's cg-DOC.*
    fetched
    bin=$w/bin/cli/prof/phitex
    : > "$r/cg.txt"
    for path in ${GATE_CG_DOC:-para4000 macros}; do
      # (a document under bench/, by its path there: idioms/idiom-def)
      doc=${path##*/}
      p=$r/cg; [ "$doc" = para4000 ] || p=$r/cg-$doc
      mkdir -p "$w/cg" && cp "bench/$path.tex" "$w/cg/"
      (cd "$w/cg" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 1 --output-dir=warm --program pdftex "$doc.tex" > "$p-warm.log" 2>&1)
      (cd "$w/cg" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_THREADS=1 \
        timeout 3000 valgrind --tool=callgrind --dump-instr=yes --callgrind-out-file="$p.out" "$bin" build -j 1 --output-dir=o1 --program pdftex "$doc.tex" > "$p-run.log" 2>&1) \
        || red "cg: callgrind run of $doc exit $?"
      # (the document's PDF against pdfTeX's: GATE_CG_MD5_<doc>)
      want=$(eval echo "\${GATE_CG_MD5_$doc:-}")
      [ -n "$want" ] || case $doc in
        para4000) want=4146793290dc0ea887d79a1d2d1b4c89 ;;
        macros) want=af7d38807b2fa1b32cd118c999a6c63b ;;
      esac
      got=$(md5sum < "$w/cg/o1/$doc.pdf" 2>/dev/null | cut -c1-32)
      echo "$doc md5 $got (pdfTeX $want)" >> "$r/cg.txt"
      [ -z "$want" ] || [ "$got" = "$want" ] || red "cg: $doc md5 $got, pdfTeX's $want"
      callgrind_annotate --threshold=100 "$p.out" > "$p.excl.txt" 2>&1
      callgrind_annotate --inclusive=yes --threshold=100 "$p.out" > "$p.incl.txt" 2>&1
      python3 -I - "$doc" "$p.incl.txt" "$p.excl.txt" "$p-run.log" "$w/cg/$doc.tex" "$w/cg/o1/$doc.pdf" >> "$r/cg.txt" <<'PY'
import re, sys, zlib
doc, incl, excl, log, tex, pdf = sys.argv[1:]
ir = {}
for line in open(incl, errors="replace"):
    m = re.match(r" *([0-9,]+) +\([ 0-9.]+%\) +(.*)$", line)
    if m:
        ir.setdefault(m.group(2), []).append(int(m.group(1).replace(",", "")))
def num(pat):
    rx = re.compile(pat)
    return max((max(v) for k, v in ir.items() if rx.search(k)), default=0)
total = next((int(l.split()[0].replace(",", "")) for l in open(excl) if "PROGRAM TOTALS" in l), 0)
settle = num(r"phitex_tex::api::Graph>::settle")
op = lambda n: num(re.escape("<phitex_tex::" + n + " as phitex_defuse::api::Operation>::run"))
ops = ["ops::Exec", "pdfout::PdfPage", "pages::finish::Finish", "pages::builder::Pages", "pages::piece::Resume", "ops::Link", "ops::Dvi", "ops::Files", "ops::Mark"]
bodies = sum(op(n) for n in ops)
L = open(log, errors="replace").read()
g = lambda pat: (lambda m: int(m[-1]) if m else 0)(re.findall(pat, L))
cmds, chunks, pages = g(r"acct: commands ([0-9]+)"), g(r"incr walk: ([0-9]+) chunks"), g(r"([0-9]+) pages?, ")
src = open(tex, encoding="latin-1").read()
letters = sum(c.isalpha() for c in src)
pars = sum(1 for l in src.splitlines() if l and not l.startswith("\\"))
# the bytes deflated: every FlateDecode stream of the PDF inflated
flate = 0
try:
    data = open(pdf, "rb").read()
    for m in re.finditer(rb"/FlateDecode[^>]*>>\s*stream\r?\n", data):
        try:
            flate += len(zlib.decompressobj().decompress(data[m.end():]))
        except zlib.error:
            pass
except OSError:
    pass
# (pdfTeX 1.40.29 under callgrind on bench/macros.tex: 942 Ir per command
# of this count, the 180000 iterations between its 20000- and
# 200000-iteration runs; 11.70 G Ir, 1.14 s in all)
print("%s -j 1 under callgrind: Ir %d; startup (all but the evaluation) %d; evaluation %d, of which node bodies %d; commands %d; evaluation per command %s Ir, node bodies per command %s Ir (pdfTeX 942 Ir per command on macros)" % (
    doc, total, total - settle, settle, bodies, cmds,
    "%.0f" % (settle / cmds) if cmds else "?", "%.0f" % (bodies / cmds) if cmds else "?"))
G = lambda x: "%.2f" % (x / 1e9)
per = lambda x, n, u: "%.0f per %s" % (x / n, u) if n else ""
lb = num(r"phitex_typeset::linebreak::line_break( |$)")
rows = [
    ("hint analysis before the build (incr::analyse_reusing)", num(r"phitex_tex::incr::analyse_reusing"), chunks, "chunk"),
    ("a piece's run (ops::Exec::run)", op("ops::Exec"), pars, "paragraph"),
    ("  of which Run::char_run", num(r"<phitex_tex::run::Run>::char_run"), letters, "letter"),
    ("  of which Run::line_break_ outside typeset::line_break", max(0, max(num(r"<phitex_tex::run::Run>::line_break_( |$)"), num(r"<phitex_tex::run::Run>::line_break( |$)")) - lb), pars, "paragraph"),
    ("  of which typeset::line_break", lb, pars, "paragraph"),
    ("  of which Run::load_slow", num(r"<phitex_tex::run::Run>::load_slow"), pars, "paragraph"),
    ("  of which Run::finisher_args", num(r"<phitex_tex::run::Run>::finisher_args"), pars, "paragraph"),
    ("  of which nodes::encode_line", num(r"phitex_tex::nodes::encode_line"), pars, "paragraph"),
    ("  of which handle_right_brace (box_end_in in it)", num(r"<phitex_tex::run::Run>::handle_right_brace"), pars, "paragraph"),
    ("a page's PDF output (pdfout::PdfPage::run)", op("pdfout::PdfPage"), pages, "page"),
    ("  of which PdfState::char_out and char_run", num(r"<phitex_pdf::PdfState>::char_out") + num(r"<phitex_pdf::PdfState>::char_run"), letters, "letter"),
    ("  of which begin_string_tail", num(r"<phitex_pdf::PdfState>::begin_string_tail"), letters, "letter"),
    ("  of which deflate::compress_with (%d bytes in)" % flate, num(r"phitex_codecs::deflate::compress_with"), flate, "byte in"),
    ("a finisher (pages::finish::Finish::run)", op("pages::finish::Finish"), pages, "page"),
    ("page steps (pages::builder::Pages::run)", op("pages::builder::Pages"), pages, "page"),
]
top = [r[1] for r in rows if not r[0].startswith(" ")]
rows.append(("the engine and everything else", total - sum(top), 0, ""))
rows.append(("total", total, 0, ""))
print("%s phases (G Ir, inclusive; %d chunks, %d paragraphs, %d letters, %d pages):" % (doc, chunks, pars, letters, pages))
for name, x, n, u in rows:
    print("  %-62s %6s  %s" % (name, G(x), per(x, n, u)))
print("%s top 15 by exclusive Ir:" % doc)
k = 0
for line in open(excl, errors="replace"):
    if re.match(r" *[0-9,]+ +\(", line) and "PROGRAM TOTALS" not in line:
        print(line.rstrip()[:200])
        k += 1
        if k == 15:
            break
PY
    done
    ;;
  *) red "unknown shard $shard" ;;
esac
echo "wall_s $(( $(date +%s) - t0 ))" >> "$r/times.txt"
echo "$status" > "$r/status"
echo "$shard $*: $status ($(grep -c . "$r/red.txt") red), $(tr '\n' ' ' < "$r/times.txt")" > "$r/summary.txt"
# (a failed build fails its job: the shards waiting on it are cancelled)
[ "$shard" = build ] && [ "$status" != green ] && exit 1
exit 0
