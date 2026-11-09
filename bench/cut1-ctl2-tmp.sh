#!/bin/bash
set -u
rm -rf $w/control && mkdir -p $w/control && git archive ${CONTROL:-47d7561f3} | tar -x -C $w/control
[ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
( cd $w/control && cargo test -q --profile fastdev -p phitex-tex --test latex -- --test-threads 8 > $r/ctl-latex.txt 2>&1 ); grep -E "test result|^    [a-z_0-9]+$" $r/ctl-latex.txt
( cd $w/control && PHITEX_CUT_CHECK=1 cargo test -q --profile fastdev -p phitex-tex --test latex -- --test-threads 8 > $r/ctl-latex2.txt 2>&1 ); grep -E "test result|^    [a-z_0-9]+$" $r/ctl-latex2.txt
