#!/bin/bash
# pages2-tmp.sh [TEST...]: in an accl `cmd` job (kept workspace): the
# pages2 structural tests (crates/engines/tex/tests/pages2.rs) built with
# profile+observe and run one process each, 600 s each; then the suites
# named in SUITES (env) the same way, whole binaries.
set -u
cd "$w/phitex"
export CARGO_BUILD_JOBS=12 PHITEX_CACHE_DIR=$w/cache
s0=$(date +%s)
T="cargo test --profile fastdev -p phitex-tex --features profile,observe"
args=(--test pages2)
for s in ${SUITES:-}; do args+=(--test "$s"); done
$T "${args[@]}" --no-run > "$r/build.txt" 2>&1 || { echo "BUILD FAILED"; grep -m5 -A8 '^error' "$r/build.txt"; exit 1; }
echo "built in $(( $(date +%s) - s0 )) s at $(git rev-parse --short=9 HEAD)"
tests=("$@")
[ ${#tests[@]} -gt 0 ] || tests=(one_loop_node_and_one_finisher_per_page the_loop_parks_at_holes_and_resumes a_word_edit_reruns_the_piece_its_finisher_and_the_writers a_paragraph_insert_refires_the_pages_after_it_once page_dimensions_read_in_text_are_exact_and_fine page_fixtures_are_byte_identical)
for t in "${tests[@]}"; do
  s0=$(date +%s.%N)
  timeout 600 $T --test pages2 -- --exact "$t" --nocapture > "$r/pages2-$t.txt" 2>&1
  e=$?
  echo "pages2 $t: exit $e $(awk -v a="$s0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.1f", b-a}') s"
  grep -m3 -E 'panicked at|assertion|left:|right:|DVI differs' "$r/pages2-$t.txt" | cut -c1-400
done
for s in ${SUITES:-}; do
  timeout 3000 $T --test "$s" > "$r/suite-$s.txt" 2>&1
  echo "suite $s: exit $? $(grep -m1 '^test result' "$r/suite-$s.txt")"
  grep -E '^test .* FAILED' "$r/suite-$s.txt" | head -20
done
