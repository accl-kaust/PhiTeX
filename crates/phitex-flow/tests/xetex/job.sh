# The XeTeX job: the font index, the release build, the
# XeTeX tests, the regressions, clippy; everything into "$r".
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
dist=$(kpsewhich -var-value TEXMFDIST)
sed -i "s#/usr/share/texmf-dist#$dist#; s#/tmp/partex-xetex-fc-cache#$r/fc-cache#" scripts/xetex/fonts.conf
if [ -n "${BLESS:-}" ]; then crates/phitex-flow/tests/xetex/bless.sh "$r/bless" > "$r/bless.log" 2>&1; export PHITEX_XETEX_ORACLE="$r/bless"; fi
cargo run -q --release -p partex-otf --example otf-index -- "$r/fonts.idx" > "$r/otf-index.log" 2>&1; echo "otf-index exit $?" >> "$r/summary.txt"
cargo build --release -p phitex-flow --all-targets > "$r/build.log" 2>&1; echo "build exit $?" >> "$r/summary.txt"; grep -E "^error" -A8 "$r/build.log" | head -60 >> "$r/summary.txt"
# (the format first, alone, with a limit; then the rest on the format it saved, with a limit)
PHITEX_FONT_INDEX="$r/fonts.idx" timeout ${FMT_LIMIT:-30m} cargo test --release -p phitex-flow --test xetex_docs -- --exact xelatex_format --nocapture > "$r/format.txt" 2>&1; echo "xelatex_format exit $? (124: over ${FMT_LIMIT:-30m})" >> "$r/summary.txt"
grep -E "xelatex.fmt|panicked|errors" "$r/format.txt" | head -5 >> "$r/summary.txt"
[ -f target/tmp/xetex-docs/xelatex.fmt ] && export PHITEX_XELATEX_FMT=$PWD/target/tmp/xetex-docs/xelatex.fmt
PHITEX_FONT_INDEX="$r/fonts.idx" timeout ${DOCS_LIMIT:-45m} cargo test --release -p phitex-flow --test xetex_docs -- --test-threads=2 ${XTESTS:-} > "$r/xetex_docs.txt" 2>&1; echo "xetex_docs exit $? (124: over ${DOCS_LIMIT:-45m})" >> "$r/summary.txt"
grep -E "^test |^test result|panicked" "$r/xetex_docs.txt" | head -80 >> "$r/summary.txt"
if [ -n "${REGRESS:-1}" ]; then
cargo test --release -p phitex-flow --lib --test wall --test shapes --test dvi --test plain --test pdf --test etex --test output > "$r/regress.txt" 2>&1; echo "regress exit $?" >> "$r/summary.txt"
grep -E "^test result|FAILED|panicked" "$r/regress.txt" | head -40 >> "$r/summary.txt"
fi
if [ -n "${CLIPPY:-}" ]; then cargo clippy -p phitex-flow --all-targets > "$r/clippy.txt" 2>&1; echo "clippy exit $?, $(grep -c '^warning' $r/clippy.txt) warnings, $(grep -c '^error' $r/clippy.txt) errors" >> "$r/summary.txt"; fi
mkdir -p "$r/out"; cp -r target/tmp/xetex-docs "$r/out/" 2>/dev/null; cp -r target/release/build/*/out/xetex-docs "$r/out/" 2>/dev/null
true
