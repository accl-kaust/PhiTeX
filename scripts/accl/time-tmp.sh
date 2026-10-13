rm -rf target/cache
PHITEX_HEARTBEAT=1 PHITEX_THREADS=1 PHITEX_HEARTBEAT_EXIT=12100000 cargo test -p phitex-tex --release --test latex latex_hello -- --exact --include-ignored --nocapture > hb.txt 2>&1; echo "exit $?"
grep -E "^hb" hb.txt | awk '{print $2,$4}' | grep -E " (2000000|12000000)$"; grep -v "^hb\|Compiling" hb.txt | tail -5
