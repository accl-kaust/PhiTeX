#!/bin/bash
# rundoc.sh BIN NAME FMT DOCSDIR: a LaTeX document on the saved format, -j 1, no .aux present
b=$1 n=$2 fmt=$3 D=$4
L=crates/phitex-flow/tests/latex
t=$D/$n; rm -rf $t; mkdir -p $t
if [ -f bench/flow-par/$n.tex ]; then cp bench/flow-par/$n.tex $t/; else cp $L/docs/$n.tex $t/; fi
cp $L/docs/$n-* $t/ 2>/dev/null; cp bench/flow-par/$n-* $t/ 2>/dev/null
exec ${WRAP:-} "$b" $t/$n.tex $(bench/flow-par/dirsof.sh $n) --jobname $n --fmt $fmt --terminal "&latex $n" -j ${J:-1} ${EXTRA:-} -o $t/$n.dvi
