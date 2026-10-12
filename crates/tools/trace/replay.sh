#!/usr/bin/env bash
# Records (unless already recorded) and converts the replay corpus: the
# LaTeX fixtures (docs/ and pdfdocs/) through pdflatex, the plain fixture
# bodies through pdftex -ini with plain.tex, to format v1 (the replay
# client's, crates/vendor/defuse/examples/replay.rs) under
# target/corpus/replay/<doc>/{trace.jsonl,edits.jsonl,edits.meta.jsonl,meta.json}
# and target/corpus/replay/INDEX.md. See REPLAY.md. One pdftex at a time;
# the pgf manual is not recorded (its log runs to many GB).
#
#   PHITEX_TRACE_COMMIT=$(git rev-parse HEAD) scripts/sandbox crates/tools/trace/replay.sh [DOC...]
#
# DOC names one of the documents below (latex-ams, pdf-tikz, plain-story,
# ...); none: all of them. Traces are kept under target/corpus/traces
# (re-recorded only when missing, or with PHITEX_RERECORD=1).
set -euo pipefail
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
traces="$repo/target/corpus/traces"
out="$repo/target/corpus/replay"
mkdir -p "$traces" "$out"
cargo build -q -p phitex-trace
bin="$repo/target/debug/phitex-trace"
commit="${PHITEX_TRACE_COMMIT:-unknown}"

docs=()
for f in "$repo"/oracle/fixtures/latex/docs/*.tex; do docs+=("latex-$(basename "${f%.tex}")|pdflatex|$f"); done
for f in "$repo"/oracle/fixtures/latex/pdfdocs/*.tex; do docs+=("pdf-$(basename "${f%.tex}")|pdflatex|$f"); done
for f in "$repo"/oracle/fixtures/plain/*.tex; do docs+=("plain-$(basename "${f%.tex}")|pdftex|$f"); done

want=" $* "
for d in "${docs[@]}"; do
  IFS='|' read -r name engine main <<<"$d"
  [ $# -eq 0 ] || [[ $want == *" $name "* ]] || continue
  trace="$traces/$name.jsonl"
  scratch="$traces/scratch-$name"
  if [ -n "${PHITEX_RERECORD:-}" ] || [ ! -s "$trace" ]; then
    src="$main"
    rm -rf "$scratch"
    echo "record $name" >&2
    "$bin" record "$src" --engine "$engine" --out "$trace" --scratch "$scratch" 2>>"$traces/record.log" || {
      echo "record $name failed (see $traces/record.log)" >&2
      continue
    }
  fi
  echo "replay $name" >&2
  "$bin" replay "$trace" --out-dir "$out/$name" --doc "$name" \
    --dirs "$(dirname "$main"):$scratch" --commit "$commit" 2>>"$out/replay.log" || {
    echo "replay $name failed (see $out/replay.log)" >&2
    continue
  }
  "$bin" validate "$out/$name/trace.jsonl" "$out/$name/edits.jsonl" 2>>"$out/replay.log"
done
"$bin" index "$out" >"$out/INDEX.md"
cat "$out/INDEX.md"
