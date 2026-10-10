#!/bin/bash
# dirsof.sh NAME: input and font directories of LaTeX document NAME (its .dirs), as latex_docs.rs makes them, mapped to this TeX Live
n=$1 L=crates/phitex-flow/tests/latex f=crates/phitex-flow/tests/latex/docs/$1.dirs
[ -f bench/flow-par/$1.dirs ] && f=bench/flow-par/$1.dirs
{ printf -- '--etex --input-dir %s ' $L
  grep '^input ' $f | cut -d' ' -f2 | while read d; do printf -- '--input-dir %s ' "$d"; done
  while read d; do grep -qx "input $d" $f || printf -- '--input-dir %s ' "$d"; done < $L/dirs.txt
  grep '^font ' $f | cut -d' ' -f2 | while read d; do printf -- '--font-dir %s ' "$d"; done; } | bench/flow-par/fix.sh
