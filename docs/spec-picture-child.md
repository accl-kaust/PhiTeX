# A picture as a child node (the opaque environment of the paragraph-node design)

Status: built (matrix5 2026-11-04, deep2 2026-11-06). §2.6 as first
written (a check that refuses the split) is replaced by §7: the child
reads before its parent and the parent makes all of the child's effects
its own, so no split is wrong and none is refused for it. It fills
§4 of the paragraph-node design ("opaque environments: one slice
begin..end, one node") on today's engine, with the engine's own child
placements (`Ctx::child`, `Ctx::placed_output_or`). The measurements are
from the scoreboard's Grail cases (accl 24244, 24657).

## 1. The problem it answers

A figure's float is one node: `\begin{figure}` opens a box of the run's
own, so the run claims every sibling slice until the box closes
(`Run::pull`, `pull_box`: "a figure's float: one node, like a picture",
ops.rs). That node reads, before its picture, values that move with
edits elsewhere:

- `\@freelist` and `\@currbox` (`\@xfloat` takes the next free float box;
  deleting an earlier float shifts every later float to another register),
- `box:<n>` (the float's register, reused by another float),
- `MT@font@list` (microtype's `\xdef` list of fonts seen, in order of first
  use; its order changes when the first user of a font is deleted),
- the font control sequences (`\TU/LinLibertine(0)/b/n/8.5`), defined
  globally by their first user,
- `\reserved@b`, acro's usage lists.

Grail delete-figure at -j1 (accl 24244): 2342 runs, 15.7 s, of which
`Changed:cs:@freelist` 17 exec runs 5.9 s and `Changed:box:92` 6 runs
3.7 s: the figure nodes after the deleted one, 345-625 ms each, almost all
of it the picture (`\input{figures/generated/*}`: pgfplots, 150-200 lines).
Delete-paragraph near the start: `Changed:cs:MT@font@list` 13 runs 3.4 s,
`Changed:mathcurf` 5 runs 1.0 s.

No checkpoint can cut such a run off: checkpoints are taken only "in a
paragraph of its own text, no file, no group or conditional of its own,
no sibling claimed" (resume.rs), and a figure's picture is in a group, in
a box, in an `\input` file, in claimed siblings.

## 2. The shape

The picture's text runs as a node of its own, placed by the figure's run
as a child (`Ctx::child`). The figure's run reads the child's output as
one value (`Ctx::placed_output_or`) and goes on. The child re-runs only
when its own text or what it itself read changes; the figure's run
re-runs on `\@freelist` and the rest, in milliseconds.

### 2.1 The table

A table in `hints` of box environments: name, its `\begin{name}` and
`\end{name}` texts, for now `tikzpicture` and `pgfpicture`. A hint, not a
rule of the engine: a wrong entry never changes output (§2.6).

### 2.2 The trigger (in the figure's run)

In `Run::command` (control.rs), when the command's first token is read
from a line of the run's text (not from a token list, not inside
`pass_text`, not under `mode_zero`), and the line's remaining bytes at
`Line::loc` begin with a table entry's begin text, and the matching end
text is found further in the same input level's text (the first
occurrence; pictures do not nest), and the run is not on the engine's
vertical list (`nest.len() > 1` or the current level's mode is
horizontal: a picture there would start a paragraph on the main list,
whose items are the engine's contributions), and `r.no_child` is off:

1. The body text: the segments from the begin to the end inclusive, with
   their origins and lines, in the form `leftover()` gives a following
   exec (`Seg`), so the child's lines come the way a piece's do
   (`own_lines`, `dline`). The run's input skips to just after the end text.
2. The locals: `State::cont(v, open)` for the run's open groups: what they
   bound and saved (`ContState`, the continuation a resume gets). Encoded
   as an association list `[[name, value]…]` and defined as a global
   field `pic:<hash of body text>:<k>` (`k` = ordinal among this run's
   pictures with that text), at the run's position. Not an argument: the
   child's identity must not change when `\@currbox` does.
3. The nest: the current level's `mode`, `prev_graf`, `par`, `clang` and
   the vertical list's `\prevdepth` / horizontal `\spacefactor` as they
   stand, as the child's arguments with the body text.
4. `cx.child(ops.pic, args)` → placement `i`. Then
   `placed_output_or(i, guess)`: the child's output if it has run, else
   the guess. The first run (no child yet, no guess) runs the body inline
   as today (§2.5); a later run of the figure takes the output.

### 2.3 The child (`Pic`, a new `Operation` in ops.rs, run through `run_node`)

- Its state table starts empty. A read that misses (`Run::load_slow`)
  first asks the `pic:` field for the name (`Ctx::get` with
  `Step::Has(name)`, then `Step::Key(name)`): a read of that key alone, so
  the child depends on the locals it reads, not on the table. A name not
  there is read from the engine at the child's position, after the
  figure's run.
- Its nest starts with one level: the parent's mode and parameters, an
  empty list. The body runs (`run_all`). At the end: every group of its
  own closed, the nest back at one level, or one level deeper where the
  picture started a paragraph (`\leavevmode` in vertical mode, as pgf's
  `\pgfsys@typesetpicturebox` does): then the second level is part of the
  output. No cut, no placement, no leftover, no `\end`, no fatal error, no
  contribution (`contribs` empty: a `\write` inside the box is a whatsit
  in the list, not a stream contribution; an `\immediate\write` is one
  and refuses).
- Its global definitions are its own, at its position (counters pgf steps:
  `\pgf@picture@serial@count`; fonts it loads first; `MT@font@list` when a
  font is new to it). A second picture of the same figure is placed after
  the first and reads them in order.
- Output, one value: `[items appended at the base level, the base level's
  mode/prevdepth/spacefactor/prevgraf after, the pushed level if any (its
  mode, items, parameters), the names it defined globally, the names it
  read]`. Items encode as `nest::Item::value`. A refusal (unbalanced,
  contribution, cut) is the output `[refused]`.

### 2.4 Back in the figure's run

On the child's output: append the items to the current level
(`fold_item` as `build` appends), set the level's parameters, push the
pushed level if any; go on after the end text. On `[refused]`: §2.6.

### 2.5 The first run

A figure's run that has never placed the child (`cx.last` has no such
placement) runs the body inline, as today, and places the child as well
only when the run is a re-run (`cx.last.is_some()`): a cold build does no
extra work. The first edit that re-runs a figure runs it inline once more
and runs the child beside it; from the next re-run on the figure takes
the child's output. (Placing the child on the first run too would run
every picture twice on a cold build: Grail has about 20 of ~1 s.)

### 2.6 Exactness

Replaced by §7 (deep2). What follows is the first design, kept for the
record of why it failed: a check at the run's end that refused the split
when the parent read or defined, after the picture, what the child
defined or read. Built (matrix5), it refused most of Grail's pictures
(`\glb@currsize`, which every caption's `\normalsize` defines after a
picture read it) and its children read their parent's publication of
their own definitions (the counters pgf steps), so a child ran again on
its own result for ever (accl 24786: 80 861 runs of one child).

### 2.7 What stays with the figure's run

`\@xfloat`, `\centering`, `\color`, `\resizebox` (the child's output is
the picture's box; `\resizebox` scales it in the figure's run), the
caption, `\label`, `\end{figure}`: all cheap. A `\label` inside the
picture (a tikz node's) writes a whatsit into the list: in the output.
`\inputlineno` and box reports inside the picture read lines as a piece
does.

## 3. What it buys (expected, to be measured)

- Grail delete-figure: the 23 figure re-runs (9.6 s of 15.7 s at -j1)
  become ~5 ms each where the pictures' own reads are unchanged. The
  children run in parallel at -j8 when they must run.
- Grail delete-paragraph near the start: no gain where the pictures read
  `MT@font@list` (every `\selectfont` inside, microtype's membership test
  `\in@` reads the whole list); see §5. The figure's own run gets cheap.
- Any document with pictures in floats or minipages: the same.
- Not covered: a picture on the main vertical list as a paragraph of its
  own (the common `\begin{figure}\centering\begin{tikzpicture}` is inside
  the float's box, so it is covered; a bare picture between paragraphs is
  not, until the page builder takes a child's items as contributions).

## 4. The code, by anchor

| what | where |
|---|---|
| command loop, the trigger | `Run::command`, control.rs:55; the line's bytes: `Line::{buf, loc}`, input.rs:348; the text rest `Input::text_rest`, `append_text` |
| the body as segments | `Run::leftover`, run.rs:5063 (`Seg`, `decode_segs`, `own_lines`, `lines_owner`) |
| locals | `State::cont(v, open)`, state.rs:2995 (`ContState`); the frames' open gids: `nest::Level::gid` |
| field with per-key reads | `Ctx::get(View, Step::Key / Step::Has)`, defuse api.rs:180; `Ctx::define_with(field, value, Locality::Global)` |
| the child placement and its output | `Ctx::child`, `placed_output_or`, `placed_output_peek`, ctx.rs:824-930; `Run::kids` is how pdfpage children are placed at `Run::finish` (run.rs:5765); place the pic child there too, in order |
| the read path to hook | `Run::load` → `Run::load_slow`, run.rs:1460/1754 |
| the nest | `nest::Level`, `nest::Item::value`, nest.rs:35-135; `build::fold_item` |
| the run's end checks | `Run::finish`, run.rs:5545: `cut`, `close_cont`, `contribs`, `leftover()`, `scope_kind()` |
| the operation registry | `Ops`, run.rs:97 (add `pic: OpId`), registration beside `pdfpage` |
| what was read / defined since | `State::note_read` (state.rs:1036), `touched`; `State::defined_since_in_order` (state.rs:2426); `Run::published_values` (run.rs:1743) |
| tests | `crates/cli/tests/corpus_repros.rs` (append): a figure with two pictures, one labelled, edit a paragraph before it, PDFs identical at -j1/-j4; a picture that `\xdef`s a macro the caption reads (the check refuses); a picture that loads a font first that the caption uses (refuses); `PHITEX_PIC_CHECK=1` over the tikz manuals sample |

## 5. Measured facts (mc 80032 on 1a700a200, Grail, -j1, refute log)

Delete the figure near the start (`figures/mrrg-alu-alternatives.tex`):
2636 runs, 17.6 s busy (exec 356 runs 12.7 s, finish 601 runs 3.7 s,
pages 1525 runs 0.1 s, resume 144 runs 0.9 s). Six figure nodes are 9.3 s
of the exec time, each run several times:

| node | runs | ms a run | first moved read, its index among the run's reads | then |
|---|---|---|---|---|
| 680 | 4 | 710 | `box:92` at 2206/29778 | `cs:TU/LinLibertine(0)/m/n/8.5` at 10611/29778, three times |
| 643 | 3 | 820 | `box:92` at 2292/55381 | that font at 2887, `LibertinusMath…/8.5` at 2888 |
| 853 | 3 | 700 | `box:92` at 3122/20674 | that font at 3852 |
| 876 | 6 | 150 | `box:92` at 2858/9495 | that font at 3531; `\@freelist` five times |
| 824 | 3 | 185 | `box:92` at 2912/9919 | that font at 5625 |
| 839 | 4 | 115 | `box:92` at 2972/13597 | that font at 13108 |

So the float's own reads (`box:92`, `\@freelist`, `\@currlist`) come at
a tenth of the run, before the picture, as §1 says: a child would stand.
The font control sequence `\TU/LinLibertine(0)/m/n/8.5` moved because the
deleted figure was its first definer (`\define@newfont`'s global `\font`),
and moved again as the next definer settled: a picture that reads it (a
real input: the font's number is in the XDV) re-runs on it with or
without the child, once per move. Which of the six read it inside the
picture and which in the caption is not in the log (the refute log gives
the read's index, not its place in the text).

Delete the paragraph near the start: 1460 runs, 8.4 s busy (exec 807 runs
6.5 s). The figures 643 (1246 ms), 853 (1071), 680 (956), 567 (752) each
run once more on `cs:MT@font@list` read at 1331-1872 of their reads:
`\begin{figure}`'s `\normalcolor`/`\selectfont`, before the picture. Node
680 also on `mathcurf` at 4632/29754. What the pictures themselves read
of the list is not in the log.

What this says about §3: the child takes the float's part (box, free
list, current list, the list read at `\begin{figure}`) off the picture's
cost; it leaves (i) the pictures' own re-runs on a font whose first
definer moved, one per move, and (ii) the 601 finisher runs (3.7 s, the
finisher waves chain4 owns) and the 144 resumes. Expected delete-figure
after the child alone: about 11 s busy at -j1 from 17.6, not 100 ms. The
font moves need a definer-independent font identity (the font's number
by its name and size, not by load order) or a first-definer relation; not
in this spec.

## 6. What building it found (matrix5, 2026-11-04, branch matrix5: code written, type-checked, NOT run)

The branch holds a first build of §2–§4 (`crates/engines/tex/src/pic.rs`,
hooks in run.rs, control.rs, ops.rs, state.rs, build.rs, pages/piece.rs,
api.rs, defuse ctx.rs). It passed `cargo check` on phitex-tex and
phitex-defuse only: no test, no document was run on it. What the engine
said against the spec as written:

- §2.2's trigger ("read from a line of the run's text") reaches none of
  Grail's figures. The six hand-written ones (`figures/*.tex`) put
  `\begin{tikzpicture}` inside `\resizebox{..}{!}{%`, so the body reaches
  the command loop as a macro argument's token list; evaluation.tex's
  generated ones come as lines of a file `\input` inside the float (a
  `FILE` level under the current line). The build matches `{name}` at
  the top level's rest in three forms: the token list (`\end{name}`
  sought in the same list), the current line with the end on it, and
  the current line followed by the file below it or by the run's own
  text segments (no pulling of siblings: no trigger when the end is not
  in what the run already holds).
- §2.6 needs a third check: the run defining, after the picture, a name
  the child defines globally too (TeX's final value is the run's, the
  split's would be the child's). And `State::defined_since_in_order`
  cannot serve any of the checks: a meta keeps only the latest
  definition, so a later group-local one (the caption's `\normalsize`
  in its group) hides a global one made after the picture, and the
  group-locals themselves (which never reach the engine, open or
  closed) would refuse every captioned figure. The build stamps the
  base-level binding (`Meta::publ_stamp`, `State::published_since`).
- The child's local definitions at its base level (`\end`'s
  `\@doendpe` sets `\par` and `\everypar` locally outside the
  environment's group) must come back to the figure's run as locals of
  its innermost group, never be published: placed after the run, they
  would outlive the float's group. The build filters them out of the
  child's publication (`Run::finish`) and carries them in the output.
- `Ctx::last.is_some()` is not "a re-run after an edit": a cold build
  re-runs nodes (Grail: 3624 runs of 1457 nodes), so §2.5 as written
  would run every re-run figure's picture twice on the cold build. The
  build arms the trigger only when `Ops::edit_stall_ms` is set (the
  watch sets it after the cold build, `Build::set_stuck_after`).
- The child must never act as a page piece: `at_outer` (candidates,
  cuts) is off for it; a fatal error inside it must not claim the rest
  of the unit (`run_node`'s Fatal arm is bypassed by the child's own
  refusal); it ends `run_all` before the "group left open" path.
- `\begin` is LaTeX's robust command: the trigger takes any macro call
  (`cmd::CALL..=LONG_OUTER_CALL`) whose token is `\begin`.
- Known difference left: the picture's log lines (overfull boxes) come
  after the figure's whole log, where pdflatex writes them in the
  middle; the PDF bar is untouched, the log is not identical.
- Not built: `PHITEX_PIC_CHECK` (§2.6's inline comparison), the tests
  of §4, any measurement.

## 7. Exactness by construction (deep2, 2026-11-06)

The child stands after its parent in the engine's order; TeX ran its
commands in the middle of the parent's. Instead of checking for the
cases where that differs, the child is made to see exactly what TeX's
picture saw, and the parent to make exactly what TeX's run made:

1. **The child reads before its parent.** `Ctx::read_before_placer`
   (defuse): every read of a field's reaching definition (`read`,
   `read_path`, `read_or_path`, `read_derived`, `def_site`, `snapshot`,
   `read_item`) is made, answered and filed at `At::Before(placer)`, so
   neither the parent's definitions (made before or after the picture)
   nor those of the parent's earlier children answer it, and a change of
   them asks the child nothing. Its views (`view_field`, the `pic:`
   field below) and its scope depth stay at its own position. Test
   `defuse/tests/before_placer.rs`: a child stepping a counter its
   placer then defines runs once, and once more when a definition before
   the placer moves.
2. **What the parent bound before the picture, the parent hands over.**
   The `pic:<hash>:<k>` field is every binding the parent holds at the
   picture (`State::defined_now`: defined in the run, at any group
   level, loaded), an association list. The child's reads go through it
   first (`Ctx::read_before_placer_over`, defuse): a field it has a key
   for is answered from there through the same path the read asks for
   (a whole value, or a projection: definedness, class, `\ifnum`
   number, comma-list membership, the append base), the read filed on
   the `pic:` field under that key, with the key's presence; any other
   field before the parent. So the child depends on what it uses of the
   parent's bindings, as it would of the engine's (a scratch macro the
   parent redefined that the picture only steps over asks it nothing).
   A binding a group's end took back to one the run never loaded is
   left out: the child reads the engine's before the parent, as the
   parent would have. Test `defuse/tests/before_placer.rs`
   (`an_overlay_answers_a_child_before_its_placer`).
3. **The child publishes and contributes nothing.** Its global
   definitions, its base-level locals, its contributions (log lines,
   font markers `T_FONT_AT`, `\write`s through `Run::contribute_stream`,
   file reads), the document lines it began and its pending appends
   (`Run::appending`, the relation `\xdef\x{\x…}` makes) are its
   output. The parent makes them its own where the picture stands
   (`Run::pic_apply`): globals defined, locals in its innermost group,
   contributions in its list at that place (so the picture's log lines
   come where pdflatex writes them), streams through its own
   `contribute_stream` (its candidate marks), lines added to its count,
   appends taken as its own pending ones. A list the parent had pending
   appends for goes to the child as pending (the `pic:` field's
   `\u{1}appending` key) and comes back with the child's added.
   So what the parent reads or defines after the picture comes after the
   picture's definitions, as in TeX, and no reader anywhere else in the
   document sees the child.
4. **What still refuses** (the parent runs that picture inline, and
   never places it again in this process): the child's text reaching
   past itself (a group or conditional left open or closed, input left,
   a cut, a fatal error, children of its own, read-time objects, a scope
   effect, a relation other than an append), and an item for the main
   list that is not a marker (the picture was not in a box).
5. **A look before the picture's start.** The child's list starts empty;
   TeX's picture is appended to the parent's list. Where the child looks
   past its start (`\lastskip`, `\unskip`, `\lastbox`… on its empty
   base level, `Run::list_last`), it takes the parent's items before the
   picture (the `pic:` field's `\u{1}list` key, read only then) in front
   of its own, and its output's list is then the parent's whole list,
   which the parent puts in place of its own (`Run::pic_take_list`).

## 8. One run per picture on a clean build (agent pic, 2026-11-13)

### 8.1 What was wrong

§2.5 as built read a picture inline whenever its child had no output yet,
and placed the child as well, for the runs after. On a clean build every
figure's run meets its picture before the child has run, so every picture
ran twice: inline (the output used) and in its child (kept for the edits).
Grail at -j1 spent 6.1 s of 19.3 s on that duplicate (note-par-2026-11-13
§2). And each figure's run, released by the page layer's frontier where it
read `\@freelist`, read its picture inline (1.1-1.5 s) before the frontier
could pass it: Grail's eight figures stood on the critical path one after
another.

### 8.2 The rule: a child exists or will exist, so the figure's run never reads the picture

At a picture's `\begin` the run (`Run::pic_begin`) computes the child's
arguments and the bindings it hands over (`pic:` field) as before, then:

| the child | first run of an evaluation (an edit's) | any later run, every run of a clean build |
|---|---|---|
| has no output | its last run's output as the guess (`Run::pic_guess`, unchanged), else as if empty | as if empty |
| refused (`[refused]`) | inline, never placed again (unchanged) | the same |
| has an output | taken (unchanged) | taken where its view holds, else as if empty |

**As if empty.** The run skips the body (`Run::pic_skip`) and goes on;
the read of the child's output is recorded as the guess `()`
(`Ctx::placed_output_or`, or `Ctx::placed_output_guessed` where the child
has an output of another view). A placement's output that differs from
what its placer saw re-asks the placer (`commit::reask_parent`), and a
commit whose own read of a placement no longer holds is refuted at once
(`read::holds`), so a run that went on as if empty never stands: it runs
again once the child has an output for the view it gave, and takes it.
The picture runs once, in its child, on whichever worker is free.

**The view holds** where my last run handed the child the same bindings
(`Ctx::last_def` of the `pic:` field equals this run's), or where every
read the child's last run made of that field gives what it gave with
this run's bindings (`Ctx::placed_reads_hold`: the same comparison the
commit's walk makes, so a view that holds by it re-asks the child for
nothing). A float's run is re-asked each time `\@freelist` moves, and
its bindings move with `\@currbox`, which no picture reads: such runs
take the output (milliseconds), where §2.5 read the picture inline again.
A child whose view holds but that is asked again or still running gives
its output as the guess it is, recorded as seen: its next commit with
another output re-asks the run. (Waiting for it, as a run waits for a
definer, was built first and deadlocked at -j8: the child's reads before
its placer were held at the page layer's frontier, which stood at the
placer; Grail never ended, accl 28411.)

**Pictures after one gone on as if empty.** Their state lacks that
picture's work (fonts it loads first, pgf's counters, its items), so a
child given it would run again once that output is in, and the pictures
after it again for each: lie-hasse's page of 24 diagrams, one more right
a pass (f940e759c). So in a run that went on as if empty at one picture,
a later picture whose child has run and whose view moved goes on as if
empty too, its child keeping the view it has; a later picture with no
child run yet is read inline, as before §8, and its child placed for the
runs after. (Gone on as if empty and placed only on the next run, each
picture ran once, but the run went again once a picture: lie-hasse's
piece, about 37 s of its own text, 24 times, -j1 860 -> 1356 s, mc
86456.) A figure of one picture, Grail's every one, reads none inline.

What is still read inline: a picture after one gone on as if empty in the
same run, whose child has not run; a refused picture (§7.4), and a picture whose
child is held for a panic (no commit of it comes). `Stats::pics_inline`
counts them.

### 8.3 Exactness

Nothing here changes what a run makes from what it read: a run that goes
on as if empty records the guess it went on, and every record that does
not hold is run again (§7.1-7.3 unchanged). The bars: PDFs byte-identical
to dev's at -j1/-j4/-j8/-j16 (Grail, amsldoc, para4000, the course cover,
pgf-pie's and pgf-umlsd's manuals, two chapters of the TikZ manual).

### 8.4 Children placed early

A child is placed by its figure's run, so it starts when that run first
reaches the picture. With §8.2 that run is cheap, and the page layer's
frontier no longer waits there for a picture: the children start as the
text's front passes their figures, in parallel with the text after them.
What the source-level placement of §8.5 would add over that is measured
first (8.6).

### 8.5 Placing children from the source (designed, not built)

The hint pass could place a child for every picture whose body it can
see: `\begin{tikzpicture}`...`\end{tikzpicture}` in a slice, or in a
file an `\input{}` of a slice names. Such a child cannot be the figure's
own: its arguments are the body as the figure's run finds it (a token
list where the picture is a `\resizebox` argument, Grail's drawn
figures; lines of a file where it is `\input` there), the list's mode and
parameters at the picture, and the bindings the figure's run made before
it (`\centering`, the float's `\normalsize`, `\@currbox`), none of which
the source gives. The early node would run on a guess of all three (the
body tokenised with the catcodes before the figure, restricted
horizontal mode, no bindings) and record what it read of each; the
figure's run, at the picture, adopts it where its own arguments match
and every recorded read of the guessed bindings and parameters holds,
else places its own child as in §8.2. Exact by construction (adoption
only where the reads hold), but it needs the early node's identity by
place rather than by arguments, a lookup of it from the figure's run
(a field it defines, read as a definer's), and a tokenisation of the
argument that matches the run's. Built only if §8.6 shows the children
starting late.

### 8.6 Measured

Picture runs on a clean build of Grail (11 pictures; acclnode11, Shelf's
files, `PHITEX_PIC_TRACE`): before, -j1 22 (11 read inline, 11 children),
-j8 33 (20 inline, 9 children, 4 children stopped); after, -j1 11 (all
children, none inline), -j8 9-10 children run to the end, none inline,
15-22 runs of children stopped part way (the long-run check of their
reads, `Ctx::recheck`, and `Ctx::yield_to_prefix`). Test:
`latex_each_picture_runs_once_on_a_clean_build`.

Clean builds (mc 86417, mcnode42 alone, EPYC 9754, schedutil, boost on;
release, warm format, median of 3; dev 81a72ba6d before):

| doc | -j1 | -j4 | -j8 | -j16 |
|---|---|---|---|---|
| Grail before | 19.09 | 16.28 | 17.00 | 17.42 |
| Grail after | 12.95 | 12.58 | 13.78 | 14.60 |
| amsldoc before | 2.39 | 2.96 | 3.13 | 3.68 |
| amsldoc after | 2.39 | 2.92 | 3.27 | 3.55 |
| para4000 before | 2.54 | 1.29 | 1.27 | 1.37 |
| para4000 after | 2.53 | 1.28 | 1.29 | 1.34 |
| course cover before | 6.59 | 6.73 | 6.72 | 6.87 |
| course cover after | 6.56 | 6.70 | 6.75 | 6.81 |
| pgf-pie manual before | 3.89 | 3.35 | 3.34 | 3.78 |
| pgf-pie manual after | 3.60 | 3.16 | 2.97 | 3.53 |
| pgf-umlsd manual before | 3.86 | 3.66 | 3.83 | 4.08 |
| pgf-umlsd manual after | 3.87 | 3.69 | 3.83 | 4.26 |

Every PDF the same at every -j, before and after: Grail 004bb415ed,
amsldoc 98c96e004c, para4000 4146793290, the cover 13349bee03, pgf-pie
601000b03e, pgf-umlsd a24a2ba54d. Edits through `watch`, before / after
(ms, 3 each): Grail one word -j1 38.9-40.6 / 39.1-39.8, -j8 40.8-58.9 /
41.0-56.5; Grail's first drawn figure deleted -j1 6955-6979 / 6727-6755,
-j8 7161-7520 / 6750-7042; amsldoc one word -j1 8.0-8.4 / 8.2-8.3, a
paragraph inserted 108.0-108.5 / 107.7-108.9; the edited PDFs the same.

So the duplicate is gone (Grail -j1 -6.1 s), and -j8 gained 3.2 s, but
Grail at -j8 is still about its -j1. What holds it there is no longer
the pictures (mc 86379, a profile build, -j8): the class (1.7 s, one
node); minted's listing after the drawn figures (0.73 s, a `\write18`)
runs only on the settled prefix (`Ctx::wait_settled`), so after every
picture before it and its figure's run again: at 7.6 s; then three runs
of about 2 s each (resume nodes that take 6 ms on their other runs and
take no such time at -j1) one after another at the page layer's frontier
(10.5-17.7 s), with `grail.aux` read back and BibTeX run four times
where -j1 runs it once. Without minted (acclnode11, Shelf has no
`latexminted`, release, 3 each) Grail is -j1 12.15 / 8.15 s and -j8
about 11.0 / 9.0 s before / after: -j8 does not beat -j1 there either.
The children themselves at -j8: 45 runs for 12 pictures (12.2 s of
work, 6.3 kept), stopped where a read moved under them, on
`cs:MT@font@list` (a font the text before first uses), `box:53`
(amsmath's `\Mathstrutbox@`, whose depth every formula sets in place,
a picture's math too), `pgf@arrow@id@count`. A picture child kept from
yielding its worker (`Ctx::yield_to_prefix`) measured no change (mc
86408, 5 each: -j8 13.1 / 13.1 s), so §8.5 was not built: the children
start in time; the text after them does not finish sooner for it.
