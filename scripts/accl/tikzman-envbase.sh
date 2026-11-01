#!/bin/bash
# envs + ladder08 on this head and on its dev parent, same node.
set -u
H=$PWD
base=${BASE:-6c14b8e6}
git worktree add -q $w/base $base && ln -s $PWD/upstream $w/base/upstream
for side in head base; do
  if [ $side = head ]; then cd $H; else cd $w/base; fi
  echo "### $side $(git log --oneline -1 | cut -c1-60)"
  cargo test -q --profile fastdev -p phitex-tex --test latex latex_environments -- --exact 2>&1 | grep -E 'test result|panicked|left|right|envs' | head -8
done
