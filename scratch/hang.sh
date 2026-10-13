#!/bin/bash
set -u
export PHITEX_CACHE_DIR=$PWD/target/cache PHITEX_TEX_TIME=1 PHITEX_THREADS=1 PHITEX_HEARTBEAT=1
scripts/sandbox cargo test -p phitex-tex --test latex --no-run 2>&1 | tail -3
out=$PWD/scratch-out; mkdir -p $out; log=$out/test.log
scripts/sandbox cargo test -p phitex-tex --test latex latex_hello -- --ignored --nocapture > $log 2>&1 &
last=$(date +%s); prev=0
while sleep 10; do
  n=$(grep -c 'round' $log); now=$(date +%s)
  if [ "$n" != "$prev" ]; then prev=$n; last=$now; fi
  pid=$(pgrep -n -f 'out/latex-[0-9a-f]* latex_hello')
  [ -z "$pid" ] && { echo "test exited"; tail -30 $log; exit 0; }
  [ $((now-last)) -ge 90 ] && grep -q UnicodeData $log && break
  [ $((now-last)) -ge 600 ] && break
done
echo "stalled; pid $pid"; grep round $log | tail -8
cat /proc/sys/kernel/yama/ptrace_scope; for i in 1 2 3 4 5; do echo "=== sample $i"; date; grep -c "^hb run" $log; gdb -p $pid -batch -ex "thread apply all bt 25" 2>&1 | grep -v '^\[' ; grep '^hb' $log | tail -3; sleep 20; done
echo "=== hb tail"; grep '^hb' $log | tail -40; grep -c '^hb run' $log
kill $pid
