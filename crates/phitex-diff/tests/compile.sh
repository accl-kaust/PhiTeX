#!/usr/bin/env bash
# Opt-in check (needs TeX Live): for each pair in tests/corpus, mark up the
# diff with phitex-diff (in the sandbox) and with Perl latexdiff, compile
# both with stock pdflatex, and compare the text of the PDFs.
#
#   crates/phitex-diff/tests/compile.sh [CASE...]
#
# Output in /tmp/tex/phitex-diff/<case>/{ours,latexdiff}/; a summary on
# stdout. pdflatex and latexdiff are the system's (trusted, on our own
# synthetic inputs), run under nice.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
corpus="$here/corpus"
out=/tmp/tex/phitex-diff
mkdir -p "$out"

(cd "$repo" && scripts/sandbox cargo build -q -p phitex-diff --example difftex) || exit 1
bin="$repo/target/debug/examples/difftex"

cases=("$@")
[ ${#cases[@]} -gt 0 ] || cases=($(ls "$corpus"))

nice_run() { nice -n 19 ionice -c3 "$@"; }

# pdflatex twice (references), nonstop: 0 if $dir/$2.pdf came out
# without errors
compile() {
  local dir=$1 job=${2:-diff}
  (cd "$dir" && for _ in 1 2; do
    nice_run pdflatex -interaction=nonstopmode -halt-on-error "$job.tex" >/dev/null 2>&1 || return 1
  done)
  [ -f "$dir/$job.pdf" ] && ! grep -q '^!' "$dir/$job.log"
}

# The PDF's words with the markup as text: [+added+] [-deleted-] (to
# compare what each diff marks, not how it is drawn)
words() {
  local dir=$1
  sed 's/^\\begin{document}/\\renewcommand{\\DIFadd}[1]{[+#1+]}\\renewcommand{\\DIFdel}[1]{[-#1-]}\n&/' \
    "$dir/diff.tex" > "$dir/marked.tex"
  compile "$dir" marked || return 1
  pdftotext "$dir/marked.pdf" - | tr -s '[:space:]' '\n'
}

ours_ok=0; theirs_ok=0; total=0
printf '%-14s %-6s %-9s %s\n' case ours latexdiff "text (ours vs latexdiff)"
for c in "${cases[@]}"; do
  total=$((total + 1))
  o="$out/$c"
  rm -rf "$o"; mkdir -p "$o/ours" "$o/latexdiff" "$o/oldtree"
  cp -r "$corpus/$c/new/." "$o/ours/"
  cp -r "$corpus/$c/new/." "$o/latexdiff/"
  if [ -d "$corpus/$c/old" ]; then
    cp -r "$corpus/$c/old/." "$o/oldtree/"
  else
    printf '\\documentclass{article}\n\\begin{document}\n\\end{document}\n' > "$o/oldtree/main.tex"
  fi
  # (ours: the sandbox sees the repository only; copy the result out)
  (cd "$repo" && scripts/sandbox "$bin" "$corpus/$c/old" "$corpus/$c/new" main.tex --changes) \
    > "$o/ours/diff.tex" 2> "$o/changes.txt"
  # (latexdiff: --flatten reads the inputs relative to each main file)
  (cd "$o/latexdiff" && nice_run latexdiff --math-markup=coarse --flatten \
     "$o/oldtree/main.tex" main.tex > diff.tex 2> latexdiff.err)
  r1=fail; r2=fail
  compile "$o/ours" && { r1=ok; ours_ok=$((ours_ok + 1)); }
  compile "$o/latexdiff" && { r2=ok; theirs_ok=$((theirs_ok + 1)); }
  cmp=-
  if [ $r1 = ok ] && [ $r2 = ok ]; then
    words "$o/ours" > "$o/ours.words"
    words "$o/latexdiff" > "$o/latexdiff.words"
    diff "$o/ours.words" "$o/latexdiff.words" > "$o/words.diff"
    if cmp -s "$o/ours.words" "$o/latexdiff.words"; then
      cmp=same
    else
      cmp="differs ($(diff "$o/ours.words" "$o/latexdiff.words" | grep -c '^[<>]') words)"
    fi
  fi
  printf '%-14s %-6s %-9s %s\n' "$c" "$r1" "$r2" "$cmp"
done
echo "compiled: ours $ours_ok/$total, latexdiff $theirs_ok/$total (details in $out)"
[ "$ours_ok" = "$total" ]
