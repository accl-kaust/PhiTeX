#!/bin/bash
# clippy on phitex-tex with the profile and observe features, and the default build (on accl, in a job).
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
cargo clippy -q -p phitex-tex --no-deps --features profile,observe -- -D warnings > "$r/clippy-po.txt" 2>&1; echo "clippy profile,observe exit $?" | tee -a "$r/summary.txt"
cargo clippy -q -p phitex-tex --no-deps -- -D warnings > "$r/clippy.txt" 2>&1; echo "clippy default exit $?" | tee -a "$r/summary.txt"
cargo fmt --check -p phitex-tex > "$r/fmt.txt" 2>&1; echo "fmt exit $?" | tee -a "$r/summary.txt"; head -30 "$r/fmt.txt" | tee -a "$r/summary.txt"
grep -E -- "^(error|warning)|-->" "$r"/clippy*.txt | head -20 | tee -a "$r/summary.txt"
