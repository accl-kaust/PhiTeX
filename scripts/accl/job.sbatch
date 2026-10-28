#!/bin/bash
# One phitex task on a commit, in the cluster image (on accl, or on mc
# with CLUSTER=mc; submitted by the local `accl` driver):
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
# The network is used only by `cargo fetch`, the upstream fetch and a
# superproject's `npm ci` (phitex-frontends: run in it, `accl submit` snapshots
# it and its phitex/ submodule; see the crates and node modules below), each
# cached in ~/shared-cache/phitex by the hash of what it depends on.
#
# Results: written to $w/results, copied at the end to
# ~/code/flinner/phitex-runs/<task>-<job>/ (summary.txt, logs).
#
# Built binaries shared between jobs (scripts/accl/gate-fast; sbatch
# --export=ALL,VAR=...):
#   ACCL_ROOT=/gw        the workspace is seen at this fixed path inside the
#                        container, so a test binary built in one job (its
#                        crate paths compiled in) runs in another. /gw is
#                        the default for every job (each container binds
#                        its own workspace there, so jobs sharing a node
#                        do not meet), so every build can use the
#                        compiler cache below; ACCL_ROOT=host keeps the
#                        workspace at its node path ($w), without it
#   ACCL_STORE_OUT=DIR   after a `cmd` task that exits 0, its $r/store is
#                        published as ~/DIR (atomically), not in the results
#   ACCL_STORE_IN=A:B    before the task, ~/A, ~/B copied into $w/bin
#   ACCL_NO_CARGO=1      the crates are not copied in (the task builds nothing)
#
# A workspace kept between jobs (an agent's edit-build loop):
#   ACCL_WS=NAME         the workspace is /tmp/$USER-ws-NAME on the node,
#                        kept after the job: its checkout moves to the
#                        commit (only the files that differ are rewritten,
#                        so their times say what changed) and its target
#                        directories stay, so a build after an edit is
#                        incremental (`--profile fastdev`: ~25 s for an
#                        edit in phitex-tex, against ~185 s for a fresh
#                        release build). Seen at ACCL_ROOT, /gw by default.
#                        Jobs with one NAME on a node run one at a time
#                        (a lock); the node is the driver's to pin (`accl`
#                        pins ACCL_WS_NODE, acclnode01 by default). Remove
#                        one with `accl ws-clean NAME`.
# and ~/code/flinner/phitex-accl/gate-control (the recorded control timings)
# is copied to $w/gate-control.
#
# The compiler cache: a job that builds at a fixed root (ACCL_ROOT, /gw
# by default) gets sccache, ~/shared-cache/bin/sccache-VERSION (the
# release's static binary, fetched and checked by the first such job), on
# a cache on the shared home, ~/shared-cache/sccache-phitex. Only at a
# fixed root: an artifact keeps its absolute paths (a test's
# CARGO_MANIFEST_DIR, the debug info), so one built under another root
# must not be reused. In the box it is PHITEX_SCCACHE; the gate's shards
# put it in RUSTC_WRAPPER once its server starts (gate-shard.sh). Nothing
# of it is in the image or rust-toolchain.toml: without it a job builds
# as before. ACCL_NO_SCCACHE=1 turns it off.
#SBATCH -J phitex -o phitex-%j.out
set -euo pipefail
# (mc's nodes have no apptainer: the user-space one, apptainer-user.sbatch)
command -v apptainer > /dev/null || PATH=$HOME/shared-cache/apptainer/usr/bin:$PATH
commit=$1 task=$2
shift 2
sif=$HOME/shared-cache/phitex/${ACCL_IMAGE:-phitex-2026-09-25-tools}.sif
cache=$HOME/shared-cache/phitex
here=$HOME/code/flinner/phitex-accl
if [ -n "${ACCL_WS:-}" ]; then
  # (a workspace kept between jobs, see the header: one job at a time)
  w=/tmp/$USER-ws-$ACCL_WS
  root=${ACCL_ROOT:-/gw}
  # (its incremental builds: the compiler cache only when ACCL_ROOT is given)
  fixed=${ACCL_ROOT:+1}
  mkdir -p "$w"
  exec 9> "$w.lock"
  flock 9
else
  w=/tmp/$USER-$SLURM_JOB_ID
  root=${ACCL_ROOT:-/gw} fixed=1
  [ "$root" != host ] || root=$w fixed=
fi
out=$HOME/code/flinner/phitex-runs/$task-$SLURM_JOB_ID
r=$w/results
# (a full node disk corrupts results silently: fail at once below 20 GB free)
free=$(df -Pk /tmp | awk 'NR==2{print $4}'); [ "$free" -ge 20971520 ] || { echo "FAILED: /tmp on $(hostname) has only $((free/1048576)) GB free (need 20); leftovers: $(ls -d /tmp/$USER-* 2>/dev/null | wc -l) of ours"; exit 75; }
rm -rf "$r"
mkdir -p "$w/cargo" "$out" "$r"
# (the results reach the shared home however the job ends; a kept
# workspace keeps all but them)
if [ -n "${ACCL_WS:-}" ]; then
  trap 'cp -a "$r/." "$out/" 2>/dev/null; rm -rf "$r"' EXIT
else
  trap 'cp -a "$r/." "$out/" 2>/dev/null; rm -rf "$w"' EXIT
fi
exec > >(tee "$out/job.log") 2>&1
[ -d "$here/tasks" ] && cp -a "$here/tasks" "$w/tasks"  # (machine mode's task scripts, gone with b8c6a5d; `cmd` needs none)

if [ -n "${ACCL_WS:-}" ] && [ -d "$w/phitex/.git" ]; then
  # (the kept checkout moved to the commit: unchanged files keep their
  # times; the target directories and the upstream copy stay)
  git -C "$w/phitex" fetch -q origin "+refs/wip/$commit:refs/wip/$commit" 2>/dev/null \
    || git -C "$w/phitex" fetch -q origin
  git -C "$w/phitex" -c advice.detachedHead=false checkout -q --force --detach "$commit"
  git -C "$w/phitex" clean -ffdxq -e '/target*' -e /upstream
else
  rm -rf "$w/phitex"
  git clone -q "$HOME/code/flinner/phitex.git" "$w/phitex"
  git -C "$w/phitex" -c advice.detachedHead=false checkout -q "$commit"
fi
# (a superproject's submodules, phitex-frontends' phitex/: each gitlink's
# commit, pushed beside it by the driver's snapshot, checked out there)
git -C "$w/phitex" ls-files -s | awk -F'\t' '$1 ~ /^160000 / {split($1, a, " "); print a[2] "\t" $2}' | while IFS=$'\t' read -r sc p; do
  git -C "$w/phitex" cat-file -e "$sc^{commit}" 2>/dev/null || git -C "$w/phitex" fetch -q origin "+refs/wip/$sc:refs/wip/$sc"
  rm -rf "${w:?}/phitex/$p"
  # (a local clone: its objects hard links, no alternates file naming the
  # node path, which the box sees elsewhere, at $root; mc job 75726)
  git clone -q --no-checkout "$w/phitex" "$w/phitex/$p"
  git -C "$w/phitex/$p" -c advice.detachedHead=false checkout -q "$sc"
done
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
ubind="" sccbind="" sccenv=""
box() {
  apptainer exec --containall --net --network none --bind "$w/noresolve:/run/systemd/resolve:ro" --bind "$w:$root" $ubind $sccbind --home "$root" --pwd "${PWD/#$w/$root}" \
    --env CARGO_HOME="$root/cargo",CARGO_NET_OFFLINE=true,TEXMFVAR="$root/texmf-var",PHITEX_SANDBOX=1,w="$root",r="$root/results$sccenv" \
    "$sif" "$@"
}
if box getent hosts export.arxiv.org >/dev/null 2>&1; then echo "FAILED: the box resolves names on $(hostname)"; exit 76; fi

# the crates, fetched once per Cargo.lock (PhiTeX's, at the top; a
# superproject's, phitex-frontends' core/, diff-wasm/ and phitex/, one
# level down), all into the one CARGO_HOME
for lock in $(ls Cargo.lock */Cargo.lock 2>/dev/null); do
key=$(sha256sum "$lock" | cut -c1-16)
# (one copy or fetch into $w/cargo at a time)
[ -z "${cpid:-}" ] || { wait "$cpid"; cpid=; }
if [ -n "${ACCL_NO_CARGO:-}" ]; then
  :
elif [ -d "$cache/cargo-$key" ]; then
  # (beside the upstream copy below)
  cp -a "$cache/cargo-$key/." "$w/cargo/" &
  cpid=$!
else
  if [ "$lock" = Cargo.lock ]; then net cargo fetch -q; else net bash -c 'cd "$1" && cargo fetch -q' fetch "$(dirname "$lock")"; fi
  # (jobs racing to fill it: each writes its own part; the first rename wins)
  p=$cache/cargo-$key.part-$SLURM_JOB_ID
  cp -a "$w/cargo" "$p" && { mv -T "$p" "$cache/cargo-$key" 2>/dev/null || rm -rf "$p"; }
fi
done
# the upstream sources, fetched once per version of the fetch script
# (PhiTeX's, or a submodule's: its upstream/ beside its scripts/)
us=scripts/fetch-upstream.sh up=upstream
if [ ! -f $us ]; then
  us=$(ls */scripts/fetch-upstream.sh 2>/dev/null | head -1) up=${us%%/*}/upstream
fi
key=$([ -z "$us" ] || sha256sum $us | cut -c1-16)
if [ -z "$us" ]; then
  :
elif [ -d "$cache/upstream-$key" ] && [ -n "${ACCL_NO_CARGO:-}" ]; then
  # (a job that builds nothing reads the sources where they are)
  mkdir -p $up
  ubind="--bind $cache/upstream-$key:$root/phitex/$up:ro"
elif [ -d "$cache/upstream-$key" ]; then
  # (a kept workspace's copy replaced: the same times, cp -a)
  rm -rf $up
  cp -a "$cache/upstream-$key" $up
else
  net $us
  p=$cache/upstream-$key.part-$SLURM_JOB_ID
  cp -a $up "$p" && { mv -T "$p" "$cache/upstream-$key" 2>/dev/null || rm -rf "$p"; }
fi
# the node modules, installed once per package-lock.json (phitex-frontends';
# PhiTeX has none): npm ci with the network, its scripts run there too
if [ -f package-lock.json ] && [ -z "${ACCL_NO_CARGO:-}" ]; then
  key=$(sha256sum package-lock.json | cut -c1-16)
  if [ -d "$cache/npm-$key" ]; then
    rm -rf node_modules
    cp -a "$cache/npm-$key" node_modules
  else
    net npm ci --no-audit --no-fund --loglevel=error
    p=$cache/npm-$key.part-$SLURM_JOB_ID
    cp -a node_modules "$p" && { mv -T "$p" "$cache/npm-$key" 2>/dev/null || rm -rf "$p"; }
  fi
fi
# (that copy only: a bare `wait` would wait for the log's tee too)
[ -z "${cpid:-}" ] || wait "$cpid"
# the compiler cache (see the header)
scc_ver=0.18.0
scc_sha256=45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89
scc_bin=$HOME/shared-cache/bin/sccache-$scc_ver
scc_dir=$HOME/shared-cache/sccache-phitex
sccbind="" sccenv=""
if [ -z "${ACCL_NO_CARGO:-}" ] && [ -n "$fixed" ] && [ -z "${ACCL_NO_SCCACHE:-}" ]; then
  if [ ! -x "$scc_bin" ]; then
    # (fetched once, in a job, never on the head: the release's static
    # binary, checked against its pinned SHA-256; jobs racing each
    # fetch their own copy and the last rename wins, the same bytes)
    tgz=sccache-v$scc_ver-x86_64-unknown-linux-musl.tar.gz
    if net curl -fsSL --retry 3 -o "$w/$tgz" "https://github.com/mozilla/sccache/releases/download/v$scc_ver/$tgz" > "$r/sccache-fetch.log" 2>&1 \
      && [ "$(sha256sum "$w/$tgz" | cut -d' ' -f1)" = "$scc_sha256" ] \
      && tar -xzf "$w/$tgz" -C "$w" \
      && mkdir -p "$(dirname "$scc_bin")" \
      && cp "$w/sccache-v$scc_ver-x86_64-unknown-linux-musl/sccache" "$scc_bin.part-$SLURM_JOB_ID" \
      && chmod +x "$scc_bin.part-$SLURM_JOB_ID" \
      && mv -f "$scc_bin.part-$SLURM_JOB_ID" "$scc_bin"; then
      echo "sccache $scc_ver fetched to $scc_bin"
    else
      echo "sccache $scc_ver not fetched, building without it: $(tail -2 "$r/sccache-fetch.log" | tr '\n' ' ')"
      rm -f "$scc_bin.part-$SLURM_JOB_ID"
    fi
  fi
  if [ -x "$scc_bin" ]; then
    mkdir -p "$scc_dir"
    sccbind="--bind $scc_bin:/opt/sccache/sccache:ro --bind $scc_dir:/opt/sccache/cache"
    sccenv=",PHITEX_SCCACHE=/opt/sccache/sccache,SCCACHE_DIR=/opt/sccache/cache,SCCACHE_CACHE_SIZE=60G,SCCACHE_IDLE_TIMEOUT=0"
  fi
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
export w sif r root ubind sccbind sccenv
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
    # (the engine, crates/core/defuse, keeps its own format and lints)
    # (no fmt check here: the image's rustfmt differs from the pinned nightly and reflows
    # lines the local one keeps; formatting is checked locally, not by the merge gate)
    box cargo clippy --workspace --exclude phitex-defuse --no-deps --all-targets -q -- -D warnings
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
    rm -rf "$w/course" "$w/aux"
    cp -a "$course/src" "$w/course"
    cp -a "$course/aux" "$w/aux"
    status=0
    if [ -n "${ACCL_WS:-}" ]; then
      box "$@" || status=$?
    else
      # (the compiler cache for the command's builds, as gate-shard.sh's
      # cache_on: its server started in the box the command runs in, its
      # counters in $r/sccache-cmd.txt when it stops; PHITEX_SCCACHE
      # emptied, so a command that would start its own, a gate shard,
      # keeps this one)
      box bash -c 'if [ -n "${PHITEX_SCCACHE:-}" ] && "$PHITEX_SCCACHE" --start-server > "$r/sccache-start.txt" 2>&1; then
          export RUSTC_WRAPPER=$PHITEX_SCCACHE CARGO_INCREMENTAL=0 PHITEX_SCCACHE=
        fi
        s=0
        "$@" || s=$?
        [ -z "${RUSTC_WRAPPER:-}" ] || "$RUSTC_WRAPPER" --stop-server > "$r/sccache-cmd.txt" 2>&1
        exit $s' cmd "$@" || status=$?
    fi
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
