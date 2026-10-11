#!/usr/bin/env bash
# Records and analyzes the corpus (the LaTeX fixture documents through
# pdflatex, the plain fixture bodies through pdftex -ini with plain.tex),
# default and --conservative, and prints the per-document table.
#
#   scripts/sandbox crates/tools/trace/corpus.sh [OUTDIR]
#
# (Inside the sandbox: every cargo command and every pdftex run goes
# through scripts/sandbox.) OUTDIR defaults to target/trace-corpus.
# REUSE_LOGS=1 re-parses the logs of an earlier run instead of running
# the engines again.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
out="${1:-$repo/target/trace-corpus}"
mkdir -p "$out"
cargo build -q --profile fastdev -p phitex-trace
bin="$repo/target/fastdev/phitex-trace"
run() { # name engine main
  local name=$1 engine=$2 main=$3
  local log="$out/scratch-$name/$(basename "${main%.tex}").log"
  if [ -z "${REUSE_LOGS:-}" ] || [ ! -f "$log" ]; then
    "$bin" record "$main" --engine "$engine" --out "$out/$name.jsonl" --scratch "$out/scratch-$name" 2>>"$out/record.log"
  else
    "$bin" parse "$log" --main "./$(basename "$main")" --engine "$engine" --out "$out/$name.jsonl" 2>>"$out/record.log"
  fi
  "$bin" analyze "$out/$name.jsonl" --json "$out/$name.json" >"$out/$name.md"
  "$bin" parse "$log" --main "./$(basename "$main")" --engine "$engine" --conservative --out "$out/$name.cons.jsonl" 2>>"$out/record.log"
  "$bin" analyze "$out/$name.cons.jsonl" --json "$out/$name.cons.json" >"$out/$name.cons.md"
  rm -f "$out/$name.jsonl" "$out/$name.cons.jsonl"
}
for f in "$repo"/oracle/fixtures/latex/docs/*.tex; do
  run "latex-$(basename "${f%.tex}")" pdflatex "$f"
done
for f in "$repo"/oracle/fixtures/plain/*.tex; do
  run "plain-$(basename "${f%.tex}")" pdftex "$f"
done
echo "## default"
plain=()
for f in "$out"/latex-*.json "$out"/plain-*.json; do
  case $f in *.cons.json) ;; *) plain+=("$f") ;; esac
done
"$bin" table "${plain[@]}"
echo
echo "## --conservative"
"$bin" table "$out"/*.cons.json
