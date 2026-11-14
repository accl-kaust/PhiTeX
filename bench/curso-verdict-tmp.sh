set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
cargo build -q --release -p phitex-cli || exit 1
B=$PWD/target/release/phitex
w=$(mktemp -d)
cp -a /usr/share/doc/texlive/latex/cursolatex/. $w/e
echo "files:"; ls -R $w/e | head -60
rm -f $w/e/cursolatex.pdf
sed 's/if x.endswith((".pdf", ".aux", ".log", ".toc", ".out")):/if x.endswith((".aux", ".log", ".toc", ".out")) or x == os.path.basename(pdf):/' bench/stream-edits-tmp.py > $w/se.py
grep -n "os.path.basename(pdf):" $w/se.py
python3 -I bench/cut1-paraedit.py $w/e/cursolatex.tex 4 > $w/edits.json
cat $w/edits.json | head -c 600; echo
PHITEX_CACHE_DIR=$w/cache timeout 3000 python3 -I $w/se.py $B $w/e cursolatex.tex 1 $w/edits.json $w/r.json --view --write 2>&1 | tail -8
python3 -c "import json;r=json.load(open('$w/r.json'));print(r['watch_md5'],r['cold_md5'],r['verdict'])"
cmp $w/e/cursolatex.pdf $w/e-cold/cursolatex.pdf | head -3
ls -la $w/e/cursolatex.pdf $w/e-cold/cursolatex.pdf
