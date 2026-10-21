# The page chain: one scan step per fire, pieces independent of pages (2026-10-21)

Status: agreed design, not built. The user's decision of 2026-10-21: the
alpha ships on this design, not on today's. It replaces the per-cut scan
of `docs/spec-predicted-breaks.md` (S2) and the `scan(i)` row of
`docs/spec-redesign.md` §1; everything else in those documents stands
unless a section below says otherwise.

## 0. Why

TeX weaves typesetting and page building because Knuth's machine held one
page. Nothing in TeX's semantics needs that order: the output is defined
by the values the sequential program computes, not by the order they are
computed in. Three things are truly serial:

1. page p's break depends on where page p−1 ended;
2. the output routine's few globals (page counter, float lists, marks)
   pass from one fire to the next;
3. text that reads the page's own state (`\pagetotal`, `\pagegoal`, …).

Everything else is parallel, and the engine corrects the rare guess that a
fire made wrong.

**What we have today** (dev 72e86b46, `pages.rs` module doc):

- **Pieces already typeset in parallel and never read the page.**
  - An `exec` piece is a source slice. It ends at every page-builder
    candidate it can end at, and the rest of its slice is a `resume` piece.
  - It gives its vertical profile (`VPROF`) and items (`PITEMS`).
- **The chain is the scan.** There is one `scan` step after every cut.
  - para4000 -j8: realised critical path 4321 nodes, nearly all
    scan→scan, about 74 µs of scan op inside a ~0.74 ms engine step. That
    is the 3.0 s wall (job 15453).
  - amsldoc: path 698 nodes, almost all scan; -j8 4.75 s is slower than
    -j1 4.13 s.
- **Why a step per cut** (`docs/note-scan-blocks.md`):
  - A page firing at a cut must be placed by a node whose reads end at the
    cut piece.
  - If the placer also read the rest after the cut, the rest (which reads
    what the routine leaves) would depend on the finisher the placer
    places: a cycle.
  - The block variant (`r5-blk`) broke this rule and is dropped.

**This design:**

- one scan step per fire, reading exactly up to its decision point;
- the scan never creates or splits a piece;
- the chain becomes about two small links per fire, run back to back by
  the engine's lane (defuse ≥ 599e14a, `CLEAN_LANE`).

## 1. Bars (each a test or a reported number)

1. **Exact bytes** at -j1/8/16/32:
   - amsldoc md5 98c96e004c1c82db1e3d562cf0181a3e;
   - para4000 md5 4146793290dc0ea887d79a1d2d1b4c89;
   - the latex corpus and every suite (pdftex, edits, pages, page_steps,
     plain, etex, hints, inputs, defuse_gaps).
2. **Realised critical path ≤ 2·F + 10 links**, where F = fires (pages
   shipped plus forced outputs). para4000 ≈ 692 pages, so ≤ ~1400 against
   4321 today.
3. **Scaling:**
   - para4000 -j8 ≤ 1.0 s release;
   - -j16 and -j32 no slower than -j8;
   - amsldoc -j8 faster than -j1.
   - Reported beside pdflatex on the same node.
4. **Edits:**
   - The hard bar is unchanged: one letter and line re-break ≤ 16 ms at -j8.
   - A one-word edit runs its piece(s), the scan steps from its page until
     a step's decision equals the old one, the finishers of the pages that
     moved, and the link.
5. **Runs per node, cold:** ≤ 1.2.

## 2. Invariants (binding)

1. **No names.**
   - No rule, comment or identifier names a LaTeX/expl3 macro,
     environment, package or file extension. Everything below is stated in
     TeX primitives (register writes, penalties, `build_page` call sites).
   - `grep` is the test.
2. **The scan never creates, cuts or splits a piece.**
   - Pieces come from the source (slices) and from each piece's own run (a
     run that reaches a candidate ends there; its rest is the next piece).
   - Both are independent of where pages break.
3. **Positions come from the source.**
   - Inserting a finish node between two pieces renumbers, moves and
     invalidates nothing (engine: kept nodes keep their results).
   - A piece is rechecked only for a field the inserted node defines or
     contributes to, up to the next shadowing definition.
4. **A scan step reads no piece after its decision point** (§4.3). This is
   what removes the placement cycle.
5. **Guesses are verified.**
   - Every guess comes from evaluation-start state or a verified engine
     read: the previous build's value, or a predicted definition.
   - A wrong guess is a counted rerun, never output.
6. **Switches.**
   - The transition runs behind a compile-time cargo feature (§8), not an
     environment switch.
   - Debug switches sit under `profile`/`observe`.

## 3. Words

| Word | Meaning |
|---|---|
| **candidate** | A point where tex.web's main control calls `build_page` in outer vertical mode: after a paragraph ends, after a box, rule, penalty, insert, mark or `\vadjust` material lands on the main list, after an alignment, at `\par` in vertical mode, at a display's entry (§1145) and at `\end`. It is the only place an output routine can run. |
| **fire** | One run of the output routine. A page fire happens when the page builder decides a break at a candidate. A forced fire happens at a penalty ≤ −10001. Several fires can happen at one candidate, back to back. |
| **decision point d_p** | The candidate at whose `build_page` call fire p happens. The break itself may lie earlier, inside material already contributed; the material after the break goes back to the contribution list. |

## 4. The design

### 4.1 Pieces

- **Slices.** The hint pass slices the source as today, with every
  paragraph end at text depth 0 a cut (the cuts rule of 2026-10-17). Open groups at a
  slice end become engine scopes.
- **Ending at candidates.**
  - A run ends at every candidate it reaches, including candidates inside
    its own groups and its own open conditionals.
  - Today a piece ends inside its own group or conditional only "where the
    page is made to fire", which is a scan-made cut (`split_piece`,
    `check_cut`); that is deleted.
  - The rest (`cont_encode`: input left, nest, open conditionals, groups)
    is the next piece, a `resume`, placed by the run itself.
- **An empty rest is not a node.**
  - para4000 has 4061 resume runs (953 ms) for about 4000 paragraphs.
  - Find what they hold, and make sure no node is made for a rest with no
    material and no state of its own.
- **What a piece reads and gives.**
  - It reads only fields: definitions, registers, records, and the
    main-list tail where its items leave it standing (`MAIN_LAST`).
  - It never reads the scan state or a page.
  - It gives `VPROF` and `PITEMS` units, definitions and `wstream` effects,
    as today.
- **No waiting on pages.** At first no finish node exists, so every piece
  runs in the first wave. When a finish node is inserted before a piece,
  only the fields it defines are rechecked against what the piece read (§2.3).

### 4.2 Finish nodes (one per fire)

- **Identity:** the fire's index, plus the position of its decision piece.
  A moved decision makes a new node; the old one is deleted, and its
  readers are rechecked.
- **Position:** right after the decision piece, in fire order. That is
  where TeX runs the routine. The finisher sees the reaching definitions
  there, which is the state at the fire.
- **Reads:**
  - its break (range and candidate);
  - the items of its range (`PITEMS`);
  - the reaching definitions;
  - the routine's state after fire p−1 (as today, guessed from the
    previous build and verified).
- **Defines:**
  - the routine's globals (`gdefs`: global assignments override pending
    local saves, §283; unchanged);
  - the material it gives back to the main list (`RTN`);
  - the page record for the writer (§6);
  - non-immediate `\write`s expanded with the state at its position.
- **Forced fires:** a penalty ≤ −10001 is a finish node like any other
  (`fire_special` today). It may ship nothing and give back material.

### 4.3 Scan steps (one per fire)

- **Input of scan p:**
  - the material left after break p−1 (the rest of d_{p−1}'s items after
    the break);
  - `RTN` of fire p−1, if fire p−1 exists;
  - the page builder's carried state (inserts held over, `\vsize` and the
    page parameters in force);
  - then the profiles of the pieces after d_{p−1}, in order.
- **Run:** TeX's page builder (§980–§1010) over those profiles, as
  `vprof.rs` does today, until it decides fire p at candidate d_p.
- **Places:** finish p after d_p, and scan p+1.
  - Several fires at one candidate are placed in order, the later scan
    steps reading the earlier finishers' `RTN`.
- **Reads end at d_p.** Scan p reads the profiles of pieces in (d_{p−1}, d_p]
  and nothing after. Because the profiles in that range stop at d_p, no
  piece the scan reads can depend on finish p, so there is no cycle.
- **Read pattern:** an engine question, settled by IncEngine2's test (§9 Q1).
  - **(a)** Grow a folded range read on `VPROF` (`acc_range_to(f, from, to)`)
    a piece at a time until the decision.
  - **(b)** Read a window of W pieces at once and accept a benign extra run.
    A piece after d_p that reads a field finish p defines refutes scan p.
    Scan p reruns and decides the same d_p, so the value is equal and the
    rerun stops there.
  - Prefer (a) if its per-read cost is small. (b) needs the engine to
    tolerate that cycle, which defuse 0ff16b8 converges.
- **Continuations:** the `place_cont` continuation node merges into scan p+1.
  Scan p+1 is that node: it carries the scan state and the leftover material.
- **Page-state readers:**
  - A piece that reads the page builder's state (`\pagetotal`, `\pagegoal`,
    `\pagedepth`, `\pagestretch`…, `\insertpenalties`, `\outputpenalty`,
    `\lastskip`/`\lastpenalty`/`\lastkern` reading `last_glue` and friends
    after a fire) reads a field the scan defines at that candidate.
  - Scan p defines the page state at each candidate of its range as a
    small field, with equal values not republished.
  - Such readers wait on the scan. The dependency is real, and rare (P0
    measures how rare).
- **Guesses:** on edits, scan p+1 may start on `RTN`'s predicted value
  (`rtn_guess`, as today). Cold, the lane makes the wait cheap.

### 4.4 The chain and the lane

- **The chain:** scan p → finish p (it reads the break) and scan p+1 (it
  reads `RTN` and the carried page state). That is at most two links per
  fire.
- **The lane:** with `CLEAN_LANE=1`, when scan p commits, finish p and scan
  p+1 run back to back in one job while the other workers typeset pieces.
- **Order between them:** scan p+1 must read something finish p defines
  (`RTN`, or `gdefs` that change the page parameters) or the two may run in
  either order. When finish p gives nothing back, scan p+1 does not wait
  for it.

### 4.5 Edits

1. The edited piece reruns.
2. If its profile is equal, nothing downstream runs.
3. If it is not, the scan step whose range holds it reruns. Each later step
   reruns until a step's decision point and leftover equal the old (value
   cutoff), then the chain stops.
4. Finishers of moved pages rerun, and the link re-emits their records.
5. Pieces after a moved fire are rechecked only for the fields the old and
   new finish nodes define.

## 5. State between pieces (P3: scope set by P0)

Do this only where P0 shows a chain. Each part is a general rule on
primitives.

1. **Counters as accumulators.**
   - A register write whose new value is the old plus a constant, or a
     constant, contributes an affine map (`set c` or `add c`) to an
     accumulator. This covers `\advance` by a constant, a constant
     assignment, and global and outer-level writes.
   - Every other register write stays a definition.
   - A reader reads the fold of the maps before it.
   - Contributions do not depend on the value read, so a piece that read a
     wrong count still contributes the right map. A cold build converges in
     two waves instead of a chain through every writer.
   - Engine question §9 Q3: a speculative read of a fold whose
     contributors are running.
   - A group's local write is undone at the group's end (the save stack).
     Only writes that outlive the piece are contributions.
2. **Interline glue at a piece's start.**
   - A piece's first box needs `\prevdepth` from the piece before. The
     piece emits the glue symbolically in its profile: the box height, and
     `\baselineskip`, `\lineskip` and `\lineskiplimit` in force.
   - The scan and the finisher compute it from the previous piece's last
     depth.
   - Only a run that reads `\prevdepth` as a value (`\the`, a comparison)
     reads the field.
3. **Fields written by every piece and read by the next.**
   - These are flags reset at every paragraph. The engine guesses them from
     the last value, and a long run of equal writes costs nothing.
   - P0 lists which fields these are and how often the guess is wrong.
     Fix only what that shows.

## 6. The writer interface (P4)

- **Page records:** a finish node gives a self-contained page record:
  - content with symbolic handles;
  - its own object count and byte length;
  - its own glyph usage;
  - the objects it introduces.
  This is as in `docs/spec-redesign.md` §1.
- **The link** numbers objects and byte offsets by prefix sums in fire
  order, and renders only changed records.
- **Writers:** the interface names no output format. pdfTeX's writer
  implements it now. The XDV + xdvipdfmx writer (r7-xetex) implements it
  later and must need no change to §4.
- **Read-time PDF commands:** they stay effects with relocatable handles
  (§1 of spec-redesign).

## 7. Later, separate specs

- **P5 References as a fixpoint:**
  - per-key read-back fields read by their readers;
  - rounds only where latexmk would stop before converging.
- **P6 The preamble:**
  - package records keyed by what they read;
  - independent loads in parallel.

Neither touches §4.

## 8. Migration and gates

1. **P0 (r6-scale, running).** From recordings: pieces that read a field a
   finish node defines, by field. Piece→piece chains by field, and their
   longest. Counter writes by kind. Page-state readers. What the 4061
   para4000 resumes hold.
2. **P1 (IncEngine2).** An engine test of §4.3's pattern, with the lane on
   and off. Settles §9 Q1–Q2.
3. **P2.** §4, behind the compile-time feature `pagechain`.
   - The old per-cut path stays the default until the new one passes
     every bar of §1.1–§1.2 at -j1/8/16/32.
   - Then the old path and the feature are deleted in one commit:
     - the per-cut scan step;
     - `SCAN_BLOCK`/`block_end`;
     - `place_cont`'s node;
     - `split_piece`/`check_cut`'s scan-made cuts;
     - empty-rest resumes.
4. **P3, P4** in that order, each with its own gate. P5/P6 are separate
   specs.
5. **Test bounds.**
   - Node and run bounds in the edits/pages/page_steps suites and the
     critical-path rule encode the old design. They are changed, never
     loosened silently.
   - Each change is reported with the old and new numbers and the reason.
   - Expected outputs are never edited.
6. **Every push:**
   - amsldoc j1 98c96e00 (and 4×j8 once per day of work);
   - the suites touched;
   - critical path and walls j1/j8/j16/j32 for para4000 and amsldoc, on
     one node, interleaved against dev.

## 9. Open questions

| Q | Question | Owner |
|---|---|---|
| Q1 | §4.3 read pattern (a) or (b). The cost of a growing `acc_range_to`, and whether (b)'s cycle is accepted. | IncEngine2, P1 test |
| Q2 | Scan p+1's reads of finish p when the routine gives nothing back: does it still wait? | IncEngine2 |
| Q3 | A speculative read of an accumulator fold whose contributors are running (§5.1), instead of waiting. | IncEngine2, only if P0 shows counter chains |
| Q4 | How many candidates sit inside a piece's own groups or conditionals. Ending at all of them (§4.1) must not make long chains of resumes inside one slice. | r6-scale, P0 |
| Q5 | The log: does `.log` order follow positions? Byte identity covers the PDF only; report differences. | P2 agent |
