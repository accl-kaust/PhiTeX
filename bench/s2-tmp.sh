#!/bin/bash
# slow8: probe biblatex j4 (and j8) 400 s: run log summary (profile+observe build)
git log -1 --format="commit %h"; date; nproc
CARGO_BUILD_JOBS=32 CARGO_TARGET_DIR=$w/target cargo build -q --profile fastdev -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > $r/build.txt 2>&1 || { tail -20 $r/build.txt; exit 1; }
for j in ${JS:-4}; do
mkdir -p $w/v-$j; w=$w/v-$j XENV="PHITEX_RUN_LOG=1" python3 -I bench/survey-tmp.py $w/target/fastdev/phitex $PWD $r/s-$j ${LIM:-400} 1 latex_biblatex_biblatex $j | sed "s/^/j$j /"
f=$(ls $r/s-$j/*.err.gz); zcat $f > $w/e; echo "j$j runs $(grep -c '^run: node' $w/e)"; python3 -I bench/runsum-tmp.py $w/e
grep '^run: node' $w/e | awk -F'\t' '{split($2,a," "); d=(a[3]-a[2])/1e9; b=(d<0.01)?"a<10ms":(d<0.1)?"b<100ms":(d<1)?"c<1s":"d>=1s"; n[b]++; s[b]+=d} END{for(k in n) printf "%s %d %.1f s\n", k, n[k], s[k]}' | sort
grep '^run: node' $w/e | awk -F'\t' '{split($2,a," "); print int(a[2]/2e10)*20}' | uniq -c | awk '{c[$2]+=$1} END{for(k in c) print k, c[k]}' | sort -n | tr '\n' ' '; echo
grep '^commit: node' $w/e | awk '{c+=$7; r+=$9; k+=$11} END{printf "commit %.1f s reask %.1f s walked %d\n", c/1e9, r/1e9, k}'
grep '^commit: node' $w/e | sort -k11 -rn | head -5
grep -E "^stall|fatal|Fatal|no legal" $w/e | head -5
done
date
