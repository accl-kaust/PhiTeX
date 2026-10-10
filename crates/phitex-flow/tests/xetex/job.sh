export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
dist=$(kpsewhich -var-value TEXMFDIST)
sed -i "s#/usr/share/texmf-dist#$dist#; s#/tmp/partex-xetex-fc-cache#$r/fc-cache#" scripts/xetex/fonts.conf
crates/phitex-flow/tests/xetex/bless.sh "$r/bless" > "$r/bless.log" 2>&1
cargo run -q --release -p partex-otf --example otf-index -- "$r/fonts.idx" > "$r/otf-index.log" 2>&1; echo "otf-index exit $?" >> "$r/summary.txt"
cargo build --release -p phitex-flow --all-targets > "$r/build.log" 2>&1; echo "build exit $?" >> "$r/summary.txt"; grep -E "^error" -A8 "$r/build.log" | head -60 >> "$r/summary.txt"
PHITEX_FONT_INDEX="$r/fonts.idx" PHITEX_XETEX_ORACLE="$r/bless" cargo test --release -p phitex-flow --test xetex_docs -- --test-threads=2 > "$r/xetex_docs.txt" 2>&1; echo "xetex_docs exit $?" >> "$r/summary.txt"
grep -E "^test |^test result|panicked" "$r/xetex_docs.txt" | head -80 >> "$r/summary.txt"
cargo test --release -p phitex-flow --test wall --test shapes --test dvi --test plain --test pdf --test etex > "$r/regress.txt" 2>&1; echo "regress exit $?" >> "$r/summary.txt"
grep -E "^test result|FAILED|panicked" "$r/regress.txt" | head -40 >> "$r/summary.txt"
mkdir -p "$r/out"; cp -r target/release/build/../../tmp/xetex-docs "$r/out/" 2>/dev/null; cp -r target/tmp/xetex-docs "$r/out/" 2>/dev/null
true
