#!/bin/bash
# (inputs not in the repository: bench/flow-par/big2.tex, tikz.tex and its .dirs, pgfman-src/, as agent P staged them; biglatex.tex comes from biglatex.py)
# bench/flow-par/scale.sh: the scaling curve. Cold builds of big2, the six LaTeX
# documents on the saved format and latex.ltx at -j 1 2 4 8 16 32 48 (JS),
# REPS repetitions (the repetition the outer loop), one run at a time; wall
# and peak RSS (meas.py), the CLI's stats lines, the outputs' md5 (DVI and
# written files; latex.ltx's dumped format) compared across -j.
# Rows in $r/raw.tsv, the table in $r/summary.txt; then `phitex-flow
# --scaling` (in process, both read policies).
set -u
R=${r:-/tmp/scale-results}; W=${w:-/tmp/scale-work}/scale; mkdir -p "$R" "$W" "$R/out"
P=$PWD
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
JS=${JS:-"1 2 4 8 16 32 48"}; REPS=${REPS:-3}; LIMIT=${LIMIT:-900}
DOCS=${DOCS:-"sections math tables figures titlepage packages"}
log() { echo "$*" | tee -a "$R/summary.txt"; }
log "commit $(git rev-parse --short HEAD); node $(cat /proc/sys/kernel/hostname); $(nproc) cpus; $(grep -m1 'model name' /proc/cpuinfo)"
t0=$(date +%s)
cargo build --release -q -p phitex-flow --bin phitex-flow > "$R/build.log" 2>&1 || { log "build failed"; tail -30 "$R/build.log"; exit 1; }
log "build $(( $(date +%s) - t0 )) s"
B=$P/target/release/phitex-flow
L=crates/phitex-flow/tests/latex
fmt=$W/latex.fmt
python3 bench/flow-par/meas.py "$R/format.out" "$B" $(kpsewhich latex.ini) $(bench/flow-par/dirsof.sh sections) --jobname latex --terminal latex.ini -j 8 --dump-fmt $fmt -o $W/fmt.dvi > "$W/m.txt"
log "format (latex.ini, -j 8): $(cat $W/m.txt) (wall s, maxrss KB, status); $(ls -l $fmt | awk '{print $5}') bytes"
mkdir -p $W/big2 $W/ltx
cp bench/flow-par/big2.tex $W/big2/
cp $(kpsewhich latex.ltx) $W/ltx/
LTXARGS="--etex --jobname latex --input-dir $L $(sed 's/^/--input-dir /' $L/dirs.txt | tr '\n' ' ' | bench/flow-par/fix.sh)"
# one run: DOC J REP CMD...
one() {
  local doc=$1 j=$2 rep=$3; shift 3
  local o=$R/out/$doc-j$j-r$rep.out m s rss st h cold
  read s rss st < <(python3 bench/flow-par/meas.py $o timeout $LIMIT "$@")
  case $doc in
    big2) h=$(md5sum < $W/big2/big2.dvi | cut -c1-12) ;;
    latex.ltx) h=$( { cat $W/ltx/*.dvi $W/ltx/latex.fmt; ls $W/ltx | grep -v latex.ltx | while read f; do cat $W/ltx/$f; done; } 2>/dev/null | md5sum | cut -c1-12) ;;
    *) h=$( (cd $W/docs/$doc && for f in $(ls); do cat $f; done) | md5sum | cut -c1-12) ;;
  esac
  cold=$(grep -m1 '^cold:' $o | tr '\t' ' ')
  ser=$(grep -m1 '^  serial:' $o | tr '\t' ' ')
  rounds=$(grep -m1 'rounds (ms)' $o | tr '\t' ' ')
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$doc" "$j" "$rep" "$s" "$rss" "$st" "$h" "$cold" "$ser" "$rounds" >> "$R/raw.tsv"
  echo "$doc -j $j rep $rep: ${s}s ${rss}KB st $st md5 $h" >> "$R/progress.txt"
}
for rep in $(seq 1 $REPS); do
  for j in $JS; do
    rm -f $W/big2/big2.dvi
    one big2 $j $rep "$B" $W/big2/big2.tex -j $j -o $W/big2/big2.dvi
  done
  for d in $DOCS; do
    for j in $JS; do
      export J=$j; one $d $j $rep bench/flow-par/rundoc.sh "$B" $d $fmt $W/docs
    done
  done
  for j in $JS; do
    (cd $W/ltx && ls | grep -v '^latex.ltx$' | xargs -r rm -f)
    one latex.ltx $j $rep "$B" $W/ltx/latex.ltx $LTXARGS -j $j --dump-fmt $W/ltx/latex.fmt -o $W/ltx/latex.dvi
  done
  log "rep $rep done at $(( $(date +%s) - t0 )) s"
done
python3 bench/flow-par/scale-table.py "$R/raw.tsv" >> "$R/summary.txt"
# A's in-process mode (fuzz::scaling): the same thread counts, both policies,
# its own corpus (the LaTeX documents on the format, latex.ini's job, big)
for pol in suspend speculate; do
  timeout 3000 "$B" --scaling ${JS// /,} $pol > "$R/scaling-$pol.txt" 2> "$R/scaling-$pol.err"
  log "--scaling $pol: status $? at $(( $(date +%s) - t0 )) s"
  cat "$R/scaling-$pol.txt" >> "$R/summary.txt"
done
