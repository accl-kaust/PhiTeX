#!/bin/bash
set -u
S=scripts/sandbox
out=$($S cargo test --profile fastdev -p phitex-tex --lib --no-run 2>&1); bin=$(echo "$out" | grep -o '([^)]*phitex_tex-[a-z0-9]*)' | tr -d '()' | head -1)
echo "bin $bin"
PHITEX_INCR_DOC=$PWD/scratch/para400.tex $S valgrind --tool=callgrind --callgrind-out-file=$PWD/scratch/cg.out $bin incr_cold_doc --ignored --nocapture --test-threads=1 2>&1 | grep -E "^incr|Collected"
$S callgrind_annotate --inclusive=yes --auto=no --threshold=100 scratch/cg.out 2>/dev/null > scratch/cg.txt
grep "PROGRAM TOTALS" scratch/cg.txt
for f in 'incr::Incr>::walk' 'hints::text_hints' 'hints::analyse$' 'Definitions>::harvest' 'pass::analyze_with' 'Analysis>::cats_of' 'hints::slices_of' 'Analysis>::hint_run$' 'Analysis>::hint$' 'Hint>::encode' 'hints::balanced' 'TextHints>::at_until' 'incr::defines' 'Analysis>::new'; do
  awk -v pat="$f" '{n=$1; gsub(",","",n); name=$0; sub(/^[^:]*:/,"",name); sub(/ \[.*$/,"",name); if (name ~ pat) t+=n} END {printf "%-28s %15d\n", pat, t}' scratch/cg.txt
done
