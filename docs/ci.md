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
| research-shelf | `research-shelf.yml` | nightly, and by hand; not required | the research corpus built with `--shelf` from Shelf as served, against TeX Live's PDFs (below) |
| release | `release.yml` | a tag `v1.6`, `v1.6.1`, `v1.61`, … on main | binaries, `SHA256SUMS`, the minisign signature, the GitHub release |

**The CI image:** `ghcr.io/accl-kaust/phitex-ci:<snapshot>`.
- It is `scripts/accl/phitex.def` as a Dockerfile (`ci/Dockerfile`): Arch
  Linux pinned to the Arch Linux Archive snapshot 2026/09/25, TeX Live
  2026.1 as those packages, and the repository's Rust toolchain. Nothing
  in it is downloaded at gate time.
- The oracles (TeX Live's own builds) come from the same pinned TeX Live,
  so a gate on GitHub compares with the same bytes as the clusters do.
- The manual corpus's documentation tree (`texlive-doc`, 3.1 GB) is in a
  second tag (`…-doc`) that only the corpus shards pull.

**The gate's jobs:** `gate.yml`, a job each (a matrix job a shard), the
commands of `scripts/accl/gate-shard.sh` without the cluster's staging
(`scripts/ci/gate-gh.sh`). This is what runs; nothing else decides a
merge.

| job | image | what |
|---|---|---|
| build | base | the CLIs (the users' release, and the profile/observe one the checks read) and phitex-tex's test binaries, passed to the others as the `store` artifact |
| build-tests | base | the tests job's binaries, beside the build |
| shard: suites | base | phitex-tex's pdftex, edits, pages, page_steps, plain, etex, hints and inputs suites |
| shard: latex-0..3 | base | the latex target's tests, dealt out (its red corpus fixtures run and reported apart) |
| shard: ams | base | amsldoc `--exact-pdf` at -j1, -j8 and -j16, md5 against TeX Live's build in this image |
| shard: amsedit | base | amsldoc's edit histories at -j4, each edit's PDF against a cold build |
| tests | base | every other test binary: each crate's integration tests (the CLI's, on TeX Live, one at a time) and phitex-live's unit tests; `scripts/accl/gate-known-reds.txt`'s reported apart |
| corpus: research-0..1 | `-doc` | the research corpus (below), byte-identical against TeX Live built in the job |
| corpus: manuals-0..7 | `-doc` | TeX Live's manuals (below), byte-identical against their oracles' pinned md5s |
| wasm | base | `scripts/wasm.sh check`: the engine as wasm32-wasip1-threads (its own build), its documents in Node at -j4 and -j1 against pdfTeX's |
| gate | — | green only when every job above is |

XeTeX is checked by the corpora (the research corpus's XeLaTeX
documents, the manuals' 75 XeLaTeX ones) and by phitex-tex's xetex tests
in the tests job; there is no XeTeX job of its own. Shelf is checked
nightly by `research-shelf.yml` (below), not by the gate: Shelf is
released on its own schedule, and the gate must not go red for a Shelf
release a pull request cannot change.

**TeX Live's manuals** (`ci/manuals.json`, `scripts/ci/manuals.py`): one
main document per TeX Live documentation package and the manuals it must
have (1,173: 1,098 pdfLaTeX, 75 XeLaTeX), each with its input tree's
hash, the engine and tools its oracle needed, and the md5 of the
oracle's PDF (TeX Live's engines to the fixpoint, built on mc from the
same image: `manuals.py oracle` then `pin`). The manuals shards build each
with the users' CLI at -j1 and -j4 (`scripts/ci/sweep.py`) against that
md5; one that differs gets its oracle built in the job for the first
differing object, and an oracle built there whose md5 is not the pinned
one is said (the image is not the one pinned from). Every manual is red
unless identical, but those in `ci/manuals-not-identical.tsv` (its id, a
tab, the cause, as the user decides): they run and are reported apart
(`known-red.txt`), and one identical again is said. On mc, every manual
at once: `CLUSTER=mc SWEEP_ARGS="--engines pdflatex,xelatex --tools
bibtex,biber,makeindex,glossaries" scripts/accl/corpus-sweep`.

**The research corpus** (`ci/research.json`, its files in `ci/research/`):
the templates researchers start from, across fields (IEEEtran, acmart,
llncs, elsarticle, revtex, aastex, mnras, amsart, achemso, apa7,
biblatex-chicago, thesis classes, beamer, …), each TeX Live's own file
copied unchanged with its source and licence in the list, and a few
written for it with the common packages (Times, Helvetica, Palatino,
TikZ, pgfplots, siunitx, algorithms, BibTeX, biber, makeindex,
XeLaTeX's fontspec, polyglossia and xeCJK). Each document is built by
TeX Live's engines to its fixpoint (`scripts/ci/manuals.py`'s oracle,
shell escape off, `SOURCE_DATE_EPOCH` fixed) and by the users' CLI
`--exact-pdf` at -j1 and -j4 (`scripts/ci/sweep.py`); a PDF that differs,
a build that fails or an oracle that fails is red, with the first
differing object (`scripts/ci/pdfdiff.py`). Two lanes, one list:
- **A, the gate** (`gate.yml`'s research shards): kpathsea over the
  image's TeX Live.
- **B, Shelf** (`research-shelf.yml`, nightly, not required): the same
  CLI with `--shelf`, a clean cache, from v1.shelf.phitex.org, against
  TeX Live's own engines building each document from a texmf tree of the
  same release's files alone, their formats made there by fmtutil
  (`shelf/scripts/tl-tree.sh`, shared with Shelf's release check
  `shelf/scripts/check.sh`), never the image's own tree: what Shelf's
  release lacks or PhiTeX reads otherwise from it shows here. A red is
  reported and fixed, never waived.
Shelf's release check can read the same list. On mc: `CLUSTER=mc
scripts/accl/gate-fast research`.

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

1. **Tag `v<version>` on main**: `v1.6` (the alpha), its updates
   `v1.6.1`, `v1.6.2`, …, then the milestones `v1.61`, `v1.618`, … (a
   digit a milestone: AGENTS.md). The tag's commit must
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
   (`shelf::RELEASE`), and research-shelf.yml checks that
   release as served.
