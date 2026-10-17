# The final spec: from the redesign to the goal (2026-10-16 night)

Goal (user, 2026-10-16): **amsldoc byte-identical at every thread count,
edits in milliseconds, cold faster than pdflatex, scaling with cores.**

`docs/spec-redesign.md` and `docs/spec-runstate.md` give the graph its
shape: nodes by content, no whole-state values, the page chain off the
finishers, the link by prefix sums, the run on fixed slots. That shape is
necessary and not sufficient. This document is everything else between it
and the goal. It is typed in ONE shot after the redesign merges, on top of
it, by one writer. Every section ends in a finish line that is a number or
a grep; the shot is done when all of them hold, not before, and no section
may be skipped for risk (the branch and the gate are where risk is paid).

No rule in this document or in the code it produces may name a LaTeX
macro, environment, package or file extension (spec-redesign §0.1).

## 0. The bars, as the gate prints them

Reference numbers are recorded, never re-measured per gate: pdfTeX native
para4000 0.734 s and para400 0.217 s on one accl core (pinned, 2026-10-17);
amsldoc with pdflatex converged (latexmk) 3 s. pdfTeX's cost is 288 ns and
about 2,900 Ir per command, 63 Ir per token.

| bar | measure | today (dcb6e93c / 566fb842) | finish line |
|---|---|---|---|
| bytes | md5 vs oracle, amsldoc and para4000, j1/j8/j16/j32, cold AND after an edit history (50 one-word edits at random places, then the PDF equal to a cold build of the final text, at j8 and j16) | amsldoc exact cold at j1/j8/j16 (31935925); edit histories at j8/j16 never run | identical at every thread count, cold and after edit histories, 0 stale reads (PHITEX_CHECK_READS=1), refuted counters reported |
| cold | wall, release, pinned whole cores | para4000 4.71 s at j8; amsldoc 34.6 s at j8 | para4000 < 0.734 s at j8; amsldoc < 3 s at j8 |
| scaling | wall at 1/4/8/16/32 vs busy | j8 = j1 | wall(N) ≤ 1.25 × busy/N + critical-path time, up to 32 |
| edit | one-word edit, release, PDF written and equal to a cold build of the edited text | 0.2 s (mid-doc, edit-tail) | ≤ 16 ms on para4000 and amsldoc, of which fixed cost ≤ 10 ms |
| per command | callgrind Ir per token on the LaTeX format run, PHITEX_THREADS=1 | 227 | ≤ 100 |
| rounds | evaluation rounds, cold and edit | 13 cold | ≤ 2 cold, 1 on an edit |
| runs | node runs ÷ live nodes, cold, j8 | 2.8 | ≤ 1.1 |
| critical path | realised, in time | 2,079 nodes on para4000 | ≤ 10% of cold wall at j8 |

The gate prints this table on every head (the `ams` and `para` shards),
using the engine's `Report::round_times` (wall, op, commit, check, idle per
round) for the cold accounting of §1.

## 1. Cold time is accounted, by class, every gate

The gate's ams and para shards print, at j1/j8/j16:

- hint pass (caller thread, before evaluation): wall and Ir;
- per round: `round_times` wall, op, commit, check, idle; nodes run;
- op time by node kind: exec, scan, finish, resume, files, link, package loads;
- the realised critical path in nodes and in time, and its node kinds.

No optimisation in this spec is judged by anything but these lines. A
section whose finish line is a time is done when the line says so.

## 2. The executor: per command at pdfTeX's cost

This finishes `spec-runstate.md` §1–§2 and adds the flat input layer. The
token arena and input `Level` stack exist; what remains is everything on
the per-token and per-command path that is not an array access.

1. **Meaning table.** `Vec<Meaning>` indexed by the control sequence's
   dense index (plus 256 active characters): `Meaning { cmd: u8, flags: u8,
   chr: i32, payload: u32 }`; the payload is the arena range of a macro's
   parameter text and body, or a slot handle. Looking up a token's meaning
   is: index, test LOADED (one predicted branch), read. First touch loads
   from the engine through the existing slot machinery; publish at the end
   compares end values as today. Deleted: the `Native` enum, `Mean` as a
   value on the hot path (`cur_mean()` becomes `cur_macro()` returning
   arena ranges), `mean_loc` translation, spill branches.
2. **No `Result` in the hot path.** get_next, get_token, get_x_token,
   x_token, expand, macro_call, pass_text, scan_toks, the_toks return `()`
   or `bool` and set the run's halt flag; the command loop checks it once
   per command.
3. **macro_call over the arena** (§389–§400): arguments are matched
   straight into a scratch region of the arena; the expansion is written in
   one pass (body with argument ranges copied in) and pushed as a level.
   Deleted: `MacroBufs`, per-call `Vec`s.
4. **Per-command taxes removed:** `state_sig()` per command becomes an
   O(1) mutation counter bumped by every state write; `heartbeat()` per
   command becomes a run-local counter and one branch; `CsId::intern` on
   every control sequence read from a line becomes a direct-mapped cache
   keyed by the hash of the buffer slice (intern once on a miss); catcodes
   below 256 live in a run-local `[u8; 256]` loaded on first use; the
   `\endlinechar` value is cached and invalidated by its slot's write;
   `has_input()` is O(1) (spent levels popped eagerly, §325); the per-line
   `lines` contribution becomes one counter contributed at the end; the
   input stack is a `Vec`, never a `VecDeque`.
5. **Open conditionals cross node boundaries** (decided 2026-10-17): a
   run may stop at a file boundary inside the true branch of open
   conditionals; it defines `cond:stack` (limit, if code, if line per
   entry) like `vlist:pd`, read on first touch by the next piece, where
   `\else`/`\fi` pop it. A run never stops while skipping a false branch.
6. **Frozen control sequences** are reserved dense indices below a
   constant, tested by index range, never by string prefix.
7. **Node boundary.** Entry load (records, first-touch slots) and publish
   (definition delta, `lines:i`, `vprof:i`, effects) stay under 5% of a
   paragraph node's time.

Finish line: grep returns 0 for `MacroBufs`, `state_sig`, `Native::`,
`VecDeque` in the run's input layer, `CsId::intern` inside `scan_cs`;
Ir per token ≤ 100 on the LaTeX format run; para4000 exec op time per
paragraph at j1 ≤ 2 × pdfTeX's whole-document time per paragraph
(0.734 s / 4000 = 184 µs, so ≤ 368 µs, today about 1,200 µs).

## 3. The page side at pdfTeX's cost

Measured on pages-skel (spec-predicted-breaks §8): a finisher costs 3.5 ms
per page, 9.4 µs per item in the fold because it decodes whole line boxes.
pdfTeX ships a page of para4000 in about 0.3 ms.

1. **Lines are native in the finisher.** `lines:j` is decoded once into
   the finisher's native list (the same `Vec<Node>` the run uses); box 255
   is built by moving those nodes, never by re-encoding through engine
   values. The output routine runs on the native executor of §2.
2. **Ship is the page record writer only:** the page's bytes with symbolic
   handles, its object count, byte length and font usage. No document-level
   state is read or written while a page ships.
3. **The scan is per page, over profiles.** `scan(p)` reads the break of
   page p−1 (its end item) and the `vprof:j` of the pieces from there to
   its fire, runs the page builder on profiles only (§980–§1010 arithmetic),
   and defines `breaks:p`. Its cost is nanoseconds per profile item.
4. **Warm builds guess breaks.** `scan(p)` reads `breaks:p−1` as a guess
   from the previous build's value (the guess cache); the engine confirms it
   when the real one settles. On an unchanged prefix all scans run in
   parallel; a moved break re-runs scans from that page until one lands
   where it was. Cold builds have no guess and run the scan chain; its
   total time is what §0's critical-path bar measures.
5. **The output routine's state after page p−1** (page number, marks,
   held-over inserts, the routine's global definitions, page parameters)
   is read by finisher p as a guess from finisher p−1's previous run, or
   from the previous *different* page on a cold build (spec-redesign §1);
   refutations are counted and must be 0 on amsldoc cold.

Finish line: finisher op time per page ≤ 0.5 ms on para4000 at j1; scan
op time per page ≤ 50 µs; realised critical path ≤ 10% of cold wall at j8
on para4000 and amsldoc; finisher refutations 0 on amsldoc cold.

## 4. Rounds: two cold, one on an edit

1. Every field the client knows will be defined later in the same build
   is declared before evaluation: the link's results, `breaks:p`, written
   file units and their order values, and each cross-reference key as it
   is written. A field discovered only while running is declared from the
   run with `Ctx::declare_loopback(f, init)` (engine dae6642), which takes
   effect inside the same evaluation.
2. Round-1 values of loop-back fields come from the guess cache (the
   previous build's values) when one exists; a first-ever build starts
   from their initial values.
3. A round re-runs only the readers of loop-back fields whose value
   changed; never the document.
4. **Cuts are forward definitions, never loop-backs.** Measured on
   redesign2 f53423cd (amsldoc j8): 13 loop rounds, 5,397 runs for 2,100
   live nodes, 931 speculation mispredictions, because the end node
   publishes the cuts (`cut:<node>`) as loop-backs that pieces before it
   read. The finisher of page p, which decides where its page ends inside
   a piece, defines that piece's cut field itself; the resume node of the
   cut reads it forward. No field written by the end node is read by a
   piece.
5. The client pushes its environment once per evaluation and only the
   entries that changed (the engine saw about 184,000 environment calls in
   one amsldoc build).

Finish line: no field defined by the end node is read by an exec, scan or resume node (a check in the census); amsldoc cold ≤ 2 rounds with an empty guess cache and 1 with
a warm one; para4000 1 round; round-2 node runs ≤ the readers of changed
loop-back fields; environment calls per evaluation ≤ the number of changed
entries + 1.

## 5. The hint pass: parallel cold, incremental on an edit

1. **Cold:** chunk boundaries and each chunk's catcode scope at its start
   come from one cheap pass; chunks are analysed in parallel on the
   executor (`Executor::for_each`, no threads outside it, wasm keeps
   working); the nodes are pushed in one batch.
2. **Edit:** the document's analysis is kept as per-unit values; an edit
   re-analyses the changed units only. Slice keys are `u64` hashes, never
   `String`s; the text diff is restricted to the edited range (common
   prefix and suffix).
3. No `format!`, no string keys, no per-slice rescans in the pass; one
   pass over the text, linear in its size.

Finish line: hint pass ≤ 10% of evaluation Ir on para4000 and amsldoc;
cold hint wall ≤ 5% of cold wall at j8; edit hint wall ≤ 1 ms.

## 6. The link: memcpy speed

1. **Cold:** the link reads every page record, computes object bases and
   byte offsets by prefix sums, the font subsets as the union of pages'
   own usage, writes the xref from the prefix sums, and assembles the file
   from the page records' bytes with handles relocated. Relocation and
   compression of independent pages run in parallel on the executor.
2. **Edit, exact mode:** the bytes of the unchanged prefix are copied from
   the previous output; only pages from the first changed record onward
   are relocated and written, unchanged pages' bytes copied with their
   handles shifted by the new base. **Edit, relaxed mode:** only changed
   page objects are rewritten and the xref patched.
3. The DVI path does the same: per-page bytes apart, fonts-defined-before
   by a prefix over per-page font use, assembly by concatenation.

Finish line: link op time ≤ 5% of cold wall at j8; ≤ 2 ms on a one-word
edit in exact mode and ≤ 1 ms relaxed, on para4000.

## 7. The preamble in parallel

Package loads are nodes whose definitions are guesses (spec-redesign §4,
`pkgrec`): the record of a load (read set and definition set, keyed by the
file's content hash, the options and the files it read) is stored beside
the cached format, machine-wide, so any document's first build uses it. A
load without a record runs speculatively (the engine's optimistic policy)
and its record is written after the build. Independent loads run in
parallel; dependent ones in their true order.

Finish line: the amsldoc preamble's critical path in time ≤ 25% of its
busy time at j8 with records present; refutations on records 0.

## 8. Edits: the fixed cost

An edit's wall = fixed cost + the nodes that must re-run. The re-run set
is spec-redesign §0.6 (its piece, scans until a break lands where it was,
the finishers of moved pages, the link). The fixed cost is:

| part | bar |
|---|---|
| text diff and hint pass on the changed units (§5) | ≤ 1 ms |
| environment push (§4.4) | ≤ 0.2 ms |
| engine edit, scheduling and settle bookkeeping (proportional to what ran: engine request below) | ≤ 3 ms |
| link (§6) | ≤ 2 ms |
| writing the file | ≤ 2 ms |

Finish line: one-word edit ≤ 16 ms total, fixed part ≤ 10 ms, on para4000
and amsldoc, at j1 and j8, PDF equal to a cold build of the edited text.

## 9. Scaling: the working set per run

Measured (2026-10-16, EPYC 7763, pinned): 4 workers cost +24% per run, and
the cause is shared-L3 pressure, not SMT and not clock. The lever is the
working set of a run:

1. The run's state is the flat tables of §2 and the arena, reset per run;
   nothing per token is allocated on the heap.
2. A run reads engine values only on first touch; it never walks engine
   indexes per command.
3. Benchmarks run on whole cores: accl `--hint=nomultithread`, pinned.

Finish line: per-run busy at 8 workers ≤ 1.15 × per-run busy at 1 worker
on para4000; the §0 scaling bar.

## 9b. The parallel shape: nothing waits that could have guessed

Measured on redesign2 f53423cd, para4000 cold (gate 12285):

| | j1 | j8 |
|---|---|---|
| evaluation round wall | 3.91 s | 4.38 s |
| op bodies summed (exec 1.13 s, finish 0.83 s, scan 0.18 s, dvi 0.02 s at j1) | 2.16 s | 3.58 s |
| raises (a run hit a field not yet defined and parked) | 0 | 30,254 |
| pending waits / gated starts | 0 / 0 | 18,469 / 31,628 |
| waits | none | 60–77 nodes waiting on one finisher, for most pages |
| realised critical path | 0.86 s | 1.16 s |

Two causes; both are bars of this shot:

1. **Nodes start before their inputs.** Every field a node cannot run
   without is in its `Operation::reads()`, so the engine's non-running
   gate holds it; a node that must run on a value not yet defined reads
   the definer's prediction (the engine's guessed read), never parks on
   it. Pieces after a page's fire read that page's finisher's outgoing
   state (page number, marks, routine definitions, page parameters) as
   a prediction from the finisher's previous run or the previous
   different page; only the link and the resume of a cut read finishers
   for real.
2. **Engine and boundary overhead per run.** Round wall minus op bodies
   is 1.75 s over 11,791 runs at j1 (~150 µs per run). The gate prints
   `round_times` (op, commit, check, idle) to split engine from client;
   the client's share (entry loads, value encoding at publish) is cut by
   §2.6; the engine's share is an engine request (§10).

Finish line: raises ≤ 1% of runs and pending waits ≤ 1% of runs at j8 and
j16; no node other than the link and cut resumes waits on a finisher
(the "waited on" lines of the trace list none); round wall minus op time
≤ 10 µs per run at j1; para4000 wall at j8 ≤ busy/8 + critical path.

## 10. Engine requests (engine vocabulary only, to IncEngine)

1. Per-evaluation bookkeeping (statistics, guess sets, settle) costs in
   proportion to the nodes that ran, not to the graph's size.
2. A reader of a guessed value is re-run when the definer commits a
   different value, and counted (dae6642; confirmed by our gate).
3. Value storage with locality per worker, so a run's reads stay in its
   core's cache (the measured +24% per run at 4 workers).
4. Scheduling and commit overhead per run ≤ 10 µs (measured: ~150 µs per run of round wall beyond op bodies at one worker on 11,791 runs, split by `round_times`).
5. `round_times` printed by our gate; any round whose idle time exceeds
   its op time is reported to IncEngine with the recording.

## 11. Deletions (same commit as the code that replaces them)

`MacroBufs`; `state_sig`; `Native` and `Mean`-as-value on the hot path;
`VecDeque` input; per-command `heartbeat`; per-line `lines` contribution;
string slice keys in the hint pass; whole-file rewrite in the link on an
edit; the finisher's re-encoding of line boxes. Each has a grep in its
section's finish line.

## 12. How the shot is run

1. Type every section, all of it, before running anything. Read code only
   to know where new code plugs in.
2. `cargo check` clean, clippy `--workspace --exclude defuse --all-targets
   -D warnings`, fmt with the pinned nightly, commit, push origin.
3. One gate-fast round. The report is the §0 table with every number
   against its finish line, failures first, then the §1 accounting.
4. Reds are fixed in one pass after the gate, never during the shot.
