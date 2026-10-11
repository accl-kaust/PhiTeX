#!/usr/bin/env bash
# Records (unless already recorded) and converts the replay corpus: the
# LaTeX fixtures (docs/ and pdfdocs/) through pdflatex, the plain fixture
# bodies through pdftex -ini with plain.tex, and the pgf manual from
# upstream/, to target/corpus/replay/<doc>/{trace.jsonl,edits.jsonl,meta.json}
# and target/corpus/replay/INDEX.md. See REPLAY.md.
#
#   PHITEX_TRACE_COMMIT=$(git rev-parse HEAD) scripts/sandbox crates/tools/trace/replay.sh [DOC...]
#
# DOC names one of the documents below (latex-ams, pdf-tikz, plain-story,
# pgfmanual, ...); none: all of them. Traces are kept under
# target/corpus/traces (re-recorded only when missing, or with PHITEX_RERECORD=1).
# PHITEX_PGF_CHAPTERS=N records only the first N \include'd chapters of the pgf
# manual (see REPLAY.md).
set -euo pipefail
repo="$(cd "$(dirname "$0")/../../.." && pwd)"
traces="$repo/target/corpus/traces"
out="$repo/target/corpus/replay"
mkdir -p "$traces" "$out"
cargo build -q --profile fastdev -p phitex-trace
bin="$repo/target/fastdev/phitex-trace"
commit="${PHITEX_TRACE_COMMIT:-unknown}"

docs=()
for f in "$repo"/oracle/fixtures/latex/docs/*.tex; do docs+=("latex-$(basename "${f%.tex}")|pdflatex|$f"); done
for f in "$repo"/oracle/fixtures/latex/pdfdocs/*.tex; do docs+=("pdf-$(basename "${f%.tex}")|pdflatex|$f"); done
for f in "$repo"/oracle/fixtures/plain/*.tex; do docs+=("plain-$(basename "${f%.tex}")|pdftex|$f"); done
pgf="$repo/upstream/pgf/doc/generic/pgf/pgfmanual.tex"
[ -f "$pgf" ] && docs+=("pgfmanual|pdflatex|$pgf")

want=" $* "
for d in "${docs[@]}"; do
  IFS='|' read -r name engine main <<<"$d"
  [ $# -eq 0 ] || [[ $want == *" $name "* ]] || continue
  trace="$traces/$name.jsonl"
  scratch="$traces/scratch-$name"
  if [ -n "${PHITEX_RERECORD:-}" ] || [ ! -s "$trace" ]; then
    src="$main"
    if [ "$name" = pgfmanual ] && [ -n "${PHITEX_PGF_CHAPTERS:-}" ]; then
      # The manual's text, its first PHITEX_PGF_CHAPTERS \include's only: a copy
      # of the main body beside the original files.
      dir="$(dirname "$main")"
      body="$dir/pgfmanual-en-main-body.tex"
      trimmed="$dir/pgfmanual-en-main-body-trimmed.tex"
      awk -v n="$PHITEX_PGF_CHAPTERS" '/^[[:space:]]*\\include\{/ { k++; if (k > n) next } { print }' "$body" >"$trimmed"
      sed 's/pgfmanual-en-main-body\.tex/pgfmanual-en-main-body-trimmed.tex/' "$dir/pgfmanual-en-main.tex" >"$dir/pgfmanual-en-main-trimmed.tex"
      sed 's/pgfmanual-en-main.tex/pgfmanual-en-main-trimmed.tex/' "$main" >"$dir/pgfmanual.trimmed.tex"
      src="$dir/pgfmanual.trimmed.tex"
    fi
    rm -rf "$scratch"
    echo "record $name" >&2
    "$bin" record "$src" --engine "$engine" --out "$trace" --scratch "$scratch" 2>>"$traces/record.log" || {
      echo "record $name failed (see $traces/record.log)" >&2
      continue
    }
    # (the manual's log runs to many GB: the trace is what is kept)
    if [ "$name" = pgfmanual ]; then rm -f "$scratch"/*.log; fi
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
