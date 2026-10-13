export PHITEX_CACHE_DIR=$PWD/target/phcache
cargo clippy --all-targets 2>&1 | grep -A6 "^warning\|^error" | head -60
cargo test -p phitex-tex --release --test plain --test ini --test etex --test core --test edits --test page_steps --test diagnostics --test pdftex 2>&1 | grep -E "^test |test result|FAILED|panicked|failures|^error" | grep -v " ok$"
cargo test -p phitex-tex --release --test plain -- story --nocapture 2>&1 | grep resumed
