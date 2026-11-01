#!/bin/bash
# Block variant diagnostics (on accl, in a job): observe build; amsldoc -j 1 with the scan and
# block traces and the page-50 defect detail; the vsize test and pdf_documents with traces.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1
cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A5 "$r/build.txt" | head -40 | tee -a "$r/summary.txt"; exit 1; }
mkdir -p "$w/bin/o"; cp target/release/phitex "$w/bin/o/phitex"
rm -rf "$w/a" && cp -a bench/amsldoc "$w/a"
( cd "$w/a" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache PHITEX_SCAN_TRACE=1 PHITEX_BLOCK_TRACE=1 PHITEX_DEFECT_PAGE=50 PHITEX_DEFECT_DUMP=1 timeout 1500 "$w/bin/o/phitex" build --exact-pdf -j 1 amsldoc.tex > "$r/ams-j1.log" 2>&1 )
echo "ams j1 exit $? md5 $(md5sum < "$w/a/amsldoc.pdf" | cut -c1-32) pages $(grep -c . /dev/null)" | tee -a "$r/summary.txt"
grep -n "defect\|Extra\|^! " "$r/ams-j1.log" | head -40 | cut -c1-400 | tee -a "$r/summary.txt"
echo "ams j1 exit $? md5 $(md5sum < "$w/a/amsldoc.pdf" | cut -c1-32)" | tee -a "$r/summary.txt"
gzip -1 "$r/ams-j1.log"
echo done
