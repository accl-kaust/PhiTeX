# The target graph

What the def-use graph of a document must look like, and the test that
asserts it. Acceptance for every scheduling change is this edge set, not
a chain fraction.

## Nodes

A document node is a **paragraph region**: the text from the end of one
paragraph-ending item to the end of the next (a blank line, `\par`, a
vertical command, a macro whose body ends a paragraph), at brace depth 0
and outside every conditional, as the syntactic pass marks them
(`hints::slices_of`); an environment or a `\begingroup` pair is no bar
to a cut. A region's run opens and closes its own boxes, math and
conditionals on its own semantic nest (`nest.rs`); a group of its own
still open at its end (an environment's `\begin`) becomes an engine
scope the following regions run in, and a `\endgroup`/`\egroup` that
closes the scope it runs in (an environment's `\end`) ends its run
there (engine effect: close), the rest of its input following it
outside the scope. A region begins in the
vertical list of the scope it runs in and ends there; one the pass
closed too early reads on into the following siblings (claimed) until it
is closed: correct, and serial for exactly that span.

Region slicing (`hints::slices_of`): a paragraph is whole; a brace or
bracket argument joins its command (`\centerline{...}`); a keyword or
unit scan joins the next slice (`\vskip 1in \centerline{...}`); a
conditional stays in one slice. A
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
open at end").

Besides the text (`pages.rs`, whose module doc is the spec): a **fold**
per piece with main-list items, placed by the piece after itself
(`follow(fold, [items, last, piece, reads, defs])`): it reads `ps`,
folds the items in and defines `ps'`; a **finisher** per page, placed
by the fold in whose items the page ends (`follow(finish, [j, data,
carried, locals, rest…])`): it runs the output routine and ships the
page, and when the page ended inside the piece it checks that running
after the piece changed nothing the piece's later commands saw (else
it asks for a cut: the piece runs again next round, folding in its own
run, stopped at that candidate, and a **resume** piece after the
finisher carries on from there with the continuation value); and the
end node that writes the DVI or PDF (`assemble.rs`). So a node is a
paragraph region, a display, an alignment, a fold, a finisher, a
resume or the end node; the nest (`nest.rs`) is local to its run.

Nothing else is a node. Expansion runs inline in the node that reads the
token; a macro body, a conditional, `\the`, `\csname` never become
nodes. There are no token-frame children, no trials, no paragraph cells, no
condition-level cells. The page builder's state is ONE field, `ps`
(global): its versions are the SSA chain of the page.

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
- At its end, when it has items for the main list: its fold node after
  it, with the items as its argument. The piece's run never reads `ps`;
  the fold does, folds the items candidate by candidate, defines `ps'`,
  and where a page finishes places the finisher. Only a piece a
  finisher asked to cut folds in its own run. Every bounded piece still
  declares `ps` and `pages:shipped` in its `may_define` (`hints.rs`) for
  that case.

A group opened and closed inside the region restores its locals at its
close (`nest.rs`: saved bindings, globals retained, §283) and nothing of
it reaches the engine.

## Edges allowed

Writing `A <- B` for "A reads what B defined".

| reader | may read | from |
|---|---|---|
| region R | regime cells (`cat:`, `cs:`, `font`, `param:`, `count:`, …) | the last node before R that changed them: a fan-out from the format and the preamble, not a chain |
| fold of region R | `ps` | the last fold or finisher before it: the page chain, one link per piece with main-list items |
| region R asked to cut | `cut:<node>` (loop-back), `ps` | the finisher that asked, last round; the fold or finisher before it |
| finisher j | its page (its argument), `\output`, the registers and fields the routine reads | the fold that placed it, and whatever the text before it defined; what the routine defines (`\count0`, marks, `\global`s) reaches the text after it like any definition |
| resume | its continuation (argument) | the finisher before it |
| end node | `pages`, `pdf:pieces`, `pdf:xpages`, the document-level fields | every finisher |
| region R (`\begin{document}`) | `wkeys:<file>`, `wdef:<field>` | the files the job writes and reads back, per field: the previous round's publication by the writer (`files`); a written line is interpreted in the reading state (the field's value before this round's read-back, `wpre:<field>`), and what it writes to another file (`\@writefile{toc}`) goes to that file's text (`wfile:<file>`) |

Forbidden, and asserted by the test:

- any link through a `par:` or `if:` field (there are none);
- a region waiting on another region for anything but a regime cell it
  changed;
- an `items` read by anything but a box end;
- a rerun caused by a change of `ps` or `pages:shipped` (an undeclared
  definer, above);
- a page step reading a stream's close (`\immediate\closeout` in the text
  is an event at its position, applied by the writer in the pages'
  order: TeX's deferred writes fire before a close that follows their
  page).

## Heights

In nodes, the critical path is the page chain: one fold per piece with
main-list items and one finisher per page, so it grows with the
document. That is TeX's page builder, which is ordered; it is not the
bound that matters. What runs on the chain is the folds only (nodes of
their own, no text, one read and one definition each), so the chain's
length in time is the sum of the folds and finishers plus one wake per
link. The paragraphs run in parallel beside it and never wait for it.

The measures, per build: runs per node (1.0 when nothing reran; parks
resume, they do not rerun), the realised critical path in time
(`Stats::realised_critical_time`) against the wall, and the folds'
cost (`PHITEX_TEX_TIME`, `pagefold:` lines). Measured 2026-10-14 on 400
plain paragraphs (66 pages, release, 12-core machine, loaded): one
thread 658 runs, operation time 0.59 s, folds 0.14 s of it (decode and
encode of the page state 0.09 s); eight threads 658 runs, operation
time 1.6 s (every run about 2.5x slower than at one thread), realised
critical path 0.93 s of 1.43 s wall.

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
`par:` or `if:` field, and on more than 1.1 runs per node. The pages
test asserts the same runs-per-node bound on the vsize document. Bytes identical to pdfTeX remain necessary; they are
not the acceptance.

## Status (measured, accl jobs 8557/8586, tex-paragraph 879f11a8/951dafc0)

- Story document: one round 0.210 s at 1 thread; critical path 248 -> 8
  after `no_scope`.
- Plain format: 52 ms (release), one node.
- Catcode stops 697 -> 20; attempts per run 3.9 -> 2.1.
- Pages (2026-10-14, `pages.rs`): the page chain is the design; runs per
  node 3.3 -> 1.05 on the vsize document once pieces declared `ps`.

## Not yet

- The page state is decoded and encoded whole at every fold: a
  persistent value (the page so far shared, not rebuilt) shortens the
  chain.
- The paragraph after a page that ends inside a piece is typeset twice
  (the cut's second pass); a light resume reusing the first pass's
  items when nothing interacted removes it.
- Per-run slowdown at 8 threads (2.5x on the local machine): not yet
  separated into machine (SMT, clocks) and ours (allocation, shared
  values).
