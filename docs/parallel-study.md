# How much of a big build can run in parallel: the PGF manual (2026-10-06)

The question: profile a large real document (the TikZ/PGF manual),
measure how much parallelism is available, and say whether "a cheap SSA
pass followed by parallel ones" gets it. This builds on DESIGN 3.10 and
the step-graph work of 2026-10-02/03 (`PARTEX_SSA_DAG`,
`scripts/ssa-parallel.py`, `cargo xtask parallel`), and does not repeat
it.

**Measured vs estimated.** A number marked *(m)* was measured for this
study. *(p)* means measured earlier (the LOG entries of 2026-10-02 and
2026-10-03, quoted here). *(e)* means estimated, with the assumption
given. A model run on a step graph (`xtask parallel`) is a measured
schedule bound on real dependencies (m). Treating it as what an
implementation would get is an estimate.

Setup: local machine with 24 cores, shared with other jobs (load 9–10),
in the `partex.slice` (16 GB cap). Release build without LTO (16
codegen units, frame pointers for perf). Worktree `par-study` on
`d27052b`. The PGF manual is TeX Live's `pgfmanual` (1180 pages),
`.aux` files settled. `pgfsub` is `bench/inputs/pgfsub.tex`: four
chapters (tutorial, shapes, arrows, decorations) through
`\includeonly`, 39.7 M commands, about 10% of the manual's 389.7 M.

## 1. Where a cold build's time goes

### Wall time and memory

| document | engine | wall | user | peak RSS |
|---|---|---:|---:|---:|
| PGF manual (1180 pp.) | pdfTeX, TeX Live 2026 | 339.7 s | 327.7 s | 122 MB (m) |
| PGF manual | partex, plain | 338.9 s | 336.1 s | 201 MB (m) |
| pgfsub (4 chapters) | partex, plain | 33.1 s | | 175 MB (m) |
| pgfsub | partex, SSA, one trip | 75.4 s | | 2.07 GB (m) |
| pgfsub | SSA + step graph dump | 95.0 s | | 3.34 GB (m) |
| synthetic text article, 731 pp. | partex, plain (under perf) | 4.9 s | | (m) |

- partex runs at pdfTeX's speed on one thread (336 s against 328 s
  user time). The 5.6 minutes are TeX's work, not partex overhead.
- The SSA build takes **2.28× the time and 11.8× the memory** of a
  plain build (pgfsub). In a sample of the SSA build, frames that are
  not inlined into the engine (`partex_ssa::*`, `SsaTracker`, the
  fold's `Readers::put`, `note_step_read`) take ≥30% of the samples.
  The rest of the tracker's cost is inlined into the engine's own
  functions (m).

### Profile by phase (perf, frame pointers; `scripts/perf-phases.py`)

Each sample is charged to the outermost frame that names a phase.
Everything else counts as expansion and main control: macro calls,
`get_next`, scanning, assignments, conditionals. Output-routine macros
run from main control, so they count there too.

| phase | PGF manual, plain (67,677 samples) | text article (4,144 samples) |
|---|---:|---:|
| expansion + main control | **98.2%** | 60.8% |
| ship-out + PDF writer (incl. deflate) | 1.30% (deflate 0.5%) | 27.2% (deflate 9.3%) |
| line breaking | 0.21% | 8.1% |
| packing (hpack/vpack) | 0.21% | 1.5% |
| page builder | 0.04% | 1.5% |
| font loading, math lists, alignments, images | < 0.05% | < 0.5% |

The top leaf functions on the manual are `get_next_slow` 9.7%,
`macro_call` 9.5%, `get_token` 7.0%, `get_x_token` 6.8%,
`end_token_list` 6.2%, `expand_nonmacro` 4.4%, `bulk_group_run` 4.0%,
`back_input_from` 3.4% and `id_lookup` 2.8% (m). The PGF manual is
macro expansion (pgfmath, keys, TikZ parsing) almost entirely.

**Consequences.**
- *Pipeline parallelism* puts ship-out, the PDF writer, deflate and
  fonts on other threads while expansion stays on one. On the manual
  that is at most **1.02×** (Amdahl on 1.8%, m). On text-heavy
  documents it is worth up to about **1.37×** (27% ship-out/PDF, e).
  Adding line breaking reaches about **1.6×** (38%), but line breaking
  feeds back into expansion (`\prevgraf`, `\lastbox`, box dimensions),
  so it is not free.
- *A pass 0 that skips typesetting* still costs 98.2% of the manual's
  build, so it cannot pay off there (section 3). On text it costs about
  61%.
- Speed "beyond belief" has to come from **running expansion itself in
  parallel**, which needs predicted entry states (carries).

## 2. The available parallelism (the step graph)

### Without speculation (p, the whole manual, 2026-10-03)

236,040 steps, 389.7 M commands, 164 M reads from outside a step, 64.5 M
definitions.

| model | whole steps | pipelined |
|---|---:|---:|
| all reads | 1.00× | 1.00× |
| blind writes (a definition that does not read the old value does not wait) | 1.55× | 2.89× |
| blind writes, only the page builder's edges kept | 20.77× | |

Every step reads something its predecessor wrote. The culprits are
TeX's cursors: `align_state`, `cond`, `save.ptr`/`level`, `selector`,
`str_ptr`, `hash_high`, plus LaTeX's per-step scratch (`\reserved@a`,
`\@let@token`, `\@currenvir`) saved inside the `document` group.
Removing classes one after another (pipelined, blind writes) gives:
macros 6.4×, registers 10.3×, parameters 18.7×, save stack 106×, hash
330×, codes 2,156×. The data flow is wide. The chain comes from state
that TeX keeps in one place.

What serializes it, by family (p, plus this study's pgfsub graph):
- **Not the page builder.** With all other carries free, the
  typesetting classes alone (page builder, nest, PDF writer and fonts,
  marks, `\write` streams) leave a phase-2 critical path of 44,592
  commands out of 39.7 M on pgfsub: **890×**, pipelined, blind writes
  (m). Under soft reads the chain through `nest`/`list.mode` limits it
  to 2.94× (m). `\count0` and page numbers are not the serial part.
- **Cursors and LaTeX scratch**: the save stack, conditionals,
  `align_state`, `\reserved@a`, `\@currenvir`. Scoped definitions
  (blind writes) and soft reads (TODO 2) remove most of them.
- **Allocators**: `str_ptr`, `hash_used`/`hash_high` (a new control
  sequence), font numbers, PDF object numbers. These are positional
  names. They need naming by content (DESIGN 3.10, "versions that are
  content").
- **The `.aux`/`\write` streams**: few reads (3.2 K on pgfsub), but at
  an offset that chains.
- **NFSS and catcode state**: `\f@size`, `\f@encoding`, `\catcode13`,
  `\catcode64`, set inside code examples and pictures (see section 3).

### With predicted carries (p)

| predictor | what validates | bound |
|---|---|---:|
| the setup's end (each chapter from the preamble's state) | 1 of 379 segments, 11,296 of 203,021 steps | rounds: segments 0.03×, steps 1.04× |
| setup's end, classes predicted greedily (steps, blind writes) | macros 1.94×, page builder 2.09×, parameters 12.4×, PDF+fonts 20.1×, counters 25.7×, codes 48.4×, registers 428× | |
| **the previous build's records** (each step from its entry state last time) | **236,037 of 236,040 steps** | **93,450×** (work / largest step) |

The steps are now windows of at most 4,096 commands. On pgfsub the
largest step is 4,096 of 39.7 M commands, a cost-only bound of 9,688×
(m). Splitting the large figures into windows (DESIGN 4.3) is done.

## 3. "A cheap SSA pass followed by parallel ones"

Scheme: **pass 0** runs sequentially and skips some of the work. At
each step boundary it records the entry state it saw as the predicted
carry. **Phase 2** runs, on workers, every skipped step and every step
pass 0 ran from a mispredicted state, each from pass 0's carry. Steps
commit in order, and a step whose reads validate is kept. Phase 2 is
pipelined: a mispredicted read waits for its definition.

`cargo xtask parallel pass0 DAG` (new, commit `4b82547`) models this on
a step graph. A read is mispredicted when its definition comes from a
skipped or tainted step **and** has a different version from the last
definition that pass 0 ran correctly (versions are content: 1,563 of
2,419 definitions of `\catcode13` share one version). A misread of a
typesetting class taints only the reader's typesetting classes. Any
other misread taints the reader entirely, and taint spreads through
later reads. Projections use Brent's range: pass 0 time + max(cp, w/P)
up to pass 0 time + w/P + cp.

### pgfsub, measured on its step graph (m; the scheme itself is e)

Pictures (from a read of `\pgfpicture` to one of `\endpgfpicture`,
outermost) cover 82.5% of the commands. Code examples cover 61.2%.
Output-routine steps cover 0.4%.

| pass 0 skips | pass 0 cost | re-run (share of commands) | phase-2 critical path | speedup, unbounded workers | 8 workers | 64 workers |
|---|---:|---:|---:|---:|---:|---:|
| *soft reads* | | | | | | |
| typesetting (output routines; typesetting classes mispredicted) | 98.2% (measured profile) | 99.6% | 39.3 M | 0.51× | 0.48–0.51× | 0.50–0.51× |
| pictures | 17.5% | 16.3% | 39.2 M | 0.86× | 0.78–0.86× | 0.85–0.86× |
| pictures + code examples | 4.2% | 3.0% | 39.2 M | 0.97× | 0.87–0.97× | 0.96–0.97× |
| *blind writes* | | | | | | |
| typesetting | 98.2% | 99.6% | 23.7 M | 0.63× | 0.59–0.63× | 0.63× |
| pictures | 17.5% | 16.3% | 23.9 M | 1.29× | 1.11–1.29× | 1.26–1.29× |
| code examples | 38.8% | 20.2% | 20.0 M | 1.12× | 1.00–1.12× | 1.10–1.12× |
| pictures + code examples | 4.2% | 3.0% | 23.9 M | 1.55× | 1.30–1.55× | 1.51–1.55× |
| *blind writes, cursors and allocators taken as predicted* (save stack, conditionals, engine scalars, nest, hash, allocators) | | | | | | |
| typesetting | 98.2% | 98.4% | 17.7 M | 0.70× | 0.64–0.70× | 0.69–0.70× |
| pictures | 17.5% | 16.3% | 17.6 M | 1.62× | 1.35–1.62× | 1.58–1.62× |
| pictures + code examples | 4.2% | 3.0% | 17.6 M | **2.06×** | 1.64–2.06× | 2.00–2.06× |

How to read it:
- **Pass 0 can be cheap.** Jumping over pictures and code examples
  leaves 4.2% of the commands, and only 3% of the steps pass 0 runs
  need running again.
- **Phase 2 is still a chain**, about 45–60% of the work at this grain.
  After the cursors, what remains misread is catcodes (`\catcode13`,
  `92`, `123`, `125`, `32`: the code examples' verbatim reading), NFSS
  (`\f@size`, `\f@encoding`, `\size@update`), `\par`, macros and
  parameters. Part of this is real: consecutive windows of one picture
  depend on each other, and pictures inherit the code example's
  catcodes. Part is the grain: a step is a window of ≤4,096 commands
  cut by count, so a span's first and last windows also hold text
  outside it, and skipping them throws away work that pass 0 could do
  exactly. The model is therefore pessimistic for a design with nodes
  at clean points (`\begin`/`\end` of the environment, top-level
  paragraph ends), which the step graph cannot express.
- **Skipping typesetting does not pay on this document.** Its pass 0
  is the whole build (98.2%). With free carries, typesetting alone
  allows 890× (section 2), but nothing makes those carries cheap
  except doing the expansion.

**Projections with the previous build's carries (e).** Assume 99.999%
validate (p), a largest step of 4,096 commands, workers running tracked
code (2.28× plain, m on pgfsub), a validation cost of 40 ns per read
(first-read `note_read`, p) done in parallel, and an in-order commit of
60 ns per definition done sequentially. On the whole manual (plain
336 s; SSA about 770 s, e by the pgfsub ratio; 164 M reads, 64.5 M
definitions): validation adds 6.6 s of work, and commit adds 3.9 s on
the critical path. The commit alone caps speedup at about 86× against
plain.

| workers | 8 | 16 | 32 | 64 |
|---|---:|---:|---:|---:|
| time (770 s/P + 6.6 s/P + 3.9 s) | 101 s | 52 s | 28 s | 16 s |
| against the plain build (336 s) | 3.3× | 6.4× | 12× | 21× |
| against the SSA build (770 s) | 7.6× | 15× | 27× | 48× |

This covers every later build: an edit session, the second and later
latexmk trips (the first trip's records predict the second's carries),
and a rebuild after a large edit. The first cold build of a new
document has no previous build to predict from.

**Memory (e).** Today's shared state is one engine (about 200 MB) plus
the SSA records (2.07 GB for pgfsub, about 11× plain). A per-worker
engine copy ("forking") costs about 200 MB per worker, 12.8 GB at 64
workers: this is what blew memory before. A per-worker copy-on-write
view over the journaled tables (`JVec`: eqtb, its objects, hash, save
stack; 512-word chunks of 4 KB) costs the chunks a step writes. Steps
average 103 definitions (8.86 M over 86 K steps, m), so the view is at
most about 0.4 MB per worker, about 26 MB at 64. A per-worker *log*
(reads, writes, effects of a ≤4,096-command step) is of the same order.
**The CoW view is the "per-worker copy-on-write chunk view" that was
proposed and never ruled on. It needs the user's decision** (see the
design below). A multi-version store read by timestamp, as in DESIGN
3.10, has no per-worker copy at all, but every accessor must change.

## 4. What blocks it in the code today

1. **One working copy of the state.** `Tex` has 260 fields mutated in
   place (eqtb, hash, save stack and the string pool as flat vectors;
   the nest, conditional stack and input stack as `Vec`s). A rebuild
   runs a step by *placing* its predicted reads into those arrays and
   *putting them back* afterwards (`run_step`, `go_cold`). Two steps
   cannot run at once on one `Tex`. DESIGN 3.10 asks that reads resolve
   through the definition index at the step's timestamp, with no
   working copy. That is not built.
2. **The recorder is single-threaded.** `SsaTracker { rec:
   RefCell<Recorder>, stamps: Cell… }` is `!Sync`, so a worker cannot
   note reads into it. Each worker needs a private log that the commit
   publishes, as in DESIGN 3.10 "Per worker".
3. **`Executor` fits phase 2 as is.** `map(items, f)` with results in
   input order is "run these steps from their carries, return their
   logs". The in-order commit and validation then run on the calling
   thread. The closure must be `Fn + Sync` over `Send` items, so a
   step must run against `&` shared state plus an owned per-worker
   shell. That is item 1 again.
4. **Positional allocators** (`str_ptr`, `hash_used`/`hash_high`, font
   and PDF object numbers) and **restores that make new versions**
   (TODO 2, `\@savsf`) fail validations that should pass (p: `label`
   edit, 4 → 146 of 247 steps validate with soft reads).
5. **Step boundaries are command-count windows**, not the clean
   top-level points of the paragraph-level DFG. A carry is cleanest at
   a paragraph end or an environment boundary at brace depth 0, where
   the cursors (save stack, conditionals, `align_state`, nest) are known
   values. Windows cut mid-group carry every cursor.
6. A related existing piece: the old machine-mode region runtime
   (`partex-incr::rounds`, `Holes::Speculate`) already runs regions in
   parallel from guessed entries over `JVec` checkpoints. That is the
   forked-state design the user ruled out, but its sweep/validate/round
   logic and `Threads` executor can be reused.

## 5. Recommended design

Speculation from predicted carries, at clean top-level points, validated
at commit (DESIGN 3.10 and the user's paragraph-level DFG). No engine
forking: one shared, immutable-once-published state; per worker, only a
shell (cursors, input stack, nest, scratch) and a log.

- **Carry source, in order of value:**
  1. **the previous build's records.** Measured 99.999% valid. This
     serves every rebuild and every latexmk trip after the first, and
     needs no pass 0.
  2. **a skeleton pass 0** for a cold first build. It jumps over
     self-contained spans whose body is grabbed as tokens (pictures,
     code examples, floats) and takes each span's exports to be its
     entry state. It is cheap (4.2% of the commands on pgfsub), but at
     today's grain phase 2 is bounded at 1.3–2.1×.
  3. not "pass 0 = expansion without typesetting": on expansion-bound
     documents it is the whole build.
- **Freeze point as a parameter.** A run may freeze at any point it
  chooses (LaTeX: `\begin{document}`, opt-in for the Overleaf
  extension). The engine assumes nothing about LaTeX. A freeze point
  is only the first clean point where carries are taken.
- **Pipeline as a side dish**: ship-out, PDF and deflate on their own
  thread. ≤1.37× on text-heavy documents (e), ≤1.02× on the manual
  (m).

**Risks.**
- Validation false negatives from non-content versions (restores,
  allocators) make carries look mispredicted when they are right.
- The serial commit (about 1.2% of plain on the manual, e) caps the
  speedup near 86×.
- A cold first build stays mostly sequential until stage 3 is shown
  to work at clean grain.
- Per-worker shells need every accessor to take an explicit state view.
  This is a large mechanical change (260 fields).
- Memory of the SSA records (11× plain) is already the larger cost,
  more than any per-worker state.

## 6. Staged plan

| stage | what | expected | effort |
|---|---|---|---|
| 0 (done) | the measurements here; `xtask parallel pass0`; `scripts/perf-phases.py` | | |
| 1 | **Content versions for restores and allocators** (TODO 2's soft reads; hash/string/font/object names by content). Re-measure the warm and pass-0 models. | Validation that should hold does hold: `label` 4 → ≥146 of 247 (p). Pass-0 phase 2 shortens (m after) | days |
| 2 | **Clean boundaries.** Cut steps at top-level paragraph ends and environment `\begin`/`\end` at brace depth 0 (windows only inside, as now). Dump the graph and re-run `pass0`. | Decides whether the skeleton pass 0 beats about 2× | days |
| 3 | **Worker shell + parallel warm re-run.** A step runs against a read-only view (DESIGN 3.10 index, or the JVec CoW view *if the user accepts it*) with a private shell and log. A rebuild's dirty steps, and a latexmk trip after the first, run through `Executor::map` from their recorded carries and commit in order with validation. **Smallest step with a real measured speedup**: the second trip of the manual. | 3.3× at 8 workers, 6.4× at 16, 12× at 32 against plain (e) | 2–4 weeks |
| 4 | **Ship-out/PDF/deflate pipeline thread** for plain and SSA builds. | ≤1.37× text, ≤1.02× manual (e) | days |
| 5 | **Skeleton pass 0** for cold first builds, on stage 2's boundaries. | Decided by stage 2's measurement; today ≤2.06× (m, window grain) | weeks |

## 7. How to reproduce

    # plain build, profile, phases
    perf record -F 197 -g --call-graph fp -o perf.data -- partex --compat=pdftex -fmt=pdflatex pgfmanual
    perf script -i perf.data -F comm,ip,sym --no-inline | scripts/perf-phases.py
    # step graph of pgfsub (95 s, 3.3 GB; dump 0.98 GB)
    PARTEX_SSA=1 PARTEX_SSA_TRIPS=1 PARTEX_SSA_DAG=sub.dag partex --compat=pdftex -fmt=pdflatex pgfsub
    # pass-0 models (about 5 minutes on pgfsub)
    cargo xtask parallel pass0 sub.dag --pass0-cost 0.982
    cargo xtask parallel pass0 sub.dag --pass0-cost 0.982 --ignore "save stack" \
      --ignore conditionals --ignore "engine scalars" --ignore "current list" \
      --ignore "hash table" --ignore allocators

All of it runs under `scripts/sandbox`, in the memory slice.
