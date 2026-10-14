# The text and the output routine, sequenced (design note, not built)

Status: shelved. The work in progress is on branch `rt-at-wip`, and none of it has landed.

## The problem

TeX runs the output routine between the text's commands (§1012, §1025).
A field both of them define is one sequence of definitions in document
order: the text's, then the routine's at a fire, then the text's again.
LaTeX's float lists work this way:

- the text does `\@cons\@currlist`, `\@next\@currbox\@freelist`;
- the routine (`\@specialoutput`, `\@makecol`, …) pops them and refills them;
- the `\if@nobreak` flag set in `\@addtocurcol` is read by the next
  paragraph's `\everypar`.

The page chain sees two separate streams:

- a page step reads text fields as "the state before the chain", which is
  the text's final value, not its value at the step's fire point;
- the text reads its own fan-out, which never contains a routine's
  definition (no page step defines anything to the engine).

pgst:j carries the routines' deltas from step to step. The tdefs fold
(`T_DEFS_AT`) lets a later text definition win over an earlier routine
delta.

## What failed: answering only the text's reads (`T_RT_AT`)

`rt-at-wip` marks a document node's read of a field in R (the global names
of `\output`'s closure) and answers it from the fold with the routine
value in force there. This is wrong whenever the routine's value was
computed from the text's future:

- float1, `\@freelist`: the routine re-assigns the final list (already
  popped by the figure), the text's earlier `\@next` pops it again, and
  the list loses one box per round. It diverges.
- figures, `\@currlist`: the routine for float A reads the final "B"
  (from the figure* node) instead of "A".

Both sides have to read at-point values.

## The sequenced fold

1. `T_DEFS_AT` carries `[field, value]` for fields in R; other fields
   stay names only.
2. `FinalState::load` folds, in document order per page part, the text's
   R definitions (from the markers) and the routines' deltas (pgdeltas).
3. pgst:j becomes the R state at fire j+1: the text's value or the
   routine's, whichever is later. Page step j+1 then reads at-point
   values. No step reads another step's definitions except through this
   fold.
4. The same sequenced state answers the text's `T_RT_AT` reads: a node's
   `rtbefore:<node>` loop-back, declared on request like `marks:before`.
5. R is `\output`'s closure from the format's definitions. It needs the
   kernel's INITEX harvest to reach the output routine: editms's lex fix,
   which lands with parlists.

## Rounds: the open problem

The markers now carry R values, so they change whenever the text's R
values change, and every exchange is a loop-back:

- the text's read of `\@currlist` depends on fire k's routine;
- fire k+1's routine depends on what the text defined after that read.

Each round settles one more exchange, so cold rounds grow with the number
of chained float exchanges (floats placed while later floats are queued),
not with the page count.

The bar is ≤ 2 cold rounds (good chains). A document with n interacting
floats would take about n + 2 rounds.

Ways to meet the bar, none chosen:

- The fold replays the text's R operations itself. They are small and
  regular in LaTeX (`\@cons`, `\@next` on box lists, a flag), so the fold
  could apply them in order without rerunning text nodes. But this is
  specific to the format: a learned or declared set of list operations,
  not a hand-written name list.
- Text nodes that touch R run inline in the chain (as `pageinline` does
  for small documents). That makes them sequential, against the
  parallel-pages rule, unless only those few nodes do it.
- Predict the R values at each fire from the previous round, and accept
  extra rounds only where a float interacts.

## Measured with T_RT_AT only (R by a scratch harvest)

- Marker cost: 60–169 reads by 2–4 document nodes on small fixtures,
  409 reads by 103 nodes on 12-book.
- 12-book: 7 loop rounds against dev's 4.
- float1: regressed. figures, float2 and rules: still differ.

The fixtures `float2` (two [t] floats in one node: definition, fire and
read inside one node) and `tablelost` are on the branch.
