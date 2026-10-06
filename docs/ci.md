# CI

What runs, where, and how to read and maintain it. The design is in
DESIGN.md; this is the operating manual of the checks around it.

## What runs where

| where | what | when |
|---|---|---|
| GitHub Actions (`.github/workflows/checks.yml`) | fmt, clippy (with and without `trace`), the wasm32 check of the core crates, `cargo test --workspace` (tests that need TeX Live skip themselves) | each push to main, each pull request |
| accl, `scripts/accl/accl submit gate` | the gate: fmt, clippy, `cargo xtask check` (trip, etrip, e2e, ssa-edits, plain and machine mode) | before a change is reported done (AGENTS.md) |
| accl, `scripts/accl/accl ci` | everything below, for one commit, as Slurm jobs | each main commit (see "Running it on every main commit"), or by hand |

`accl ci` submits five stages (`scripts/accl/ci.sbatch`):

1. **build** (`build`, 24 CPUs): the release binary and partex's own
   `pdflatex` and `xelatex` formats (`scripts/ci/formats.py`), and the
   format `partex build` keeps for itself.
2. **manuals** (a 48-task array on `build`, 2 CPUs and 16 GB each, at most
   28 at once): the manual corpus, each task a shard of `ci/manuals.json`
   (`scripts/ci/manuals.py run`).
3. **torture** (`build`, 24 CPUs): trip and etrip (plain and machine mode),
   e2e (plain and machine mode), ssa-edits, the l3build suites, the escape
   tests (`scripts/ci/torture.py`).
4. **perf** (pinned to `acclnode04`, 16 CPUs; the other stages keep off it):
   the measured runs (`scripts/ci/perf.py`).
5. **score** (`light`): the scoreboard, the comparison with the last main
   run, the perf history and its report (`scripts/ci/score.py`,
   `scripts/ci/report.py`). It runs no document.

The CI stays under half of the `build4` quota (192 CPUs and 787 GB a
user, shared with the gate): at most 96 CPUs at once (torture 24, perf 16,
manuals 28 x 2), and every CI job is submitted with `--nice=1000`, below
the gate.

Everything runs in the CI image `partex-ci-2026-09-25`
(`scripts/accl/partex-ci.def`: the partex image plus `texlive-doc` and the
language collections, from the same Arch Linux Archive snapshot).

## Running it

    scripts/accl/accl ci [--main] [REV]     # prints: RUN SCORE-JOB
    scripts/accl/accl ci-wait RUN [SCORE-JOB]

`ci` snapshots the tree (or takes `REV`), pushes it to accl, syncs the
harness (`scripts/ci/`, `ci/`) to `~/code/flinner/partex-phitex-ci/harness`
there, and submits the stages. `--main` marks a run of a main commit: its
scoreboard becomes the next run's baseline. `ci-wait` waits for the score
job and copies the run (without binaries) to
`~/code/flinner/partex-phitex-runs/ci-RUN/` here, with `report.html` and
`history.jsonl` beside it; it prints `summary.txt`.

On accl everything lives under `~/code/flinner/partex-phitex-ci/`:
`runs/RUN/` (each stage's results, `scoreboard.{json,md}`,
`summary.txt`), `oracle/IMAGE/` (TeX Live's outputs, cached), `main/`
(the main runs' scoreboards; `latest.json` is the baseline),
`history.d/` and `history.jsonl` (perf), `report.html`.

Other commands: `accl ci-discover` (the manual candidates of the image,
into `runs/discover/candidates.json`), `accl ci-oracle LIST` (TeX Live's
builds of a list, into the cache; the list is kept in `lists/`),
`accl ci-backfill N REV...` (the perf history of older commits, N at a
time on the perf node). Environment: `CI_SHARDS` (the array's last index,
47), `CI_MODES` (`plain,build,machine,ssa`), `CI_PERF_NODE`,
`CI_IMAGE`.

## The manual corpus

The manuals are TeX Live 2026.1's own documentation sources
(`/usr/share/doc/texlive` in the image, svn r78408, the same revision as
the installed packages). `manuals.py discover` takes, for each package
directory under `doc/latex`, `doc/xelatex` and `doc/generic`, its main
document (a file with `\documentclass` and `\begin{document}`, ranked by
name: the package's name, `doc`, `manual`, `guide`; examples and tests
last), plus the manuals listed in `FORCED`. Each gets an engine: a
`% !TEX program` or arara line, else `xelatex` for fontspec, polyglossia,
xeCJK, unicode-math and the like, `lualatex` for LuaTeX packages
(excluded: partex has no LuaTeX), else `pdflatex`.

TeX Live then builds each candidate (`ci-oracle`): its directory copied to
`/w/run/ID`, the engine run with `-no-shell-escape` in a latexmk-like loop
until no file the job wrote changes (at most 8 passes), with BibTeX on
each `.aux` that names a database when its citations change, biber when
the `.bcf` changes, makeindex on each changed `.idx` (with the
directory's own `.ist` of that name), and glossaries' `makeindex -s`.
`manuals.py pin` writes `ci/manuals.json`: the documents TeX Live builds
and settles, each with its input tree's SHA-256, its engine, the tools
it needs, its passes; and the excluded ones with the reason (TeX Live
fails, does not settle, times out, needs LuaTeX or shell escape).

Each CI run checks the hashes (a changed tree is an error, never a silent
new oracle), takes TeX Live's outputs from the cache, and builds each
manual with partex in four modes, each from a fresh copy in the same
directory:

| mode | how |
|---|---|
| `plain` | `partex --compat=tex -engine=pdftex|xetex -fmt=…` in the same loop; BibTeX and makeindex as `partex -bibtex`, `partex -makeindex`; biber TeX Live's |
| `build` | `partex build --no-machine` (one process to the fixpoint, its tools in process); pdflatex documents needing only BibTeX and makeindex |
| `machine` | `partex build` (machine mode, its store under the scratch directory); the same documents |
| `ssa` | one `PARTEX_SSA=1` process, in trips to its fixpoint; documents needing only BibTeX and makeindex |

Compared byte for byte: the PDF and every file the job wrote (the `.aux`
files, `.toc`, `.out`, `.bbl`, `.bcf`, `.idx`, `.ind`, `.nav`, …), a file
on one side only included. Not compared: `.log`, `.blg`, `.ilg`, `.glg`,
`.fls`, SyncTeX, terminal output (AGENTS.md: the log need not match).
Both sides run with `SOURCE_DATE_EPOCH=1758800000`, `FORCE_SOURCE_DATE=1`,
`TZ=UTC`, `LANG=C.UTF-8`, and for XeTeX `scripts/xetex/fonts.conf` (TeX
Live's fonts only).

## Reading the scoreboard

`scoreboard.md` has a row per suite: `manuals-ENGINE/MODE` (a case per
manual), `manuals-ENGINE/oracle` (excluded or broken inputs), `trip`,
`trip-machine`, `etrip`, `etrip-machine`, `e2e`, `e2e-machine`,
`ssa-edits`, `l3build-latex2e`, `l3build-latex3`, `security`.

A manual's mode is `identical`, `differs`, `error` (no PDF, or a panic),
`timeout` (a pass over 20 minutes or a mode over 90), or `n/a` (the mode
cannot build it: a XeTeX document in `partex build`, a biber document in
SSA mode). For `differs` the table gives the **first differing object**
(`scripts/ci/pdfdiff.py`): both PDFs in qpdf's QDF form, walked object by
object; the first that differs, by its object number in TeX Live's file,
its `/Type`, its page when QDF says, and the line within it
(`manuals/ID.json` has both lines). "bytes differ, content the same" means
compression, object streams or numbering. When the PDFs are the same and
a feedback file is not, the files are named instead.

The run fails (`summary.txt`, `scoreboard.json`'s `result`) when a case
fails that `ci/known-failing.txt` does not list, when a case that passed
in the last main run does not, or on a perf regression (below).

## Known failures

`ci/known-failing.txt` lists the cases that fail today, one per line:

    manuals-pdflatex/ssa  latex_memoir_memman   # SSA mode: \foo, since 2026-10-06
    l3build-latex2e       latex2e/base/testfiles/*-pdf-*   # PDF tests: ...

(suite and id as the scoreboard prints them; fnmatch patterns allowed). A
listed case that fails is reported, not fatal; one that passes is listed
under "Fixed": remove its line. A case that passed on main and fails now
is a regression even when listed. To start, copy the failing cases of a
main run's `scoreboard.json` into the file with a reason each.

## Baselines

- The scoreboard's baseline is `main/latest.json`, written by every
  `--main` run. To reset it (after a deliberate change of the corpus or
  the image), run `accl ci --main` on the main commit.
- The oracle's outputs are cached by the input tree's hash and the
  harness version (`HARNESS_VERSION` in `manuals.py`): bump the version
  when the loop or the comparison changes, and the next run rebuilds them.
- A new image (`CI_IMAGE`) has its own cache; the pinned hashes in
  `ci/manuals.json` then usually change: `accl ci-discover`, `accl
  ci-oracle runs/discover/candidates.json`, then `manuals.py pin` (in the
  image) and commit the new `ci/manuals.json`.

## Performance

`perf.py` builds each document of `DOCS` (amsldoc, usrguide, the beamer
user guide, memman, pgfmanual, polyglossia under XeTeX, and the course
where it is on accl) 3 to 5 times on `acclnode04`, and records per
document, as medians with their range and peak RSS:
TeX Live from nothing to its fixpoint (`texlive.cold`), partex plain in
the same loop (`plain.cold`), `partex build --no-machine`
(`build.cold`), `partex build` cold and again unchanged
(`machine.cold`, `machine.warm`), one SSA process to its fixpoint
(`ssa.wall`, `ssa.cold_ms`) and the word edit rebuilt in it
(`ssa.edit_ms`), one pass over the settled files on each side
(`texlive.pass`, `plain.pass`), and TeX Live's rerun to its fixpoint after
the same edit (`texlive.edit`). Then ssa-edits' cases one at a time, each
case's median rebuild.

Each run's entry (commit, date, subject, node, load, medians and ranges)
is `history.d/RUN.json`; `history.jsonl` is made from them in commit date
order, and `report.html` from it: a self-contained page (inline SVG, no
external file) with, per document, the cold builds, the edit, the settled
pass, peak memory and the partex/TeX Live ratios across commits; hovering
a point shows the commit, date, subject, node and range. A copy of the
history is committed as `bench/results/history.jsonl`.

**Regressions.** Against the last main entry (by commit date, same
node): a median more than 5% worse with the two ranges apart (this run's
fastest slower than the baseline's slowest) fails the run; worse by more
than 5% with overlapping ranges is a warning. Peak RSS counts the same
way. A baseline from another node is compared but never fails.

## Running it on every main commit

Proposed, not installed: a cron entry on `acclhead1` that runs
`scripts/accl/ci-poll` (fetch main from the accl remote, and if its head
has no run in `main/`, submit `ci --main` for it). The head node runs only
`git fetch` and `sbatch`; the jobs run on the compute nodes.

## Security

Documents and packages are untrusted code. The threats: a document that
runs commands (`\write18`, `\input|`, `\openout|`, a package that shells
out), writes outside its directory (`\openout` to an absolute path, a
dot file, `../`), reads secrets (`\input /home/…/.ssh/…`), or reaches the
network; and a result file that injects markup into a report.

The measures:

- **Shell escape off, everywhere.** Every engine run gets
  `-no-shell-escape` (or `partex build --no-shell-escape`) and
  `shell_escape=f` in its environment (kpathsea reads it before
  `texmf.cnf`), TeX Live's and partex's alike. A manual that needs shell
  escape is excluded with that reason, never run with it. partex's
  `\write18` with shell escape off logs `disabled` and runs nothing; its
  pipes (`\input|`, `\openout|`) are web2c's: with shell escape off the
  name is a file name.
- **Writes confined.** `openout_any=p`: no dot files, nothing absolute
  outside `TEXMF_OUTPUT_DIRECTORY`/`TEXMFOUTPUT`, no `../`. partex refuses
  the same names as TeX Live (`Kpse::out_name_ok`, a port of kpathsea's
  `kpse_out_name_ok`), with TeX's "I can't write on file".
- **Reads: confined by the sandbox, not by kpathsea.** `openin_any=p` is
  set, but TeX Live 2026's kpathsea allows any read whatever it says
  (`kpathsea_name_ok`, "as of 2026 … allow anything for reading"), so
  partex, matching it, does too. What a document can read is what the
  container holds: the public image, its own directory and the scratch
  directory. No home directory, no keys, no credentials are mounted.
- **The container.** Every stage runs documents in `apptainer exec
  --containall --cleanenv --no-home --net --network none`, the image
  read-only, only the job's node-local scratch writable (bound at `/w`),
  the harness and the oracle cache read-only; the environment is cleared
  (no `SSH_AUTH_SOCK`, no tokens, no git credentials) and the harness
  passes each run an explicit environment. Each run has a wall-clock
  limit (its process group killed), a CPU-time limit, no core files and a
  file-size limit; each task has Slurm's memory and time limits. Locally
  the same harness runs through `scripts/sandbox` (bubblewrap, no
  network, empty `$HOME`) inside `partex.slice`.
- **Inputs from one place.** Manuals come only from the image's TeX Live
  tree, pinned by the image (an Arch Linux Archive snapshot) and by each
  input tree's SHA-256 in `ci/manuals.json`; a mismatch is an error.
  Nothing is downloaded at CI time: the crates and the upstream test
  suites (pinned by git commit) are fetched once into the shared cache by
  their own steps, outside any container that runs documents.
- **Privileges.** The jobs that run documents have no push rights and no
  deploy keys (the home directory is not in the container); their results
  leave as files in `/w/results`, copied out by the stage script; the
  oracle's new outputs likewise. The score stage, which writes the
  baseline and the history, runs no document and treats every result as
  data: the report and the Markdown escape every file name, log excerpt
  and commit subject. The GitHub workflow has no secrets, `permissions:
  read-all`, and no `pull_request_target`.
- **The escape tests** (`scripts/ci/escape.py`, suite `security`): probe
  documents that run `\write18` (plain, an allowed command, a chained
  one, at shipout), test `\pdfshellescape`, open pipes for reading and
  writing, `\input|`, and `\openout` a dot file, an absolute path, `../`
  and a `..` name, each built by TeX Live (pdflatex with and without the
  flag, xelatex) and by partex (plain with and without the flag,
  machine, SSA, XeTeX, `partex build` with and without the flag). A
  probe passes when no probe file exists anywhere in the scratch tree,
  every `\write18` is logged as not run, `\pdfshellescape` is 0 and each
  write is refused. The sandbox checks: no secret-looking variables, no
  network (TCP and DNS), a file in the host's home invisible, the image
  read-only. Any failure fails the run.

What is not enforced: `partex build` without `--no-shell-escape` and
without `shell_escape=f` runs restricted shell escape, as TeX Live's
default does (`shell_escape = p`); the CI always passes both. Reads are
not restricted beyond the container (above).
