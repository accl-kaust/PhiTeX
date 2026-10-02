#!/usr/bin/env bash
# bench/edits.sh DOCDIR EDITSFILE [BIN]: the canonical edits, measured.
#
# For each line `name<TAB>spec` of EDITSFILE (`#` comments and blank lines
# skipped), one process in DOCDIR builds the job in machine mode and then
# rebuilds it once after `spec`, applied in memory (PARTEX_MACHINE_EDIT,
# machinehost.rs). Every edit starts from the same unedited build, not from
# the previous edit, and is one pass: a `\label` is not followed by the
# second pass a watch makes. Only the rebuild is measured; the cold build
# before it (about 55 s on the course) is in the log.
#
# `spec` is `path|from|to` (several joined by `||`): in the input file whose
# path ends with `path`, the first `from` becomes `to`. `<NL>` in a spec is
# a newline. `path` is relative to DOCDIR, and each `from` must occur
# exactly once in that file (checked before the run).
#
# Typing: specs joined by `;;` are successive edits in the same process,
# each followed by its rebuild (as a watch session makes them, on the
# regions the previous rebuild recorded). Each `from` must occur once in
# the file as the edits before it left it. The row reports the last
# rebuild; with ALL=1 in the environment, one row per rebuild (NAME#1,
# NAME#2, ...).
#
# DOCDIR holds MAIN (default course.tex), `_out/` with the job's .aux, .out
# and .toc from a full build (copied into each run), and `_cache/formats-*/`
# with a native pdflatex format made by the binary. It must lie under the
# repository or ~/code/tmp, which scripts/sandbox binds. Logs go to
# DOCDIR/runs/bench-NAME/out/. PARTEX_* variables set by the caller pass
# through (e.g. PARTEX_MACHINE_RENAME=1). BIN defaults to
# target/release/partex, built in the sandbox if missing.
#
# Output: a Markdown table on stdout. Columns: the rebuild's wall time; the
# commands re-executed (`executed_cost`); the old regions re-executed over
# the regions before the edit; the new regions recorded (one cut each); the
# restores of old states (`restores`, `replay_ns`); the link.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
[ $# -ge 2 ] || { sed -n '2,3p' "$0" >&2; exit 2; }
doc=$(cd "$1" && pwd)
edits=$(realpath "$2")
bin=${3:-$repo/target/release/partex}
main=${MAIN:-course.tex}
job=${main%.tex}

case "$doc" in
  "$repo"/* | "$HOME"/code/tmp/*) ;;
  *) echo "edits.sh: $doc is not under $repo or ~/code/tmp (the sandbox binds only those)" >&2; exit 2 ;;
esac
if [ ! -x "$bin" ]; then
  echo "edits.sh: building $bin" >&2
  (cd "$repo" && scripts/sandbox cargo build --release -p partex-cli >&2)
fi
fmts=("$doc"/_cache/formats-*/pdflatex.fmt)
[ -f "${fmts[0]}" ] || { echo "edits.sh: no _cache/formats-*/pdflatex.fmt in $doc" >&2; exit 2; }
fmt=${fmts[0]%.fmt}
[ -f "$doc/_out/$job.aux" ] || { echo "edits.sh: no _out/$job.aux in $doc" >&2; exit 2; }

# The number after `name: ` in the Stats line $2 (the first such field).
field() { grep -oE "[{ ]$1: [0-9]+" <<<"$2" | head -1 | grep -oE '[0-9]+$'; }

# Each `from` of spec $1 occurs once in its file, as the earlier edits of
# a `;;` sequence left it.
check() {
  python3 - "$doc" "$1" <<'EOF'
import os, sys
doc, spec = sys.argv[1], sys.argv[2]
texts = {}
for step in spec.split(";;"):
    for edit in step.split("||"):
        path, frm, to = edit.split("|", 2)
        f = os.path.join(doc, path)
        if not os.path.isfile(f):
            sys.exit(f"edits.sh: no {f} (a spec's path is relative to DOCDIR here)")
        if f not in texts:
            texts[f] = open(f, encoding="utf-8", errors="surrogateescape").read()
        n = texts[f].count(frm)
        if n != 1:
            sys.exit(f"edits.sh: `{frm}` occurs {n} times in {f}")
        texts[f] = texts[f].replace(frm, to, 1)
EOF
}

printf '| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |\n'
printf '|---|---:|---:|---:|---:|---:|---:|\n'
while IFS=$'\t' read -r name spec || [ -n "$name" ]; do
  [[ -z $name || $name == \#* ]] && continue
  spec=${spec//<NL>/$'\n'}
  check "$spec"
  out=$doc/runs/bench-$name/out
  rm -rf "$doc/runs/bench-$name"
  mkdir -p "$out"
  for ext in aux out toc lof lot; do
    if [ -f "$doc/_out/$job.$ext" ]; then cp "$doc/_out/$job.$ext" "$out/"; fi
  done
  # (the spec inside the sandbox's `env`: the sandbox passes PARTEX_*
  # variables through line by line, which would cut one with a newline)
  status=0
  (cd "$doc" && "$repo"/scripts/sandbox env PARTEX_MACHINE=1 PARTEX_MACHINE_EDIT="$spec" \
    SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 \
    "$bin" --compat=pdftex -fmt="$fmt" -interaction=batchmode -output-directory="$out" "$main" \
    </dev/null >"$out/term.txt" 2>"$out/err.txt") || status=$?
  log=$out/err.txt
  mapfile -t rebuilds < <(grep 'machine: rebuilt in' "$log" || true)
  steps=$(( $( (grep -o ';;' <<<"$spec" || true) | wc -l) + 1 ))
  if [ "${#rebuilds[@]}" -lt "$steps" ]; then
    echo "| $name | failed (exit $status, see $log) | | | | | |"
    continue
  fi
  # (the regions before each rebuild: the cold build's, then those the
  # rebuild before it left)
  total=$(grep -m1 -oE '[0-9]+ now; indexed' "$log" | cut -d' ' -f1)
  # (each rebuild's link, in order: the in-process path links after each)
  mapfile -t links < <(grep -oE 'linked [0-9]+ regions in [0-9.]+ ms' "$log" | awk '{print $5}')
  for i in "${!rebuilds[@]}"; do
    rebuilt=${rebuilds[$i]}
    last=$(( i + 1 == ${#rebuilds[@]} ))
    if [ "${ALL:-0}" = 1 ] || [ "$last" = 1 ]; then
      ms=$(sed -E 's/.*rebuilt in ([0-9.]+) s.*/\1/' <<<"$rebuilt" | awk '{printf "%.0f", $1 * 1000}')
      replay=$(awk -v n="$(field replay_ns "$rebuilt")" 'BEGIN{printf "%.1f", n / 1e6}')
      label=$name
      if [ "${#rebuilds[@]}" -gt 1 ] && [ "${ALL:-0}" = 1 ]; then label="$name#$(( i + 1 ))"; fi
      shown=${links[$i]:-?}
      printf '| %s | %s | %s | %s / %s | %s | %s, %s | %s |\n' "$label" "$ms" \
        "$(field executed_cost "$rebuilt")" "$(field dirty_regions "$rebuilt")" "$total" \
        "$(field recorded_regions "$rebuilt")" "$(field restores "$rebuilt")" "$replay" "$shown"
    fi
    total=$(field regions "$rebuilt")
  done
done <"$edits"
rev=$(git -C "$repo" rev-parse --short HEAD)
git -C "$repo" diff --quiet HEAD || rev="$rev plus uncommitted changes"
printf '\n%s, %s, one in-process rebuild per edit after a cold build of %s; %s (repo at %s), %s.\n' \
  "$(basename "$edits")" "$(basename "$doc")" "$main" "$bin" "$rev" "$(date '+%Y-%m-%d %H:%M')"
