# A picture as a child node (the opaque environment of the paragraph-node design)

Status: design, 2026-11-04 (matrix4). Nothing of it is built. It fills
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

### 2.6 Exactness: what makes the split wrong, and the check

The child stands after the figure's run in text order, but in TeX its
commands ran in the middle of the figure's. Two things can differ:

- (a) the child reads a field the figure's run defines globally after the
  picture (the engine shows the child the later value): `\c@figure` if
  the picture read it, a font the caption loads first and the picture
  used, `MT@font@list` when the caption appends to it and the picture
  read it;
- (b) the figure's run reads, after the picture, a field the child
  defines globally (the run reads the value before the picture): a font
  the picture loaded first that the caption uses, `MT@font@list` when the
  picture appended to it and the caption reads it, a pgf node name
  (`\pgf@sh@ns@…`) used by text after the picture.

The figure's run keeps, from the placement on, the fields it read
(`State::note_read`'s `touched`, locals included: a global definition in
the child overrides a local binding too) and the fields it defined
globally (its base-level definitions, `defined_since_in_order`). At its
end, with the child's output (its defined and read names): if reads-after
∩ child-defined ≠ ∅ or defs-after ∩ child-read ≠ ∅, the run's product is
wrong. It is thrown away and the run is made again from its start with
`no_child` set (the `run_node` closure loops once), and the body's text
hash goes into `Ops::pic_refused` so the figure never tries again in this
build. The same for `[refused]`. A wrong entry in the table costs one
re-run of the figure; it never changes output.

`PHITEX_PIC_CHECK=1` (observe): the first run with a child also runs the
body inline and compares the two item lists and the state changes; a
difference is a bug in this spec, not a hint violation.

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

## 5. Measured facts (accl 24657, Grail, -j1, refute log) — filled in below
