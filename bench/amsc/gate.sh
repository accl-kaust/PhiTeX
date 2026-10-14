#!/bin/bash
# smoke LABEL (not committed): fresh formats, one build, then the fixed smoke set in parallel; <= 15 min
export PHITEX_CACHE_DIR=$PWD/target/pcache; rm -rf $PHITEX_CACHE_DIR target/cache/*.fmt; mkdir -p $PHITEX_CACHE_DIR ../results
TOP=$PWD; R=$PWD/../results; git log --oneline -1; t0=$(date +%s)
cargo test -p phitex-tex --profile fastdev --test plain --test latex --test chain --test page_steps --no-run 2>&1 | grep -oE "Executable .*\((.*)\)" > $R/exe.txt
cargo test -p phitex-cli --profile fastdev --test ladder --test corpus_repros --no-run 2>&1 | grep -oE "Executable .*\((.*)\)" >> $R/exe.txt
# repro oracles not yet made: pdflatex to its fixpoint, pinned as scripts/ci/common.py pins them
for t in $PWD/oracle/fixtures/corpus-repros/*.tex; do p=${t%.tex}.pdf; [ -f $p ] && continue; n=$(basename ${t%.tex}); d=$R/oracle-$n; mkdir -p $d; cp $t $d/; (cd $d && for i in 1 2 3 4 5; do cp $n.aux prev.aux 2>/dev/null; SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC pdflatex -interaction=nonstopmode $n.tex > /dev/null 2>&1; cmp -s $n.aux prev.aux && break; done); cp $d/$n.pdf $p && cp $d/$n.pdf $R/repro-$n.pdf; echo "oracle made: $n"; done
exe() { echo "$TOP/$(grep "tests/$1.rs" $R/exe.txt | sed -E 's/.*\((.*)\)/\1/' | head -1)"; }
echo "build $(( $(date +%s) - t0 )) s"
T=$PWD/crates/engines/tex; C=$PWD/crates/cli
(cd $T && timeout 900 $(exe latex) --ignored --exact pdflatex_hello > $R/s-hello.out 2>&1)
(cd $T && timeout 600 $(exe plain) > $R/s-plain.out 2>&1) &
(cd $T && PHITEX_CHAIN=1 timeout 600 $(exe latex) --exact latex_paragraph_nodes --nocapture > $R/s-paras30.out 2>&1) &
(cd $T && timeout 600 $(exe latex) --exact latex_environments --nocapture > $R/s-envs.out 2>&1) &
(cd $T && PHITEX_CHAIN=1 timeout 600 $(exe chain) --ignored --exact para400 --nocapture > $R/s-para400.out 2>&1) &
(mkdir -p $R/gen && cp $PWD/bench/para400.tex $R/gen/ && cd $T && PHITEX_GEN=$R/gen timeout 600 $(exe page_steps) --ignored --exact generated_documents --nocapture > $R/s-p400edit.out 2>&1) &
(cd $C && timeout 900 $(exe ladder) --ignored --exact r03_structure --nocapture > $R/s-r03.out 2>&1) &
(cd $C && timeout 900 $(exe ladder) --ignored --exact r08_hyperref --nocapture > $R/s-r08.out 2>&1) &
[ -f $C/tests/corpus_repros.rs ] && (cd $C && timeout 600 $(exe corpus_repros) > $R/s-corpus.out 2>&1) &
(cd $TOP && timeout 900 cargo test -q -p phitex-cli --profile fastdev --bins --test cli > $R/s-cli.out 2>&1) &
wait
for s in hello plain paras30 envs para400 p400edit r03 r08 corpus cli; do echo "== $s: $(grep -E '^test result|FAILED|panicked at' $R/s-$s.out | head -4 | tr '\n' ' ' | cut -c1-300)"; done
grep -hE "^chains (paras30|envs)|^cold (paras30|envs)|^edit (paras30|envs)" $R/s-paras30.out $R/s-envs.out | cut -c1-230
echo "paras30: undeclared $(grep -c 'undeclared definition of' $R/s-paras30.out), regime $(grep -c 'hint regime violated' $R/s-paras30.out)"
grep -E "^para4?0+0: [0-9]+ nodes run, [0-9]+ runs" $R/s-para400.out | cut -c1-200
grep -E "^para400 " $R/s-p400edit.out | cut -c1-200 | head -8
grep -hE "^ROW" $R/s-r03.out $R/s-r08.out | cut -c1-400
# amscorrect: amsldoc -j1 and -j8, exact, against pdflatex's (bench/amsc/amsldoc-oracle.pdf, same image and dates)
for j in 1 8; do
  d=$R/ams$j; rm -rf $d; mkdir -p $d; cp $TOP/bench/amsc/amsldoc.tex $d/
  (cd $d && SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC timeout 900 /usr/bin/time -f 'WALL %e BUSY %U+%S RSS %M' $TOP/target/fastdev/phitex build -v -j $j --exact-pdf amsldoc.tex > $R/ams-j$j.term 2>&1)
  cp $d/amsldoc.pdf $R/amsldoc-j$j.pdf 2>/dev/null; cp $d/amsldoc.log $R/amsldoc-j$j.log 2>/dev/null
  echo "amsldoc j$j: errors $(grep -c '^error' $R/ams-j$j.term) | $(grep -oE '[0-9]+ pages' $R/ams-j$j.term | head -1) | $(grep -oE '[0-9]+ loop rounds' $R/ams-j$j.term | head -1) | $(grep -oE 'WALL.*' $R/ams-j$j.term) | $(cmp $TOP/bench/amsc/amsldoc-oracle.pdf $R/amsldoc-j$j.pdf 2>&1 | grep -oE 'byte [0-9]+|EOF.*' || echo IDENTICAL)"
done
cmp -s $R/amsldoc-j1.pdf $R/amsldoc-j8.pdf && echo "amsldoc j1 = j8" || echo "amsldoc j1 != j8"
for doc in ${AMSC_DOCS:-}; do d=$R/fx-$doc; mkdir -p $d; cp $TOP/bench/amsc/$doc.tex $d/; (cd $d && SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC timeout 300 $TOP/target/fastdev/phitex build -v -j 1 --exact-pdf $doc.tex > $R/fx-$doc.term 2>&1; cp $doc.pdf $R/fx-$doc.pdf); done
(cd $TOP && cargo clippy -q -p phitex-tex -p phitex-cli --all-targets --profile fastdev -- -D warnings > $R/clippy.out 2>&1; echo "clippy exit $? $(grep -c "^error" $R/clippy.out) errors")
echo "total $(( $(date +%s) - t0 )) s"
