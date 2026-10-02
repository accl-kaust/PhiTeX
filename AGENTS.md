# Rules for agents working on partex-PhiTeX

This repository is partex (imported at 21edd4e) with PhiTeX's
`phitex-syntax` and `phitex-ir` crates. Its design is `DESIGN.md`; the
work now is DESIGN 4.3 ("partex-PhiTeX: windows"), which overrides 4.2
where they differ. The rules below the line are partex's and still
hold, except where this section changes them.

- **Branches and commits.** Each agent works on its own branch
  `np/<name>` in its own worktree under `~/code/tmp/np-<name>`, and
  **commits there** (small commits, each passing its tests). The
  coordinator merges into `main`. Merge `main` into your branch when
  the coordinator says it moved; resolve conflicts keeping both sides'
  intent.
- **The gitignored inputs** (`refs/`, `upstream/`) are hard-linked into
  each worktree: never modify a file in them in place.
- **Heavy runs** (the course, anything over a few GB): only through
  `scripts/heavy` (one at a time on the machine, memory capped). The
  user's own `partex watch` holds about 32 GB: never touch it, never
  `pkill`/`killall` by name (kill only PIDs you started), never write
  to `/tmp/tex`.
- **The course** (the 295-page benchmark): a clean copy is
  `~/code/tmp/np-course` (sources, `_out/` with the job's `.aux`,
  `.out` and `.toc`). Never run in it: copy it to
  `~/code/tmp/np-<name>-course` first. Its edits are
  `bench/edits/course.txt`.
- **LOG.md**: append your entry under a dated heading naming your
  agent (`## 2026-10-02 — <title> (agent <name>)`); on a merge
  conflict keep both sides in time order.

---

# partex's rules

Read `DESIGN.md` first: it is the whole design, self-contained;
`git log` and `LOG.md` have the current state. `DESIGN.md` is the only
design document: keep it current and self-contained rather than adding
new ones. `DESIGN_ARCHIVE.md` is the old document, history only: never
extend it or depend on it (DESIGN's appendix A maps its section numbers,
which older code comments cite).

## Working with the coordinator

- An agent works on its own branch, in its own worktree when two
  agents run at once (at most two), one task of about 30 minutes at a
  time, and **never commits**: it leaves the tree ready (formatted,
  tests passing, DESIGN.md and LOG.md updated) and reports. The
  coordinator reviews, commits on the branch with the trailer, merges
  into `master`, runs the gate (and the course sanitizer for machine
  changes) on the merge, and pushes.
- **Ask when unsure.** End the task with the question instead of
  guessing; the coordinator answers and the task continues.
- Never touch the user's running `partex watch` processes or anything
  under `~/Seafile`; stop your own runs with `timeout -s KILL` (a
  signal to the sandbox wrapper does not reach partex inside it).

## DESIGN.md and LOG.md (mandatory, with every change)

- Keep `DESIGN.md` current: when a design changes, or a measurement
  changes what the design should be, update the section in the same
  branch.
- Append to `LOG.md`, a dated record in time order, what you changed,
  discovered, measured and decided, with:
  - commit hashes;
  - numbers with their conditions (document, binary, warm or cold);
  - the reason: what prompted the change, and the alternatives rejected.
- The user will write a book about the design from these two files, so
  write for a reader learning it.
- On a merge conflict in `LOG.md`, keep both sides, in time order.

## Sandbox (mandatory)

- **Run every `cargo` command and all code from the internet inside
  bubblewrap** via `scripts/sandbox [--net] <cmd>`. That includes build
  scripts, l3build and its Lua, anything under `upstream/`, and test runs
  that execute upstream files.
- Network is off by default. Pass `--net` only for fetch steps
  (`scripts/sandbox --net cargo fetch`, `scripts/sandbox --net scripts/fetch-upstream.sh`),
  then build and run offline.
- Never weaken the sandbox, for example by binding `$HOME` or making
  `~/.rustup` or `$CARGO_HOME/bin` writable, without asking the user.

## Compatibility

- The goal is **100% source compatibility**. The oracle is the installed
  TeX Live 2026 binaries (pdfTeX 1.40.29, e-TeX, XeTeX, LuaTeX 1.24), not
  upstream `.tlg` files and not our own judgement.
- Never edit expected outputs, or loosen a comparison, to make a test pass.
  A difference from the oracle is a bug in partex.
- Output has two modes (DESIGN.md 1.2):
  - **exact**: byte-identical to pdfTeX; e2e compares bytes.
  - **fast**: identical content, compressed, packed or numbered freely.
    Its check is `scripts/pdfcheck same OURS.pdf ORACLE.pdf`, and any
    difference it reports is a bug.
- `scripts/pdfcheck uncompressed DOC.tex` checks everything but
  compression byte for byte: both engines, with compression off.
- Port behaviour, not code: `tex.web`, `etex.ch` and `pdftex.web` are
  specifications. Design data structures and algorithms natively (no
  sentinel tokens in lists, linked stacks or mirrored word layouts).
- Cite WEB section numbers for the behaviour you implement (`// §123`).
  `§N` in Rust code always means a tex.web section (`cargo xtask sections`
  counts them); cite pdfTeX's as "pdfTeX §N".
- Do not emulate internal statistics: memory usage, string-pool, hash,
  font-memory, trie and stack counts, `\dump` statistics, TeX's exact
  capacity limits (runaway recursion must still stop with an error, just
  not at tex.web's exact point). They are masked in oracle comparisons (`xtask/src/mask.rs`); don't
  design anything around them.

## Architecture constraints

- Core crates must build for `wasm32-unknown-unknown`. `partex-core` is
  `no_std` + `alloc`; all effects go through the `Host` trait.
- Engine state is accessed only through accessors that call the `Tracker`
  (`DESIGN.md` 3.2, 3.15). Parallelism only through the
  `Executor` trait, with results merged in program order.
- Every IR has a canonical, round-tripping text form (`DESIGN.md`,
  5.3).
- Run `scripts/sandbox cargo xtask check` before committing.
- Every acceleration (memoization, speculation, parallelism, compiled IR)
  must be switchable off, and must produce byte-identical output to
  reference mode.
- Machine mode (DESIGN.md 4.1, being removed): boundaries fall at clean points, the
  page builder is replayed as a fold, and a checkpoint is a set of
  shared roots. Any change to boundaries, replay, the page builder, the
  tracker or checkpoints must pass e2e in machine mode and
  `PARTEX_MACHINE_SANITIZE=1` on e2e `machine_edits` and on the course,
  and must report its number on the five-edit harness (`bench/edits.sh`).
  Nothing in the tracking is keyed on a control sequence's name.
- Dynamic SSA (DESIGN.md 3.1): its first sentence, "a call runs again
  if and only if a version it read changed", is the invariant, and it
  overrides any other line of DESIGN that contradicts it (such a line is
  a bug in DESIGN, fixed there). Any change to the SSA runtime, the
  tracker, the records or the calls reports the course's one-word edit
  rebuilt in the same process: its ms, the calls re-run, the readers
  marked and the reads checked. A rebuild whose checking grows with the
  document rather than with the edit is a bug in the dependency graph,
  fixed before anything else. Never add a mechanism DESIGN does not
  state; write its form into DESIGN first.

## Quality

- `cargo fmt`, `cargo clippy` (pedantic, workspace lints) clean;
  `unsafe_code` is denied unless justified in review.
- Performance claims need a benchmark. Keep `tracing` spans and the
  TeX-level profiler working; record benchmark JSON in `bench/results/`.
- `upstream/` is fetched from pinned commits by `scripts/fetch-upstream.sh`
  and is not committed. To change a pin, update the script and regenerate
  `corpus/manifest.json` with `scripts/sandbox cargo xtask corpus`.
