#!/bin/bash
# runcase.sh NAME: lay out e2e incremental case NAME and compare pure (one
# process, rebuilds) with plain cold runs, stage by stage.
R=$(cd "$(dirname "$0")/../.." && pwd)
X=$R/target/x
c=$1
d=$X/e2e/$c
rm -rf "$d"
read job kind n < <(${SANDBOX-$R/scripts/sandbox} python3 $R/scripts/pure/case.py "$c" "$d")
if [ "$kind" = pdf ]; then compat=--compat=pdftex; cp $X/fmtp/plain.fmt "$d/src/"; else compat=--compat=tex; cp $X/fmtk/plain.fmt "$d/src/"; fi
bash $R/scripts/pure/edit.sh "$d" $compat -output-comment=partex -interaction=nonstopmode '&plain' "$job"
