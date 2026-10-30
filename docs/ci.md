# CI and releases

The rule (the user, 2026-10-30): **main never breaks.** dev may break
(merge early, fix forward); main moves only to a commit that passed every
check below. A release is a tag on main.

## Lanes

Everything that decides a merge to main runs on **GitHub's hosted
runners** (public repository: free; 4 vCPUs, 16 GB, 6 h a job, 20 jobs at
once). The clusters (mc, accl) are for development and measurement only;
no merge waits on them.

| lane | workflow | runs on | what |
|---|---|---|---|
| checks | `checks.yml` | every push to dev and main, every pull request | fmt, clippy -D warnings, the wasm builds, the viewer's bundle, types and tests, the unit tests (every crate's but phitex-live's; no TeX Live on the runner) |
| gate | `gate.yml` | every pull request to main, and by hand | the shards below, in the CI image, a job each |
| image | `ci-image.yml` | by hand, when the image changes | builds the CI image and pushes it to GHCR |
| release | `release.yml` | a tag `v1.6`, `v1.61`, … on main | binaries, `SHA256SUMS`, the minisign signature, the GitHub release |

**The CI image:** `ghcr.io/accl-kaust/phitex-ci:<snapshot>`.
- It is `scripts/accl/phitex.def` as a Dockerfile (`ci/Dockerfile`): Arch
  Linux pinned to the Arch Linux Archive snapshot 2026/09/25, TeX Live
  2026.1 as those packages, and the repository's Rust toolchain. Nothing
  in it is downloaded at gate time.
- The oracles (TeX Live's own builds) come from the same pinned TeX Live,
  so a gate on GitHub compares with the same bytes as the clusters do.
- The manual corpus's documentation tree (`texlive-doc`, 3.1 GB) is in a
  second tag (`…-doc`) that only the corpus shards pull.

**The gate's shards:** `gate.yml`, a matrix job each, the commands of
`scripts/accl/gate-shard.sh` without the cluster's staging.

| shard | what |
|---|---|
| build | the release CLI and the workspace's test binaries, passed to the other shards as an artifact |
| suites | phitex-tex's pdftex, edits, pages, plain, etex, hints and inputs suites |
| latex 1..4 | the latex target's tests, dealt out |
| tests | every other test binary: each crate's integration tests (the CLI's, on TeX Live, one at a time) and phitex-live's unit tests |
| ams | amsldoc `--exact-pdf` at -j1, -j4 and -j16, md5 against the oracle; its edit histories at -j4 |
| xetex | the XeTeX corpus, and Grail's cold build byte-identical |
| corpus 1..8 | the manual corpus, byte-identical against TeX Live (the `-doc` image) |
| wasm | the engine as wasm32-wasip1-threads, its documents |
| shelf | a clean cache, `--shelf` against the release this commit builds in, as served from v1.shelf.phitex.org: the release check's documents identical |

**Speed is not gated.** Hosted runners are shared and their timings are
noise. Thread counts above the runner's 4 cores still check exactness.
Speed bars are measured on mc and reported, never a merge condition.

**Safety:** documents are untrusted code. Every engine and tool run has
shell escape off, kpathsea's paranoid file access, CPU and wall-clock
limits and no home (`scripts/ci/common.py`). The workflows have no
secrets except release.yml's signing key, a read-only token except
ci-image.yml's package write, and no `pull_request_target`: a pull
request's code never runs with privileges.

## Merging to main

1. **A pull request dev → main** on GitHub. Nobody pushes to main directly.
2. **checks runs** by itself on the pull request.
3. **gate runs** by itself on the pull request; its last job, `gate`,
   is green only when every shard is.
4. **GitHub's branch protection on main** requires:
   - a pull request;
   - `checks` and `gate` green on its head commit;
   - the branch up to date with main;
   - no force push and no deletion;
   - all of this for admins too.
5. **Merging is a fast-forward** of main to the pull request's head, so the
   commit that passed is the commit on main. Then main goes to origin
   (Forgejo, its main protected the same way: no force push).

A red gate is fixed on dev, and the pull request picks up the new head;
there is no override.

## Releasing

1. **Tag `v<version>` on main**: `v1.6` (the alpha), then `v1.61`,
   `v1.618`, …, a digit a release (AGENTS.md). The tag's commit must
   carry a green `gate`. The release workflow checks this and stops
   otherwise.
2. **`.github/workflows/release.yml`** builds the CLI for x86_64 and
   aarch64 Linux and for macOS (arm64, x86_64) and writes `SHA256SUMS`.
   Its last job runs in the GitHub environment `release`, which:
   - holds the secret key (`MINISIGN_SECRET_KEY`; the public key is
     `ci/minisign.pub`, key D15F554581F5B4BF, also in install.sh);
   - deploys only from `v*` tags;
   - waits for the user's approval of the run.
   It signs `SHA256SUMS`, adds GitHub's build provenance for each tarball
   (`gh attestation verify phitex-….tar.gz -R accl-kaust/PhiTeX`), and makes
   the GitHub release with the tarballs, `SHA256SUMS` and
   `SHA256SUMS.minisig`.
   **A dry run** is a push to the branch `release-dry` (`git push github
   dev:release-dry`): every step but the release, signed with a key made for
   the run.
3. **`CHANGELOG.md`** gets the version's entry in the same pull request. The
   site (PhiTeX.org) regenerates `news.json` from it at its next deploy
   (docs/spec-distribution.md §4).
4. **Shelf** is released on its own schedule (built on mc, deployed by
   `deploy.sh`). A PhiTeX release names the Shelf release it builds in
   (`shelf::RELEASE`), and the gate's Shelf check runs against that
   release as served.
