#!/usr/bin/env bash
# Opt-in check (needs TeX Live): N random old/new pairs made from the
# corpus (examples/fuzz.rs), each diffed; every pair whose new version
# compiles with pdflatex must give a diff that compiles too.
#
#   crates/phitex-diff/tests/fuzz.sh [N] [SEED]
#
# Pairs in target/phitex-diff-fuzz, compiled in /tmp/tex/phitex-diff-fuzz.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
n=${1:-100}
seed=${2:-1}
pairs="$repo/target/phitex-diff-fuzz"
out=/tmp/tex/phitex-diff-fuzz
rm -rf "$pairs" "$out"
mkdir -p "$out"

(cd "$repo" && scripts/sandbox cargo run -q -p phitex-diff --example fuzz -- \
  crates/phitex-diff/tests/corpus "target/phitex-diff-fuzz" "$n" "$seed") || exit 1

run() { (cd "$1" && nice -n 19 ionice -c3 pdflatex -interaction=nonstopmode -halt-on-error "$2" >/dev/null 2>&1); }

valid=0; ok=0; both=0; both_ok=0; failed=()
for d in "$pairs"/*/; do
  k=$(basename "$d")
  cp -r "$d" "$out/$k"
  # (the new version alone must compile for the pair to count)
  run "$out/$k/new" main.tex || continue
  valid=$((valid + 1))
  # (pairs whose old version compiles too: a real history's)
  old_ok=0
  run "$out/$k/old" main.tex && old_ok=1
  both=$((both + old_ok))
  cp "$out/$k/diff.tex" "$out/$k/new/diff.tex"
  if run "$out/$k/new" diff.tex && ! grep -q '^!' "$out/$k/new/diff.log"; then
    ok=$((ok + 1))
    both_ok=$((both_ok + old_ok))
  else
    failed+=("$k$([ $old_ok = 1 ] || echo '(old broken)')")
  fi
done
echo "fuzz: $ok/$valid diffs compile where the new version does; $both_ok/$both where both versions do (of $n pairs)"
[ ${#failed[@]} -eq 0 ] || echo "failed: ${failed[*]} (in $out)"
[ ${#failed[@]} -eq 0 ]
