#!/bin/bash
# One phitex task on a commit, in the cluster image (on accl; submitted by
# the local `accl` driver):
#
#   sbatch [slurm options] job.sbatch <commit> <task> [args...]
#
# Tasks:
#   gate                 fmt --check, clippy -D warnings, cargo xtask check
#   cmd <command...>     any command in the checkout (the course in $w/course,
#                        its aux files in $w/aux; $w and $r are exported)
#
# The sandbox is the container: a read-only image, an empty home, no
# network (`--network none`), and only the node-local workspace writable.
# bubblewrap cannot nest inside Apptainer, so the checkout's
# scripts/sandbox is replaced by a pass-through (in the workspace only).
# The network is used only by `cargo fetch` and the upstream fetch, each
# cached in ~/shared-cache/phitex by the hash of what it depends on.
#
# Results: written to $w/results, copied at the end to
# ~/code/flinner/phitex-runs/<task>-<job>/ (summary.txt, logs).
#
# Built binaries shared between jobs (scripts/accl/gate-fast; sbatch
# --export=ALL,VAR=...):
#   ACCL_ROOT=/gw        the workspace is seen at this fixed path inside the
#                        container, so a test binary built in one job (its
#                        crate paths compiled in) runs in another
#   ACCL_STORE_OUT=DIR   after a `cmd` task that exits 0, its $r/store is
#                        published as ~/DIR (atomically), not in the results
#   ACCL_STORE_IN=A:B    before the task, ~/A, ~/B copied into $w/bin
#   ACCL_NO_CARGO=1      the crates are not copied in (the task builds nothing)
# and ~/code/flinner/phitex-accl/gate-control (the recorded control timings)
# is copied to $w/gate-control.
#SBATCH -J phitex -o phitex-%j.out
set -euo pipefail
commit=$1 task=$2
shift 2
sif=$HOME/shared-cache/phitex/${ACCL_IMAGE:-phitex-2026-09-25-tools}.sif
cache=$HOME/shared-cache/phitex
here=$HOME/code/flinner/phitex-accl
w=/tmp/$USER-$SLURM_JOB_ID
out=$HOME/code/flinner/phitex-runs/$task-$SLURM_JOB_ID
r=$w/results
root=${ACCL_ROOT:-$w}
# (a full node disk corrupts results silently: fail at once below 20 GB free)
free=$(df -Pk /tmp | awk 'NR==2{print $4}'); [ "$free" -ge 20971520 ] || { echo "FAILED: /tmp on $(hostname) has only $((free/1048576)) GB free (need 20); leftovers: $(ls -d /tmp/$USER-* 2>/dev/null | wc -l) of ours"; exit 75; }
mkdir -p "$w/cargo" "$out" "$r"
# (the results reach the shared home however the job ends)
trap 'cp -a "$r/." "$out/" 2>/dev/null; rm -rf "$w"' EXIT
exec > >(tee "$out/job.log") 2>&1
[ -d "$here/tasks" ] && cp -a "$here/tasks" "$w/tasks"  # (machine mode's task scripts, gone with b8c6a5d; `cmd` needs none)

git clone -q "$HOME/code/flinner/phitex.git" "$w/phitex"
git -C "$w/phitex" -c advice.detachedHead=false checkout -q "$commit"
cd "$w/phitex"
echo "node $(hostname), $SLURM_CPUS_ON_NODE CPUs allocated, commit $(git rev-parse --short=12 HEAD), task $task $*"
echo "start $(date +%T)"

# `net` runs a fetch step with the network; `box` everything else, without.
net() {
  apptainer exec --containall --bind "$w" --home "$w" --pwd "$w/phitex" \
    --env CARGO_HOME="$w/cargo" "$sif" "$@"
}
# (Apptainer binds the host's /run/systemd/resolve, whose resolved socket
# answers DNS queries on some nodes even without a network: an empty
# directory hides it)
mkdir -p "$w/noresolve"
ubind=""
box() {
  apptainer exec --containall --net --network none --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w:$root" $ubind --home "$root" --pwd "${PWD/#$w/$root}" \
    --env CARGO_HOME="$root/cargo",CARGO_NET_OFFLINE=true,TEXMFVAR="$root/texmf-var",PHITEX_SANDBOX=1,w="$root",r="$root/results" \
    "$sif" "$@"
}
if box getent hosts export.arxiv.org >/dev/null 2>&1; then echo "FAILED: the box resolves names on $(hostname)"; exit 76; fi

# the crates, fetched once per Cargo.lock
key=$(sha256sum Cargo.lock | cut -c1-16)
if [ -n "${ACCL_NO_CARGO:-}" ]; then
  :
elif [ -d "$cache/cargo-$key" ]; then
  # (beside the upstream copy below)
  cp -a "$cache/cargo-$key/." "$w/cargo/" &
  cpid=$!
else
  net cargo fetch -q
  # (jobs racing to fill it: each writes its own part; the first rename wins)
  p=$cache/cargo-$key.part-$SLURM_JOB_ID
  cp -a "$w/cargo" "$p" && { mv -T "$p" "$cache/cargo-$key" 2>/dev/null || rm -rf "$p"; }
fi
# the upstream sources, fetched once per version of the fetch script
key=$(sha256sum scripts/fetch-upstream.sh | cut -c1-16)
if [ -d "$cache/upstream-$key" ] && [ -n "${ACCL_NO_CARGO:-}" ]; then
  # (a job that builds nothing reads the sources where they are)
  mkdir -p upstream
  ubind="--bind $cache/upstream-$key:$root/phitex/upstream:ro"
elif [ -d "$cache/upstream-$key" ]; then
  cp -a "$cache/upstream-$key" upstream
else
  net scripts/fetch-upstream.sh
  p=$cache/upstream-$key.part-$SLURM_JOB_ID
  cp -a upstream "$p" && { mv -T "$p" "$cache/upstream-$key" 2>/dev/null || rm -rf "$p"; }
fi
# (that copy only: a bare `wait` would wait for the log's tee too)
[ -z "${cpid:-}" ] || wait "$cpid"
# the container is the sandbox
cat > scripts/sandbox <<'EOF'
#!/usr/bin/env bash
# (cluster job: the Apptainer container is the sandbox; see job.sbatch)
[ "${1:-}" = "--net" ] && shift
exec "$@"
EOF
chmod +x scripts/sandbox
# (and Slurm's allocation is the memory cap: scripts/heavy passes through)
cat > scripts/heavy <<'EOF2'
#!/usr/bin/env bash
# (cluster job: the allocation caps memory; see job.sbatch)
exec "$@"
EOF2
chmod +x scripts/heavy

export -f box
export w sif r root ubind
# (binaries another job built, and the recorded control timings)
if [ -n "${ACCL_STORE_IN:-}" ]; then
  mkdir -p "$w/bin"
  for s in ${ACCL_STORE_IN//:/ }; do
    cp -a "$HOME/$s" "$w/bin/" || { echo "FAILED: no store $s"; exit 77; }
  done
fi
[ -d "$here/gate-control" ] && cp -a "$here/gate-control" "$w/gate-control"
case $task in
  gate)
    # (the vendored engine is an archive of another tree: not formatted or linted here)
    # (no fmt check here: the image's rustfmt differs from the pinned nightly and reflows
    # lines the local one keeps; formatting is checked locally, not by the merge gate)
    box cargo clippy --workspace --exclude defuse --all-targets -q -- -D warnings
    status=0
    box cargo xtask check > "$r/gate.log" 2>&1 || status=$?
    tail -25 "$r/gate.log"
    echo "gate exit $status" | tee "$r/summary.txt"
    ;;
  cmd)
    # (the course too, as the course task has it: $w/course, $w/aux;
    # ACCL_COURSE, passed with `--export=ALL,ACCL_COURSE=DIR` in ACCL_OPTS,
    # names another copy with the same src/ and aux/)
    course=${ACCL_COURSE:-$HOME/code/tmp/np-course}
    cp -a "$course/src" "$w/course"
    cp -a "$course/aux" "$w/aux"
    status=0
    box "$@" || status=$?
    # (a build for other jobs: published whole or not at all, and kept
    # out of the results)
    if [ -n "${ACCL_STORE_OUT:-}" ] && [ -d "$r/store" ]; then
      if [ "$status" = 0 ]; then
        d=$HOME/$ACCL_STORE_OUT
        mkdir -p "$(dirname "$d")"
        # (a store already there, an older build's with fewer binaries,
        # is replaced: published whole)
        cp -a "$r/store" "$d.part-$SLURM_JOB_ID" && {
          mv -T "$d.part-$SLURM_JOB_ID" "$d" 2>/dev/null || {
            rm -rf "$d.old-$SLURM_JOB_ID" && mv -T "$d" "$d.old-$SLURM_JOB_ID" && mv -T "$d.part-$SLURM_JOB_ID" "$d" && rm -rf "$d.old-$SLURM_JOB_ID"
          } || rm -rf "$d.part-$SLURM_JOB_ID"
        }
      fi
      rm -rf "$r/store"
    fi
    ;;
  *) echo "unknown task $task"; exit 2 ;;
esac
cp -a bench/results/. "$r/bench-results/" 2>/dev/null || true
echo "end $(date +%T) status $status"
exit $status
