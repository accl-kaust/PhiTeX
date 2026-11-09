#!/bin/bash
set -u
T="cargo test -q --profile fastdev -p phitex-tex --features observe --test latex"
$T --no-run 2>&1 | tail -2
for v in check default; do
  e=(); [ $v = check ] && e=(PHITEX_CUT_CHECK=1)
  env "${e[@]}" PHITEX_CUT_TRACE=1 $T latexmk_biber -- --nocapture > $r/b-$v.txt 2>&1
  echo "== $v: $(grep -E 'test result' $r/b-$v.txt)"; grep -E "^cut:.*: cut" $r/b-$v.txt | head -5; grep -c "^cut:" $r/b-$v.txt
done
