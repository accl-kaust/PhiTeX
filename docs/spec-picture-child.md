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
