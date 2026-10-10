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
# (each test its own process with its own limit, PAR at a time: a hang
# holds one slot, not the run; its output whole in $r/tests/NAME.txt)
bin=$(sed -n 's/.*Running tests\/xetex_docs.rs (\(.*\))$/\1/p' "$r/format.txt" | head -1)
echo "test binary: $bin" >> "$r/summary.txt"
mkdir -p "$r/tests"
"$bin" --list 2>/dev/null | sed -n 's/: test$//p' | grep -E "${XTESTS:-.}" | grep -vx xelatex_format > "$r/tests.list"
export PHITEX_FONT_INDEX="$r/fonts.idx" r bin
# (HANG=NAME,...: those first, one at a time, the first schedule alone
# (one thread), with A's PHITEX_HANG_DUMP watchdog: $r/hang/NAME.txt)
if [ -n "${HANG:-}" ]; then
  mkdir -p "$r/hang"
  for t in $(echo "$HANG" | tr ',' ' '); do
    # (a virtual memory cap, so a blowup dumps before the job is killed)
    ( ulimit -v ${HANG_VMEM_KB:-31457280}; PHITEX_XETEX_ONE=1 PHITEX_HANG_DUMP=${HANG_DUMP:-60} timeout ${HANG_LIMIT:-8m} "$bin" --exact "$t" --nocapture > "$r/hang/$t.txt" 2>&1 )
    echo "hang $t exit $? ($(wc -c < "$r/hang/$t.txt") bytes)" >> "$r/summary.txt"
    grep -vx "$t" "$r/tests.list" > "$r/tests.list.tmp"; mv "$r/tests.list.tmp" "$r/tests.list"
  done
fi
xargs -P ${PAR:-3} -I{} sh -c 'timeout '"${TEST_LIMIT:-12m}"' "$bin" --exact {} --nocapture > "$r/tests/{}.txt" 2>&1; echo "{} exit $?" >> "$r/tests.exit"' < "$r/tests.list"
{ echo "tests (exit 0 ok, 101 failed, 124 over ${TEST_LIMIT:-12m}):"; sort "$r/tests.exit"; } >> "$r/summary.txt"
grep -h -E "panicked|livelock" "$r"/tests/*.txt | cut -c1-300 | head -40 >> "$r/summary.txt"
# (CLI=1: `phitex --compat=xelatex` on documents, its PDF against the
# oracle's; fonts as the oracle's fontconfig saw them)
if [ -n "${CLI:-}" ]; then
  cargo build --release -p phitex-cli > "$r/cli-build.log" 2>&1; echo "cli build exit $?" >> "$r/summary.txt"
  root=$PWD; oracle=${PHITEX_XETEX_ORACLE:-$root/crates/phitex-flow/tests/xetex/oracle}
  for d in crates/phitex-flow/tests/xetex/docs/fontspec.tex crates/phitex-flow/tests/latex/docs/ams.tex; do
    n=$(basename "$d" .tex); w="$r/cli/$n"; mkdir -p "$w"; cp "$d" "$w/"
    ( cd "$w" && SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 FONTCONFIG_FILE="$root/scripts/xetex/fonts.conf" PHITEX_CACHE_DIR="$r/cli-cache" \
      timeout ${CLI_LIMIT:-15m} "$root/target/release/phitex" --compat=xelatex -interaction=batchmode "$n.tex" > out.txt 2>&1 )
    echo "cli $n exit $?" >> "$r/summary.txt"
    if cmp -s "$w/$n.pdf" "$oracle/docs/$n.pdf"; then echo "cli $n: the PDF is xelatex's" >> "$r/summary.txt"; else echo "cli $n: the PDF differs ($(cmp "$w/$n.pdf" "$oracle/docs/$n.pdf" 2>&1 | head -1))" >> "$r/summary.txt"; fi
  done
fi
# (SYNCTEX=1: phitex-cli's SyncTeX against xelatex's; both files kept in $r/synctex)
if [ -n "${SYNCTEX:-}" ]; then
  cargo test --release -p phitex-cli --test synctex synctex_against -- --include-ignored --nocapture --test-threads=1 > "$r/synctex.txt" 2>&1; echo "synctex_against (xetex, pdftex) exit $?" >> "$r/summary.txt"
  grep -E "^xelatex|^pdflatex|^latex|^pdftex|^all|forward|panicked|^test " "$r/synctex.txt" | head -20 >> "$r/summary.txt"
  mkdir -p "$r/synctex"; for d in target/tmp/synctex/xelatex-*; do mkdir -p "$r/synctex/$(basename "$d")"; cp "$d"/*.synctex.gz "$d"/*.log "$r/synctex/$(basename "$d")/" 2>/dev/null; done
fi
# (CLI_TESTS=1: phitex-cli's own tests)
if [ -n "${CLI_TESTS:-}" ]; then
  cargo test --release -p phitex-cli --test cli > "$r/cli-tests.txt" 2>&1; echo "phitex-cli --test cli exit $?" >> "$r/summary.txt"
  grep -E "^test result|FAILED|panicked" "$r/cli-tests.txt" | head -20 >> "$r/summary.txt"
fi
if [ "${REGRESS:-1}" != 0 ]; then
cargo test --release -p phitex-flow --lib --test wall --test shapes --test dvi --test plain --test pdf --test etex --test output > "$r/regress.txt" 2>&1; echo "regress exit $?" >> "$r/summary.txt"
grep -E "^test result|FAILED|panicked" "$r/regress.txt" | head -40 >> "$r/summary.txt"
fi
if [ -n "${CLIPPY:-}" ]; then cargo clippy -p phitex-flow -p phitex-cli -p partex-xdvipdfmx --all-targets -- -D warnings > "$r/clippy.txt" 2>&1; echo "clippy (-D warnings: phitex-flow, phitex-cli, partex-xdvipdfmx) exit $?, $(grep -c ^error $r/clippy.txt) errors" >> "$r/summary.txt"; fi
mkdir -p "$r/out"; cp -r target/tmp/xetex-docs "$r/out/" 2>/dev/null; cp -r target/release/build/*/out/xetex-docs "$r/out/" 2>/dev/null
true
