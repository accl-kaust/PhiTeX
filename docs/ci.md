# CI

What remains of the CI is the TeX Live oracle: TeX Live's own documentation
sources (`ci/manuals.json`, the pinned list), each built by TeX Live's engines
to a settled fixpoint in an Apptainer container with no network and shell
escape off (`scripts/ci/manuals.py`, `scripts/ci/common.py`,
`scripts/accl/ci.sbatch`), the outputs kept in a cache on accl by the input
tree's hash. It is the oracle a CI will compare `phitex` with (the PDF
and the files the job reads back, not the `.log`). The stages that ran the
machine-mode binary and its modes (the engine suites, perf, the scoreboard,
the backfill, the course) were removed with it; GitHub's runners still run
the cheap checks (`.github/workflows/checks.yml`).

Until that CI exists, the gate is `scripts/accl/accl submit gate` (fmt,
clippy, `cargo xtask check`) plus the tests of phitex-red and the CLI
(`crates/phitex-cli/tests/cli.rs`: the LaTeX documents through `latex` and
`pdflatex`, pdfTeX's PDF documents, the fixpoint, the watch), each run as
one `scripts/accl/accl submit cmd …` job sized from measurement.
