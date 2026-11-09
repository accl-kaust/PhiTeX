#!/bin/bash
set -u
T="cargo test -q --profile fastdev -p phitex-tex --features observe --test latex"
$T --no-run 2>&1 | tail -2
for v in default nocut; do
  e=(); [ $v = nocut ] && e=(PHITEX_CUT_MIN=18446744073709551615)
  env "${e[@]}" PHITEX_CUT_TRACE=1 $T latexmk_biber_j1 -- --nocapture > $r/b-$v.txt 2>&1
  echo "== $v: $(grep -E 'test result' $r/b-$v.txt)"; grep -E "^cut:.*: cut" $r/b-$v.txt | head -5; grep -c "^cut:" $r/b-$v.txt
done
