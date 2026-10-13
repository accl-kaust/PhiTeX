#!/bin/bash
bin=$(cargo test --profile profiling -p phitex-tex --features profile --test profile --no-run --message-format=json 2>/dev/null | grep -o '"executable":"[^"]*"' | tail -1 | cut -d'"' -f4)
[ -x "$bin" ] || { echo nobin; exit 1; }
export PHITEX_SPEC=predicted PHITEX_THREADS=1 PHITEX_OBS=1
for d in para0 para400 para4000; do
  PHITEX_PROFILE_DOC=$PWD/scratch/$d.tex $bin --ignored --exact profile_doc --nocapture > $r/obs-$d.txt 2>&1
  grep -E "tex counters|ValueIntern|ValueGet|^$d:" $r/obs-$d.txt
  PHITEX_PROFILE_DOC=$PWD/scratch/$d.tex valgrind --tool=callgrind --callgrind-out-file=$r/cg-$d.out $bin --ignored --exact profile_doc --nocapture > $r/vg-$d.txt 2>&1
  grep refs $r/vg-$d.txt
  gzip $r/cg-$d.out
done
echo done
