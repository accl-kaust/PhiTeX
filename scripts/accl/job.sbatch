#!/bin/bash
# One partex task on a commit, in the cluster image (on accl; submitted by
# the local `accl` driver):
#
#   sbatch [slurm options] job.sbatch <commit> <task> [args...]
#
# Tasks:
#   gate                 fmt --check, clippy -D warnings, cargo xtask check
#   course <what> [...]  the course: plain | ssa | rebuild | label (tasks/course.sh)
#   edits [ARGS...]      the SSA edit harness (scripts/ssa-edits ARGS)
#   watch [COURSE] [N]   `phitex watch` against `watch --ssa` on the course
#                        (tasks/watch.sh), N sessions each
#   cmd <command...>     any command in the checkout (the course in $w/course,
#                        its aux files in $w/aux; $w and $r are exported)
#
# The sandbox is the container: a read-only image, an empty home, no
# network (`--network none`), and only the node-local workspace writable.
# bubblewrap cannot nest inside Apptainer, so the checkout's
# scripts/sandbox is replaced by a pass-through (in the workspace only).
# The network is used only by `cargo fetch` and the upstream fetch, each
# cached in ~/shared-cache/partex by the hash of what it depends on.
#
# Results: written to $w/results, copied at the end to
# ~/code/flinner/partex-phitex-runs/<task>-<job>/ (summary.txt, logs).
#SBATCH -J partex -o partex-%j.out
set -euo pipefail
commit=$1 task=$2
shift 2
sif=$HOME/shared-cache/partex/${ACCL_IMAGE:-partex-2026-09-25-tools}.sif
cache=$HOME/shared-cache/partex
here=$HOME/code/flinner/partex-phitex-accl
w=/tmp/$USER-$SLURM_JOB_ID
out=$HOME/code/flinner/partex-phitex-runs/$task-$SLURM_JOB_ID
r=$w/results
mkdir -p "$w/cargo" "$out" "$r"
# (the results reach the shared home however the job ends)
trap 'cp -a "$r/." "$out/" 2>/dev/null; rm -rf "$w"' EXIT
exec > >(tee "$out/job.log") 2>&1
cp -a "$here/tasks" "$w/tasks"

git clone -q "$HOME/code/flinner/partex-phitex.git" "$w/partex"
git -C "$w/partex" -c advice.detachedHead=false checkout -q "$commit"
cd "$w/partex"
echo "node $(hostname), $SLURM_CPUS_ON_NODE CPUs allocated, commit $(git rev-parse --short=12 HEAD), task $task $*"
echo "start $(date +%T)"

# `net` runs a fetch step with the network; `box` everything else, without.
net() {
  apptainer exec --containall --bind "$w" --home "$w" --pwd "$w/partex" \
    --env CARGO_HOME="$w/cargo" "$sif" "$@"
}
box() {
  apptainer exec --containall --net --network none --bind "$w" --home "$w" --pwd "$PWD" \
    --env CARGO_HOME="$w/cargo",CARGO_NET_OFFLINE=true,TEXMFVAR="$w/texmf-var",PARTEX_SANDBOX=1,w="$w",r="$r" \
    "$sif" "$@"
}

# the crates, fetched once per Cargo.lock
key=$(sha256sum Cargo.lock | cut -c1-16)
if [ -d "$cache/cargo-$key" ]; then
  cp -a "$cache/cargo-$key/." "$w/cargo/"
else
  net cargo fetch -q
  # (jobs racing to fill it: each writes its own part; the first rename wins)
  p=$cache/cargo-$key.part-$SLURM_JOB_ID
  cp -a "$w/cargo" "$p" && { mv -T "$p" "$cache/cargo-$key" 2>/dev/null || rm -rf "$p"; }
fi
# the upstream sources, fetched once per version of the fetch script
key=$(sha256sum scripts/fetch-upstream.sh | cut -c1-16)
if [ -d "$cache/upstream-$key" ]; then
  cp -a "$cache/upstream-$key" upstream
else
  net scripts/fetch-upstream.sh
  p=$cache/upstream-$key.part-$SLURM_JOB_ID
  cp -a upstream "$p" && { mv -T "$p" "$cache/upstream-$key" 2>/dev/null || rm -rf "$p"; }
fi
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
export w sif r
case $task in
  gate)
    # (the trip tests' oracle references, made once here: the check runs
    # its trip and etrip variants at once, and each would make them)
    box cargo xtask oracle --engine tex --filter "^trip" > "$r/oracle.log" 2>&1
    box cargo xtask etrip --oracle >> "$r/oracle.log" 2>&1 || true
    box cargo fmt --all --check
    box cargo clippy --workspace --all-targets -q -- -D warnings
    status=0
    box cargo xtask check > "$r/gate.log" 2>&1 || status=$?
    tail -25 "$r/gate.log"
    echo "gate exit $status" | tee "$r/summary.txt"
    ;;
  course)
    box cargo build --release -p partex-cli -q
    # (the course, node-local: its sources and the settled aux files)
    cp -a "$HOME/code/tmp/np-course/src" "$w/course"
    cp -a "$HOME/code/tmp/np-course/aux" "$w/aux"
    status=0
    box "$w/tasks/course.sh" "$@" || status=$?
    ;;
  watch)
    # (`phitex watch` and `watch --ssa` head to head, tasks/watch.sh; the
    # course from ~/code/tmp/<first argument>, np-course/src by default)
    box cargo build --release -p partex-cli -q
    cp -a "$HOME/code/tmp/${1:-np-course/src}" "$w/course"
    shift || true
    status=0
    box "$w/tasks/watch.sh" "$@" || status=$?
    ;;
  edits)
    box cargo build --release -p partex-cli -q
    status=0
    bin=target/release/phitex; [ -x $bin ] || bin=target/release/partex
    box scripts/ssa-edits --bin $bin --work "$w/edits" "$@" | tee "$r/summary.txt" || status=$?
    ;;
  cmd)
    # (the course too, as the course task has it: $w/course, $w/aux)
    cp -a "$HOME/code/tmp/np-course/src" "$w/course"
    cp -a "$HOME/code/tmp/np-course/aux" "$w/aux"
    status=0
    box "$@" || status=$?
    ;;
  *) echo "unknown task $task"; exit 2 ;;
esac
cp -a bench/results/. "$r/bench-results/" 2>/dev/null || true
echo "end $(date +%T) status $status"
exit $status
