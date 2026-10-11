#!/usr/bin/env bash
# A ship candidate's CLI oracle gate, as one cluster job:
#
#   ACCL_COMMIT=REV ACCL_OPTS="-c 3 --mem=16G -t 00:50:00 --exclude=harrier" \
#     scripts/accl/accl submit cmd bash scripts/flow-gate-cli.sh [PART]
#
# PART (the argument, or the env) shards it into two jobs
# (scripts/flow-gate-submit.sh REV cli submits both): tikz (TikZ in
# cli.rs and tests/pdflatex_docs.rs, and synctex.rs) and rest (the
# others); unset, all of it in this job.
#
# The `phitex` binary on phitex-flow (crates/phitex-cli/tests/cli.rs and
# synctex.rs), one heavy run at a time:
# - `latex` on the ten LaTeX documents: the DVI and the written files are
#   TeX Live's `latex`'s (the recorded oracles, the date masked);
# - `pdflatex` on them, and on the documents of pdfTeX's PDF-only
#   commands where the tree has them (tests/latex/pdfdocs/: hyperref,
#   graphicx, xcolor's colour stack, TikZ, positions, objects, virtual
#   fonts): the PDF and the written files, byte for byte;
# - `pdftex -ini -etex` on the eight -ini documents and the ten plain
#   ones: the PDF, byte for byte;
# - SyncTeX (synctex.rs, TeX Live's `synctex` as the parser): the files
#   are written and asked the same questions as pdfTeX's; its tallies are
#   reported, not a bar.
# - phitex-flow's own pdfTeX corpora at its schedules (tests/pdf.rs, the
#   -ini documents of every pdfTeX primitive; tests/pdflatex_docs.rs):
#   the PDF byte for byte against the recorded pdftex/pdflatex oracles.
# lualatex_external and the fixpoint and watch tests are not this gate's.
# Sized from job 8045 on 37a9ffe3: 7.5 min, 1.5 cores, 7.1 GB (21 + 3
# tests passed, 53 .synctex.gz files). Results in $r
# (target/flow-gate-cli outside a job): cli.txt,
# synctex.txt, summary.txt.
set -uo pipefail
cd "$(dirname "$0")/.."
R=${r:-target/flow-gate-cli}
mkdir -p "$R"
PART=${1:-${PART:-}}
case "$PART" in
'' | tikz | rest) ;;
*) echo "PART: tikz or rest, not $PART" >&2; exit 2 ;;
esac
cargo build --release -q -p phitex-cli 2>&1 | tail -3
: >"$R/cli.txt"
: >"$R/flow-pdf.txt"
: >"$R/synctex.txt"
s=0
f=0
t=0
if [ "$PART" = tikz ]; then
	cargo test --release -p phitex-cli --test cli -- pdflatex_pdfdoc_tikz --test-threads=1 >"$R/cli.txt" 2>&1 || s=$?
	cargo test --release -p phitex-flow --test pdflatex_docs -- pdf_tikz >"$R/flow-pdf.txt" 2>&1 || f=$?
else
	cargo test --release -p phitex-cli --test cli -- latex_ pdftex_ini_documents --skip lualatex_external \
		${PART:+--skip pdflatex_pdfdoc_tikz} --test-threads=1 >"$R/cli.txt" 2>&1 || s=$?
	cargo test --release -p phitex-flow --test pdf --test pdflatex_docs -- ${PART:+--skip pdf_tikz} >"$R/flow-pdf.txt" 2>&1 || f=$?
fi
if [ "$PART" != rest ]; then
	cargo test --release -p phitex-cli --test synctex -- --test-threads=1 >"$R/synctex.txt" 2>&1 || t=$?
fi
n=$(find target -name '*.synctex.gz' -newer "$R/cli.txt" 2>/dev/null | wc -l)
{
	echo "# CLI oracle gate${PART:+ part $PART} on $(scripts/flow-gate-rev.sh)"
	echo "== latex, pdflatex, pdftex -ini (exit $s)"
	grep -E '^test |^test result' "$R/cli.txt"
	grep -A3 'panicked at' "$R/cli.txt" | head -60
	echo "== phitex-flow tests/pdf.rs and tests/pdflatex_docs.rs (exit $f)"
	grep -E '^test |^test result' "$R/flow-pdf.txt"
	grep -A3 'panicked at' "$R/flow-pdf.txt" | head -60
	echo "== synctex (exit $t; $n .synctex.gz files written)"
	grep -E '^test |^test result|tall' "$R/synctex.txt" | head -60
} | tee "$R/summary.txt"
