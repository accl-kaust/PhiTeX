# Agent Q's pdfTeX job (accl submit cmd bash scripts/accl/q-q3.sh): the
# oracles blessed, the crate's tests, clippy; each step under a time limit
# (scripts/flow-gate.sh's way), so a hang fails its step, names it, and
# the job goes on. Results only into "$r"; summary.txt is the digest.
set -uo pipefail
T=crates/phitex-flow/tests
S="$r/summary.txt"
: > "$S"
step() {
  local lim=$1 name=$2; shift 2
  timeout -k 30 "$lim" "$@" > "$r/$name.txt" 2>&1; local s=$?
  echo "== $name exit $s$([ $s -eq 124 ] && echo " (timed out at $lim)")" >> "$S"
  grep -E '^test result|\.\.\. FAILED|panicked at|^error' "$r/$name.txt" | grep -v ' 0 failed' | sort | uniq -c | head -40 >> "$S"
}
(pdftex --version | head -2; pdflatex --version | head -1) > "$r/versions.txt" 2>&1
step 40m build scripts/sandbox cargo test -p phitex-flow --release --no-run
# (the oracles: pdftex and pdflatex on the test documents, every one tried)
step 10m bless-pdf env PHITEX_FLOW_BLESS=1 scripts/sandbox cargo test -p phitex-flow --release --test pdf oracles
step 30m bless-latex env PHITEX_FLOW_BLESS=1 scripts/sandbox cargo test -p phitex-flow --release --test pdflatex_docs oracles
mkdir -p "$r/bless/pdf" "$r/bless/latexpdf" "$r/logs"
cp $T/pdf/*.pdf "$r/bless/pdf/"; cp $T/pdf/*.pos "$r/bless/pdf/" 2>/dev/null
cp $T/latex/pdf/* "$r/bless/latexpdf/"
w=$(mktemp -d)
for n in literal colorstack obj trailerid xform annot outline ximage ximage2 random misc savepos rerun readback thread pageref vf interword; do
  cp $T/pdf/$n.tex $w/; (cd $w && TEXINPUTS=".:$(dirname "$(kpsewhich example-image.png)"):" SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 timeout 60 pdftex -ini -etex -interaction=batchmode $n.tex >/dev/null 2>&1); cp $w/$n.log "$r/logs/" 2>/dev/null
  echo "$n tex errors: $(grep -c '^!' $w/$n.log)" >> "$S"
done
for n in hyper graphics color tikz zsavepos objs vfont; do
  cp $T/latex/pdfdocs/$n.tex $w/; (cd $w && SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 timeout 300 pdflatex -interaction=batchmode $n.tex >/dev/null 2>&1); cp $w/$n.log "$r/logs/" 2>/dev/null
  echo "$n tex errors: $(grep -c '^!' $w/$n.log)" >> "$S"
done
# (the library's tests, tag_tests among them; each test binary; the LaTeX
# document binaries one test at a time: they build the format per test)
export PHITEX_FLOW_PDF_OUT="$r/ours"
mkdir -p "$r/ours"
step 10m lib scripts/sandbox cargo test -p phitex-flow --release --lib
for f in $T/*.rs; do
  t=$(basename "$f" .rs)
  case $t in
    latex_docs|pdflatex_docs)
      for n in $(scripts/sandbox cargo test -p phitex-flow --release --test "$t" -- --list 2>/dev/null | sed -n 's/: test$//p'); do
        step 30m "$t-$n" scripts/sandbox cargo test -p phitex-flow --release --test "$t" -- --exact "$n" --test-threads=1
      done ;;
    *) step 20m "$t" scripts/sandbox cargo test -p phitex-flow --release --test "$t" -- --skip latex_edits_equal_cold_and_latex --test-threads=2 ;;
  esac
done
step 30m clippy scripts/sandbox cargo clippy -p phitex-flow --all-targets -- -D warnings
echo "clippy diagnostics: $(grep -c '^warning\|^error' "$r/clippy.txt")" >> "$S"
cat "$r/versions.txt" >> "$S"
