# Continuations as fields, placements relocated (the design, 2026-10-28)

Status: decided by the user on 2026-10-28 ("Start"), the rewrite after the
state model (docs/spec-state-model.md). This file is its spec. Branch
`conts`; tests `crates/engines/tex/tests/cuts.rs` and
`crates/core/defuse/tests/relocate.rs`.

## 1. The defect

Measured on course3 at one thread (tip 7f67945ad, 95 page fires, 388 live
rest-of-piece nodes): 1248 rest-of-piece placements over a cold build,
finishers placed 334 times and run 1446 times (15 each), finisher keys
changing 303 times although 213 distinct keys exist for 95 fires; at
eight threads 2917 rest-of-piece runs, 640 finisher placements. Every one
of these is the same mechanism:

1. **A rest-of-piece node's identity is its state.** A piece cut at
   candidate k places its rest (`Resume`) with arguments `[cont, flags,
   piece, claim guess, k]`, where `cont` is the piece's whole run state at
   the cut (`Run::cont_encode`: input left, nest, conditionals, groups,
   hidden base values, counters) and `claim guess` what the rest will
   take. The engine keeps a placement only when (op, args) match the last
   run's (`commit::match_placements`), so any change in the piece's state
   at k, from a counter, a page parameter, a macro, makes a new node.
   Everything placed after it (the finishers of the pages firing at k,
   keyed through the piece's chain) is then remade too.
2. **A kept placement whose place moves is remade.** The commit keeps a
   matched placement only where it still stands (the kept check: anchor,
   slot, floor, stranded); when the finishers firing at k change, the
   rest's place moves and the engine removes the node and makes a new
   one with a new id, inheriting the old record's definitions as
   predictions but not its identity, so every key chained through it
   changes.

The cut itself is not the defect: a cut is how the text is divided into
nodes at the candidates a page may fire at, and no page decides it.

## 2. The model

### 2.1 Identity

A rest-of-piece node is identified by the piece and the candidate:
arguments `[flags, piece, k]`. Nothing in its arguments depends on the
piece's run state.

### 2.2 The continuation is a field

At the cut, the piece defines `cont:<piece>:<k>` (Global) with the
continuation value it encodes today (`cont_encode`'s value), and
`claim:<piece>:<k>` with the claim guess (or the two in one field). The
rest reads them at its start (`Ctx::read`), claims from the guess, and
runs as today (`cont_load`). A change of the piece's state at k is then
a definition that changed, which re-asks the rest, which reruns in
place: no new node, no new key, nothing after it remade.

A finisher of a page firing at k reads the piece's check data as today
(`contribute_units` with the candidate's reads and defs, `give_way`): its
inputs are unchanged by this spec.

### 2.3 Placements are relocated, not remade

Engine (`crates/core/defuse/src/commit.rs`): when `match_placements` finds
the last run's node for a placement (same op and args) but the kept check
finds it no longer standing where the placement wants it (anchor, slot
or floor differ, or stranded), the commit **relocates** the node: under
the segments' locks it unfiles the node's definitions, contributions and
brackets at its old position (readers re-asked as for a removal, through
the existing walk), relabels the node to the new place (segment and
position, as `place` gives a fresh node), files its record's definitions
there as provisional predictions (as `pred.inherit` does for a remade
node today), and marks it dirty so it runs again and its reads are
checked at the new position. Its id, its record and the nodes placed by
it are kept; its placements keep their anchor (the node itself).
`Report::placements_made` counts a relocation as 0 made; a new counter
`Report::placements_moved` counts it. A relocation across units, or of a
node another node's claim holds, is a remake as today (say so in a
comment with the reason).

### 2.4 Keys

Finisher keys (`PageKey`, chained through a cut piece's rest) are unchanged
in form; with 2.1 and 2.3 the chain is stable across a cold build: a key
changes only when the page really fires in another piece.

Measured after 2.3 (the ledger, 2026-10-28): the chain was not stable. On
course3 at one thread the fires moved between neighbouring pieces while
the breaks settled (the loop's key set changed in 64 of 170 runs), 384
finishers were made for 95 pages, and each one made anew unfiled the old
one's definitions, re-asking every finisher after it through the page
counters (count:189 362 wakes, count:0 126). Keys as the fires' ordinals
were measured worse again, relocation or not (a page appearing or
vanishing moves every finisher after it: 2322 relocations, 2046 finisher
runs at one thread). So a key is minted where the page first fires (the
piece and a number past the last run's keys in it) and **kept by the
page's start** (`Checkpoint::start`, `LoopKeys`): a page fired again from
the same first item takes the key it had, wherever it fires now, and 2.3
relocates its finisher after the other piece.

## 3. What is deleted

- In `Resume`'s arguments: `cont` and the claim guess.
- In `fold_here_`: the encoding of the continuation into arguments; it
  becomes the definition of the two fields.
- In `commit.rs`: the path that remakes a matched placement that moved
  (replaced by relocation); `pred.inherit` for that case stays for the
  cases that remain remakes.

Kept: cuts at candidates, `Halt::Recut`, `order_after_own_page`,
`CandMark`, `preload_candidate`, `give_way`, the page key chain.

## 4. Bars (course3, fastdev, local, the recipe in latex-status.md; all md5 right)

- Cold build at -j1 and -j8: rest-of-piece placements made == rest-of-piece
  live nodes at the end (every rest placed once), relocations reported;
  finisher placements made <= fires + 10 (95 fires); finisher runs per
  node under 6 at -j1 (from 15.2).
- runs/node: course3 -j1 under 2.2 (from 2.90), -j8 under 4 (from 5.13);
  course1 -j1 under 1.3 (from 1.45).
- Edits (the state agent's recipe, -j8, 10 edits): course1 paragraph
  insert's history PDF equals the cold build of the final text (today it
  differs: 7c0e2f90 vs 087de4ae on the same history on both 1cfe9e0a9 and
  7f67945ad; find the cause, it may be this mechanism or not, and fix it
  if it is a stale continuation); amsldoc and course1 word edits unchanged
  (8.7 ms, 58 ms).
- tests/cuts.rs and tests/relocate.rs green; the engine suite green but
  the load-timing tests; the tex suites on one accl gate at the end.

## 5. Tests

`crates/core/defuse/tests/relocate.rs`:
- `a_moved_placement_keeps_its_node_and_its_placements`: a placer whose run
  places key X after child A (run 1) and after child B (run 2, after an
  edit of the placer's text): X is the same NodeId, runs again once, its
  definitions stand at the new place, a reader of them after B is
  re-asked once, `placements_moved == 1`, `placements_made == 0` for X.
- `a_relocated_node_s_own_placements_follow_it`: X places Y; after X moves,
  Y is the same node, after X.
- `a_relocation_re_asks_the_readers_at_the_old_place`: a reader between the
  old and the new place that read X's definition is re-asked and answers
  from before X.

`crates/engines/tex/tests/cuts.rs` (plain TeX, the `doc()` of tests/state.rs):
- `a_rest_of_a_piece_is_keyed_by_the_piece_and_the_candidate`: a
  multi-paragraph group (`\begingroup` … paragraphs … `\endgroup`) cut at
  its paragraph ends: `placements_of("resume") == nodes_by_op("resume")`
  after a cold build at -j1 and -j8.
- `a_changed_continuation_reruns_the_rest_in_place`: a word edit in the
  first paragraph of the group (its state at the cut changes: a counter
  stepped in it): the rest reruns, `placements_made("resume") == 0`.
- `a_moved_page_fire_relocates_the_rest_and_keeps_the_finisher`: a paragraph
  inserted before the group moves the page fire from one candidate of the
  group to another: `placements_made("finish") <= 1`,
  `placements_moved >= 1`, output equals the cold build.
- `a_paragraph_insert_history_equals_the_cold_build`: a document with an
  environment, a `\mark`, a deferred `\write` and `\count0` typeset in the
  headline: insert at the front, then a word edit, then a second insert;
  after each, the output equals a cold build of the text (the course1
  defect's shape, small).
