#!/bin/bash
# (temporary) where a streamed page's file costs: callgrind on this commit's release CLI (line
# tables), the watch with its viewer at -j1, amsldoc's clean build and EDITS one-word edits,
# collected only inside `page_alone` (and, apart, the whole run's inclusive costs).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
bin=$PWD/target/release/phitex
# (DOC: a TeX Live manual by its directory/main under /usr/share/doc/texlive, the CI image;
# else amsldoc)
rm -rf $w/e && mkdir -p $w/e
if [ "${DOC:-}" = grail ]; then cp -a $w/bin/grail-src/. $w/e/ && rm -f $w/e/grail.pdf; main=grail.tex
elif [ -n "${DOC:-}" ]; then cp -a /usr/share/doc/texlive/$(dirname $DOC)/. $w/e/ && rm -f $w/e/*.pdf; main=$(basename $DOC); else cp bench/amsldoc/amsldoc.tex $w/e/; main=amsldoc.tex; fi
(cd $w/e && PHITEX_CACHE_DIR=$w/cache $bin build -j1 $main > /dev/null 2>&1)
rm -f $w/e/${main%.tex}.pdf
python3 -I bench/cut1-paraedit.py $w/e/$main ${EDITS:-2} > $w/edits.json
cat > $w/cg.sh <<EOF
#!/bin/bash
exec valgrind --tool=callgrind --callgrind-out-file=$r/cg.%p.out ${CG_EDITS:+--instr-atstart=no} $( [ "${TOGGLE:-}" = all ] || echo "--toggle-collect=${TOGGLE:-*page_alone*}" ) $bin "\$@"
EOF
chmod +x $w/cg.sh
PHITEX_CACHE_DIR=$w/cache PHITEX_TIME_LIMIT_SECS=0 timeout 10000 python3 -I bench/stream-edits-tmp.py $w/cg.sh $w/e $main 1 $w/edits.json $r/e.json --view --write > $r/e.txt 2>&1
cat $r/e.txt | tail -5
f=$(ls -S $r/cg.*.out | head -1); callgrind_annotate --inclusive=yes $f 2>/dev/null | head -70 | cut -c1-220
echo "== exclusive"
callgrind_annotate $f 2>/dev/null | head -45 | cut -c1-220
