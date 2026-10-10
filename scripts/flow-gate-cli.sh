#!/usr/bin/env bash
# A ship candidate's CLI oracle gate, as one cluster job:
#
#   ACCL_COMMIT=REV ACCL_OPTS="-c 4 --mem=16G -t 01:30:00 --exclude=harrier" \
#     scripts/accl/accl submit cmd bash scripts/flow-gate-cli.sh
#
# The `phitex` binary on phitex-flow (crates/phitex-cli/tests/cli.rs and
# synctex.rs), one heavy run at a time:
# - `latex` on the ten LaTeX documents: the DVI and the written files are
#   TeX Live's `latex`'s (the recorded oracles, the date masked);
# - `pdflatex` on them: the PDF and the written files, byte for byte;
# - `pdftex -ini -etex` on the eight -ini documents and the ten plain
#   ones: the PDF, byte for byte;
# - SyncTeX (synctex.rs, TeX Live's `synctex` as the parser): the files
#   are written and asked the same questions as pdfTeX's; its tallies are
#   reported, not a bar.
# lualatex_external and the fixpoint and watch tests are not this gate's.
# Results in $r (target/flow-gate-cli outside a job): cli.txt,
# synctex.txt, summary.txt.
set -uo pipefail
cd "$(dirname "$0")/.."
R=${r:-target/flow-gate-cli}
mkdir -p "$R"
cargo build --release -q -p phitex-cli 2>&1 | tail -3
s=0
cargo test --release -p phitex-cli --test cli -- latex_ pdftex_ini_documents --skip lualatex_external --test-threads=1 >"$R/cli.txt" 2>&1 || s=$?
t=0
cargo test --release -p phitex-cli --test synctex -- --test-threads=1 >"$R/synctex.txt" 2>&1 || t=$?
n=$(find target -name '*.synctex.gz' -newer "$R/cli.txt" 2>/dev/null | wc -l)
{
	echo "# CLI oracle gate on $(git rev-parse --short HEAD 2>/dev/null || echo '?')"
	echo "== latex, pdflatex, pdftex -ini (exit $s)"
	grep -E '^test |^test result' "$R/cli.txt"
	grep -A3 'panicked at' "$R/cli.txt" | head -60
	echo "== synctex (exit $t; $n .synctex.gz files written)"
	grep -E '^test |^test result|tall' "$R/synctex.txt" | head -60
} | tee "$R/summary.txt"
