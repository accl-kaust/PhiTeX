export PHITEX_THREADS=1 PHITEX_CACHE_DIR=$PWD/target/pc
B=$(scripts/sandbox cargo test -p phitex-tex --test trip --no-run 2>&1 | grep -o "target/[^ )]*trip-[0-9a-f]*" | tail -1)
echo "== etrip (debug)"
RUST_MIN_STACK=16000000 timeout 900 scripts/sandbox $B --ignored --nocapture --exact etrip_test 2>&1 | tail -40
echo "== trip bt"
scripts/sandbox bash -c "(sleep 120; kill -INT \$(pgrep -f 'out/trip-.*trip_test' | grep -v \$\$ | tail -1)) & gdb -batch -ex run -ex 'thread apply all bt 60' --args $B --ignored --nocapture --exact trip_test 2>&1 | grep '^#\|^Thread\|SIGINT' | sed 's/ (.*) at / at /' | cut -c1-200 | head -200"
