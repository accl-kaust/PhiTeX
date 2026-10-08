#!/usr/bin/env bash
# Opt-in check (needs TeX Live): every markup type and subtype, and the
# colors, compiled with stock pdflatex on a few corpus pairs (a table and a
# figure, sections, citations and references, display math, hyperref).
#
#   crates/phitex-diff/tests/styles.sh [CASE...]
#
# Output in /tmp/tex/phitex-diff-styles/<case>/<style>/; a line per case and
# style on stdout. pdflatex is the system's (trusted, on our own synthetic
# inputs), run under nice.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
corpus="$here/corpus"
out=/tmp/tex/phitex-diff-styles
mkdir -p "$out"

(cd "$repo" && scripts/sandbox cargo build -q -p phitex-diff --example difftex) || exit 1
bin="$repo/target/debug/examples/difftex"

cases=("$@")
[ ${#cases[@]} -gt 0 ] || cases=(figure_table sections cite_ref display_math hyperref)

# (each type with the default subtype, each subtype with the default type,
# and colors: a name, an expression, hex)
styles=()
for t in UNDERLINE CTRADITIONAL TRADITIONAL CFONT FONTSTRIKE CCHANGEBAR CFONTCHBAR CULINECHBAR CHANGEBAR INVISIBLE BOLD; do
  styles+=("--type=$t")
done
for s in COLOR MARGIN LABEL ZLABEL ONLYCHANGEDPAGE; do
  styles+=("--subtype=$s")
done
styles+=("--add-color=#008000 --del-color=orange"
  "--type=CFONT --subtype=COLOR --add-color=blue!60!black --del-color=#AA0000"
  "--type=CULINECHBAR --del-color=magenta"
  "--type=CTRADITIONAL --add-color=teal")

nice_run() { nice -n 19 ionice -c3 "$@"; }

ok=0; total=0
for c in "${cases[@]}"; do
  for st in "${styles[@]}"; do
    total=$((total + 1))
    name=$(printf '%s' "$st" | tr -c 'A-Za-z0-9=-' '_')
    o="$out/$c/$name"
    rm -rf "$o"; mkdir -p "$o"
    cp -r "$corpus/$c/new/." "$o/"
    # shellcheck disable=SC2086
    (cd "$repo" && scripts/sandbox "$bin" "$corpus/$c/old" "$corpus/$c/new" main.tex $st) > "$o/diff.tex" 2> "$o/difftex.err"
    r=fail
    if (cd "$o" && for _ in 1 2; do
          nice_run pdflatex -interaction=nonstopmode -halt-on-error diff.tex >/dev/null 2>&1 || exit 1
        done) && [ -f "$o/diff.pdf" ] && ! grep -q '^!' "$o/diff.log"; then
      r=ok; ok=$((ok + 1))
    fi
    printf '%-13s %-6s %s\n' "$c" "$r" "$st"
  done
done
echo "compiled: $ok/$total (details in $out)"
[ "$ok" = "$total" ]
