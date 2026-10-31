cargo test -q --profile fastdev -p phitex-tex --test latex -- latexmk_ > $r/t.txt 2>&1; grep -E "^test .*FAILED|test result|panicked" $r/t.txt > $r/tests.txt
DOCS="lu amsidx ams objref linkif ret2" J="1 8 8 8 8 8 8 8 8" TRACE="" bash bench/r3/trace.sh > /dev/null 2>&1
for p in $r/*-j*.pdf; do echo "$(basename $p) qpdf $(qpdf --check $p > /dev/null 2>&1; echo $?)"; done >> $r/qpdf.txt
cat $r/tests.txt; sort $r/summary.txt | uniq -c | sed 's/ nodes.*//'; sort -k3 $r/qpdf.txt | awk '{print $3}' | sort | uniq -c
