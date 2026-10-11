# phitex-red: TeX as graph reduction

Written fresh. Depends on NO phitex-flow code or types. Semantics are TeX's
(tex.web, etex, pdftex) as observable in the PDF; the *execution model* is not
Knuth's machine.

## The model

* The document is a term. Unread text is a thunk `Pending { env, input }`:
  `env` is the persistent map of every name's meaning (`env.rs`), `input` the
  persistent rest of the text (`term::Input`, an immutable cons list of text
  spans and token lists — a value, not a machine stack).
* Reducing a thunk rewrites it in place into one node plus the thunk for the
  rest: `Pending(e, i) => Node(op, reads, out) ; Pending(e + out.delta, i')`.
  A node is one command, one macro application, one conditional select, or a
  run of plain characters.
* A node is a pure function of (the tokens it consumed, the values it read).
  Every read goes through `Ctx` and is recorded. Its result is
  `Out { delta, emits }` and the rest input. Nothing else exists.
* Macros are function values (`val::Macro`). A call is `Apply { f, args }`:
  args matched from the input are token lists; the result is the substituted
  body prepended to the rest. Memoised on (f, args).
* `\if...` is `Select { test, then, else }`. True: continue, with the open
  conditional recorded in env (`Key::Conds`); a later `\else`/`\or` skips raw to
  its `\fi`. False: the untaken arm's extent is found by a raw scan and kept as
  an unreduced thunk; reduction continues after it. No φ, ever.
* Groups and lists are scopes in env, not stacks of a machine: `Env` has a
  group chain (definitions, `\aftergroup`) and a list chain (mode, spacefactor,
  prevdepth, prevgraf, clang). They interleave independently, as TeX's do.
* Material is *emitted* by nodes (`Emit`), never accumulated in env. A list's
  contents are the emits between its open node and its close node; the close
  node (pack, linebreak, align, math) reads them as one value. `ctx.tail()` reads
  the list's last item (a recorded read).
* The page builder is a fold over the main vertical list's emits, memoised on
  (state, item). A fire applies `\output` to (box255, marks, inserts): a thunk
  reduced in the env at that point; its global delta is a node between the two
  main-list nodes. Main reduction speculates "no fire"; validated by reads.
* `\input` rewrites to the file's text followed by the rest of the line.
  `\end` truncates: nothing after it is demanded.
* The PDF is the only root. Demand flows from it: pages demand page boxes,
  boxes demand lists, lists demand nodes, nodes demand thunks.
* Loop-back: files the job writes (`.aux`, `.toc`, …) are never read from disk.
  Reading one reads `Key::File(name)`, answered by the fixpoint driver with the
  previous round's written text (none on a cold first round). Rounds repeat
  until the written texts equal the read ones, bounded (5). BibTeX and makeindex
  are functions inside the loop.
* Edits: a CST splice maps to text spans. Nodes whose consumed span is touched
  are un-reduced back to a thunk; nodes whose reads changed are re-applied; the
  first node after which the new thunk's reads all equal an old node's reuses
  the old chain from there. Equal outputs stop propagation.
* Parallelism: reduce ready frontier nodes concurrently (line breaking, packing,
  math, page bodies, PDF streams are pure applications); speculatively reduce
  thunks at likely boundaries on a predicted env, adopt only if every read
  matches.

## BANNED (any of these in a diff is a bug)

A mutable global/thread machine state; a mutable input stack; `cond_ptr` or a
skipping mode; a save stack; a semantic nest array; φ; pieces/cuts/joins/rounds
of re-reading text; matching runs by text; re-running TeX; anything taken from
phitex-flow.

## Construct table

| TeX | here |
|---|---|
| reading a token | `Ctx::next` tokenizes the input head with the catcodes read from env (`Key::Cat(c)`, recorded) |
| `\def\f#1.#2{b}` | `Key::Cs(f) := Val::Meaning(Meaning::Macro(Arc<Macro>))` delta |
| `\f a.b` | `Apply` node: args matched (`macros::expand_macro`), body pushed |
| `\expandafter` `\noexpand` `\csname` `\the` `\number` … | expansion node: rewrites input |
| `\if…\else…\fi` | `Select` (cond.rs); `Key::Conds` holds open conditionals |
| `{` `}` `\begingroup` | `ctx.open_group(kind)` / `ctx.close_group()` |
| `\count0=5` `\let` `\font` `\catcode` | `ctx.define(key, val, global)` |
| `\afterassignment` | `Key::AfterAssignment`; the next `define` pushes it |
| `\futurelet` | peek via `ctx.next` + `ctx.back` |
| modes | `ctx.mode()` from the list chain; letters in V open a paragraph list |
| `\par` | close paragraph list → `linebreak::line_break` → emit lines to enclosing V list |
| `\hbox{}` `\vbox{}` | open group+list; at `}` close → `pack::hpack/vpack` → `boxes::box_end` |
| `$…$` | math list → `math::mlist_to_hlist` |
| `\halign` | `align.rs`: cells are thunks with u/v templates pushed around them |
| page builder | `page.rs` fold |
| `\output` | `output.rs`: thunk of the routine's tokens in env at the fire |
| `\write` `\openout` | whatsits; their text at shipout becomes the file's written text (loop-back) |
| `\shipout` | `Emit::Ship(page)` → `pdf.rs` / `dvi.rs` |

## Ownership (one agent per row; only touch your files)

| Agent | Files |
|---|---|
| A1 | `graph.rs` `memo.rs` (implements `Ctx`, `Env` internals in `env.rs` bodies) |
| A2 | `sched.rs` |
| A3 | `edit.rs` |
| A4 | `fixpoint.rs` |
| B1 | `tokenize.rs` `input.rs` |
| B2 | `macros.rs` |
| B3 | `scan.rs` `expand.rs` |
| B4 | `cond.rs` |
| B5 | `assign.rs` `groups.rs` `arith.rs` |
| C1 | `list.rs` (chars, ligs/kerns main loop, spaces, glue/kern/penalty/rule, paragraphs start/end) |
| C2 | `pack.rs` `boxes.rs` |
| C3 | `linebreak.rs` |
| C4 | `hyph.rs` |
| C5 | `math.rs` |
| C6 | `align.rs` |
| C7 | `font.rs` (TFM, `\font`, fontdimen, hyphenchar, skewchar) |
| D1 | `page.rs` |
| D2 | `output.rs` `io.rs` (`\write` `\openout` `\read` `\openin` `\message` `\special` `\shipout`) |
| D3 | `pdfprim.rs` (pdfTeX primitives) |
| D4 | `derive.rs` (BibTeX, makeindex, loop-back keys) |
| E1 | `pdf.rs` (writer: objects, page streams kept per page, xref) |
| E2 | `dvi.rs` `diag.rs` (DVI; errors and their recovery; `\show*`) |
| E3 | `api.rs` (Build, watch, CST splices) + `crates/phitex-cli` wiring behind `PHITEX_ENGINE=red` |
| E4 | `tests/` |
| E5 | `pdffont.rs` (map files, Type1 subset, TrueType/OTF, VF, ToUnicode) |
| E6 | `image.rs` (PNG, JPEG, PDF inclusion) |

Shared files (`lib.rs` `tok.rs` `val.rs` `item.rs` `term.rs` `env.rs` `ctx.rs`
`prim.rs`) are the contract: do not change them. Need something more from them
or from another agent's module? Write the call against the name you need and
list it under `NEEDS` in your report.

## Wave 1b contract (added after wave 1)

* `graph.rs` is one module in three files (`include!`): `graph/envrep.rs`
  (environment), `graph/chain.rs` (main chain, node store, answers),
  `graph/reduce.rs` (reducing one thunk, `Ctx`). Each has one owner.
* `Env::hash() -> u128`: incremental; equal hash = equal env. Edits stop at the
  first new thunk whose (input position, env hash, page fold state) equals an
  old one, and keep the old chain from there wholesale.
* **Groups (replaces the save-stack-shaped frame):** a group is the env
  snapshot taken at its opening plus the global definitions made inside it. At
  its end the env is `snapshot + globals-in-group` (+ `\aftergroup` tokens
  pushed). No restoring of shadowed values entry by entry.
* **Fires:** after each main vertical list item, the page fold steps. A fire is
  a node (`Op::Fire`) inserted between two main-list nodes: `\output` is reduced
  as a thunk in the env at that point (Output group); the routine's global
  definitions are the fire node's delta, applied before the next main node;
  then `Fire.back` items are contributed again. Main reduction speculates "no
  fire"; a node that read a page quantity is re-validated after a fire.
* `pool::Pool`: pure applications are spawned on it; callers join.
* `term::Checkpoint`: (thunk, env hash, fold, node) kept at paragraph
  boundaries; interior nodes may be dropped and re-reduced from a checkpoint.
