#!/bin/bash
# One shard of the gate on GitHub's hosted runners (docs/ci.md,
# .github/workflows/gate.yml): scripts/accl/gate-shard.sh's shard run in
# the CI image ($IMAGE), the checkout at /src, results in out/r,
# workspace out/w; the build shard's store, if any, from out/store (as
# job.sbatch's ACCL_STORE_IN puts it in $w/bin). Exits 0 only on a
# green shard; its red lines printed.
#
#   scripts/ci/gate-gh.sh SHARD [ARGS...]
set -u
if [ "${1:-}" != --inside ]; then
  mkdir -p out
  # (the compiler cache: sccache-action's static binary on GitHub's
  # cache service, given to the shard as PHITEX_SCCACHE, as the clusters
  # give theirs; none when the job did not set it up)
  cache=()
  if [ -n "${SCCACHE_PATH:-}" ]; then
    cache=(-v "$SCCACHE_PATH:/usr/local/bin/sccache:ro" -e PHITEX_SCCACHE=/usr/local/bin/sccache
      -e SCCACHE_GHA_ENABLED=true -e ACTIONS_RESULTS_URL -e ACTIONS_RUNTIME_TOKEN -e ACTIONS_CACHE_URL -e ACTIONS_CACHE_SERVICE_V2)
  fi
  exec docker run --rm -v "$PWD:/src" -w /src -e IMAGE="$IMAGE" -e GATE_TEST_TIMEOUT -e GATE_TEST_PAR "${cache[@]}" \
    "$IMAGE" bash scripts/ci/gate-gh.sh --inside "$@"
fi
shift
export r=/src/out/r w=/src/out/w
mkdir -p "$r" "$w"
if [ -d /src/out/store ]; then
  mkdir -p "$w/bin"
  cp -a /src/out/store/. "$w/bin/"
  for x in "$w"/bin/tests/*; do [ -e "$x" ] && ln -sfn "$x" "$w/bin/"; done
fi
GATE_TIME_PY=$(cat scripts/accl/gate-time.py)
GATE_LATEX_ORDER=$(tr '\n' ' ' < scripts/accl/gate-latex-order.txt)
export GATE_TIME_PY GATE_LATEX_ORDER GATE_EDITKINDS=scripts/accl/gate-editkinds.py
export GATE_TEST_TIMEOUT=${GATE_TEST_TIMEOUT:-300} GATE_TEST_PAR=${GATE_TEST_PAR:-4} CARGO_BUILD_JOBS=4
bash scripts/accl/gate-shard.sh "$@"
st=$(cat "$r/status" 2>/dev/null || echo red)
echo "== $* : $st"
cat "$r/red.txt" 2>/dev/null
# (results readable by the runner's user, which uploads them)
chmod -R a+rX /src/out
[ "$st" = green ]
