# The target graph

What the def-use graph of a document must look like, and the test that
asserts it. Acceptance for every scheduling change is this edge set, not
a chain fraction.

## Nodes

A document node is a **paragraph region**: the text from the end of one
paragraph-ending item to the end of the next (a blank line, `\par`, a
vertical command, a macro whose body ends a paragraph), at brace depth 0
and outside every conditional, as the syntactic pass marks them
(`hints::slices_of`). A region is balanced: its run opens and closes its
own groups, boxes, math and conditionals on its own semantic nest
(`nest.rs`) and leaves nothing to another node. A region begins in the
vertical list of the scope it runs in and ends there; one the pass
closed too early reads on into the following siblings (claimed) until it
is closed: correct, and serial for exactly that span.

Region slicing (`hints::slices_of`): a paragraph is whole; a brace or
bracket argument joins its command (`\centerline{...}`); a keyword or
unit scan joins the next slice (`\vskip 1in \centerline{...}`); a
conditional and a `\begingroup`...`\endgroup` pair stay in one slice. A
slice the pass sees open and close no scope (`hints.rs` `no_scope`)
declares no scope effect, so nothing waits on it for one.

A loading command (`\documentclass`, `\usepackage`, `\RequirePackage`,
`\input`, `\include`) with its arguments is one slice. Its run reads
package-kind files (`.cls`, `.sty`, nested `\RequirePackage`) inline; a
document-kind file it `\input`s is sliced into regions with hints, as
the main text is (`Run::doc_file_regions`).

A **format** (`plain.tex`, `latex.ini` and everything it `\input`s) is
ONE node, run sequentially, its files read in place through the
engine's environment provider (`api.rs` `Graph::provide`). Its hint is
`Any`; its definitions are final before any document node runs, and
its macro bodies bound the hints of document slices that call them. A
group still open at the format's end is an error ("format: group left
open at end"); no node leaves an engine scope open.

Besides the text: one `breaks` node after it (the page builder's fold),
its children the `pagestep`s (one per page, the output routine run
inline), and the `dvi`/PDF writer. So a node is a paragraph region, a
display, an alignment, a page step, `breaks` or the writer; the nest
(`nest.rs`) is local to its run.

Nothing else is a node. Expansion runs inline in the node that reads the
token; a macro body, a conditional, `\the`, `\csname` never become
nodes. There are no token-frame children, no trials, no page-state
cells, no paragraph cells, no condition-level cells.

## What a region's run leaves

- Contributions to the accumulator `items`, tagged with the id of the
  vertical list it began in (0 for the main list): its paragraph's
  lines, interline material and the vertical material around them, in
  document order. The main vertical list is the fold of all regions'
  contributions in document order.
- `\prevgraf` of that list (`pg:<list>`), a definition.
- The definitions it makes outside every group of its own (global ones,
  and locals at its base level), bounded by its hint: the union of the
  bounds of the commands in the region.
- A `T_PAGE_PARAMS` item when it changed `\vsize`, `\maxdepth`,
  `\topskip` or `\holdinginserts`, so the fold uses them from there.

A group opened and closed inside the region restores its locals at its
close (`nest.rs`: saved bindings, globals retained, §283) and nothing of
it reaches the engine.

## Edges allowed

Writing `A <- B` for "A reads what B defined".

| reader | may read | from |
|---|---|---|
| region R | regime cells (`cat:`, `cs:`, `font`, `param:`, `count:`, …) | the last node before R that changed them: a fan-out from the format and the preamble, not a chain |
| `breaks` | `items` (the whole main list), the page parameters, the insert classes | every region (one fan-in) |
| `pagestep` j | its page's material (its argument), `\count0..9`, the insert boxes, the marks, `\output` | `breaks`; the previous steps for what their routines defined globally (page numbers) |
| `dvi` | `pages` | every `pagestep` |
| `breaks` (round 2) | `pagefx:<j>` | what routine j left (loop-back, previous round): held inserts, the leftover list; planned (branch para-vsize): page parameters the routine set |
| region R (planned, branch para-marks) | `marks:before:<N>` | the marks in force before node N (a per-node loop-back from the page chain) |

Forbidden, and asserted by the test:

- any link through a `par:` or `if:` field (there are none);
- a region waiting on another region for anything but a regime cell it
  changed;
- an `items` read by anything but `breaks` or a box end.

## Heights

Critical path: the format's definition chain (cached) + one region (1)
+ `breaks` (1) + the page steps chained through `\count0` (one per
page) + `dvi` (1). The test asserts `height <= 6 + pages` per fixture.

## Why this shape

- Regions are independent of each other: the only thing one region can
  change for a later one is a global definition, which the hint bounds
  and the engine fans out. Width is the number of paragraphs.
- The page builder is arithmetic over the main list; it runs once over
  all of it. The page steps are independent given their material; what
  the routines define globally (page numbers) is a chain of small nodes
  the engine speculates through (`Speculation::Predicted`).
- An edit re-runs one region; `breaks` re-runs on any change of the main
  list (cheap); page steps re-run only where their material changed.
- Per-run working set is the region's own lists, not a scan of every
  contribution since the list opened (the former `list_fold` scan was
  quadratic in document length).

## The test

`chains` prints, per fixture, every link with its cause and the two
nodes' source positions; it fails on any forbidden link above, on any
`par:` or `if:` field, on `height > 6 + pages`, and on an evaluation
with no page step. Bytes identical to pdfTeX remain necessary; they are
not the acceptance.

## Status (measured, accl jobs 8557/8586, tex-paragraph 879f11a8/951dafc0)

- Story document: one round 0.210 s at 1 thread; critical path 248 -> 8
  after `no_scope`.
- Plain format: 52 ms (release), one node.
- Catcode stops 697 -> 20; attempts per run 3.9 -> 2.1.
- Pages: height 22 for 4 pages, still a chain (census pending).

## Not yet

- An output routine that leaves material or changes `\vsize` costs a
  second round (loop-back `pagefx:<j>`); a cold prediction for the page
  starts (the engine's prediction source for a field) would let page
  steps start before `breaks` finishes.
- `\pagegoal` read from the text answers the empty page's values.
