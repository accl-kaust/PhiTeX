#!/bin/bash
# bench/flow-par/sched.sh: the scheduler's events (PHITEX_SCHED) of cold builds of
# CASES (big2, a LaTeX document on the saved format, latex.ltx) at each of
# JS threads, one run at a time, READS policy (suspend, the CLI's cold
# build, by default); bench/flow-par/sched.py's report of each in $r/summary.txt.
set -u
R=${r:-/tmp/sched-results}; W=${w:-/tmp/sched-work}/sched; mkdir -p "$R" "$W"
P=$PWD
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
CASES=${CASES:-"big2 titlepage latex.ltx"}; JS=${JS:-"1 8 48"}; READS=${READS:-suspend}
log() { echo "$*" | tee -a "$R/summary.txt"; }
log "commit $(git rev-parse --short HEAD); node $(cat /proc/sys/kernel/hostname); $(nproc) cpus; cases $CASES; -j $JS; reads $READS"
cargo build --release -q -p phitex-flow --bin phitex-flow > "$R/build.log" 2>&1 || { log "build failed"; tail -30 "$R/build.log"; exit 1; }
B=$P/target/release/phitex-flow
L=crates/phitex-flow/tests/latex
fmt=$W/latex.fmt
"$B" $(kpsewhich latex.ini) $(bench/flow-par/dirsof.sh sections) --jobname latex --terminal latex.ini -j 8 --dump-fmt $fmt -o $W/fmt.dvi > "$R/format.out" 2>&1
mkdir -p $W/big2 $W/ltx; cp bench/flow-par/big2.tex $W/big2/; cp $(kpsewhich latex.ltx) $W/ltx/
LTXARGS="--etex --jobname latex --input-dir $L $(sed 's/^/--input-dir /' $L/dirs.txt | tr '\n' ' ' | bench/flow-par/fix.sh)"
for c in $CASES; do
  for j in $JS; do
    f=$R/sched-$c-j$j.tsv; rm -f $f
    case $c in
      big2) cmd=("$B" $W/big2/big2.tex -j $j --reads $READS -o $W/big2/big2.dvi) ;;
      latex.ltx) cmd=("$B" $W/ltx/latex.ltx $LTXARGS -j $j --reads $READS -o $W/ltx/latex.dvi) ;;
      *) cmd=(bench/flow-par/rundoc.sh "$B" $c $fmt $W/docs) ;;
    esac
    export J=$j EXTRA="--reads $READS"
    pin=$(python3 bench/flow-par/cpus.py percore $j 2>/dev/null || true)
    read s rss st < <(PIN=$pin PHITEX_SCHED=$f PHITEX_SCHED_DEPS=${HIND:-} python3 bench/flow-par/meas.py $R/out-$c-j$j.txt timeout 1200 "${cmd[@]}")
    log "$c -j $j (cpus $pin): ${s}s ${rss}KB status $st"
    grep -E '^cold:|^  serial|rounds' $R/out-$c-j$j.txt | tee -a "$R/summary.txt"
    if [ -n "${HIND:-}" ]; then echo "== hindsight $c (-j $j)" | tee -a "$R/summary.txt"; python3 bench/flow-par/hindsight.py $f 2>&1 | tee -a "$R/summary.txt"; else python3 bench/flow-par/sched.py $f 2>&1 | tee -a "$R/summary.txt"; fi
    gzip -f $f
  done
done
