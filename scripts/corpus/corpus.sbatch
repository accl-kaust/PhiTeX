#!/bin/bash
# One array task of the real-document corpus (scripts/corpus/corpus.py):
#
#   sbatch --array=0-N corpus.sbatch <stage> <run> <list.tsv>
#
# <stage>: oracle | phitex. <list.tsv>: one document a line,
#   id  kind(arxiv|manual)  source  main  threads  flags
# (arxiv: source = the e-print file under $C/arxiv; manual: the directory
# under the image's /usr/share/doc/texlive). Task k takes lines k, k+N, ...
#
# Every step runs in the container: no network (--net --network none,
# checked inside before each step), --containall --cleanenv --no-home,
# the image read-only, binds only this task's node-local scratch ($w/work
# at /w, read-write), the e-print and oracle PDF ($w/in at /in, read-only),
# the phitex binary ($w/tool at /t, read-only) and the harness (/h,
# read-only), and an empty directory over /run/systemd/resolve (Apptainer
# binds the host's there, and on some nodes its io.systemd.Resolve socket
# answers DNS queries: a channel out without any network interface).
# Nothing under $HOME is bound. After each container exits
# only *.pdf, *.log, *.json, *.txt of /w/out are copied to the results.
#SBATCH -J corpus -o corpus-%A_%a.out
set -euo pipefail
stage=$1 run=$2 list=$3
C=$HOME/shared-cache/phitex-corpus
H=$HOME/code/flinner/phitex-corpus-harness
sif=$HOME/shared-cache/phitex/${CORPUS_IMAGE:-phitex-ci-2026-09-25}.sif
bin=${CORPUS_BIN:-$C/bin/current}
w=/tmp/$USER-corpus-${SLURM_ARRAY_JOB_ID:-$SLURM_JOB_ID}-${SLURM_ARRAY_TASK_ID:-0}
R=$C/results/$run
mkdir -p "$w"/{work,in,tool,noresolve} "$R"
trap 'rm -rf "$w"' EXIT
head_ip=10.0.0.1
echo "stage $stage run $run node $(hostname) job ${SLURM_JOB_ID} task ${SLURM_ARRAY_TASK_ID:-0}/${SLURM_ARRAY_TASK_COUNT:-1} cpus $SLURM_CPUS_ON_NODE image $(basename "$sif") $(date -Is)"
if [ "$stage" = phitex ] || [ "$stage" = diag ] || [ "$stage" = target ]; then
  cp "$bin/phitex" "$w/tool/phitex"
  cp "$bin/pdf-relaxed-eq" "$w/tool/pdf-relaxed-eq" 2>/dev/null || echo "no pdf-relaxed-eq in $bin"
  cp "$bin/commit" "$w/tool/commit" 2>/dev/null || true
  echo "phitex $(cat "$w/tool/commit" 2>/dev/null)"
fi
mkdir -p "$w"/work/{home,texmf-var,cache}

box() {
  apptainer exec --containall --cleanenv --no-home --net --network none \
    --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w/work:/w" --bind "$w/in:/in:ro" --bind "$w/tool:/t:ro" --bind "$H:/h:ro" --pwd /w \
    --env TEXMFVAR=/w/texmf-var \
    "$sif" python3 -I /h/corpus.py "$@"
}
copyout() {  # copyout DEST
  mkdir -p "$1"
  find "$w/work/out" -maxdepth 1 -type f \( -name '*.pdf' -o -name '*.log' -o -name '*.json' -o -name '*.txt' \) \
    -exec cp {} "$1/" \;
}

i=0
k=${SLURM_ARRAY_TASK_ID:-0} n=${SLURM_ARRAY_TASK_COUNT:-1}
while IFS=$'\t' read -r id kind src main threads flags; do
  [ -z "$id" ] && continue
  case $id in \#*) continue ;; esac
  if [ $((i % n)) -ne "$k" ]; then i=$((i + 1)); continue; fi
  i=$((i + 1))
  dest=$R/$id/$stage
  if [ -f "$dest/done" ]; then echo "$id: done already"; continue; fi
  rm -rf "$w/work/doc" "$w/work/run" "$w/work/out" "$w/in"/*
  mkdir -p "$w/work/out"
  t0=$(date +%s.%N)
  status=0
  if [ "$kind" = arxiv ]; then
    cp "$C/arxiv/$src" "$w/in/src"
    box prepare --src /in/src --doc /w/doc --out /w/out > "$w/work/out/prepare.txt" 2>&1 || status=$?
  elif [ "$kind" = local ]; then
    cp -a "$C/$src" "$w/in/doc"
    box prepare-local --main "$main" --doc /w/doc --out /w/out > "$w/work/out/prepare.txt" 2>&1 || status=$?
  else
    box prepare-manual --dir "$src" --main "$main" --doc /w/doc --out /w/out > "$w/work/out/prepare.txt" 2>&1 || status=$?
  fi
  if [ $status -eq 0 ] && grep -q '"status": "ok"' "$w/work/out/prep.json"; then
    main=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["main"])' "$w/work/out/prep.json")
    case $stage in
      target)
        [ -f "$R/$id/oracle/oracle.pdf" ] && cp "$R/$id/oracle/oracle.pdf" "$w/in/oracle.pdf"
        tools=; [ "$kind" = manual ] && tools=--tools
        box target --doc /w/doc --main "$main" --out /w/out --threads 8 --timeout 900 $tools > "$w/work/out/target-term.txt" 2>&1 || status=$? ;;
      diag)
        [ -f "$R/$id/oracle/oracle.pdf" ] && cp "$R/$id/oracle/oracle.pdf" "$w/in/oracle.pdf"
        box diag --doc /w/doc --main "$main" --out /w/out --timeout 900 --threads "${DIAG_J:-1}" > "$w/work/out/diag-run.txt" 2>&1 || status=$? ;;
      oracle)
        tools=; [ "$kind" = manual ] && tools=--tools
        box oracle --doc /w/doc --main "$main" --out /w/out $tools > "$w/work/out/oracle-term.txt" 2>&1 || status=$? ;;
      phitex)
        opdf=; [ -f "$R/$id/oracle/oracle.pdf" ] && { cp "$R/$id/oracle/oracle.pdf" "$w/in/oracle.pdf"; opdf="--oracle-pdf /in/oracle.pdf"; }
        to=$(python3 -c 'import json,sys
try: print(int(max(120, 20*json.load(open(sys.argv[1]))["wall_s"])))
except Exception: print(900)' "$R/$id/oracle/oracle.json")
        # (pdflatex again on this node: the ratio's denominator, same node as phitex)
        tools=; [ "$kind" = manual ] && tools=--tools
        box oracle --doc /w/doc --main "$main" --out /w/out $tools --tag oracle-here > "$w/work/out/oracle-here-term.txt" 2>&1 || true
        box phitex --doc /w/doc --main "$main" --out /w/out --threads "$threads" --timeout "$to" --edit $opdf \
          > "$w/work/out/phitex-term.txt" 2>&1 || status=$? ;;
    esac
  fi
  echo "$status" > "$w/work/out/status.txt"
  copyout "$dest"
  [ $status -ne 90 ] && echo "$(date -Is) $(hostname)" > "$dest/done"
  echo "$id: $stage exit $status $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.1f", b-a}')s"
  [ $status -eq 90 ] && { echo "NETWORK CHECK FAILED: stopping"; exit 90; }
done < "$list"
echo "end $(date -Is)"
