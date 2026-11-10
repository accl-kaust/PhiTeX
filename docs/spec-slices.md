# Slices: finer nodes inside expansion, in SSA terms

Status: design, 2026-11-10 (branch `fable52`), rewritten after the
user's ruling ("no replay design; def-use does not need replay"). The
first version of this file described a cache of recorded stretches
checked and applied on a run again. That is the old main branch's
design (an interpreter first, a trace beside it) and is dropped; §9
says what was learnt from building it and what happens to its code.

Read first: docs/spec-layers.md (the layers), docs/spec-state-model.md,
docs/spec-picture-child.md.

## 0. The frame, and what is banned in it

A document is a program; a build evaluates a graph of nodes over
versioned values. A node is a function of **exact value versions**: each
of its uses names the one definition that reaches it (use-def). If its
operands are the same versions, it is the same node with the same
result: it is not visited, checked or applied. The other direction
(def-use) is an index only: from a definition that changed to the nodes
that use it, where the change is pushed and stops as soon as a value
comes out equal.

Not the design, and not to be written into it again:

| banned | what it is |
|---|---|
| recording what a stretch of a run read and wrote, to check and apply it later | a trace cache beside the graph (main's regions, this file's first version) |
| a continuation: the input stack, the nest and the open groups' table handed to the next node as one value | Knuth's machine state, moved between nodes |
| a parent that walks its text again and takes each child's output | one cursor, with stops |
| a cut chosen by a count of commands | a window |

The self-test: a sentence that needs "the run's state at the cut" to
make sense is about the machine, not the graph.

## 1. The problem

Expansion (layer 2) is where the graph is coarse. A node there is a
paragraph, or a picture written as `\begin{tikzpicture}` in the text,
and everything inside it is hidden: one node, all of its uses, all of
its definitions. So:

- an edit of one statement of a picture is a new version of the node's
  text, and the node, with every other statement, runs (2400 circles
  and a label: 2.4 s for the label);
- a value that moves between passes (a `\ref`, a counter) is one use of
  a paragraph node that also holds a picture (2.9 s for one number);
- a guess that fails in a parallel build throws away the whole node;
- inside the node nothing can run beside anything else. The
  paragraph-node design said so itself: "no parallelism inside a
  picture (sequential by construction)".

On a two-page file with two pictures, 8.2 s of a 15 s clean build is
nodes running again in the later passes, and what had to run by the
values that really moved is 0.16 s (mc 83571).

## 2. The values

What a coarse node hides is state of the machine. In the graph each is
a value with definitions, and the engine already has the form for each:

| inside a node today | as a value | engine form that exists |
|---|---|---|
| the tokens still to be read (a macro's body, an argument, a loop's body) | a token list is a value; a node uses a **range** of it | views and paths on a value (`Index`, `Len`, `Key`): a use of a part moves only when that part does |
| a binding (a macro, a register) made outside every group | a field with definers in text order | fields; a read is its reaching definition |
| a local binding inside an open group | a definition in the group's scope | scopes (`Locality::Scope`, `ScopeAt`), used today for groups that span paragraphs |
| the items of a box being built | contributions to the box's list, folded where the box closes | contributions and folds over a position range; sequences are ropes |
| a value only ever stepped or accumulated (a counter, a picture's bounding box) | a relation on what reaches it (`+c`; `min`, `max`) | `Pub::Rel`, `Append`; `min`/`max` to add |
| a number only printed (a page number, an object number) | a symbol, filled in at output | docs/spec-deferred-reads.md, spec-relative-counters.md |

Nothing in this table is new machinery of a different kind. What is new
is using it **inside** what is one node today.

## 3. The nodes

A **body** is a stretch of expansion whose token range is known before
it runs: a simple group whose close is in the same token list as its
open (`\begingroup ... \endgroup`, `{ ... }`). A loop's iteration and a
picture's statement are bodies.

A body is a node:

- **uses**: its token range (a view of the list it sits in); each
  binding it reads, by its reaching definition (a field, or a scope's
  definition of an enclosing group); nothing else;
- **definitions**: the global bindings it makes; its contributions to
  the list open around it; relations on accumulated values.
  Its local bindings die at its close and are nobody's.

The node that reaches a body **places it and goes on**. It does not run
the body, and it does not take its output, because it uses none: the
body's items go to the list's fold and its definitions to the graph,
where whoever reads them finds them. If the placer later reads a field
the body defines, that is a use with a reaching definition like any
other; the engine parks or guesses it as it does between paragraphs
today.

So a picture of a loop and a label is: the picture's own thin node,
which places its statements; the loop statement, one node (§3.2); the
label's statement, one node; and the box's close, which reads the fold
of their contributions.

### 3.1 What makes a body legal, and who checks

A body must stay inside its range and its group: read no input beyond
its close, leave no group or conditional open, touch nothing of the
list before its own items. This is checked when the body runs, as the
picture child checks its shape today (`pic_shape`). A body that breaks
it is not a node: its placer reads it inline, and the place is
remembered so it is not tried again. Exactness never rests on the
guess that a group is a body.

### 3.2 A loop is one node that holds its result (user, 2026-11-10)

A loop's iterations are **not** nodes and the values they pass to each
other are not values of the graph. The loop is one body; the graph holds
only its net result: the values it used that were made outside it, and
for each value it changed, what it left (as loop-closed SSA keeps only a
loop's exit values). 2400 iterations are one record, one commit, one set
of uses.

For a value the loop only steps or accumulates, what it leaves is the
**composed relation**, not a value: 2400 comparisons with the bounding
box so far compose inside the run to one `min`/`max` with the loop's own
extremes, as a register only stepped already composes to one `+c`
(`State`'s step map, `Pub::Rel`). The loop then does not use the box
that reaches it, and the statement after it does not use the loop's
box: statements do not chain, at the cost of composing inside a run,
with no node per iteration.

What this gives up: iterations do not run beside each other, and an
edit of the loop's own line runs the loop (every iteration changed
anyway). Bodies are a picture's statements, a paragraph's boxes and
groups: where the text has structure, not where a macro repeats.

### 3.3 How fine

Every body can be a node; whether it is one is a folding decision that
does not change meaning: a body is folded into its placer where a node
costs more than the body (the node's record and commit against the
body's commands). The fold is decided per place from what the body did
the last time it ran there, and on a first run from its token range's
length. A wrong decision costs time only.

The cost of a node is what bounds this, and is being cut on dev now
(read records 80 → 24 bytes, reader lists sized by use).

## 4. An edit, a later pass, a clean build

**An edit of a picture's label.** The picture's text is a value; the
edit is a new version of the range one statement uses. That statement's
node runs. Its contribution to the box's list changes, so the fold at
the box's close moves from that item on (a rope: the items before it
are shared), the box is packed again, the paragraph's lines are broken
again if the box's size moved. The loop's nodes use nothing that moved
and are not visited.

**A later pass moves a `\ref`.** The node that typesets the number runs.
A picture in the same paragraph is other nodes with other uses.

**A clean build.** Every node is dirty. A placer runs ahead placing
bodies; bodies run on any thread as their operands are ready; the box's
close folds what they contributed. The serial part is what TeX makes
serial: the placers' own chain, and values a body really reads from the
body before it.

**A guess that fails.** The node that used the wrong version runs again.
It is a body, not a paragraph.

## 5. Where bodies still chain, and what unchains them

- **The bounding box.** pgf compares every point with the box so far
  and overwrites it if the point is outside. Read as a value, every
  statement uses the one before it. As a relation (`min`/`max` of what
  reaches it and the point) no statement uses another, and the picture's
  close reads the fold. The pattern is general (compare, then set to the
  value compared) and is recognised by what the commands do, not by
  pgf's names. Until then statements chain through four dimensions:
  edits stay exact and fine (a statement runs again only if the box
  before it moved), a clean build guesses.
- **Object numbers and counters stepped by a body**: relations, as
  outside pictures.
- **Soft paths and other lists a statement builds and empties**: local
  to the statement's group, so nobody's.

## 6. How it is built: a box as a scope (found in the code, 2026-11-10)

The engine's own model already is the design, one level up: the
document is a list (the main list) that no node owns; text is cut into
slices; each slice is a node; a group that spans slices is a **scope**
whose locals are scope definitions (`leave_scope`, `Locality::ScopeAt`);
a node's items are `items` contributions and a reader folds them. A
picture is one node today for one reason only: its box is a list of the
run's own, a run cannot leave such a list open (`can_leave_scope`: every
open level must be a group sharing the list below), so the run pulls
every sibling slice until the box closes (`Run::pull`, `pull_box`).

So no children, no placer, no taking of outputs. **A box can be left
open as a scope**, and what is inside it is cut into slices like any
text:

| step | what | what is there already |
|---|---|---|
| 1 | A run at rest inside a box of its own (no macro's tokens left on the input, no conditional open) ends there and leaves the box as a scope: its mode and list id in the scope's kind, its items so far as `items` contributions under that list id, its parameters (space factor, depth) as the fold's | `leave_scope`, `SCOPE_KIND` carries the list id and mode; `append(lid, …)` to a list not the run's own is an `items` contribution `[lid, node]` (build.rs) |
| 2 | A list id that is one list for every node inside (today a per-run counter) | derive it from the opener's node |
| 3 | A node inside the scope appends to that list as contributions; the node that closes the box folds them and packs | `list_fold` for a list not the run's own folds the `items` contributions (its comment: "no list is opened by one node and read by another", the rule this lifts); the fold reads all items of the unit, to be a range from the opener |
| 4 | The picture's text cut into slices at its statements (a hint of the text's shape, as paragraphs are cut; a wrong cut costs a pull, never exactness: a run not at rest at its text's end pulls the next slice, as now) | `hints::slices_of`, `Run::pull` |
| 5 | A loop statement is one slice, one node; what it accumulates leaves a composed relation (§3.2, §5) | the step map, `Pub::Rel`; `min`/`max` to add |
| 6 | The same for a box in a paragraph, a `codeexample`'s body, a float | follows from 1 to 4 |

Bars on the two-page file (`bench/f52-small-tmp.sh`), PDFs identical to
pdfTeX's at every thread count: a label's edit in the 2400-circle
picture runs the label's slice, the box's close and the paragraph, and
the engine's run log shows the loop's node not run, under 100 ms; a
section added before the first runs no picture node; the clean build's
later passes run no picture node.

A slice that cannot stand as a node pulls its neighbours, as now, so
nothing depends on a hint.

### 6.1 Lists and groups left apart (built 2026-11-10; on by default, `PHITEX_BOXSCOPE=0` turns it off)

Every picture sits in a paragraph, and a paragraph is a list the run
owns too: leaving only the box would leave nothing, the run still could
not end. A paragraph cannot simply be a scope either: TeX's nest and its
save stack are two stacks (§211, §268), `{\bf start} and go on` closes
the group and the paragraph goes on, and the engine's scopes nest.

So the two are left apart (`crate::boxscope`):

- every group open is an engine scope (a box's own group among them);
  its kind carries the mode and the id of the list it is in;
- every list open gets an id unique among nodes, its items so far are
  `items` contributions under it (the main list's way), and the node
  that ends it folds them;
- a paragraph is left only under a group (the group's kind names it),
  its own parameters (language, hyphen minima, `clang`) a field
  `par:<list it is in>`, which every kind in the list below reads
  (`Run::par_kind`, `Run::vert_kind`);
- lists are left only where a box is being built among them: what
  leaving pays for is a box's contents run as nodes of their own. A
  paragraph under groups alone is broken whole at its end whatever its
  nodes, and each slice of it left would be one node more (huaz's
  `\lstinline` groups: 2771 nodes against 725, its clean build twice
  as long);
- a run stops to leave its lists only where it could stop for a
  continuation: nothing half done that only it holds, a word whose
  lookahead met the text's end among them (`Run::open_state_refusal`); and a
  character run's lookahead at its text's end reads the next slice on
  whatever the run has open, the paragraph being another node's.

The picture's text is cut at its statements (after `\begin{tikzpicture}`,
after each `;` or brace group followed by a command, depth 0). A run
that cannot leave (input of a macro pending, a conditional open) reads
on, as now.

What each list left open is (no gaps left for later): its id is from
the scope depth it is left at (lists open at once are left at different
depths), never from the opener's node, so a re-cut of the opener changes
no kind; its items are contributions to its own accumulator
`bitems:<id>`, the opener's a start entry holding what it had. The
field's fold is a reset monoid (`ResetMonoid`): a start discards what
came before it, so the fold at a reader is its own list's entries,
whatever lists of the same id ended before it, and a change in those
leaves it equal (`Run::left_list_fold`). Nothing it reads names the
opener, so an opener run again (an edit of the text before the picture)
with the same items wakes no reader of the list. A `\par` inside
a group in a paragraph left open ends the paragraph
(`Run::end_left_paragraph`): `par:<list>` cleared, and every kind in it
reads that field (`Run::par_kind`), so the groups go on in the vertical
list as in TeX; a display (`$$`, §1145) in such a paragraph ends it the
same way before breaking its lines.

A box whose group a later node closes is packaged there, but appended
in the node after it: the close ends the engine scope, and the closing
run's reads are the scope's to its end, `\baselineskip` for the
interline glue among them (§679, §1086). The box, its context and its
adjustments go to the field `boxend:<list>`, and an internal token put
back reads them outside the scope (`Run::box_end_outside`,
`Run::box_end_left`).

## 7. Parallel work

Nothing is added for it. Bodies are nodes the engine schedules; commits
stay partitioned by position (a body's place is its group's place in the
text order, its definitions and contributions filed there). What decides
the scaling of a picture is §5; what decides it for text is the same
rule one level up, already in docs/spec-layers.md (line breaking out of
expansion, pages as their own nodes).

## 8. Open

- A placer needs a body's place in the order before the body runs, and
  the places of what it places next. Children are placed "after me,
  before my continuation" today; a placer that goes on needs its own
  later reads to be after the bodies it placed. Whether the engine's
  child placement gives that or a placer must end and continue is the
  first thing to settle in step 2.
- `\aftergroup` tokens a body leaves, and a body that ends inside a
  macro whose rest the placer reads: the range is known, the tokens put
  back are a definition the placer uses.
- The cost of 2400 nodes for one picture against one node: §3.2 folds
  the inner loop into the outer one's bodies if it does not pay.
- Memory: a body's record against the picture's one record.

## 9. What was built first, and what becomes of it

Built on this branch on 2026-11-09/10 and **not the design**: a
recorder of what each simple group of a run read and left
(`crate::slice`, `State::rec_*`), a shadow check, and a replay that
skipped a group whose recorded inputs stood (`PHITEX_SLICES=replay`).
It gave byte-identical PDFs on the two-page file and a label's edit in
120 ms, with 2.4 times the memory, and it is a trace cache: the run is
walked again, every kept stretch checked.

Kept from it, as facts: a loop iteration or a statement inside a
picture uses a few hundred values and defines a handful (so it is a
cheap node); dependencies are found by running, not predicted; the
places where a group's range is in one token list are common in pgf.

Its code: the replay and the pool are deleted before anything of this
branch is merged. The recorder stays as a profile-build probe ("what
would this body's uses be") until step 2 runs, then goes too.
