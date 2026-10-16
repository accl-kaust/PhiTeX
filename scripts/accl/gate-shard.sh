#!/bin/bash
# One shard of the fast gate (scripts/accl/gate-fast), as job.sbatch's
# `cmd` task runs it: inside the container, in the commit's checkout,
# $r the results directory, $w the workspace. gate-fast passes this
# file's text to `bash -c` (the commit under test need not have it).
#
#   gate-shard.sh lint                  clippy -D warnings (no fmt: version skew)
#   gate-shard.sh suites                the local suites of phitex-tex
#   gate-shard.sh latex K N             the latex target's tests, shard K of N
#   gate-shard.sh ams ORACLE_MD5 CTL    amsldoc --exact-pdf at -j 1 and -j 8;
#                                       cold watch timing at -j 1/8/16, head
#                                       and control commit CTL
#   gate-shard.sh para BAR_MD5          bench/para4000.tex (plain pdfTeX) cold
#                                       at -j 1/8/16: md5 against BAR_MD5,
#                                       wall, us per paragraph, realised
#                                       critical path, guess counters
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
  suites)
    S="pdftex edits pages plain etex hints relaxed inputs defuse_gaps"
    a=()
    for s in $S; do a+=(--test "$s"); done
    $T "${a[@]}" --no-run > "$r/build.txt" 2>&1 || red "build: $(grep -m1 '^error' "$r/build.txt")"
    built
    for s in $S; do
      ( s0=$(date +%s); $T --test "$s" -- -Z unstable-options --report-time > "$r/t-$s.txt" 2>&1; echo "EXIT $?" >> "$r/t-$s.txt"; echo "$s $(( $(date +%s) - s0 ))" >> "$r/suite-walls.txt" ) &
    done
    wait
    for s in $S; do reds "$r/t-$s.txt" "$s::"; done
    ;;
  latex)
    k=$1 n=$2
    $T --test latex --no-run > "$r/build.txt" 2>&1 || red "build: $(grep -m1 '^error' "$r/build.txt")"
    built
    $T --test latex -- --list 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/all.txt"
    $T --test latex -- --list --ignored 2>/dev/null | sed -n 's/: test$//p' | sort > "$w/ign.txt"
    # (the order: GATE_LATEX_ORDER, names heaviest first, else by name;
    # the shards deal them out in turn)
    comm -23 "$w/all.txt" "$w/ign.txt" > "$w/run.txt"
    if [ -n "${GATE_LATEX_ORDER:-}" ]; then
      { tr ' ' '\n' <<< "$GATE_LATEX_ORDER" | grep -Fxf "$w/run.txt"; grep -Fxvf <(tr ' ' '\n' <<< "$GATE_LATEX_ORDER") "$w/run.txt"; } > "$w/ord.txt"
    else
      cp "$w/run.txt" "$w/ord.txt"
    fi
    awk -v k="$k" -v n="$n" '(NR-1)%n==k' "$w/ord.txt" > "$r/tests.txt"
    if [ -s "$r/tests.txt" ]; then
      # (one process, the tests in its threads as a plain `cargo test`
      # runs them: the prelude cache is written once; nightly's
      # --report-time gives each test's wall for the resharding)
      $T --test latex -- --exact $(cat "$r/tests.txt") -Z unstable-options --report-time > "$r/t-latex.txt" 2>&1
      echo "EXIT $?" >> "$r/t-latex.txt"
      sed -nE 's/^test (.*) \.\.\. (ok|FAILED) <([0-9.]+)s>$/\1 \3 \2/p' "$r/t-latex.txt" > "$r/test-walls.txt"
      reds "$r/t-latex.txt" "latex::"
      # (a crash takes the tests that had not finished with it)
      if ! grep -q '^EXIT 0$' "$r/t-latex.txt"; then
        while read -r t; do
          grep -qE "^test $t \.\.\. (ok|FAILED)" "$r/t-latex.txt" || red "latex::$t (did not finish)"
        done < "$r/tests.txt"
      fi
    fi
    # the corpus fixtures that are red (ignored "red: corpus ..."): run,
    # reported apart (known-red.txt; one that passes is news), never red
    grep '^corpus_' "$w/ign.txt" | awk -v k="$k" -v n="$n" '(NR-1)%n==k' > "$r/known.txt"
    if [ -s "$r/known.txt" ]; then
      $T --test latex -- --ignored --exact $(cat "$r/known.txt") > "$r/t-known.txt" 2>&1
      grep -E '^test .* \.\.\. FAILED$' "$r/t-known.txt" | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' > "$r/known-red.txt"
      grep -E '^test .* \.\.\. ok$' "$r/t-known.txt" | sed -E 's/^test (.*) \.\.\. ok$/\1 (passes now)/' >> "$r/known-red.txt"
    fi
    ;;
  ams)
    # ams ORACLE_MD5 CONTROL: the head's and the control's release CLI
    # built side by side on this node; amsldoc --exact-pdf at -j 1 and 8
    # (md5 against the oracle, the round counters); then the cold watch
    # build (bench/ams-cold-tmp.sh's command) at -j 1, 8, 16 for both,
    # timed by op class (gate-time.py, passed as $GATE_TIME_PY)
    want=$1 ctl=${2:-}
    printf '%s\n' "$GATE_TIME_PY" > "$w/gate-time.py"
    ( CARGO_BUILD_JOBS=9 cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt" ) &
    if [ -n "$ctl" ]; then
      rm -rf "$w/control" && mkdir -p "$w/control" && git archive "$ctl" | tar -x -C "$w/control"
      ln -s "$PWD/upstream" "$w/control/upstream"
      ( cd "$w/control" && CARGO_BUILD_JOBS=9 cargo build -q --release -p phitex-cli > "$r/build-control.txt" 2>&1; echo "EXIT $?" >> "$r/build-control.txt" ) &
    fi
    wait
    grep -q '^EXIT 0$' "$r/build.txt" || red "build: $(grep -m1 '^error' "$r/build.txt")"
    [ -z "$ctl" ] || grep -q '^EXIT 0$' "$r/build-control.txt" || echo "control $ctl did not build: $(grep -m1 '^error' "$r/build-control.txt")" >> "$r/ams.txt"
    built
    bin=$PWD/target/release/phitex
    for j in 1 8; do
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
    done
    # the cold watch builds, head and control alternating per -j (same
    # node, same command, a fresh cache each)
    for j in 1 8 16; do
      for side in head control; do
        if [ $side = head ]; then b=$bin; else b=$w/control/target/release/phitex; [ -x "$b" ] || continue; fi
        rm -rf "$w/ams" "$w/wcache" && cp -a bench/amsldoc "$w/ams"
        SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/wcache NO_COLOR=1 \
          timeout 900 python3 -I "$w/gate-time.py" run "$b" "$w/ams" amsldoc.tex $j "$r/cold-$side-j$j.json" "$r/cold-$side-j$j.err" \
          || echo "cold $side -j $j: exit $?" >> "$r/ams.txt"
      done
    done
    ls "$r"/cold-*.json > /dev/null 2>&1 && python3 -I "$w/gate-time.py" show $(ls "$r"/cold-*-j1.json "$r"/cold-*-j8.json "$r"/cold-*-j16.json 2>/dev/null) > "$r/cold.txt" 2>&1
    ;;
  para)
    # para BAR_MD5: the head's release CLI; the format made once untimed,
    # then each -j cold in its own output directory (same cache: the
    # format's, not the document's)
    want=$1
    cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt"
    grep -q '^EXIT 0$' "$r/build.txt" || red "build: $(grep -m1 '^error' "$r/build.txt")"
    built
    bin=$PWD/target/release/phitex
    mkdir -p "$w/par" && cp bench/para4000.tex "$w/par/"
    (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 "$bin" build -j 8 --output-dir=warm --program pdftex para4000.tex > "$r/para-warm.log" 2>&1)
    for j in 1 8 16; do
      L=$r/para-j$j.log
      rm -rf "$w/par/o$j"
      s0=$(date +%s.%N)
      (cd "$w/par" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CHECK_READS=1 NO_COLOR=1 \
        timeout 900 "$bin" build -j $j --output-dir=o$j --program pdftex para4000.tex > "$L" 2>&1)
      e=$?
      s1=$(date +%s.%N)
      m=$(md5sum < "$w/par/o$j/para4000.pdf" 2>/dev/null | cut -c1-32)
      [ -n "$m" ] || m=none
      sum() { grep -E 'round guesses: ' "$L" | grep -oE "[0-9]+ $1" | awk '{s+=$1} END {print s+0}'; }
      cp_n=$(grep -oE 'realised cp [0-9]+ nodes [^,]+' "$L" | awk '{if ($3+0 >= m) {m=$3+0; t=$5}} END {print m+0, t}')
      nodes=$(grep -oE '[0-9]+ nodes run of [0-9]+ live' "$L" | tail -1 | awk '{print $1"/"$5}')
      wall=$(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.2f", b-a}')
      echo "para4000 -j $j: exit $e md5 $m bar $want $([ "$m" = "$want" ] && echo same || echo DIFFERS) wall $wall us/paragraph $(awk -v w="$wall" 'BEGIN{printf "%.0f", w*1e6/4000}') nodes ${nodes:-?} cp $cp_n claims_refuted $(sum 'claims refuted') effects_refuted $(sum 'effects refuted') defs_refuted $(sum 'definitions refuted') stale_reads $(grep -oE 'check_reads: [0-9]+ stale' "$L" | awk '{s+=$2} END {print s+0}')" >> "$r/para.txt"
      [ "$e" = 0 ] || red "para4000 -j $j: exit $e $(grep -m1 -E 'panicked at|error' "$L" | cut -c1-160)"
      [ "$m" = "$want" ] || red "para4000 -j $j: md5 $m, bar $want"
    done
    ;;
  *) red "unknown shard $shard" ;;
esac
echo "wall_s $(( $(date +%s) - t0 ))" >> "$r/times.txt"
echo "$status" > "$r/status"
echo "$shard $*: $status ($(grep -c . "$r/red.txt") red), $(tr '\n' ' ' < "$r/times.txt")" > "$r/summary.txt"
exit 0
