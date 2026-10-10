# Agent Q's short rerun (accl submit cmd bash scripts/accl/q-short.sh): the
# library's tests (tag_tests), tests/pdf.rs, one pdflatex document; each
# step under a time limit. Results only into "$r"; summary.txt the digest.
set -uo pipefail
S="$r/summary.txt"
: > "$S"
step() {
  local lim=$1 name=$2; shift 2
  timeout -k 30 "$lim" "$@" > "$r/$name.txt" 2>&1; local s=$?
  echo "== $name exit $s$([ $s -eq 124 ] && echo " (timed out at $lim)")" >> "$S"
  grep -E '^test result|\.\.\. FAILED|panicked at|^error' "$r/$name.txt" | grep -v ' 0 failed' | sort | uniq -c | head -40 >> "$S"
}
step 40m build scripts/sandbox cargo test -p phitex-flow --release --no-run
step 10m lib scripts/sandbox cargo test -p phitex-flow --release --lib
step 20m pdf scripts/sandbox cargo test -p phitex-flow --release --test pdf -- --test-threads=2
step 30m pdflatex-hyper scripts/sandbox cargo test -p phitex-flow --release --test pdflatex_docs -- --exact pdf_hyper --test-threads=1
