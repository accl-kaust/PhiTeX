# Predicted page breaks: the page chain leaves the critical path

Status: design, 2026-10-15 night. Owner of the design: Phi. Implementation in
slices by an agent against this document; each slice ends with the numbers in
§6.

## 1. The problem, in numbers

para4000 cold on engine c169ee5, 24-CPU box: the same 11 099 runs at 1 and at
16 threads, nothing refuted, and the critical path is 5 894 nodes of the
11 099. Wall 10.4 s at j1, 9.5 s at j16. The chain is, per piece in line order:

    exec(piece k) ──► fold(k) ──► fold(k+1) ──► … ──► finish(page p) ──► place(p) ──► resume ──► exec(piece k+1 after the cut) …

Every fold depends on the previous fold (`vtail`), every finisher on the fold
that fires it, every resume on its finisher (page number, marks, the output
routine's global definitions, page params), and the pieces after a cut on that
resume. The engine replays the recording 4.1x faster at 16 threads with the op
bodies removed, so the engine scales; the shape does not.

The output of a page depends on three things only:

| input of page p | who produces it today | real dependency |
|---|---|---|
| the vertical items that fall on p | the folds in line order | the pieces (parallel) and the break positions |
| the break positions of p−1 and p | the page builder running in every fold | the vertical profile of the items since the last break |
| the state the output routine leaves after p−1 (`count:0`, marks, inserts held over, `gdefs`, page params) | finisher p−1 | finisher p−1, but almost always equal to a cheap guess |

The first two are not sequential in nature. The third is sequential but tiny,
and the engine already has the machinery for it: a guessed read, confirmed or
refuted when the definer settles (the stale-guess fixes of today).

## 2. The target shape

    exec(piece k)  ─ profile:k ─┐
    exec(piece k+1)─ profile:k+1 ┼──► scan (prefix of profiles, runs the page builder on profiles only) ──► breaks:p for every page
    …                           ┘                       │
                                                        ▼
    finish(p) reads breaks:p and the items of its range, guesses the routine state after p−1 ──► page record p (apart, on pdf:pre)
    finish(p+1) … in parallel with finish(p)
                                                        │
    place(p) in line order (cheap: replays the record on pdf:state) ──► end node

Rules:

1. **A piece exposes its vertical profile** as one field, `vprof:k`, written
   by the exec node: the sequence of vertical items it contributes to the main
   list, each reduced to what the page builder (§980–§1010) reads: box
   height/depth, glue (natural, stretch, shrink, orders), kern, penalty, mark
   (position only), insertion (class, height, split parameters), whatsit
   (position only). No characters, no horizontal content. Size: tens of bytes
   per line of the paragraph.
2. **The scan is a fold over profiles only.** It runs the page builder
   verbatim on the profile sequence, with `\vsize`, `\topskip`, `\maxdepth`,
   insert parameters from the state the scan reads, and it emits `breaks:p` for
   each page: the index range of items on the page, the best place and cost,
   the insert splits, the marks that are first/bot on the page. A scan step
   costs what §1000–§1010 cost: nanoseconds per item. The scan is still a
   chain, but a chain of tiny steps; its length in time is the page builder's
   own cost, not the finishers' and not the exec's.
3. **A finisher reads its range and the guess.** `finish(p)` reads
   `breaks:p`, the pieces' vertical lists for its range (the real items, by
   piece id, parallel-readable), and the routine state after p−1 as a *guessed*
   read of finisher p−1's definitions: page number p−1+1, `\topmark` from the
   scan, inserts held over from the scan, `gdefs` as they stood before p−1 ran
   (the usual case: the routine defines the same things every page). It builds
   box 255, runs the output routine on it, and writes the page record apart on
   `pdf:pre`, exactly as today. The engine confirms the guess when p−1 settles
   or refutes it and re-runs p. In a document whose routine is uniform (every
   LaTeX class we target), the refutation count is 0 on a cold build.
4. **The scan state is also a guess.** The scan assumes the routine does not
   change `\vsize`, insert parameters or held-over inserts in a way that moves
   a later break. When finisher p settles with different outgoing page params
   or held-over inserts than the scan assumed, the engine refutes the scan from
   p on; the scan re-runs from that page (its steps are cheap) and the
   finishers after p re-run. That is the misprediction path; its count is a
   reported number (§6) and must be 0 on amsldoc cold.
5. **Resume stays** where the output routine's definitions change the text
   that follows (a cut in the sense of today: the pieces after the cut read the
   routine's `gdefs` through the resume). With predicted breaks the resume reads
   the finisher's definitions as a guess too, so the pieces after the cut run
   before the finisher settles. Nothing after a page waits for the page unless
   the guess is wrong.
6. **Fold nodes disappear from the chain.** `vtail` (the main list tail a
   fold defines) becomes a function of the scan: the items on page p are the
   pieces' lists in the index range `breaks:p`. The fold's other job (own-fold
   on a stopped pass, cut placement) moves to the scan step that emits the
   break.
7. **Place stays sequential and cheap**, as today: it replays the page record
   on `pdf:state` in line order and renumbers objects. It is the only chain
   left, and it is O(record size).

Exactness is unchanged: the page builder decisions are the same code on the
same numbers, the output routine runs on the same box 255, and the PDF is
assembled by the same place nodes. Byte identity with pdfTeX is the gate on
every slice.

## 3. What a guess may and may not cover

| routine output after p−1 | guessed as | refuted when |
|---|---|---|
| `count:0` and other counters the routine steps | previous value + the routine's known step (from the first page's run) | the routine stepped differently |
| `gdefs` (global definitions by the routine) | the definitions it made on the previous settled page | a definition differs in value |
| held-over inserts (footnotes split, floats deferred) | what the scan computed | the routine held different material |
| page params (`\vsize`, `\topskip`, …) for the next page | unchanged | the routine or a piece changed them (then the scan re-runs from here) |
| marks | from the scan positions | never (marks are text, the scan has them exactly) |
| `pdf:pre` base (fonts, forms, objects seen) | the page-base digest as today | as today |

A guess is never applied to text the routine writes to the page itself (the
running head reads `count:0` through the guess and is re-run with the page if
the guess was wrong, so the record is never wrong in the output).

## 4. Slices, measurement first

| slice | what lands | numbers to report |
|---|---|---|
| S0 measure | per-op time totals (exec, fold, finish, place, resume) on amsldoc and para4000 at j1 and j16; share of wall on the chain | the table; the share of fold and finish time on the critical path |
| S1 profiles | `vprof:k` written by exec nodes; a test compares a scan over profiles against the folds' real breaks on the pdftex/pages/latex fixtures and amsldoc (same break positions, same costs) | 0 differences; profile bytes per piece |
| S2 scan | the scan node(s) replacing the page-builder work in folds; folds reduced to list assembly by range; finishers read `breaks:p` | critical path in nodes and time; byte identity |
| S3 guessed routine state | finishers read p−1's definitions as guesses; resume reads as guesses | refutations on cold amsldoc = 0; wall at j1/j4/j8/j16; critical path time |
| S4 edits | an edit in the middle re-runs: its piece, the scan from its page while breaks move (stops as soon as a break lands where it was), the finishers of moved pages, their places | amsldoc one-word edit: nodes, ms; pages re-finished = pages whose breaks moved |

Each slice keeps every suite byte-identical and keeps amsldoc exact at j1 and
j8. A slice that needs a special case per environment or document class is
wrong; the profile and the scan are TeX's own page builder on fewer bytes.

## 5. What is not in scope

The paragraph nodes' own cost (the single-thread per-command speed), the
allocator/contention loss inside op bodies (measured separately with perf;
fixed where the profile points), and the exact object numbering (place stays).

## 6. The bar

amsldoc cold, same PDF at every thread count, release build on accl:

| | j1 | j4 | j8 | j16 |
|---|---|---|---|---|
| today (11800) | 10.6 s | – | 10.7 s | 12.8 s |
| after S3, bar | ≤ today | ≤ 0.35 × j1 | ≤ 0.2 × j1 | ≤ 0.15 × j1 |

para4000: critical path ≤ 1 500 nodes (from 5 894), wall at j16 ≤ 0.2 × j1.
Edit: amsldoc one-word edit stays ≤ 7 nodes + the finishers of pages whose
break moved, ≤ 0.2 s.

## 7. Node identity (finding 2026-10-15 23:40)

Today the fold is spawned with the piece's items and the parent's raw node id
in its arguments, so an edit re-makes the fold instead of re-running it, and
every reader of its `vtail` is dirtied by the rebind even when the value is
identical. In the target shape no node carries content or a node id in its
identity: the scan step and the finisher are identified by position (their
index and keys) and read the pieces' items and profiles through fields the
exec nodes define. A changed piece then re-runs the same scan node, whose
value cutoff stops the chain as soon as the breaks agree.

## 8. S0 findings (2026-10-16 03:40, para4000, pages-skel 924edfaf)

- Exec nodes read neither `ps` nor `vtail` (0 reads). The chain readers are the
  5201 folds and the 692 finishers; the end node reads `ps` once. The realised
  critical path is exactly all folds plus all finishers: 72% of its time is the
  finishers (3.5 ms per page), 26% the folds (166 µs; 9.4 µs per item in
  `fold_items`, which decodes whole line boxes), 1% the end node.
- The page builder runs only in the fold (`PageState::fold_items` →
  `PageBuilder::contribute`, `fire_up` builds box 255 there); the exec only
  places `T_BUILD_PAGE` markers. The fold also resolves page params (`pagepar`)
  in 85% of runs; the exec should resolve them.
- `vtail` is a pure fold over items (tail, sf, pd, since_bp) with no page
  decision in it; readers (`\lastskip`, `\prevdepth`, `\unskip`) wait on the
  page chain for nothing.
- The 2.2x per-op slowdown at 8–16 threads is CPU clock: the chain hops
  between idle, low-P-state cores (schedutil here, ondemand on accl EPYC 7763);
  cycles +20%, busy +96%, LLC misses flat, SMT excluded by a 4c×2t control.
  Fix: run a commit's single woken successor on the same worker (engine), or
  the performance governor.
- On amsldoc ~40% of exec nodes join the chain through `needs_own_fold`
  (`fin_touch` union): `aftertoken` 582, `param:maxdepth` 117, `count:0` 46,
  `param:vsize` 18, `cs:if@newlist` 11. Narrow to value-changing global
  definitions.
- The fold's `ps` value averages 128 KB (max 272 KB, 13 000 sub-values): the
  page so far with full line boxes. The profile form (§2) replaces it.
- Consequence for the slices: S2 (scan) shrinks the chain time by at most a
  quarter; S3 (finishers in parallel on guessed routine state) carries the
  bar. The finisher's own 3.5 ms is being profiled separately.
