# HANDOFF: pure SSA (branch `pure-ssa`)

This branch holds the TeX layer on the TeX-agnostic φ core (`crates/phi`), behind
`PARTEX_SSA=1 PHITEX_SSA_PURE=1`. It is a one-shot rewrite of SSA mode: every
command of `main_control` is one step of a φ unfold, engine state is either core
*names* or the step *state* (`PState`), and outputs are effects linked at the end.
The design notes are DESIGN.md §3.17 (checkpoint 1, approved).

## Where it stands (2026-10-09)

Done and byte-identical (pure vs plain partex, same binary):
- `-ini` test document, cold build and 2 rebuilds (edits).
- e2e incremental cases with plain format: `bye`, `verbatim`, `pagetail`, cold
  build and every edit stage.
- A synthetic plain document (400 paragraphs, 30.5K steps, PDF): identical at
  W=1 (1.05 s) and **W=4 (3.0 s, slower: see "Next")**.
- φ change `fee9137` (`StepCx::source_or_insert`, `StepCx::spelling`, toy tests),
  which the φ core agent is landing on main.

Failing or deferred (by the coordinator, until after the first parallel speedup):
- `effects` (stage 1+ log), `readback` (a file the job writes and then `\input`s
  by lines: stores are names, but a stored file read by lines needs a call over
  the store, half done in `finish`), `cutoff_pdf` (rebuild PDF), the DVI cases
  `incremental`, `cutoff`, `tokens` (they differ even cold; DVI output through
  effects, `Shared::page_sink` returns None).
- LaTeX: not tried yet.

**The last commit is untested**: the page builder after a paragraph's end or start
is now a step of its own (`set_defer_page(true)` in `pure/driver.rs`, `T::PURE`
in `build.rs` new_graf and in `maincontrol.rs` after the deferred `build_page`).
Rebuild, then rerun the W=1 cases and the W=4 synthetic document.

## Architecture as built

- `crates/partex-core/src/pure/`
  - `tracker.rs` `PureTracker`: logs each command's slot reads (with the
    version seen), writes, group open and close (close after unsave's restores),
    file ends, loads (`pure_load`), and the job's written files (stores). It also
    sorts slot families into Name, State or Interner (`kind`). Level 0's list
    fields (`List` slot < COUNT) are names. File handles are named by their files
    (`reopen`).
  - `version.rs`: a slot's version, matching the read hooks.
  - `state.rs` `PState`: the input state, nest levels 1+, the save stack, the
    conditionals, align_state, after_token. Its version is computed from the
    engine. Level 0 is kept from the engine on `set`.
  - `lang.rs` `TexLang` (`phi::Lang`):
    - Step: if the engine is not in `st`, place it (`ps.set`). Then make the
      **frontier**: the engine's arrays are the run's frontier, loaded with
      `defined_since(last)` or else `defined_reaching()`. Not recorded.
    - Run one command (`resume`).
    - Replay the log: reads are registered with `cx.read` (a mismatch is a hard
      **enforcer** error), definitions go at each slot's last live write (global
      when the slot's level is 1), groups are replayed.
    - Loads become sources (`source_or_insert`). An `\input` level is
      `cx.call(Op::File(h, level))` plus `Step::Call`.
    - The step consumes the lines of its own file, and puts its effects on
      chain 0 as `Op::Fx` leaves.
    - The step key is hashed from the cursor, the input place, and k.
  - `files.rs` `FileDoc` / `DOCS`: each file's lines and identities; the
    driver refreshes them between runs (prefix and suffix line ids kept).
  - `driver.rs` `Build`: runs the engine to the first command (format load).
    The first file is the **root unfold** `Op::File(h, level|ROOT)` with init
    passed as `args[0]`. Also `refresh`, `run`, `effects`, `history`, and
    `enable_workers`, which installs a factory of worker engines: copy-on-write
    `fork_with` views of the format snapshot.
  - `shared.rs` `Shared<H>`: one host behind a mutex, used by all engines.
  - `file_entries`: speculative entries after each blank line. The guess is the
    file's first state with the blank line read (`InputState::after_blank_line`)
    and is canonicalised by placing it in the engine.
- `crates/partex-cli/src/pure.rs`: the cold build, the link (`effects::link`
  plus `SsaLinker::write_full`), and one rebuild per `PARTEX_SSA_REBUILD` line
  (`refresh` then `run`).
- `scripts/sandbox` now passes switches whose values hold newlines.
- Debug switches:
  - `PHITEX_PURE_DEBUG=1`: each step's access log.
  - `PHITEX_PURE_SPEC=1`: each speculative guess compared with the state the run
    in order made there.
- Dry-step rounds are off (`round_min_ns = u64::MAX`), because a worker would
  run steps out of order on a stale frontier.

## Findings behind the next steps

`PHITEX_PURE_SPEC=1` on the synthetic document shows all 401 guesses right
(equal state versions). The W=4 slowness is therefore not guesses but **reads of
names across paragraphs**:
- every `\par` step ran `build_page` and read and wrote the page builder's names;
- `\prevdepth` (level 0) chains each paragraph's first line to the previous one.

When grafted, a segment's reads of those names come from the graph at the
speculation's start, so the core wakes them and the main thread reruns nearly
everything ("woken 2.65M").

## Approved design for the next steps (user, via the coordinator)

- No single-threaded main executor. Every worker executes segments and
  validates them against the field-level reads each segment made, checked
  against the definitions reaching its entry. Only a thin in-order commit
  stays sequential: it stamps and grafts, a few hash compares per segment.
- A failed validation reruns from the first step that read the mismatching
  field (Block-STM style), not the whole segment. A later provisional
  validation is redone only if an earlier commit changes what it read.
- The page builder is its own resumable, convergent scan, never in a
  paragraph's entry state and never read by a paragraph's steps.
- Order:
  1. Read-set acceptance, so the main thread commits more than it executes
     and W=4 beats W=1.
  2. Remove main-as-executor: workers pull entries, run and validate them;
     the committer only stamps.
  3. Bring back parallel rounds for rebuilds.
- Report the first speedup and the top fields that differ at rejected entries.

## Next concrete steps

1. Build and test the page-step change (W=1 cases, then the W=4 synthetic
   document). Count main-thread reruns. Then make entries sparser
   (every N blank lines) and look at what the grafted segments' first steps read.
2. `\prevdepth`: the first line's interline glue must not chain paragraphs.
   Either a paragraph's line-break step does not read level 0's `prev_depth`
   (the page step joins), or accept one rerun per segment.
3. Read-set acceptance and workers that validate (needs the φ core: the
   coordinator's φ agent owns graft and acceptance; coordinate with it).
4. The φ agent's queue: stale slot, landing fee9137, `defined_since` across
   call boundaries (today every file call or return falls back to
   `defined_reaching`), a content-hash interner (cs spelling -> p by hash,
   versions hashing spellings: required for W>1 with new control sequences),
   the parallel graft, the e2e harness for PURE, enforcer coverage.
5. Then the deferred cases above, LaTeX (latexdoc, the course, pgfsub), the
   carry-over checklist (DESIGN 3.17 / older HANDOFF), the memory budget
   (course ≤ 8 GB peak, ≤ 2 GB after sealing), memoised macro expansion
   (required), timings at W=1/4/8 on the cluster.

## Building and running without the cluster or local paths

Toolchain: the pinned nightly in `rust-toolchain.toml` (rustup installs it).
Locally, every cargo or binary run goes through `scripts/sandbox` (bubblewrap).
Where bubblewrap is missing (a cloud box), set `SANDBOX=` (empty) for the
`scripts/pure/*` scripts and run cargo directly.

Packages (Debian/Ubuntu):

    apt install build-essential pkg-config bubblewrap python3 \
      texlive-base texlive-binaries texlive-fonts-recommended \
      texlive-latex-base texlive-latex-recommended

What the tests read from TeX Live: `plain.tex` and hyphen.tex (texlive-base),
cm `*.tfm` and the AMS Type 1 `cmr10.pfb` and friends
(`fonts/type1/public/amsfonts/cm`), `pdftex.map`, kpathsea's `texmf.cnf`.
partex finds the tree through its own kpathsea (`partex-kpse`). If the tree is
not at the system path, set `TEXMFCNF`/`TEXMF` as kpathsea expects.

Build (fastdev profile: release speed, thin LTO):

    scripts/sandbox cargo build --profile fastdev -p partex-cli   # target/fastdev/phitex
    scripts/sandbox cargo test -p phi --release                   # the core's tests

Formats (once). The scripts expect these directories under `target/x`:

    mkdir -p target/x/fmtp target/x/fmtk
    (cd target/x/fmtp && ../../../scripts/sandbox ../../fastdev/phitex --compat=pdftex -ini -interaction=nonstopmode '\input plain \dump')
    (cd target/x/fmtk && ../../../scripts/sandbox ../../fastdev/phitex --compat=tex -ini -interaction=nonstopmode '\input plain \dump')

The comparison scripts (`scripts/pure/`):
- `cmp.sh DIR ARGS`: DIR/src is the job. Runs it plain in DIR/a and pure in
  DIR/b, then compares every output (logs without their first line).
  `PUREENV="PHITEX_SSA_WORKERS=4 PHITEX_PURE_SPEC=1"` adds switches to the pure
  run. `BIN=` sets the binary (default `target/fastdev/phitex`).
- `edit.sh DIR ARGS`: DIR/src plus DIR/edits/1..n. One pure process rebuilds
  after each edit (`PARTEX_SSA_REBUILD`), and each stage is compared with a
  cold plain run. Files are stamped as e2e stamps them, with
  `SOURCE_DATE_EPOCH` fixed.
- `runcase.sh NAME`: lays out xtask's e2e incremental case NAME (parsed from
  `xtask/src/e2e.rs`) in `target/x/e2e/NAME` and runs `edit.sh` with the
  plain format.
- `gen.py N`: a synthetic plain document of N paragraphs.

Examples:

    bash scripts/pure/runcase.sh bye        # also verbatim pagetail effects readback cutoff_pdf incremental cutoff tokens
    mkdir -p target/x/syn/src && scripts/sandbox python3 scripts/pure/gen.py 400 > target/x/syn/src/syn.tex
    cp target/x/fmtp/plain.fmt target/x/syn/src/
    bash scripts/pure/cmp.sh target/x/syn --compat=pdftex -fmt=plain -interaction=nonstopmode syn
    PUREENV="PHITEX_SSA_WORKERS=4" bash scripts/pure/cmp.sh target/x/syn --compat=pdftex -fmt=plain -interaction=nonstopmode syn

The pure run's stderr (`DIR/b/err.txt`) holds the `phitex: pure ssa build N:`
reports: steps, placements, whole vs since frontiers, names loaded, reads,
definitions, groups, file calls, the core's report, and families not versioned.

## Rules (from the task)

- fmt and clippy pedantic clean; `unsafe_code` denied.
- No subagents; no pgrep or pkill.
- No force-push. Push only to the `accl` remote, never to main or github; the
  user lands the work.
- No commit trailers, and no claude.ai links in commits.
- Never commit the course's text: use synthetic tests.
- Heavy local runs go under systemd-run memory caps and `nice`.
- No heuristics standing in for semantics. Nothing may live outside names and
  PState: the enforcer must catch it.
