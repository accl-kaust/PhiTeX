# CI

What remains of the CI is the TeX Live oracle: TeX Live's own documentation
sources (`ci/manuals.json`, the pinned list), each built by TeX Live's engines
to a settled fixpoint in an Apptainer container with no network and shell
escape off (`scripts/ci/manuals.py`, `scripts/ci/common.py`,
`scripts/accl/ci.sbatch`), the outputs kept in a cache on accl by the input
tree's hash. It is the oracle a flow CI will compare `phitex` with (the PDF
and the files the job reads back, not the `.log`). The stages that ran the
machine-mode binary and its modes (the engine suites, perf, the scoreboard,
the backfill, the course) were removed with it; GitHub's runners still run
the cheap checks (`.github/workflows/checks.yml`).

Until a flow CI exists, the gate is `scripts/flow-gate.sh` on the cluster
(phitex-flow's suites at several schedules, each step under a time limit)
plus the CLI's tests (`crates/phitex-cli/tests/cli.rs`: the LaTeX documents
through `latex` and `pdflatex`, pdfTeX's PDF documents, the fixpoint, the
watch), each run as one `scripts/accl/accl submit cmd …` job sized from
measurement.

The oracle stage: `scripts/accl/accl ci-discover` lists the candidates the
CI image's doc tree has (`candidates.json` in the run's directory); `scripts/
accl/accl ci-oracle LIST` builds a list (the candidates, or `ci/manuals.json`)
with TeX Live as a Slurm array of shards into the cache; `python3
scripts/ci/manuals.py pin --list CANDIDATES.json --cache DIR --out
ci/manuals.json` turns the cache's results into the pinned list and the
excluded documents with their reasons.
