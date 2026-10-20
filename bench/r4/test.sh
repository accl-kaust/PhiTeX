#!/bin/bash
# r4-latexmk (accl cmd task): phitex-tex tests matching TESTS (default the
# seeded rebuild and the latexmk ones) in TARGETS (default latex), and
# clippy on the touched crates when CLIPPY=1.
set -u
export CARGO_BUILD_JOBS=16
for t in ${TARGETS:-latex}; do
  CARGO_TARGET_DIR=$w/t timeout 5400 cargo test -q --release -p phitex-tex --test "$t" -- ${TESTS:-rebuild_reads latexmk_} --test-threads ${TT:-4} > "$r/test-$t.txt" 2>&1
  echo "test $t: exit $? $(grep -E '^test result' "$r/test-$t.txt" | tail -1)" | tee -a "$r/summary.txt"
  grep -E '^test .* FAILED|panicked at' "$r/test-$t.txt" | head -20 | tee -a "$r/summary.txt"
  grep -E 'rounds' "$r/test-$t.txt" | head -60 > "$r/rounds-$t.txt"
done
if [ -n "${CLI_TESTS:-}" ]; then
  CARGO_TARGET_DIR=$w/t timeout 3600 cargo test -q --release -p phitex-cli --test cli -- $CLI_TESTS --test-threads 1 > "$r/test-cli.txt" 2>&1
  echo "test cli: exit $? $(grep -E '^test result' "$r/test-cli.txt" | tail -1)" | tee -a "$r/summary.txt"
  grep -E '^test .* FAILED|panicked at' -A3 "$r/test-cli.txt" | head -20 | tee -a "$r/summary.txt"
fi
if [ -n "${CLIPPY:-}" ]; then
  CARGO_TARGET_DIR=$w/t cargo clippy --workspace --exclude defuse --no-deps --all-targets --keep-going -q --message-format short -- -D warnings > "$r/clippy.txt" 2>&1
  echo "clippy: exit $? $(grep -cE '^[^ ]+:[0-9]+:[0-9]+: (error|warning)' "$r/clippy.txt") findings" | tee -a "$r/summary.txt"
  grep -E '^[^ ]+:[0-9]+:[0-9]+: (error|warning)' "$r/clippy.txt" | sort -u | head -40 >> "$r/summary.txt"
fi
