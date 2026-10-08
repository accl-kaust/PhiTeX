#!/bin/bash
# The pure SSA tracer's measurements (DESIGN 3.17, "Measured"), a job.sbatch
# `cmd` task run from the checkout ($w the workspace, $r the results, the
# course in $w/course with its aux in $w/aux):
#
#   pure-stats.sh [DOC...]      (default: course pgf tikz)
#
# For each document, plain builds under PARTEX_PURE (no output changes):
# value-numbered (the model), strict (no value numbering, a footnote), the
# effect chains and the interline glue split, and the field projections
# (allocators' ordinals, hook appends, catcodes, all three); then two edits
# as traces diffed (scripts/pure-diff.py): a word (the course's `word` of
# bench/edits/par.txt; pgfsub's first of bench/edits/pgfsub.txt) and a
# \pageref (or \ref) value changed in the .aux, keeping its width; and the
# same two edits as rebuilds of current SSA mode (PARTEX_SSA=1, one trip),
# whose steps and commands run are the baseline. Reports in $r/<doc>/,
# the summary in $r/summary.txt.
set -uo pipefail
P=$PWD
cargo build --release -p partex-cli -q || exit 1
B=$P/target/release/phitex
docs=("$@")
[ ${#docs[@]} -eq 0 ] && docs=(course pgf tikz)
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1
F=$w/fmt
mkdir -p "$F"
(cd "$F" && "$B" --compat=pdftex -ini -interaction=batchmode -jobname=pdflatex \
  -translate-file=cp227.tcx '*pdflatex.ini' </dev/null >/dev/null 2>&1)
[ -f "$F/pdflatex.fmt" ] || { echo "FORMAT FAILED" | tee -a "$r/summary.txt"; exit 1; }

# DOC's sources in $w/src-DOC, settled, with MAIN its main file
setup() {
  local doc=$1 d=$w/src-$1
  case $doc in
    course) cp -a "$w/course" "$d"; cp -a "$w/aux/." "$d/"; MAIN=course.tex ;;
    pgf) cp -a upstream/pgf/doc/generic/pgf "$d"; cp bench/inputs/pgfsub.tex "$d/"; MAIN=pgfsub.tex ;;
    tikz) mkdir -p "$d"; cp tests/e2e/petals.tex "$d/"; MAIN=petals.tex ;;
  esac
  if [ "$doc" != course ]; then
    for i in 1 2 3; do
      (cd "$d" && "$B" --compat=pdftex -fmt="$F/pdflatex" -interaction=batchmode "$MAIN" \
        </dev/null >/dev/null 2>&1)
    done
  fi
}

# trace DIR OUT [ENV...]: one tracer build of DIR's copy (the sources
# untouched: a copy per run)
trace() {
  local src=$1 out=$2
  shift 2
  local d
  d=$(mktemp -d "$w/run.XXXXXX")
  cp -a "$src/." "$d/"
  (cd "$d" && env "$@" PARTEX_PURE="$out.json" "$B" --compat=pdftex -fmt="$F/pdflatex" \
    -interaction=batchmode "$MAIN" </dev/null >/dev/null 2>"$out.err")
  rm -rf "$d"
}

# edit_word DIR: the word edit in DIR's sources
edit_word() {
  local d=$1 spec
  if [ "$doc" = course ]; then
    spec=$(grep -P '^word\t' bench/edits/par.txt | cut -f2)
  elif [ "$doc" = pgf ]; then
    spec=$(grep -vP '^#' bench/edits/pgfsub.txt | head -1 | cut -f2)
  else
    spec='petals.tex|\begin{document}|\begin{document}Petals'
  fi
  python3 - "$d" "$spec" <<'EOF'
import sys, os
d, spec = sys.argv[1], sys.argv[2]
path, old, new = spec.split("|")[:3]
old, new = old.replace("<NL>", "\n"), new.replace("<NL>", "\n")
p = os.path.join(d, path)
s = open(p, encoding="latin-1").read()
assert old in s, (p, old)
open(p, "w", encoding="latin-1").write(s.replace(old, new, 1))
EOF
}

# edit_ref DIR: a \pageref's (else a \ref's) value in DIR's .aux changed,
# its width kept (a digit changed); prints the key
edit_ref() {
  python3 - "$1" "${MAIN%.tex}.aux" <<'EOF'
import sys, os, re
d, aux = sys.argv[1], sys.argv[2]
src = ""
for root, _, files in os.walk(d):
    for f in files:
        if f.endswith(".tex"):
            src += open(os.path.join(root, f), encoding="latin-1").read()
p = os.path.join(d, aux)
a = open(p, encoding="latin-1").read()
labels = {m.group(1): m for m in re.finditer(r"\\newlabel\{([^}]*)\}\{\{([^}]*)\}\{(\d+)\}", a)}
for kind, field in (("pageref", 3), ("ref", 2)):
    for k in re.findall(r"\\" + kind + r"\{([^}]*)\}", src):
        m = labels.get(k)
        if not m:
            continue
        v = m.group(field)
        if not re.fullmatch(r"\d+", v) and not re.search(r"\d$", v):
            continue
        last = int(v[-1])
        nv = v[:-1] + str(last + 1 if last < 9 else last - 1)
        s, e = m.span(field)
        open(p, "w", encoding="latin-1").write(a[:s] + nv + a[e:])
        print(f"{kind}{{{k}}}: {v} -> {nv}")
        sys.exit(0)
print("no reference found")
sys.exit(1)
EOF
}

# ssa_rebuild DIR EDITFN OUT: current SSA mode, a cold build and one
# rebuild after the edit, one trip each
ssa_rebuild() {
  local src=$1 fn=$2 out=$3 d
  d=$(mktemp -d "$w/ssa.XXXXXX")
  cp -a "$src/." "$d/"
  mkdir -p "$d.edited" && cp -a "$src/." "$d.edited/"
  (doc=$doc MAIN=$MAIN; $fn "$d.edited" >"$out.edit")
  (cd "$d" && env PARTEX_SSA=1 PARTEX_SSA_TRIPS=1 \
    PARTEX_SSA_REBUILD="cp -a $d.edited/. $d/" \
    "$B" --compat=pdftex -fmt="$F/pdflatex" -interaction=batchmode "$MAIN" \
    </dev/null >/dev/null 2>"$out.err")
  grep -a 'ssa rebuild 1:.*steps run' "$out.err" | head -1 >"$out.txt"
  rm -rf "$d" "$d.edited"
}

for doc in "${docs[@]}"; do
  o=$r/$doc
  mkdir -p "$o"
  setup "$doc"
  S=$w/src-$doc
  echo "== $doc ($MAIN)" | tee -a "$r/summary.txt"
  # the model, strict, split; names for the projections (DIFFS_ONLY=1:
  # only the edits)
  if [ -z "${DIFFS_ONLY:-}" ]; then
  trace "$S" "$o/vn" PARTEX_PURE_VN=1 PARTEX_PURE_NAMES="$o/names.tsv" &
  trace "$S" "$o/strict" &
  trace "$S" "$o/split" PARTEX_PURE_VN=1 PARTEX_PURE_SPLIT=1 &
  trace "$S" "$o/spec" PARTEX_PURE_VN=1 PARTEX_PURE_SPLIT=1 PARTEX_PURE_SPEC_TAIL=1 &
  wait
  for c in alloc hooks catcodes all; do
    python3 scripts/pure-fields.py "$o/names.tsv" $c >"$o/fields-$c.txt"
    trace "$S" "$o/proj-$c" PARTEX_PURE_VN=1 PARTEX_PURE_SPLIT=1 \
      PARTEX_PURE_FIELDS="$o/fields-$c.txt" &
  done
  wait
  fi
  # the edits, traced
  E1=$w/edit-word-$doc E2=$w/edit-ref-$doc
  cp -a "$S" "$E1" && edit_word "$E1"
  cp -a "$S" "$E2" && edit_ref "$E2" >"$o/ref-edit.txt"
  trace "$S" "$o/t0" PARTEX_PURE_VN=1 PARTEX_PURE_SPLIT=1 PARTEX_PURE_TRACE="$w/t0-$doc" &
  trace "$E1" "$o/t1" PARTEX_PURE_VN=1 PARTEX_PURE_SPLIT=1 PARTEX_PURE_TRACE="$w/t1-$doc" &
  trace "$E2" "$o/t2" PARTEX_PURE_VN=1 PARTEX_PURE_SPLIT=1 PARTEX_PURE_TRACE="$w/t2-$doc" &
  # current SSA mode on the same edits
  ssa_rebuild "$S" edit_word "$o/ssa-word" &
  ssa_rebuild "$S" edit_ref "$o/ssa-ref" &
  wait
  python3 scripts/pure-diff.py "$w/t0-$doc" "$w/t1-$doc" >"$o/diff-word.txt" 2>&1
  python3 scripts/pure-diff.py "$w/t0-$doc" "$w/t2-$doc" >"$o/diff-ref.txt" 2>&1
  rm -f "$w"/t[012]-"$doc".*
  [ -z "${DIFFS_ONLY:-}" ] && python3 scripts/pure-report.py "$o"/vn.json "$o"/strict.json "$o"/split.json "$o"/spec.json \
    "$o"/proj-*.json >"$o/report.txt" 2>&1
  {
    grep -h '^==\|^critical\|^setup\|^the body\|^page chain\|^body nodes\|^memory' "$o/report.txt"
    echo "-- word edit (pure):"; grep -h '^re-evaluated nodes' "$o/diff-word.txt"
    echo "-- word edit (current SSA):"; cat "$o/ssa-word.txt"
    echo "-- reference edit $(cat "$o/ref-edit.txt") (pure):"; grep -h '^re-evaluated nodes' "$o/diff-ref.txt"
    echo "-- reference edit (current SSA):"; cat "$o/ssa-ref.txt"
  } | tee -a "$r/summary.txt"
done
