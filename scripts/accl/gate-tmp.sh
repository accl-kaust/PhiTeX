cargo clippy -p phitex-tex --all-targets --message-format short 2>&1 | grep -E "^crates|^error" | head -30
for t in plain ini etex core edits page_steps hints diagnostics pdftex; do echo "== $t"; cargo test -p phitex-tex --profile fastdev --test $t 2>&1 | grep -E "^test result|FAILED"; done
echo "== hello"; cargo test -p phitex-tex --profile fastdev --test latex hello -- --include-ignored 2>&1 | grep -E "^test result|FAILED|panicked"
echo "== 12M"; PHITEX_HEARTBEAT=1 PHITEX_THREADS=1 PHITEX_HEARTBEAT_EXIT=12000000 cargo test -p phitex-tex --release --test latex latex_hello -- --exact --include-ignored --nocapture 2>&1 | grep -E "^hb" | awk '{print $2,$4}' | grep -E " (2000000|12000000)$"
