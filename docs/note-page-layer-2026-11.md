# The page layer, November 2026: what the text after a page reads, page keys, finisher reruns

Measured on accl between 2026-11-02 00:00 and 07:00. Every number has its
job id. Each section gives the fact, the cause, what was tried with its
cost, and a proposed change.

## 1. Text after a page fire reads the values from before the routine

**Fact.** In TeX the output routine runs at the `build_page` call where
the page fires (§994), so everything the text reads after that point
sees what the routine left: its global definitions, `\count0`, the
registers it steps.

In PhiTeX a piece that passes such a candidate without ending there
reads every field at its own position, which is before the page's
finisher. A piece runs on past a candidate when it can't end there:
inside a group or a conditional of its own (`cuttable_here`: not
`open_span`, or a forced penalty).

- gudoc (latex/gu): pages 10-14 printed 8,9,8,9,10. tabularx's
  `\TX@ckpt` is `\edef`'d from `\the\value{page}` in a float after the
  break, then put back with `\global`. The forced read gave the value
  before the page (`PHITEX_DEF_TRACE=count:0`, accl 22450, 22460).
- classicthesis-xe (corpus, accl 22578, 22646, 22665): `\thepage` and
  LaTeX's `\glb@currsize` at Chapter01's blank lines after a break read
  as they were before the routine.
- A minimal case: paragraphs in a `\begingroup` reading a macro that a
  shipout hook defines globally (oracle/fixtures, `routine_global_after_own_page`,
  accl 22714). The paragraphs after two of the breaks printed the count
  from before the page.

**In dev now (6f4e4af9a).** A read-driven rule. A piece's read of
`\count0`, or of a register the routine defines (`defer::deferrable`),
after a candidate of its own that a page fires at ends the run there
(`Halt::Recut`), and its rest becomes a resume after the finisher. The
registers it held as it passed the candidate are noted
(`Run::page_regs_held`). This fixed gudoc and webquiz, and
classicthesis-xe's probes matched (accl 22532, 22688).

Its limits:
- Registers only.
- Other fields, such as macros, are known only from `Ops::fin_defs_start`,
  which is empty in a build's first evaluation. A generalisation to every
  field a finisher defined was correct from the second evaluation on but
  missed the minimal case, which has one evaluation.

**Tried 1: cf999f531, reverted in d119ef1b5.** At every fresh candidate
the piece could end at and runs on past, the piece read the page builder
over the page so far (`own_page_fire`) and recut where a page fires.

- Correct: classicthesis and classicthesis-xe exact at -j1 and -j4 (accl
  22740), 30 already-identical manuals exact at -j4 (22737), and the
  minimal case identical (22727).
- But it gave every such piece a page-state read, which `page_fold` never
  had. Four work bounds went red:
  - cuts `a_rest_of_a_piece_is_keyed`: 6 rests placed, bound 4
  - edits `moving_breaks_rescan_touched_pages_and_one`: 28 page-builder
    runs, bound 3
  - pages `vsize_document_pages`: 345 runs for 119 nodes
  - pages2 `a_paragraph_insert_refires_the_pages_after_it_once`: 10 loop
    runs for 7 nodes

  All four are green at 0e1e4280a (accl 22737, 22790, 22821).

**Tried 2: snapshot d9c49fb7f, not committed.** The narrow field. The
loop defines `pages:fires:<piece>:<k>` (true) for each page it fires in
a piece's own items. The piece reads only that field, `At::End(unit)`,
at each candidate it could end at and runs on past, and recuts there.

- Correct: the minimal case is identical to pdflatex at -j1 and -j4, and
  the base differs (accl 22877). corpus_repros 72 of 72 (22876).
- Bounds (22876):
  - cuts `a_moved_page_fire`: 6 finishers made, bound 1
  - pages2 `the_loop_parks_at_holes_and_resumes`: 44 loop runs in 15
    nodes, bound 7
  - pages2 `one_loop_node_and_one_finisher_per_page`: 17 loop placements,
    bound 7
  - pages2 `a_paragraph_insert_refires_the_pages_after_it_once`: 27 runs
    of 7 nodes
- Cause of the cost: the piece learns where pages fire only after the
  loop has folded its first, uncut shape. Then it cuts, its resumes are
  made, and the loop's rests after it are placed again. Every page that
  fires inside a group pays one discovery round.

**Proposal.** Keep the read-driven rule (it costs nothing where nothing
is read). Extend its field set from the routine's own *declaration*
instead of from the finishers that already ran. The finisher declares
`may_define` from the routine's hint (`hints::routine_hint`), which is
known before any finisher runs; for LaTeX it is often `Any`. For `Any`,
fall back to the narrow field (tried 2), but only for pieces that read a
field in the routine's hint after an own candidate. Alternatively, accept
tried 2's discovery round and restate the four bounds as "one discovery
round per fire inside a group".

This needs a decision: exactness here costs either a hint-quality
dependency or a round.

## 2. Page keys under moved starts (fixed, 2201bc4b2)

**Fact.** A page's key was the item the page began at, else a fresh
number in the piece it fired in. Text inserted or deleted before a page
moves both. So every page after such an edit got a new key, and its
finisher was a new node:
- cuts `a_moved_page_fire_relocates_the_rest_and_keeps_the_finisher`:
  11-13 finishers made for 8 pages. It had been red since continuations
  became fields (fb6d25e42; bisect accl 22670).

**Change.** The key is the page's place among the document's fires
(`ScanState::j`, `LoopKeys::key`). Where a page kept from the last run
holds that place, it gets a spare key past every place. The finisher is
the same node, asked again with its new page, and its placement moves to
the piece it fires in.

**Measured.**
- Suites (accl 22858, 22863): cuts 5 of 5; pages, pages2, page_steps,
  edits, edit_threads and stale_loop green; corpus_repros green.
- 29 identical manuals plus gudoc, hep-paper and verifiche exact at -j1
  and -j4.
- amsldoc `\cite` edit under watch -j1: 385 → 69 ms, runs 135 → 20
  (accl 22866 → 22865). Grail's did not move (9.3 → 9.6 s, 304 runs), for
  the reasons in §3.

Tried first and dropped: reusing an old key by (piece, nth) (accl 22785).
The moved-fire test stayed red and pages2's loop test broke.

## 3. Finisher rerun triggers and cost

**Grail's 4.4 s finisher** (accl 22851, cold -j1, profile+observe).

What it is:
- The last page's finisher runs hyperxmp's XMP packet:
  41.9 M commands in 4.0-4.4 s.
- `\iterate` runs 6,946,924 times: `\hyxmp@add@to@xml` (hyperxmp.sty
  l.1257) sets `\lccode` for all 65536 characters in a group on each call
  under a Unicode engine, and is called about 106 times.
- Every other finisher takes about 18 ms (40 k commands).

PhiTeX's share is the per-assignment cost of codes past 255: each one is
a numbered spill field. That is the interpreter's area, not this note's.

**Load-order dependency between finishers** (scaling's run log, accl
22666, commit e35b8080a).
- stringenc's tables are loaded globally by whichever routine first
  converts a string (`cs:SE<-pdfdoc@..`, `SE->pdfdoc@..`,
  `ver@se-pdfdoc.def`, `se-utf32be.def`).
- Later finishers read them as undefined ("-") and load them themselves.
- When the pages before the last fired under new keys, each new finisher,
  placed before the last one, loaded the tables first. Its definitions
  (undefined → defined) refuted the last finisher's reads: 199 refutes
  each from 4 new finishers, and the 4.4 s run repeated 11 times per edit.
- On dev a7b868bf7 it already runs once per edit (accl 22866). With
  2201bc4b2 the finishers keep their nodes, so the definer of the tables
  does not change under a moved start.

Any later change to page keys or finisher placement must keep this
dependency exact: the first finisher in TeX's order loads the tables,
and the ones after it read them as defined.

## 4. Unrecorded page-state reads (resume, P2)

Not measured by me. What the page layer gives a finisher that a
checkpoint would also need to record:
- the piece's state at the candidate: locals and the names defined after
  it (`preload_candidate`, check data `[field, standing, value]`);
- lazy values read through the `pitems` window (`fin_lazy_value`);
- the page start's parts (`pages:start:*`).

A checkpoint inside a routine that does not record these reads cannot be
resumed exactly. This needs measurements from the resume side.

## 5. Not page-layer, noted

- elsarticle, els-cas and rvwrite: page 12's header pushes
  `\current@color`, which in TeX is the text's local colour at the break
  (inside elsdoc's vquote). One observe run (accl 22525) wrote it right;
  the release sweeps (22532, 22590) did not. It looks run-order
  dependent. The same family as §1: a break inside the text's group, with
  the routine run on the piece's state at the candidate.
- milsymb: section.4's `/XYZ` destination on its `\begin{landscape}` page
  (misc2's dest-under-`\pdfsetmatrix` family).

## 6. A piece goes on past a candidate: what a routine changed that it read across it

**Rule.** A text piece that goes on past a candidate where a page may fire
(a paragraph's start, a candidate inside groups or conditionals of its own)
defines `pages:used:<piece>`: each field it read across such candidates,
with the runs of candidates it read across. A read crosses candidate `k`
when the value it sees was bound before `k` and read after it. The finisher
of a page firing at candidate `k` compares each field its routine defined
globally with the value it began with (the piece's value at the candidate).
Where one it changed was read across `k`, or a box it used up, or it left
`\aftergroup` tokens, it defines `pages:conflict:<piece>:<k>` with every field
it changed. The piece reads that field at `k` and puts those values in force
there, in one pass (`Run::apply_fire_conflict`, `State::apply_global`). Code:
`State::note_read`, `Run::order_after_fires` (pages/piece.rs),
`Run::define_fire_conflict` (pages/routine.rs).

**What is no conflict.** A value the piece bound itself after the candidate,
before reading it: the routine's value is never seen (`\font@name`, set by
every `\selectfont`; scratch macros and registers such as `\Hy@gtemp` and
skip registers), and the piece's later definition stands over the routine's
(`Run::give_way`). A field the routine changed and the piece did not read
after the candidate. `aftertoken` left as nothing where nothing was pending.
An older binding seen again counts: the group's close restores it and the
next read dates it by its old definition (§279, §283).

**How reads are seen.** One comparison per read in `State`: a slot's last-read
stretch against the candidates passed, on `Run::load` and the meaning paths
(`cmd_chr_fast`, `macro_at`, `mean_of`, `meaning_entry`, `same_macro`,
`cmd_chr_of`), with a step (`State::add`) and a symbol (`defer::symbol_for`)
as reads. Reads from the engine by a way past the table (a skipped branch's
class of a token) are dated from the run's read list at each candidate. Kept
in checkpoints (`Base::read_across`). What a run reads through memos or takes
from markers cannot be dated (`state::read_by_position`: category and other
codes, font cells, `\endlinechar`, page parameters, boxes, math families), so
everything of those it holds counts as read across all its candidates.

**The chain.** A conflict re-asks the text at once (`Ctx::settle_now`). Held to
a quiet point, a chain through the pages took one pass per page: past 12
conflicts the layout did not settle and the output was a pass's that never
settled (texpower's page links one low, run to run). The fixture
`routine_global_read_by_position` has 38 pages of them.

**Tried and dropped.**
- A second pass from the start at the first conflict (`Halt::Recut`, then
  `Halt::Refire`, branch pages3): 2-4x runs, each recut moves the scope's
  definers and wakes their readers (accl 23294, 23331).
- Cutting up front at every such candidate: needs every paragraph's start
  (an engine scope's paragraphs are outer ones). amsldoc then has 4x nodes and
  6x runs, and amsldoc and multicol come out wrong (accl 23130, 23134, 23154).
