export PHITEX_CACHE_DIR=$PWD/target/phcache
cargo clippy --all-targets 2>&1 | grep -A6 "^warning: [^p]\|^error" | head -40
cargo test -p phitex-tex --profile fastdev --test plain --test ini --test etex --test core --test edits --test page_steps --test hints --test diagnostics --test pdftex 2>&1 | grep -E "^test |test result|FAILED|panicked|failures|^error" | grep -v " ok$"
timeout 400 cargo test -p phitex-tex --profile fastdev --test latex -- latex_hello --exact --ignored --nocapture 2>&1 | grep -E "resumed|test result|panicked"
