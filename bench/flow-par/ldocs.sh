#!/bin/bash
# bench/flow-par/ldocs.sh: latex_docs on this head, one test at a time, every
# failure's text kept
set -u
R=${r:-/tmp/ldocs-results}; mkdir -p "$R"
timeout 4200 cargo test --release -p phitex-flow --test latex_docs -- --test-threads=1 > "$R/latex_docs.log" 2>&1
echo "latex_docs status $? $(grep -E '^test result' $R/latex_docs.log | tail -1)" | tee "$R/summary.txt"
grep -E '^test .* (FAILED|ok|ignored)' "$R/latex_docs.log" | tee -a "$R/summary.txt"
grep -E 'panicked' -A6 "$R/latex_docs.log" | head -40 | tee -a "$R/summary.txt"
