# CORE: the command set of TeX on the graph

> **Note (W, the CLI slice):** `crates/partex-core` is gone; the test font is `crates/phitex-flow/testdata/cmr10.tfm` (`concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")`): rebase paths that named `../partex-core/testdata/cmr10.tfm`.

## Crates

Pure, link them: partex-kpse (kpathsea: texmf.cnf, ls-R, path search; the input finder the CLI gives the build), partex-otf (OpenType faces/shaping/OT math/TECkit; X, and Q for TrueType embedding), partex-xdvipdfmx (XDV→PDF byte for byte; X's backend; its image readers are dvipdfmx's, for the PDF it writes: XeTeX's pictures are measured by XeTeX's own readers, X's xpic.rs copy, not dvipdfmx's, and pdfTeX's by pdfTeX's), phitex-codecs (inflate, md5, png, pdfread, pdftext, zlib crc; yours), partex-bibtex and partex-makeindex (in-process, between fixpoint runs), phi (allowed, unused), phitex-syntax/doc/draw/diff/git (CLI/viewer).

**The file finder is a given of the build, never a node** (agent K).
`kpse::Kpathsea` is `tfm::Kpse` over partex-kpse: kpathsea's own lookup
(texmf.cnf, the `ls-R` databases, `kpse_find_file(name, format, false)`
as the program `Build::set_kpse` names: `kpsewhich -progname=PROG
-format=FORMAT NAME` is its oracle, `tests/kpse.rs`), each `KpseFormat`
kpathsea's of the same name. The CLI makes one per engine and tree
(`tex::Tree`; a format's build one that never searches `.`) and gives it
to every build: `\input`/`\openin` names (format `tex`, after the input
directories: web2c's `-output-directory`), TFM files (`tfm`) and PDF
output's files (`map`, `type1 fonts`, `enc files`… by extension) are
database lookups, and no build walks TeX Live's tree. Where TeX Live is
(`SELFAUTOLOC`, kpsewhich on `PATH`) is the caller's to find, never the
library's, which runs no process (`tests/wall.rs`): the CLI always gives
the finder, the `phitex-flow` binary unless `--no-kpse` (`--kpse PROG`
names the program), the tests through `tests/common` (`give_kpse`; off
with `PHITEX_NO_KPSE=1`). A build given no finder walks TeX Live's
directories as before (`tfm::Index`, `tfm::pdf_file`): the fallback.
What a lookup finds changes nothing about reads: the path's bytes are the
`Key::File` read exactly as before (a font's, a PDF file's, as the build's
`Env::font_file` and the fold's `Files`), so the graph keeps its shape.

Never link, deleted: partex-core, partex-engine, partex-incr, partex-ssa, partex-trace, phitex-ir, partex-cli; copy pure bodies only, naming the source file in the commit.

## The command set

Every TeX82 primitive (325: partex-core's `init_prim_tex`, less encTeX's 14),
classified by what it is on the graph (AGENTS_BRIEF.md): **(i)** an
expandable primitive or input rewriting — a *step* inside a node, which
rewrites the input (`RState.pending`) and defines nothing; **(ii)** an
assignment or scope — a *node* whose results are definitions of `Key`s;
**(iii)** material — a node that emits items; **(iv)** layout and output;
**(v)** IO and diagnostics. The table at the end lists each one: its
tex.web command code, its graph form and its status here.

| class | primitives | done | partial | not yet |
|---|---|---|---|---|
| (i) expandable, input rewriting | 40 | 37 | 3 | 0 |
| (ii) assignments, scopes | 151 | 148 | 3 | 0 |
| (iii) material | 73 | 72 | 0 | 1 |
| (iv) layout and output | 43 | 31 | 12 | 0 |
| (v) IO and diagnostics | 18 | 15 | 1 | 2 |
| all | 325 | 303 | 19 | 3 |

("done": tex.web's semantics, checked against `tex -ini` by `tests/core.rs`
(logs of programs that do not typeset) and `tests/dvi.rs` (DVI files and
logs of typeset documents, byte for byte); "partial": the parts that fit,
with what is missing in the status column.)

## Checklist (every agent)

- Every field added to `RState` is in `eq_but_pos` (or in
  `same_but_pos` if it is positional): a field left out is a state two
  runs compare equal while TeX would differ, and an edit keeps the wrong
  nodes.

## Where each quantity lives

There is no eqtb, save stack, nest or input stack. Each quantity TeX keeps
in a table is a `Key` with a reaching definition over the nodes (scoped by
the `Open`/`Close` nodes; a close defines nothing; `\global` reaches
everything after); each quantity of the list being built or of the reader
is a field of `RState`, the value a node hands its successor.

| tex.web region | here |
|---|---|
| control sequences, active characters (eq_type/equiv) | `Key::Cs(name)`, `Key::Active(c)` → `Val::Meaning` (`Prim`, `Macro{params, body, long, outer}`, `Char(cat, c)`, `CharGiven`, `MathCharGiven`, `CountRef/DimenRef/SkipRef/MuskipRef/ToksRef(n)`) |
| `\catcode`, `\lccode`, `\uccode`, `\sfcode`, `\mathcode`, `\delcode` | `Key::Cat(c)` (`Val::Cat`), `Key::Lccode/Uccode/Sfcode/Mathcode/Delcode(c)` (`Val::Int`) |
| integer parameters (55) | `Key::IntPar(IntPar::…)` (`Val::Int`) |
| dimension parameters (21) | `Key::DimenPar(DimenPar::…)` (`Val::Int`, sp) |
| glue parameters (15) and muglue parameters (3) | `Key::GluePar(GluePar::…)` (`Val::Glue`; `GluePar::is_mu`) |
| token list parameters (9) | `Key::TokPar(TokPar::…)` (`Val::Toks`) |
| `\count`, `\dimen`, `\skip`, `\muskip`, `\toks` n | `Key::Count/Dimen/Skip/Muskip/Toks(n)` |
| `\box` n | `Key::Box(n)` (`Val::Void`, or `Val::Box(Arc<BoxNode>)`: dimensions, glue setting, contents) |
| `\parshape` | `Key::Parshape` (`Val::Parshape`) |
| current font | `Key::Font` (`Val::Font(Arc<Font>)`; initially the null font) |
| a font (`font_info`…) | `Arc<Font>`: the TFM's data at a size, a pure function of the file's bytes and the size; its identity is its name and size (`Font::key`, `cmr10 655360`), so equal loads are equal values |
| whether a font is loaded (`font_ptr`'s fonts) | `Key::FontLoaded(font key)`, global: `\font` reads it and defines the font's parameters only at the first load (tex.web 1260) |
| internal font numbers | none in values: the DVI fold numbers the fonts by their first `Key::FontLoaded` definition in document order (`dvi::font_order`) |
| a font identifier `\f` | `Key::Cs("f")` = `Meaning::Font(Arc<Font>)` |
| `\fontdimen` n of a font | `Key::FontDimen(font key, n)` (always global; defined for every parameter at the first load) |
| `\hyphenchar`, `\skewchar`, `font_id_text` | `Key::HyphenChar(key)`, `Key::SkewChar(key)`, `Key::FontIdent(key)` (global) |
| `\textfont`, `\scriptfont`, `\scriptscriptfont` | `Key::FamFont(size, fam)` (`Val::Font`) |
| the current list (`head`…`tail`), `nest` | none: each node emits items (`Node.emit` horizontal, `Node.vemit` vertical; at the outer level `Node.para`), and the node ending a paragraph or a box gathers them; `RState.level` (boxes open), `RState.inner`, `RState.mode` say which list a node emits into |
| `cur_box`, `box_context`, the saved state of `package` | `RState.groups` entries' `BoxGroup` (kind, context, spec, the enclosing list's mode/space factor/prev depth), handed by the box's `Close` node to its packaging node in `RState.boxing` |
| the contribution list, the current page, `page_so_far`… | the page fold (`page::Scan`): over the outer vertical material in document order, at `Item::BuildPage` (what TeX reads where it calls `build_page`) |
| `dvi_buf`, `font_used`, `last_bop`… | the DVI fold (`dvi::assemble`) over the pages |
| `cur_group`, the save stack's group boundaries | `RState.groups` (kind, for mismatch errors; and the `\aftergroup` tokens of each open group) — scoping itself is `Chain::resolve` |
| `align_stack`, `preamble`, `cur_align`, `cur_span`, `cur_loop`, `align_state` | the preamble a value in the alignment's group entry (`align::AlignSpec`, with the enclosing list's state and alignment); the entry's column, span, `\omit`, end and `align_state` in `RState.al` (Conflict 25) |
| `error_count`, `history` (fatal) | `RState.error_count`: counted by every error, 0 again when a paragraph ends (§1096); the hundredth stops the reader and the build (Conflict 28) |
| `cond_ptr`, `if_limit` | `RState.conds`: per open conditional, the node that opened it, which ends may come, its test |
| `\global`/`\long`/`\outer` prefixes | `RState.global/long/outer`, set by the prefix node, consumed by the assignment node |
| `after_token` (`\afterassignment`) | `RState.after_assignment` |
| mode, `space_factor`, `prev_depth`, `prev_graf` | `RState.mode`, `RState.space_factor`, `RState.prev_depth`, `RState.prev_graf` |
| `end_line_char` as the line was read | `RState.eol` (read when a line is begun, as TeX's `input_ln` does) |
| the input stack, `back_input`, `ins_list` | `RState.pending` (rewritten input; `Tok::DontExpand` is `\noexpand`'s mark) |
| a math list (`head`…`tail` in math mode), `incompleat_noad` | none: math nodes emit `Item::Noad(MItem)` entries (noads, style changes, choices) and what the list's gatherer applies to its last noad (`Set`: a field filled; `Replace`; `Limits`; `Above`: `\over`'s split); the packaging node after a math group's `Close` gathers them (`mlist::gather`, tex.web's `fin_mlist`) |
| math mode, `cur_fam`, a subformula's `saved(0)` | `RState.mode = M` (`inner`: an inline formula or subformula; a display is not inner); `\fam` is `Key::IntPar(Fam)`, defined `-1` inside a formula by `\0mathinit`; the field a subformula fills is its group record's `math::Target` |
| `cur_style`, `cur_size`, `cur_mu`, `cur_f`… | inside `mlist::mlist_to_hlist`, a pure function of the list, the style, `mlist::Params`, the 48 family fonts and their `\fontdimen`s and `\skewchar`s |
| `\message`, `\immediate\write` output | `Node.log`, gathered in document order into `Output.log`; `print::log` lays it out as TeX's log |

The initial values (`val::initial`) are exactly IniTeX's (tex.web 222-240;
the current font is `\nullfont`), but `\year`, `\month`, `\day`, `\time`:
1970, 1, 1, 0 (what `SOURCE_DATE_EPOCH=0` would give; IniTeX reads the
clock). Documents declare what they need: `tests/core.rs`'s prelude goes to
both TeX and the graph; `tests/flow.rs`'s `doc()` begins with its catcodes,
`\font\tenrm=cmr10 \tenrm` and its parameters.

## Conflicts with the design, and graph-shaped proposals

1. **`\aftergroup`** — fits: the tokens are kept per open group in
   `RState.groups`; the `Close` node rewrites the input with them (input
   rewriting, no definition). Done.
2. **`\afterassignment`** — fits: `RState.after_assignment`, consumed by the
   next assignment node, which rewrites the input. Done.
3. **`\outer`** — TeX checks every token met while scanning an argument, a
   definition, a skipped arm or a balanced text against "is it an outer
   macro". Done literally, every scanned token would be a read of its
   meaning: a node's operands would include every name in every argument.
   Proposal: one value `Key::OuterNames` (the set of names ever bound to an
   `\outer` macro, defined globally — as a union — by each node defining an
   outer macro); a scanning node reads it once and checks tokens locally.
   Its version changes only when a new outer macro appears (plain's
   `\new…`, at format time). Now: the flag is kept, `\meaning` and `\ifx`
   see it; the checks (errors only) are not made.
4. **`\input`** — a node whose result is text that becomes pieces: design
   conversation, not implemented. Proposal: a file is a value
   `Key::File(name)` (content-versioned); the `\input` node reads it and its
   result is a call: the file's text cut into pieces that run between this
   node and the next, their first reads resolving at the call. Line numbers
   are a field of the position, never in a version. Decided: see
   "`\input` (decided)" below.
5. **`\endinput`** — stops reading a file at the end of the current line:
   a node changing which pieces of that file exist. Belongs with 4: the
   call's result is the text up to the line of the first executed
   `\endinput`. Not implemented (error).
6. **`\openin`/`\read`/`\ifeof`** — a read stream is a position in a file:
   `Key::ReadStream(n)` = (file value, line), defined by `\openin` and by
   each `\read` (which also defines the macro). Done: see "IO" below.
   `\input` and `\endinput` (4, 5): the TeX side is done, the file read in
   the piece until the core cuts it (IO).
7. **`\setbox`/`\box`/`\copy`/`\wd`** — a box register's value is material:
   `Val::Box(Arc<BoxNode>)`. Conflict: `\box n`, `\unhbox n` (use) empty
   the register and `\wd n=` changes it *without* respecting grouping
   (tex.web 1079, 1110, 1247: `box(n):=null`, `mem[box(n)+c]:=…` are not
   `eq_define`s): after `{\box0}` the register is void even if it was set
   outside the group. Decided and done: one general rule, a definition
   **at the level of the definition it replaces** (`Def.level`): the node
   defines `Key::Box(n)` with the scope of the reaching definition it
   read — global when that one was global, made at the piece's outer
   level or before the piece (pieces are cut at depth 0, so no group
   spans pieces), else living as long as that definition's group
   (`Def.level = Some(k)`: the `k`-th group enclosing the node's own,
   counted from the node so that it survives nodes moving;
   `Chain::resolve` treats it as alive while that group is open,
   `Chain::def_group`). `Exec::change_box` makes it for `\box n`,
   `\unhbox n`, `\unvbox n` and `\wd n=` (`tests/core/boxlevel.tex`);
   the fold's `\box255` and insertion boxes will use it too. For agent A
   (merge): `Def` has the new field `level` (`None` everywhere else) and
   `Chain` the map `parent_of` (`Open` node → enclosing `Open` node),
   kept by `push_arc`; `run_piece` is unchanged. Gap: a kept node is
   validated by the *values* it read, not by the scope of the definition
   it read, so an edit that only moves the replaced definition into
   another group (same value) keeps the old level.
8. **`\global` on registers** — fits: `Def.global`; TeX's restore rules
   (a global assignment inside a group survives; a later local one in the
   same group is undone) are exactly reaching definitions (checked in
   `tests/core/groups.tex`).
9. **`\prevdepth`, `\spacefactor`, `\prevgraf`** — state of the list being
   built: fields of `RState` (values, not Keys), assigned and read there.
   Conflict: their natural updates come from material — `\spacefactor` from
   each character's `\sfcode` (a read of `Key::Sfcode(c)` by each character
   node: fits), `\prevdepth` and `\prevgraf` after a paragraph from the line
   breaker's result, which is a separate task. Proposal: after `\par`,
   RState holds "the depth / line count of paragraph P" (a reference to the
   `Par` node's result); a node that reads them (or the next interline glue)
   reads the paragraph's lines and parks on the line-break task (the
   brief's "blocking only when something reads the lines' dimensions").
   Now: `\spacefactor` is exact (each character node reads its `\sfcode`,
   a paragraph never crosses a piece). `\prevdepth`: the interline glue is
   exact — the gatherer of each vertical list (the page fold for the main
   one) tracks the depth (`Item::Append`, `Item::PrevDepth`), so material
   never reads it; a node *reading* `\prevdepth` sees `RState.prev_depth`,
   which after a paragraph on the main vertical list is not the last line's
   depth (gap, as is `\prevgraf` there; inside a box both are exact).
10. **`\futurelet`** — fits: the node reads the second token's meaning,
    defines the name, rewrites the input with both tokens. Done.
11. **`\csname` of an undefined name** — TeX defines it `\relax` (local):
    a definition made by a step; it is a result of the node the step is in
    (and `Kind` stays that node's command). Done.
12. **Page builder state**: `\pagegoal`, `\pagetotal`…, `\deadcycles`,
    `\insertpenalties`, `\outputpenalty`, the marks (`\topmark`…). TeX reads
    them synchronously; here the page builder is a scan over dimensions.
    Proposal: such a read is a read of the page scan's state at the
    reading node's position — it parks until the scan has consumed all the
    material before it (the true dependency). The page builder also
    *defines* `\outputpenalty` (globally, tex.web 1013): with `\output`
    that becomes a definition made by the fold at the firing call, read by
    the output routine's nodes (19). Now: they are `Key`s (`Key::Page`,
    `Key::Mark`) an output routine's run defines at its start ("The TeX
    side of output routines"); elsewhere an empty page's values and empty
    marks until the fold defines them.
    **Decided (coordinator, 2026-10-10; after re-cuts, before `\input`
    in groups): a mid-stream read of a page quantity is a read of the
    fold's output at that position.** `\pagegoal`…`\pagedepth`,
    `\pagestretch`…, `\deadcycles`, `\insertpenalties` exist twice in
    TeX: as the page builder's state (`page_so_far`, §982, read by
    `\the` through §421) and as what an assignment sets (§1245
    `alter_page_so_far`, done: `Item::PageSoFar`, applied by the fold when
    it receives it). The read outside a routine is a `ReadRef` on the scan
    at `(piece, node)`: under Suspend it parks until the scan has consumed
    every item emitted before that node, and gets the scan's value there;
    under Speculate it predicts the last build's value at that position,
    and the scan validates it when it gets there (a refuted prediction
    marks the reader, like any changed read). It is **not** the fold
    writing the names index as it scans (definitions at every position
    would be a second, sequential history of the page state, and every
    reader after it would read the fold's writes): the scan is asked,
    at one position, by the nodes that read it.
13. **The current list's last item**: `\lastpenalty`, `\lastkern`,
    `\lastskip`, `\/` read it; `\unskip`, `\unkern`, `\unpenalty` remove
    it. Done: the removals emit `Item::Remove`, which the list's gatherer
    (the paragraph's `\par`, a box's packaging node, the page fold)
    applies. The reads are **the one read of material**
    (`Exec::last_item`, `Exec::current_list`): the node gathers the current
    list from the items the nodes before it emitted (back to the list's
    start: the paragraph's or box's first node; on the main vertical list,
    back to this piece's last paragraph) and takes its last item. Not a
    `Key`: the node becomes `Kind::Par`, so it runs again whenever a node of
    its paragraph does (a conditional reading it keeps its kind — then a
    stale value is possible: a gap). On the main vertical list after a
    paragraph's lines, or at a piece's start, the last item is the value
    of `Key::MainTail` (`Val::Tail`): each node at the outer level that
    contributes to the list defines it globally (`material::main_tail`: a
    paragraph's lines count as a box, whatever line breaking makes), so
    a node reading it from another piece reads it as a name (LaTeX:
    `\addvspace`'s `\lastskip` after a list, at the next piece's start).
    Gaps: after a removal (`\unskip`…) in a node of its own the key is
    none, not the item before the one removed; TeX's `\lastpenalty` of a penalty
    the page builder has just set to 10000 is not seen.
    `\lastbox` (a removal whose value the node needs): done, the same read
    and an `Item::Remove(3)`.
14. **Groups and conditionals across a piece boundary** (a blank line inside
    `\begingroup…\endgroup` or an `\if…\fi`; `\obeylines` or a changed
    `\endlinechar`, where a blank line is not a `\par`): the cut assumes the
    start state at every blank line. Done (cuts, below): a piece after a
    cut is entered in the state the piece before it leaves there (a
    conditional open, a prefix pending: an entry edge, assumed canonical
    until that piece has run); a piece leaving a group, a box or a
    paragraph open, or whose last command reads on past its text, is one
    piece with the text after it. (A group open across pieces needs the
    names index to end a definition's reach at a close in a later piece:
    not yet, so such pieces are joined — decided for now, coordinator
    2026-10-10. The alternative for later, when a document has a large
    group whose inside is not a serial chain: visibility by positions and
    levels — a local definition at level L made in piece k reaches
    position m iff every piece entry between k and m has depth ≥ L and
    m's current depth ≥ L; the entry depths are known once the
    predecessors settle, so it is a reaching-definition rule, not a
    restore.)
15. **`\mag` and `true` units** — TeX freezes `\mag` at its first use
    (`mag_set`, a hidden global). Done (F, TRIP): `Key::MagSet`, defined
    globally by `prepare_mag` (a `true` dimension, or a `\shipout` while
    none is fixed); the postamble's magnification is the first page's.
16. **`\show`, `\showthe`, error contexts** — `\showbox` and `\showlists`
    are done (`graph/diag.rs`): their text is a raw log entry whose line
    numbers are placeholders the log's gatherer fills (`G` k: where the
    `k`-th enclosing group's list began; `Q` level: a paragraph's first
    line; `X`, `T`: an error context's two lines, laid out with
    `error_line` 79 and `half_error_line` 50, tex.web 317), so no line
    number is in a node's result. The context shows a backed-up list at
    the top (`<to be read again>`, `<recently read>`: a list read to its
    end stays in `RState.pending` until the next token is read, as TeX's
    input level does), `...` for the levels between (IniTeX's
    `\errorcontextlines` is 0), and the reader's line; a macro's level at
    the top is not named yet. `\showlists` shows each open list (the main
    vertical list: what this piece contributed since the page builder was
    last called); the current page (`### current page:`) is the page
    fold's state and is not shown — exact where the page is empty.
    `\show` and `\showthe` are not yet.
17. **The DVI file is a fold**: TeX writes a font's definition where it is
    first used, reuses `w`/`x`/`y`/`z` movements only within what its
    half-buffer (`dvi_buf_size` 16384) still holds (`dvi_gone`), points each
    `bop` at the page before, and writes the postamble's fonts last-loaded
    first. Every page's bytes depend on the pages before, so `dvi::assemble`
    writes the whole file in one sequential pass over the pages (cheap next
    to building them); `dvi::page_body` is empty. Proposal for incremental
    shipping: a page's bytes as a function of (page, the fonts defined
    before it, `dvi_gone` at its start) — equal inputs, equal bytes.
18. **`\output`** (not implemented; the first proposal, superseded by the decisions below): the page fold, firing,
    defines (as a node would) `\box255` = the page box, `\outputpenalty`,
    `\topmark`/`\firstmark`/`\botmark` and `\insertpenalties`; the output
    routine's tokens are read as a piece of their own — its nodes resolving
    names at the firing call (a position in the outer vertical material:
    after the node carrying the firing `Item::BuildPage`) — and its result
    (what it ships, the material it puts back, its definitions, which reach
    the nodes after that call) is fed back to the fold: contributions first
    (tex.web 1026). Readers after the firing call of anything the routine
    defines park on the fold reaching that call, which is the true
    dependency. Without `\output` the default output ships `\box255`
    (done, tex.web 1023). Decided: see "Output routines, insertions and
    marks (decided)" below.
19. **`\the\font`** gives TeX's frozen font identifier (`font_id_base+f`,
    printing as the name the font was last defined by, meaning the font).
    Here it gives the name itself (`Key::FontIdent(key)`), whose meaning is
    that font unless the name was redefined since: approximation.
20. **`\badness`** is the badness of the last box packed anywhere — a read
    of the packager's history (lines are packed by another task, pages by
    the fold). Proposal: like Conflict 12, a read parking on the producer
    of the last packing before the node. Now: 0.
21. **`\end`** stops TeX: the `\end` node emits `Item::End` and stops
    reading its piece, but the pieces after it still run (their messages
    would appear in the log); the page fold ignores material after `End`.
    A document whose last piece ends without `\end` is finished as if it
    had one (TeX would stop at its terminal prompt). The log's
    `(\end occurred inside a group…)` lines are written, and the pieces
    after the one holding `\end` are left out of the log, the errors and
    the written files; the core is to not run them (`ends_document`: IO).
22. **The DVI comment and the date**: TeX Live's `tex` takes the
    preamble's comment date from the clock (it ignores
    `SOURCE_DATE_EPOCH`), the graph from `\year`… (1970.01.01:0000
    initially). `tests/dvi.rs` masks the comment's bytes (15 to 41) when it
    compares files; everything else is compared byte for byte.
23. **Hyphenation**: done (see Hyphenation below). `\noboundary` is not
    yet; `\accent`, `\vadjust`, `\insert` (the item), leaders, `\special`
    and deferred `\write`s are done (IO, the remaining material).
24. Small departures, documented: `\if…`'s inserted `\relax` (a `\fi` met
    while the test is read) is the name `\relax`, not a frozen one; a
    macro's parameter shows as `#n` whatever its parameter character; a
    `^^` reduction is not reduced again.
25. **Alignments** (`graph/align.rs`). `align_stack`, `cur_align`,
    `cur_span`, `cur_loop`, `align_state`, `preamble` are values: the
    preamble (`AlignSpec`) in the alignment's group entry of
    `RState.groups`, read once by the node after `\halign`; the current
    column, span, `\omit`, how the entry ended and `align_state` in
    `RState.al`, saved in the spec of an alignment begun inside an entry and
    given back at its end. `cur_loop`'s growing of the preamble (§793) is
    the spec's `col_of`: column `j` past the preamble has the templates and
    tabskip glue of `loop + (j-n) mod (n-loop)`; how many columns there are
    is `fin_align`'s to find in the rows. Widths are never accumulated:
    each entry is an unset item (its column, its span), and `fin_align`'s
    node computes them from all the rows. A `\tabskip=` in a preamble is,
    as in tex.web 782, a definition (`eq_define`, in the alignment's group:
    cells see it in `\the\tabskip`), and the glue the spec keeps for the
    columns after it is the value. `align_state` is TeX's: braces counted
    as `next_raw` reads them while an alignment is open, a token put back
    by `back` uncounted (§325), the u template's end a mark token that sets
    it to 0 (§324); a tab or `\cr` read with it 0 — by any node, in any
    scanning — inserts the v template, as `get_next` does (§342). Gaps:
    `\csname` of an undefined name in an entry's first token defines
    `\relax` in the enclosing group (the peek is in the node opening the
    entry's group); `\aftergroup` tokens are not counted as `back_input`
    would count them; "interwoven preambles" is an error of the graph, not
    TeX's fatal error; an entry ended inside a group of its own (`off_save`
    at `\endtemplate`) is not supported.
26. **Display alignments** (`$$\halign{…}$$`, §1145, §1206) live in the
    display's math group (Math, below). `\halign` in a display whose
    formula is empty is `init_align` with the depth of the vertical list
    before the display (kept in the math group's record) as the alignment
    list's; when it is set, `fin_align` makes the group record's kind
    `MathKind::DisplayAlign` (the rows, shifted by `\displayindent`, and
    the depth after them): a definition of the group's field, no emit.
    After it only assignments come before `$$` (`do_assignments`): the
    `$` node checks the second `$` ("Display math should end with $$"),
    anything else is read again after "Missing $$ inserted" (TeX's logs,
    Conflict 27); the display's parameters are read inside the group
    (`math_end`), its `Close` emits nothing, and the packaging node emits
    `\predisplaypenalty`, `\abovedisplayskip`, the rows,
    `\postdisplaypenalty`, `\belowdisplayskip` and resumes the paragraph
    (§1200, the same `resume_after_display` as a formula's). Gap: an
    `\halign` after material in the display is an error of the graph
    (TeX's "Improper \halign inside $$'s" flushes the formula).
27. **TeX's errors in the log** (alignments, displays): the message, the
    help and the context are TeX's (tex.web 72–90, 311–318: the two lines,
    `half_error_line` 50, `error_line` 79, `\errorcontextlines`). The input
    levels shown are those the alignment code knows are there (the
    inserted and put-back tokens, the v template, the token TeX puts back
    after a character's lookahead read `\endtemplate`); a level the node
    did not make (a macro's body the token came from) is not shown. The
    line number is the line the node ended on (`LINE L`), which is the
    error's line unless the node read past the line's end after it.
    "in alignment at lines a--b" takes `a` from the line the `\halign`
    node ended on (after its `{`), where TeX takes the line of `\halign`.
28. **The hundredth error** (tex.web 82): `error_count` is a field of
    the reader's state, handed from node to node like the mode; every error
    counts (TeX's, and the graph's own "not supported yet"), and the end of
    a paragraph sets it to 0 (`end_graf`, §1096). The node that makes the
    hundredth writes TeX's "(That makes 100 errors; please try again.)"
    instead of the help, and from its state on nothing is read (`next_raw`
    gives nothing, the next node is the piece's `End`, which breaks no
    paragraph); it emits `Item::Halt`, at whatever level it is, into the
    page fold's stream, which ends there without `its_all_over` (`jump_out`:
    the page so far is not shipped). The log ends at that line (no ` )`:
    the file is not closed). A loop of errors is therefore bounded, as in
    TeX. Gaps: a piece starts with the count at 0 (TeX's count goes on
    across a blank line in vertical mode), and the pieces after the halt
    still run, as after `\end` (Conflict 21): their errors are in the
    build's list, not in the log.

## IO: files, streams, writes, `\input`, `\end` (agent E)

Files are values; nothing opens or writes a file while commands run
(`graph/io.rs`; the shapes gate holds: no file handle or stream object
lives across commands).

| TeX | graph |
|---|---|
| a file's bytes | a read of `Key::File(name)` → `Val::Bytes` (or `Val::Void`), answered by the `Env` (`Env::file`, a pure function of the name during a build: `Build::set_input_dirs`, read once per build). The TeX side does kpathsea's rule for inputs: `name.tex` first unless the name ends so, then `name` — each try a read. A changed file marks exactly its readers (the build compares the reads of every `Key::File` with the file at each update) |
| `\jobname` | a read of `Key::Job` (`Env::jobname`, `Build::set_jobname`; `texput` by default) |
| `read_open[n]`, `read_file[n]` | `Key::ReadStream(n)` → `Val::Stream{data, next, normal}` (the file's bytes and where its next line begins) or `Val::Void` (closed); always defined globally (no save stack in TeX) |
| `\openin n=name` / `\closein n` | a node defining `Key::ReadStream(n)` (§1275) |
| `\read n to \x` | a prefixed node (§1225, §482–486): reads `Key::ReadStream(m)`, tokenizes the next line with the codes and `\endlinechar` in force (the line is a text the reader enters above the pending input — an `RState::texts` entry, `TextKind::Read` — and leaves at its end), more lines while braces are open, the rest of a line after an unmatched `}` dropped; defines `\x` (a macro without parameters) and the stream anew, advanced (a new value); at the file's end the stream is closed and the line is empty (a `\par`), with "File ended within \read" if braces were open; a non-control-sequence target is "Missing control sequence inserted" (and `\inaccessible` defined); stream 16 or a closed one is the terminal: an error (batch mode) |
| `\ifeof n` | a read of `Key::ReadStream(n)`: closed? |
| `write_open[j]`, `write_file[j]` | none while commands run: events, replayed by the log's gatherer (`logs::replay`) in document order |
| `\immediate\openout/\write/\closeout` | the node's log holds an event (`logs::io_event`: kind, stream, payload); a `\write`'s text is expanded in the node (§1370, no mode, `\newlinechar` a line break, unprintables `^^`) |
| `\openout`, `\write`, `\closeout`, `\special` | whatsit items (`Item::Whatsit`) in the current list, in any mode; a `\write` holds its tokens unexpanded, a `\special` its expanded text |
| `ship_out`'s `out_what` (§1374) | an explicit `\shipout`: its node does the box's whatsits (DVI order, none inside leaders) — its `\write`s expanded with the definitions reaching it — between its `[n` and `]`. A page the page builder ships: at the join, `graph::page_io` expands its `\write`s with the definitions reaching the node whose `BuildPage` (or `End`) shipped it (`logs::page_places`: the call's node from the log fragments' marks; a pure function of the box, the place and those definitions: a candidate for a task and memo) |
| the files written | `Output::writes`: (name, contents) per file, in the order first opened; `\openout` adds `.tex` to a name without extension (web2c's `\openout j = `name'.` log line, at the replay: on in extended mode — `etex` and pdfTeX — off for `tex`, texmf.cnf `log_openout.tex = f`); a `\write` to a closed stream, 16 or 17 is a log line |
| `\special` in the DVI file | `xxx1`/`xxx4` after `synch_h`, `synch_v` (§1368) |

**`\input` (decided design, TeX side complete; the core's part is A's).**
`\input` met at the top is a node of its own (`next_x_m(top)` returns it,
as a conditional): it scans the name (§526), reads `Key::File` (the
kpathsea rule above), logs ` (./name` (§537), and its **result is the
file's text in its out state**: `node.out.texts` gains an entry
`PendingText { text, at, pos: 0, tstate: N, eol: EOL_UNREAD, eof: false,
kind: TextKind::File(name), endinput: false }` — the stack e-TeX's
`\scantokens` texts are on (one stack, so that any nesting of files,
`\scantokens` texts and `\read` lines reads in TeX's order) — `text` the
file's lines (`graph::file_text`: web2c's `input_ln`, trailing spaces
dropped, each line ending in `\r`, an empty file one empty line), `at` the
number of rewritten token lists pending at the call (`pending[..at]` is
read after the file); the caller's `tstate`/`eol` stay in `RState` while
the text's are its own. Met while another command scans, `\input` is a
step of that node (the same entry).

*Now (single-piece emulation, exact TeX semantics):* the reader reads the
entry's text in the same piece, above the piece's text (`Exec::txt`); at
its end (after `\everyeof`, unless `\endinput` ended it) it logs `)`,
pops it and goes on with `pending[..at]` and the rest of the calling
line. So the first file line and the rest of the calling line
carry the predecessor's state, as decided; nothing is cut.
*Interface for the core (A):* for a node `i` of piece `k` whose command
is `\input` (`Kind::Other`, `cmd` meaning `Prim::Input`) and whose
`out.texts` has one more entry than its input, `TextKind::File`, at
`pos: 0`:
1. cut the entry's `text` with `graph::cut` into pieces F1..Fm, inserted
   after node `i` of piece `k` in the order;
2. F1 runs with entry state `node.out` *minus that entry*, with `pending`
   empty, `tstate: N`, `eol: EOL_UNREAD`; F2..Fm keep the constant start
   state (their cuts stay canonical);
3. the piece R holding the rest of the calling line runs from Fm's `End`
   state with `pending = pending[..at] ++ Fm's pending`, `tstate`/`eol` the caller's,
   `pos` = `node.out.pos`, and its log begins with `)`;
4. `Key::LineBase` answered per piece is the piece's line offset *in its
   own file* (F1: 0).
With that, `RState::texts` holds `\scantokens` texts, `\read` lines and `\input`s met
inside another command's scanning (those stay in-piece: a node cannot
span pieces).
*`\endinput`* (§378): now the innermost text's `endinput` flag (an
`\input` file's or a `\scantokens` text's); it ends when the reader
passes the current line's end, without `\everyeof` (etex.web §362). In pieces: proposal —
`RState.endinput` set by the step when the file is a piece of its own; the
reader stops at that line's end (as `\end` stops), and the core drops the
file's later pieces (they ran on the guess that none occurs). In the main
file it is an error (not supported).

**`\inputlineno` (design).** A line number is never part of a version. A
node reading `\inputlineno` reads `Key::LineBase` — the lines of its file
before its piece, answered by the `Env` per piece (`Env::line_base`; the
build's is a prefix sum of the pieces' line counts, made at the first such
read) — and adds its line within the piece (from its position in the
piece's text). A line count changed above changes `Key::LineBase` only for
the pieces below; the build compares each reader's value at every update
and marks exactly the nodes that read it. Inside a file read in the
piece, the line is the file's (from the entry's position). The `\end`
node reads it too (the lines of the conditionals still open). *Gap (for
A's `run_piece`):* a node whose text and reads are unchanged is kept even
when lines were added before it *within its own piece*; proposal: a node
whose reads include `Key::LineBase` is kept only if its line within the
piece (`line_of(old text, old pos) == line_of(new text, new pos)`) is.

**`\end`, `\dump` (§1335).** The `\end` node (on the main vertical list)
logs what `final_cleanup` writes after the job's last page — a ` )` per
file still open, `(\end occurred inside a group at level n)`, and per open
conditional, innermost first, `(\end occurred when \ifx on line n was
incomplete)` — as an entry the log puts after the main file's ` )`
(`print::ENDJOB`); then it clears the groups, conditionals and files
(TeX pops them) and stops reading. `\dump` is `\end` plus `Beginning to
dump on file JOB.fmt` and ` (preloaded format=JOB y.m.d)` (the format is
the graph's prefix: no file); inside a group, TeX's fatal error.
*Interface for the core (A):* `graph::ends_document(run)` — the piece's
material holds `Item::End` — ends the document: the core is not to run
the pieces after it. Now: they run, but the build leaves them out of the
log, the errors and the written files (the page fold already ignores
material after `End`).

**Approximations (IO).** A `\special`'s text is printed with the escape
character at the `\special` (TeX prints at shipout); characters 128–255
written are not `^^`-coded; an `\input` file's diagnostics (box reports'
`at line n`) give the calling line; `\endinput` in the main file and
`\read` from the terminal are errors; a deferred `\write` of a page the
page builder ships is expanded at the join, serially (pure; not yet a
task); `\dump`'s format date is `\year.\month.\day` (TeX's is the clock's,
even under `SOURCE_DATE_EPOCH`: the `dump` test leaves the date out), and
the memory statistics TeX logs after it are not made.

## The remaining material (agent E)

| TeX | graph |
|---|---|
| `\leaders`, `\cleaders`, `\xleaders` (§1078) | `scan_box` with `BoxContext::Leaders(100..102)` (a box, or a rule: §1084); the glue after it (`\hskip`… in horizontal, `\vskip`… in vertical lists, else "Leaders not followed by proper glue") becomes `Item::Leaders{g, sub, b, zero}`: glue everywhere glue is (`Item::glue`, `Item::is_glue`: line breaking, the page builder, `\unskip`, `\lastskip`, `vert_break`, `prune_page_top`, §816's last glue), plus the box's height and depth in `hpack` (§656) and its width in `vpack` (§671); shipped as tex.web §626–628, §635–637 (aligned, centred, spread; rule leaders); shown as §190 |
| `\accent` (§1123–1125) | a node: the accent character; `do_assignments` (§1270: prefixed assignments in this node, `\setbox` refused); the next character in the font then current; kerns `(for accent)` by both fonts' slants (`\fontdimen1`) and the accent font's x-height (`\fontdimen5`, read as Keys), the accent boxed and shifted when the heights differ |
| `\lastbox` (§1080–1081) | the one read of material (Conflict 13): the current list's last box, unless a discretionary replaces it, is the node's value (unshifted) and an `Item::Remove(3)` its gatherer applies; on the main vertical list TeX's list is the contributions since the last page-builder call (none: "You can't use `\lastbox' in vertical mode") |
| `\vsplit n to h` (§977–979) | `split::vsplit`, a pure function of the box's list, `h`, `\splitmaxdepth`, `\splittopskip` (`vert_break` §970–976, `prune_page_top` §968); the part split off packed to `h` (reports with `\vbadness`, `\vfuzz`), the rest packed naturally into the register at the scope of the definition it replaces (Conflict 7); `Key::SplitMarks` — every class's first and last mark in the part split off (`Item::Mark` is class 0, e-TeX's `Item::Marks` the others), one `Val::SplitMarks` — defined globally by every `\vsplit` (which clears every class's, etex.web's `vsplit_init`), read by `\splitfirstmark`, `\splitbotmark`, `\splitfirstmarks n`, `\splitbotmarks n` |
| `\vadjust`, `\insert n` (§1097–1100) | a group like a box's (`BoxKind::Insert(n, _)`, 255 for `\vadjust`), `normal_paragraph` inside; its `}` reads `\splittopskip`, `\splitmaxdepth`, `\floatingpenalty` inside the group; the packaging node emits `Item::Adjust(list)` / `Item::Ins{n, height+depth, depth, split_top, float_cost, list}` (reads nothing else: no register of the class); `hpack` with an adjust list moves `Ins`, `Mark`, `Marks` and `Adjust`'s material out (§655): after a paragraph's line (`linebreak`'s `Line.adjust`, no change there) and after an `\hbox` appended in vertical mode; on the main vertical list the page builder is called (§1100) |
| `\topmark`, `\firstmark`, `\botmark` (and e-TeX's `\topmarks n`…) | not wired: the page builder's marks are A's (marks are `Item::Mark` / `Item::Marks(n, …)` in the lists the page fold sees); they stay empty (approx) |

**Approximations (material).** The page fold does not handle insertions
(A/B: an `Item::Ins` on the main vertical list is contributed to the page
and ignored there; TeX moves it to `\box n` and its size into
`\pagegoal`); `\lastbox` on the main vertical list sees the items
emitted since the last page-builder call in its piece (TeX's
contributions: exact but for a kern the page builder left there, §1000).

## Math

Parts 34–36 and §1136–§1206 of tex.web, copied: `mlist.rs` (the noads and
`mlist_to_hlist`, §680–§767, a pure function) and `graph/math.rs` (the
commands). Checked by `tests/dvi.rs`: `math1`–`math4` (inline formulas,
scripts, fractions and generalized fractions, radicals, operators with
and without limits, `\left`/`\right` with extensible delimiters, accents
with skews, styles, `\mathchoice`, `\nonscript`, `\mskip`, `\mkern`,
`\vcenter`, boxes in formulas, `\mathsurround`, line breaks inside and
after formulas, lines with formulas in their reports, displays with
`\eqno`/`\leqno`, short skips, the squeeze, an equation number on its
own line, displays under `\hangindent` and `\parshape` and in a `\vbox`)
at all five schedules, and `plainmath` (plain.tex's math macros) at one
thread (eight: the `run_piece` sharing bug, as `plaintext`). DVI and log
byte for byte against `tex -ini`.

- **A formula is a group.** `$`, a subformula's `{`, `\left` and each
  `\mathchoice` part open a math group (`GroupKind::Math`, its record a
  `math::MathKind`). The group's `Close` node (`$`, `}`, `\right`)
  defines and emits nothing: it reads what TeX reads before the group
  ends — `\mathsurround`, the 48 family fonts, the spacing and penalty
  parameters, for a display its width, indent, size, skips, penalties,
  interline glue and packing parameters (`math::MathEnd`) — and hands it
  to the packaging node after it, which gathers the list the nodes inside
  emitted and converts it. The family fonts' `\fontdimen`s are read
  lazily by the packaging node (global values).
- **A noad appended before a group opens.** Where TeX appends a noad to
  the enclosing list and then opens a group for one of its fields (`^{`,
  `\mathop{`, `{` in math, `\radical…{`), the node emits the noad and
  hands the group to the next node (`\0mathgroup`), which opens it: a
  node's items are in the list of the level it ends at. The packaging
  node emits `MItem::Set(field, list)` (§1186: a lone ord noad gives its
  nucleus; a lone accent noad replaces an ord noad whose nucleus it was).
- **`^`, `_`, `\limits`, `\over` read the list** (`sub_sup` looks at the
  last noad, §1176): the node gathers its math list from the items before
  it, the one read of material (Conflict 13), and becomes `Kind::Par`.
- **A display** is three nodes: the `$$` node ends the paragraph so far —
  its lines are a separate task's at the outer level, and the node breaks
  them too (the same pure function), for `\predisplaysize` (§1146) and
  `\prevgraf`; `\0display` opens the group with `\displaywidth` and
  `\displayindent` from the shape (§1149); `\0mathinit` defines them and
  `\fam` inside it. After an equation number's group, its packaging node
  keeps the box in the display's record and closes the display
  (`\0displayend`). The display's packaging node emits the vertical
  material (§1199–§1205) and resumes the paragraph (§1200, `\prevgraf`
  + 3), which a later `\par` breaks with that `\prevgraf`.
- **Math nodes** (`Item::Math`, `\mathon`/`\mathoff`): widths in `hpack`,
  movements in the DVI fold, break points in line breaking (§866: no
  break at glue inside a formula, a break at `\mathoff` before glue, its
  width zero at a break), `$` in the short display, `\mathsurround` in
  `show_box`. `\ifmmode` is true in math.
- `\showlists` and `\showbox` display math lists (§690–§698: noads,
  fractions, delimiters, styles, choices, `\mskip`, `\mkern`,
  `\nonscript`, an incomplete fraction "this will begin denominator of:"),
  checked by `tests/dvi/showmath.tex`. The errors of a formula are TeX's
  in TeX's order (`tests/dvi.rs`'s `math_errors_in_order`): the font check
  of §1195 is read by the closing `$` inside the group, before "Display
  math should end with $$", and again for a display after its equation
  number; `off_save` in math inserts what closes the innermost group
  (`$`, `\right.`, `}`, `\endgroup`, tex.web 1064).
- Display alignments (`$$\halign…$$`): Conflict 26.
- Not yet: `\accent` in math (and outside), `\vadjust` and `\insert` in
  formulas. Approximation: the check for enough symbol and extension font
  parameters (§1195) counts the parameters the font was loaded with (TeX
  also counts those `\fontdimen` added to the last loaded font); `\right`
  inserted by `off_save` is the name `\right`, not a frozen one.

## Hyphenation

Knuth's algorithm as pure functions (`hyph.rs`, tex.web §891–965), checked
by `tests/hyph.rs`: `hyphen.tex`'s patterns and exceptions, 368 words
(plus `\uchyph`, minima and a later `\hyphenation`) break exactly where
`tex -ini`'s `\showhyphens` breaks them. Integrated (`graph/lang.rs`,
`hyph::hyphenate_after`, `linebreak.rs`): `tests/dvi/hyph1.tex` and
`hyph2.tex` (hyphen.tex loaded by the `\patterns` node) are TeX's DVI
and log at all five schedules; 60,800 dictionary words through the whole
line breaker (`\showhyphens` in a `\vbox`, both minima 2/3 and 1/1) give
TeX's log line for line at 8 threads (checked once, not a test).

| TeX | graph |
|---|---|
| the trie of one language (§920–958) | `Key::Patterns(lang)` → `Val::Patterns(hyph::Patterns)`: an immutable `Arc` value, each pattern's letters to its digits; it answers as the packed trie does (§923–924) |
| the exception dictionary (§926–941) | `Key::Exceptions(lang)` → `Val::Exceptions(hyph::Exceptions)`, one per language (TeX keys its one table by word + `cur_lang`: the same thing) |
| `\patterns{…}` | a node reading `\language` (`IntPar`, made 0–255 by `hyph::cur_lang`), the old `Key::Patterns(lang)` and the `\lccode` of each character it reads; it defines `Key::Patterns(lang)` = `old.add(toks)` **globally** (TeX's tables have no save stack); its errors are the node's |
| `\hyphenation{…}` | likewise: reads `\language`, `Key::Exceptions(lang)`, the `\lccode`s; defines `Key::Exceptions(lang)` globally (a word given again replaces the old one) |
| `\language`, `\lefthyphenmin`, `\righthyphenmin`, `\uchyph` | `IntPar`s; a paragraph takes language and minima where it starts (§1091, `norm_min`) and where `\language` changes in it (a language whatsit, §1376), `\uchyph` where it ends (§891) |
| `\hyphenchar` | `Key::HyphenChar(font key)` (done), read by the paragraph for each font its words use (§891: outside 0–255, no word of that font is hyphenated) |
| `trie_not_ready` (§891, §960) | decided: `\patterns` is always accepted. The one divergence: TeX packs the trie at the first paragraph whose line breaking reaches the second pass, and gives "Too late for \patterns" (ignoring them) for a `\patterns` after that; here they are added and used by later paragraphs |
| `clang`, a paragraph's `prev_graf` language field | `RState.hyph_lang` (`clang`, the language and minima the paragraph began with, tex.web 1091, 1200), saved by box and math groups like the space factor |
| the language whatsit (§1341, §1376–1377) | `Item::Lang{lang, lhm, rhm}`, emitted by `\setlanguage` and where a run of characters begins in a paragraph whose `\language` changed (`fix_language`); a breakpoint before glue like any whatsit, shown as `\setlanguage1 (hyphenmin 2,3)` |

The line breaker's second pass (§894–918) hyphenates the word after each
glue it passes, in its own copy of the paragraph's list
(`hyph::hyphenate_after`: which words are tried — after glue, letters of
one font with nonzero `\lccode`, at most 63, followed by glue, a penalty,
a mark, a whatsit or an explicit kern — then `hyphenate`, then the
reconstitution of §903–§918 with the font's ligatures and kerns around
each discretionary, which is where `raffish` loses a break); the third
pass works on the hyphenated list, as TeX's does. What it reads is read by
the node that breaks the paragraph (`\par`, `$$`, a box's end) into
`LineParams::hyph` (`hyph::HyphParams`): the paragraph's starting
language and minima, `\uchyph`, the `\lccode` of each character in the
list, each font's `\hyphenchar`, `\tracinglostchars`, and the
`Key::Patterns(l)`/`Key::Exceptions(l)` of each language it uses — reads
like any other, so a changed `\patterns` or `\hyphenation` re-breaks
exactly the paragraphs that read it.

## Output routines, insertions and marks (decided)

Today the page fold (`page::Scan`) is a pure fold over the outer vertical
material: at each `Item::BuildPage` (the call, with the parameters TeX
reads there) it moves contributions to the page (tex.web §980–1010), and
at a break it ships `\box255` (§1023, the default output). The decisions
below (coordinator, 2026-10-09) say what changes when there is an output
routine. The core changes (index positions as `(piece, node)` pairs,
routine runs as elements of the material sequence) are agent A's after
the merge; the routine's execution as nodes is designed here.

**What fits as is** (nodes emitting items; the fold a pure function of the
items):

- `\mark{…}`: a node emitting `Item::Mark` (done); marks move out of lines
  and out of an `\hbox` in vertical mode (done). The fold computes
  `top_mark`, `first_mark`, `bot_mark` (§1012–1014) from the marks on each
  page: fold state, pure.
- `\vadjust{…}`, `\insert n{…}` as material: a group like a box's; its
  packaging node (§1100, reading `\splittopskip`, `\splitmaxdepth`,
  `\floatingpenalty` at the close, before the group's definitions go)
  emits `Item::Adjust(list)` / `Item::Ins{n, height+depth, list,
  split_top, depth, float_cost}`. **`\insert n` reads nothing else** (no
  register of its class): it only builds its item. In a paragraph the
  items move out of the line after it (`hpack`'s adjust list, §655, §889,
  as marks do now); after an `\hbox` in vertical mode likewise (§1076).
- `\shipout`: a node whose result is a page (done); the DVI fold takes the
  pages in document order.
- The default output (no `\output`): done.

**Only the fold reads, at the fire**: `\output`, `\holdinginserts`,
`\maxdeadcycles`, and each insertion class's `\count n`, `\dimen n`,
`\skip n`, `\box n` (dimensions when an insertion arrives on the page,
§1008–1010; contents at the fire, §1018–1022). The fold is a reader with
edges: it resolves these at the calling node's position (available: the
fold cannot reach that call before the pieces up to it have run), and a
changed value re-runs the fold from that page. `\par` nodes do not read
them (`PageCall` keeps only what every call reads: `\vsize`,
`\maxdepth`, `\topskip`, what shipping reads, `\hsize` for `\end`).

**Firing with a routine** (§1012–1028): at a break where `\output` is not
empty and `\deadcycles < \maxdeadcycles`, the fold's step is a **routine
run**:

- The fold *defines*, at the fire:
  - `\box255` = the page box (`vpack` of the page to `\vsize`, §1017).
    Like every box TeX assigns without the save stack, it is a definition
    **at the level of the definition it replaces** (Conflict 7, `Def.level`):
    in practice local to nothing (TeX requires `\box255` void before,
    §1015, and after, §1028).
  - each insertion class's `\box n` (§1018–1022, unless
    `\holdinginserts>0`): the old box's contents plus the page's
    insertions of class n, by the same rule.
  - `\outputpenalty`, globally (§1013 `geq_word_define`; made by the
    default output too).
- The routine's tokens, `{` + `\output` + `}` (§1025), are read as **a
  sequence of nodes at a position in the document order**: `(piece k,
  node i)`, where the fold fired (the node carrying the firing
  `Item::BuildPage`). They are ordinary nodes; RState starts as tex.web
  sets it (internal vertical mode, `prev_depth` ignored,
  `normal_paragraph`, the output group open); `\topmark`, `\firstmark`,
  `\botmark`, `\insertpenalties` and `\deadcycles` (incremented, §1025)
  are values the fold hands it, like RState fields.
- Its **definitions enter the names index at that position**: the index's
  positions become `(piece, node)` pairs (a piece's exports sit at
  `(k, END)`), a routine run's definitions at `(k, i)`. Readers after that
  position whose source is before it are marked by the existing edge
  logic. On a cold build a reader assumes no routine defines the name and
  is validated when the routine's nodes install — exactly the treatment of
  any piece not yet run. (No `Key::OutputWrites`.)
- Its other results, joined back by the fold (§1026):
  - the pages its `\shipout` nodes produce, in order (to the DVI fold);
    each `\shipout` sets `\deadcycles` to 0 (§640), returned with the run;
  - the **material** its internal vertical list holds at its end (what it
    `\unvbox`ed or built) and the held-over insertions: put *before* the
    remaining contributions (§1026), which the fold's next step reads;
  - errors: `\box255` not void at the end, an unbalanced routine.
- `\end` (§1054 `its_all_over`) is a fold step too: while the page or
  contributions are not empty or `\deadcycles≠0`, it appends `\hbox
  to\hsize{}`, `\vfill`, `\penalty-'10000000000` and fires again.

`\deadcycles`, `\insertpenalties`, `\pagegoal`… and the marks are `Key`s
(coordinator, 2026-10-09, superseding "fold state, not `Key`s"):
`Key::Page(k)` and `Key::Mark(which, class)`. A routine run defines them
at its start (the fold's values at the fire); outside a routine a read sees
what the last definition before it made (a run's exports), else an empty
page's values; `\shipout` defines `\deadcycles` 0 wherever it is (§638).
The fold reads `\deadcycles` at the fire and at `\end` like any name.

### The TeX side of output routines (done: `graph/output.rs`)

The handoff for the page fold (agent A's sequence):

1. **At a fire with a routine** (`\output` not empty, `\deadcycles <
   \maxdeadcycles`, read at the firing place): the fold computes the page
   box (`vpack` to the best size, as the default output does), the marks
   (`output::fire_marks(prev, &page)`: §1012–1014 for every class, from
   the marks after the previous page) and `OutputReads { output_penalty
   (§1013), insert_penalties (§1018), dead_cycles (the value read at the
   fire, before §1025's increment), marks, page_so_far }`, and calls
   **`output::output_entry(&page, &reads, &output_tokens) -> RState`**
   (`output_tokens`: `\output`'s value, braces included). The state is a
   piece start's but for its input: `\0output`, then the routine's tokens;
   `RState::fire` carries the fold's values to the run's first nodes.
2. **The run**: a piece whose text is **empty** and whose chain starts
   from that state (`Chain::with_entry`; `Chain::prefix` keeps it, so a
   re-run of the routine with `run_piece` given the entry shares nodes as
   a piece's does). `output::run_routine(&entry, env)` is the cold run
   (what `tests/output.rs` uses). An empty text is safe for the reader
   (the input ends where the pending tokens do: the `End` node), for
   `logs` (every node is at the text's line 1) and for the context lines
   of errors. Its nodes:
   - node 0 (`\0output`, `Kind::Open`): defines, **globally**, `\box255`
     (the page), `\outputpenalty`, `Key::Page(DEAD_CYCLES)` (incremented)
     and every `Key::Mark(0..=2, class)` handed in; opens the output group
     (`GroupKind::Output`, `\currentgrouptype` 8) with an internal vertical
     list at level 1 (`prev_depth` ignored, `prev_graf` 0); reads the `{`.
   - node 1 (`\0outputpage`): inside the group, `Key::Page(0..=7)`
     (`page_so_far`) and `Key::Page(INSERT_PENALTIES)`; `normal_paragraph`.
   - the routine's nodes. `\shipout` is a node as before (`Item::Shipout`,
     `\deadcycles` 0).
   - the group's `}` (`Kind::Close`): "Unbalanced output routine" (§1100,
     the rest of the routine's text skipped); a paragraph in progress ends
     first; the group closes; "Output routine didn't use all of \box255"
     (§1028; `\box255` voided, globally); both errors carry TeX's help
     lines after the message (`output::UNBALANCED`, `output::UNUSED`). The
     node's **material** (`Para::before` at the run's outer level) is the
     pages its `\shipout`s made (`Item::Shipout`, in order), then the
     routine's vertical list at its end (the **leftover**, interline glue
     in it); its result is **`Node::output() -> Some(&OutputEnd { shipped,
     dead_cycles })`** (`Rare::output`; `shipped` counts the pages before
     the leftover; `output::output_end(&run)` gives it with the leftover).
   - anything read after the `}` (an `\aftergroup` token) runs at the run's
     outer level (vertical mode): its material follows.
3. **What the fold does with the run**: its **definitions** enter the
   names index at the firing place `(k, i)` (its exports: `\box255` void,
   `\outputpenalty`, `\deadcycles`, the marks, whatever the routine
   defined globally); its **pages** (the run's material, `Item::Shipout`)
   go to the DVI in order; the **leftover** (the rest of the `Close`
   node's material) goes before the remaining contributions (after held-over insertions, §1026), then the fold calls
   the page builder (§1026's `build_page`) with the firing call's
   parameters, then feeds the run's other material; its **errors** are the
   document's at that place. `OutputEnd::dead_cycles` equals the run's
   export of `Key::Page(DEAD_CYCLES)`: the fold reads it (or the export)
   at `\end` (§1054) and at the next fire (`Output loop---N consecutive
   dead cycles`, §1024, is the fold's). Line numbers in the run's log
   entries are line 1 of an empty text: the log's gatherer gives them the
   firing node's line.
4. Not here: the fold's insertions (`\insert` and `\vadjust` are items,
   `Item::Ins` and `Item::Adjust`, whose packaging nodes read
   `\floatingpenalty`, `\splittopskip`, `\splitmaxdepth` inside their
   group; `\vsplit` is `split.rs` and defines `Key::SplitMarks`, which
   `\splitfirstmark…` read; the page marks are `Key::Mark(0..=2, class)`;
   `\holdinginserts`, each class's `\count`/`\dimen`/`\skip`/`\box` are
   names the fold reads at the firing place, as it reads `\output` and
   `\maxdeadcycles`), `\showlists` inside a routine (its list is not one
   `\showlists` shows yet), and the "The following box has been deleted:"
   diagnostic of §1028 (errors' help and diagnostics are not in the log).
   `tests/output.rs` checks plain's `\output` on a page with marks in its
   headline against `tex -ini` (the DVI file, byte for byte), the two
   errors against TeX's, and what a routine reads and leaves.

### The fold side (done: `page.rs`, `build.rs`)

One mechanism: at a fire with a routine the fold (`Scan::fire_up`) builds
the page box, the marks of every class (`output::fire_marks`) and
`OutputReads`, and hands them with `\output`'s tokens to the build
(`page::Routine`); `BuildFold::fire` makes the entry with
`output::output_entry`, runs it with `output::run_routine` (memoised by
the page, the reads, the tokens and what the routine read), takes its
pages and leftover from its `Close` node's material and enters its
exports at the firing place `(k, i)`, with the fold's own definitions
(the insertion boxes, §1018–1022). At every fire, the default output's
included, the fold defines `\outputpenalty` (§1013) and, once there are
marks, `Key::Mark(0..=2, class)`; the default output defines
`\deadcycles` 0 once the fold has fired a routine.

A definition at `(k, i)` reaches piece `k`'s nodes after `i`, those that
would otherwise read one of the piece's own earlier definitions included
(`Env::between`), and the pieces after `k`: piece `k`'s export of a name
gives way to a definition between its nodes made after its own last one
(`export_of`). A node reading the name in piece `k` after `i` is marked
when the definition changes.

**The page scan's incremental resume rule.** Routine runs are elements of
the material sequence, like pieces. The resume point is the last page
started before the first changed element (piece or routine run). When a
fire moves (a page break moved), the routine run's position moves, and its
definitions are treated like a moved piece's exports (the existing
`touched` rule). There is no cycle: a run at `(k, i)` reaches only
positions after it, and its material is consumed after it.

## Cuts (decided, 2026-10-10)

The cutter proposes where pieces begin; where a run actually stops
decides. "Where the reader is is itself a read."

- Proposals: a blank line at brace depth 0 (plain catcodes), a job's
  start, and about every `Options::cut_lines` lines (`CUT_LINES` = 200) a
  line beginning with a control sequence at brace depth 0, chosen by a
  hash of the line (at least `n/4` lines after the last cut, at most
  `4n`): after an edit the cuts fall where the lines are the same. The
  value is a scheduling knob: every document gives the same result for
  any (`tests/dvi.rs` `any_cut_gives_tex_s_result`: 3, 20, blank lines
  only).
- Entry: a piece after a cut (`Entry::After(prev, ctx)`) is entered in the
  state the piece before it left there (`RState::entered_after`: the next
  line's start, the conditionals open as seen from outside, the
  paragraph-level fields a cut starts afresh with). Until that piece has
  run the state is assumed: the canonical start, or in a file the start
  of the file (`ctx`: the state its `\input` left). A run on another
  state than its predecessor leaves is run again (`entry_holds`, the
  pieces entered from a piece, `deps`), as any read is.
- Joins: a run whose last command reads on past its text's end
  (`Run::crossed`: the reader ran out of the piece's text anywhere but
  where it looks for the next command — an argument, a definition, a
  number, a skipped arm), or that leaves a state no piece is entered in
  (`RState::enterable`: a paragraph, a group, a box, an alignment open),
  reads on into the texts after it (`Expect::join`, `Build::place_list`):
  joined with as many as make it twice as long, its run stops at the
  first place one of them begins where a command begins in a state a
  piece is entered in (`Env::stop_at`, `Kind::Cut`), and the rest is cut
  again from there. The joined piece is kept by the next layouts
  (`Build::known_join`): its run never stops at the first place to say
  so again. A piece whose predecessor reads on into it does not run.
- The sequence changes when a run that says otherwise than the layout
  assumed has settled (all before it final): at once for a join (the
  pieces after it begin in the middle of a command; runs waiting for an
  earlier piece stop, the others finish and are kept), else once every
  piece has settled. A layout ignores a last run entered otherwise than
  it now says (`old_entry_holds`).
- A run validates what it read while it runs (`PieceEnv::check`, every
  4096 steps): a definition made again since — its piece ran again —
  abandons it; so does a prediction refuted, and a long run on an
  assumed entry or predictions while a piece before it is not settled is
  given up (its piece then waits).
- After a piece that stopped at a fatal error (the hundredth error,
  "capacity exceeded") the next piece is entered in the assumed state,
  not that one's (nothing after the error is read; the log ends there).
  (Defining `Key::Ended` there instead let a speculative run on wrong
  predictions that met a hundred errors end the layout.)
- Re-cuts (`Kind::Cut`): a reader passing a blank line at depth 0 in the
  canonical state where the cutter saw none ends its run there; the rest
  is cut again.

## Reads of folds (decided, 2026-10-10)

Two values are folds over the document's positions, not definitions any
node makes: the page builder's state (`\pagegoal`…`\pagedepth`,
`\deadcycles`, `\insertpenalties`: `Key::Page(0..=9)`; Q's `\pdflast*`
counters are next), and a file the run writes (`\openout`/`\write`,
immediate at their nodes, deferred at their pages' ship positions;
`\closeout` ends what was written). A node's read of one is a read of
the fold's value at the node's position (Conflict 12, decided).

Invariant: a fold's read is only ever validated against the fold's value
at that node (`Names::read_now`/`read_holds`, the one check install and
`still_dirty` make; the names index answered it twice: 5e3ea81 at
install, then `reads_still`, which dropped a refuted file read's mark).

Invariant: a node kept by sharing (`run_piece`'s in-place copy of the
old chain) has had every check a node looked at gets: the copy stops
before a `Cut` node, and before an `Input` node when the layout reads
the piece's `\input`s in it (F's TRIP update class: the shared node
ended the run at the file the layout read inline, its run saying
"input" at every round's end, 64 rounds). A written file a read finds
no fold value for at its place is guessed as a read last found it
(`Names::file_guess`), so the layout a read of it makes comes back;
the probes validate it like any prediction.

Invariant: a written file's content at a read is what the writes that
reached it in this build's sequence wrote, kept pages included: the
routines' call logs (their deferred `\write`s among them) are kept with
the pages and marks (`Build::call_logs`), all of them when every page
is kept, those before the scan's and after its convergence otherwise
(B2 had let a LaTeX document keep its pages without them: run 2 of the
fixpoint wrote the `.aux` without a kept page's `\@writefile`).

- **The read** (`graph::Env::fold_read`, `Names::fold_value`): answered
  with the fold's value at `(piece, node)` as known — this build's,
  else the last build's (`Build::fold_vals`, by piece id and node),
  else the names index's (the page builder's fires' definitions;
  IniTeX's values before any) or the file as found in the directories.
  A node's own definitions do not answer a page quantity (`Env::folds`:
  an assignment `\pagegoal=…` is an item the fold applies). Every such
  read is entered in the run's reads, at its node (`graph::positional`:
  not one per value); the page quantities have no edges.
- **Validated by the fold.** The scan, before the first item of each
  node of a piece's material (and after the piece's material), compares
  the reads of the page quantities at the nodes up to there with its
  page so far (`Scan::page_quantity`, tex.web 421: `\pagegoal` is
  `\maxdimen` and the others 0 while the page is empty; the goal and
  totals as the insertions left them), keeps each value
  (`Names::set_fold_value`) and, at the first that differs, marks the
  piece from that node (`check_page_reads`, `mark_from`) and stops there,
  as a fire's definitions do. The scan does not converge with the last
  build's pages before a run whose reads it has not validated
  (`PState::fold_unchecked`). Files: the log's replay takes what was
  written of each file at each node reading one (`logs::Entry::Probe`,
  `logs::replay_probed`); a read that saw something else marks its piece
  from its node and the build goes another round (`Build::keep_round`).
  LaTeX's `\enddocument` reads back the `.aux` it wrote and closed: on a
  cold build, one more round runs that piece's tail.
- **The Suspend policy's one speculative exception, and why.** Under
  `Reads::Suspend` a read waits for the value it reads. A fold's value at
  node n depends on the items of the reading piece's own nodes before n,
  and the scan sees a run's items only when the run is installed: waiting
  would wait for itself. So these reads are predicted and validated under
  both policies; the result is the same (validation is by exact value,
  a refuted read runs again from its node). The generalization that
  removes the exception: a run publishing its items per node, so the scan
  can pass node n−1 while the run is at node n — which also removes Q's
  second round and is the brief's §6.4 overlap. Not built yet.
- `Output::files_read`: every file read (each name once, in document
  order) and where it was found (`None`: not found), for the CLI's
  `-recorder` and a watch's dependencies.
- Tests: `tests/dvi.rs` `pagequant` (each quantity read mid-page:
  empty page, after a paragraph, skips of each order, an insertion, a
  split insertion with `\floatingpenalty`, a deep box, inside a paragraph
  after `\pagegoal=`, after a page break), `readback` (immediate and
  deferred writes read back with `\input` and `\read`, a file before it
  is opened and after `\closeout`; not while open: TeX's buffering), both at five schedules against `tex`;
  `reads_of_folds_follow_edits`; `tests/latex_docs.rs`: the documents'
  `.aux` read back, no gap; TRIP's `tripos.tex` in one build
  (`Session::run`).

## Source places and item provenance (SyncTeX, inverse search)

- **A node's source place** (`Build::source_place(k, node)`): the
  piece's file (`None`: the main file; a piece after `\input`'s file
  piece names that file) and line: the lines of the file before the
  piece (what `Key::LineBase` answers) plus the node's line in the piece
  (`Run::node_pos`, kept by lean runs too). A node reading a file read
  inside its piece is placed at the `\input`.
- **F8 (F's: a kept node's `\inputlineno`)**: a node's line in its piece
  is not a read; an edit that changes the number of lines before a node
  in the same piece keeps it unless the node read its line
  (`Key::LineBase`): then it runs again (`run_piece`'s `line_ok`).
  `tests/fuzz/fixed/f8-kept-node-line-in-piece.repro`; F's F9 (story,
  a line removed, the log's line numbers) is the same finding, made on a
  branch without this fix (`tests/fuzz/log-only/`, F's: the fuzz compares
  the DVI and written files).
- **Which node made an item** (`item::Origins`): a side table per list,
  not per item — runs of item indices with the distance back from the
  node that made the list (the paragraph's `\par` node: `Para::origins`;
  a box's closing node: `BoxNode::origins`; a line of a paragraph inside
  a box: its box's). Not part of a list's value (equal lists are equal
  whoever made them). A distance, so a paragraph whose nodes were all
  kept where an edit moved them still says it; a node reusing its old
  paragraph does so only with equal origins. Lines of the main vertical
  list are line breaking's, memoised by equality: a `Line` says which
  item of the paragraph's list each of its box's items came from
  (`Line::from`, runs; a hyphenated word's items, a discretionary's and
  the skips at the item they stand at), and the paragraph's origins give
  the node (`Line::node_of`). `Build::material_node(k, item, path)`:
  from an item of piece k's material (the page builder's `Org`), down
  `path` (a line's box's item, a box's item…), to its node;
  `Build::material_of(k)`. Not yet: alignments, math lists, inserts and
  the output routine's lists carry no origins (their items lead to the
  node that made the whole); TeXXeT's reversed lines are approximate.
- **Files by kpathsea** (`tfm::Kpse`, X's shape; `Build::set_kpse(kpse,
  prog)`, `kpse::Kpathsea`): a given of the build — a name becomes a path
  by kpathsea's lookup (format `tex` after the input directories, `tfm`
  for fonts, the PDF files' formats; the program's name); reads stay
  `Key::File` (see "Crates").

## `\input` (decided)

- A file is a value, `Key::File(name)`: the file's bytes are a read, so a
  changed file re-runs the `\input` node.
- The `\input` node's **result is the file's text**; its pieces (cut at
  blank lines at brace depth 0, as the main file is) are inserted after it
  in the order.
- **The entry-state constant is unchanged** for the file's interior
  pieces: their cuts stay canonical (each starts with the constant start
  state and reads nothing at its start).
- The **first** file piece and the piece holding **the rest of the calling
  line** carry their predecessor's output state as their entry: a real
  edge, sequential at that boundary only (TeX inserts the file in the
  middle of the caller's input: a group, a paragraph, a conditional or a
  macro's expansion may be open, and the rest of the line and the pending
  tokens are read after the file's end).
- `\endinput` (Conflict 5): the file's later pieces run on the guess that
  none occurs, and are dropped when one does (the piece where it runs ends
  at the end of that line).
- Line numbers: `(file, line)` is a field of the position, read by
  `\inputlineno` and error contexts, never in a version.
- Two paths (done). An `\input` fetched as the command itself where the
  reader is on the piece's own text — the outer vertical list, no group
  or box open, no rewritten text above it, no `\endinput` pending, not
  in an output routine, `\everyeof` empty — ends its node's run, whatever
  is pending: a macro's expansion, a conditional open (`Exec::input_file`).
  The node logs `(name`; the file's pieces join the sequence, cut as any
  text is (`Build::place_list`), the first entered with the node's state
  (`RState::file_start`: the tokens pending set aside), the others after
  their predecessor (assumed: the file's start); the rest of the calling
  line enters with the file's last state and the tokens pending
  (`RState::rest`). A file whose end leaves what the rest of the line goes
  on with (a paragraph or a group open) or whose last command reads on
  past it is read in its caller (`Build::inline`). Anywhere else (an
  `\input` expanded by `\expandafter` or while a command scans, inside a
  group, a box, a paragraph, another `\input` read in the piece) the file
  is read in the piece as a text on the `\scantokens` stack
  (`TextKind::File`, `Exec::start_input`). Files are
  `Key::File(name)` = `Val::Bytes` answered by the build (`Given`: the
  files given in memory with `Build::set_file`, then the input
  directories); `Key::LineBase` is the lines of its own file before a
  piece of the sequence.
- **The current rule, as TRIP holds it** (F, 2026-10-10; `tests/trip.rs`
  must stay green under any change to it). A file-piece node is made
  only for an `\input` that `Exec::command` fetches as the command
  itself (`at_start`, at the plain boundary, no rewritten text). Read
  in the piece instead:
  - an `\input` expanded by `\expandafter`: `at_start` is off while
    `\expandafter` expands, so the token it puts back is read before the
    file (tex.web 368; trip.tex line 424, `\expandafter\stopinput\input
    tripos\endinput\input`);
  - an `\input` met while a file name is scanned: it inserts `\relax`
    (tex.web 378, `Exec::name_in_progress`);
  - an `\endinput` before an `\input` read in the piece ends the new
    file's first line, not the caller's (`force_eof` belongs to the next
    line end, in whatever file: `Exec::start_input`).

  Making more `\input`s file pieces must keep those three as TeX has
  them: the tokens pending at the `\input` are read before the file, and
  `force_eof` moves to the file.

## e-TeX (extended mode)

`Options::etex` (default true; fixed for a `Build`'s life) is etex.web's
extended mode: e-TeX's 66 primitives are named (`val::initial`,
`prim::ETEX_PRIMITIVES`) and registers go to 32767. With it false the graph
is TeX82 (`tests/core.rs`, `tests/dvi.rs` compare with `tex`); with it true
`tests/etex.rs` compares with `etex -ini -etex`. The code is
`src/graph/etex.rs`, with hooks in `expand.rs` (conditionals, `\the`-like
steps, `\protected` in expanded text), `scan.rs` (items, register numbers),
`assign.rs` (prefixes, `\interactionmode`) and `graph.rs` (the rewritten
texts of `\scantokens`).

| etex.web quantity | here |
|---|---|
| `eTeX_mode` | `Key::ETeXMode`: a value of the format no document defines; read only where TeX82 and e-TeX differ outside the names (a register number past 255) |
| `interaction` | `Key::Interaction` (global; IniTeX's 3 initially — the oracle runs with `-interaction=batchmode`, so programs set it before reading it); `\batchmode`… and `\interactionmode=` define it and end the log's line (`new_interaction`'s `print_ln`) |
| the new integer parameters, `\everyeof` | `IntPar::Tracingassigns` … `TeXXeTstate`, `TokPar::Everyeof` |
| `cur_if`, `if_limit` for `\currentif…` | `RState.conds` (`Cond.code`: the conditional's code, +32 after `\unless`; `Cond.limit`) |
| `cur_group`, `cur_level` for `\currentgroup…` | `RState.groups` (kind, and a box group's `BoxGroup`: hbox, adjusted hbox, vbox, vtop, disc) |
| a pseudo file (`\scantokens`) | `RState.texts`: a text the input was rewritten with, above the rewritten token lists it was made over; read lazily with the catcodes in force (its own position, tokenizer state and end-of-line character), `\everyeof` once at its end |
| the protected mark (`protected_token`) | `Macro.protected` |
| mark classes | `Item::Marks(n, toks)` (class 0 is `Item::Mark`) |
| TeXXeT's math nodes | the math node, `Item::Math { w, after, lr }` with `lr` the beginning subtype (6 `\beginL`/`\endL`, 10 `\beginR`/`\endR`; 0: a formula's edges) |

Conflicts and gaps, with graph-shaped proposals:

1. **`\scantokens`' end inside a scan**: TeX checks `scanner_status` at a
   file's end ("File ended while scanning definition of …", which is why
   `\everyeof{\noexpand}` is idiomatic). Not made: the end is silent.
   Proposal: the scanner status as an `RState` field set by the scanning
   step, read by the end of a text. `\tracingscantokens`' `( ` and line
   display need the line field (Conflict 16).
2. **`\lastnodetype`** is the one read of material (Conflict 13), with its
   gaps: on the main vertical list after a paragraph's lines (TeX's
   `last_node_type`, set by the page builder) it reads as -1.
3. **`\showgroups`, `\showtokens`, `\showifs`**: the display, `! OK.`
   and the error's context are log text, as in batch mode. The context is
   `\showbox`'s (`diag::show_context`: a backed-up list at the top, `...`
   for the levels between, the file level's two lines, tex.web 311–318),
   one mechanism: its placeholders (`X`, `T`) are laid out by the log's
   `rebase` once the absolute line number is known, cut at
   `half_error_line` 50 and `error_line` 79. Where each open group or
   conditional was entered is a placeholder too (`E`k, `C`k: the k-th
   from the outside), read through the node's input state when the log is
   gathered: the node indices `Group::at`, `Cond::open` are positions,
   never compared when deciding what an edit keeps (`eq_but_pos`), and a
   kept node's state has them moved to where those nodes are now. Not
   yet: a `\scantokens` text's own level in the context (the file's line
   is shown).
   The entry lines of groups and conditionals are their opening nodes'
   lines (`Group.at`, `Cond.open`: `LINE N<node> LINE`).
4. **`\readline`**: reads a line of `Key::ReadStream(n)` as `\read` does
   (`io.rs`, agent E), the terminal refused in batch mode as TeX does;
   `readline_toks` makes the line's tokens.
5. **`\pagediscards`, `\splitdiscards`**: the items the page builder or
   `\vsplit` discarded, saved while `\savingvdiscards` is positive.
   Proposal: the page fold defines `Key::Discards(k)` at its firing call
   (as Conflict 18's `\box255`); a reader parks on the fold. Now: empty
   (exact while `\savingvdiscards` is not positive); in horizontal mode the
   paragraph ends first (`head_for_vmode`), as TeX's `un_vbox`.
6. **`\topmarks n`…`\splitbotmarks n`**: the number is scanned; a read of
   `Key::Mark(which, n)`, as `\topmark` (Conflicts 12, 18; an output
   routine's run defines them).
7. **TeXXeT**: `\beginL`… are math nodes when `\TeXXeTstate` is positive
   ("Improper `\beginL`" otherwise). Done as etex.web: `hpack`'s checks
   (`pack::lr_fix`: an unmatched end becomes a kern, missing ends are
   appended), the lines' LR nodes in `post_line_break` (the LR stack a
   local of the function), and the reversal when shipping (`dvi.rs`:
   `reverse` and the edge nodes on a working copy of the box's list, the
   LR stack and `cur_dir` locals of the page's writer). Not yet: the
   report of LR problems (`\endL or \endR problem`, in `hpack`'s and
   `ship_out`'s diagnostics), the LR state across a display (`LR_save`,
   `LR_box`, `just_reverse`, display boxes' `dlist`).
8. **`\middle`** (with B's math): `\middle` closes the `\left` group like
   `\right` (a right noad with the middle flag, `mlist::Kind::Right(d,
   true)`) and opens it again, its list the one so far, in a node of its
   own (`\0middlenext`, as `\mathchoice`'s parts); a fraction's numerator
   begins after the last left or middle noad (`delim_ptr`); in
   `mlist_to_hlist` a right noad is followed as a left one (etex.web §727,
   §760).
9. **The penalties arrays** (`\interlinepenalties`, `\clubpenalties`,
   `\widowpenalties`, `\displaywidowpenalties`): `Key::PenaltiesArray(k)`
   = `Val::Penalties(Arc<[i32]>)` (empty: null), assigned, read by `\the`,
   read by the paragraph node into `LineParams` (§890 as etex.web; the
   display's paragraph reads `\displaywidowpenalties`); `\par` in
   vertical mode resets `\interlinepenalties` (`normal_paragraph`).
10. **`\lastlinefit`** is read by line breaking (etex.web §1575–§1583:
    the active nodes' shortfall and glue, the last line's badness from the
    line before, the last `\parfillskip` adjusted). `\savinghyphcodes`,
    `\predisplaydirection`: parameters, assigned and read; hyphenation
    and display math do not read them yet. The `\tracing…` parameters
    are values only (no tracing output: programs keep them non-positive).
11. **Kept pieces and the mode**: `Options::etex` is read by
    `val::initial`; changing it between two updates of one `Build` is not
    supported (pieces kept from the earlier update read the earlier mode).

e-TeX's primitives:

| primitive | etex.web cmd | graph form | status |
|---|---|---|---|
| `\beginL`, `\endL`, `\beginR`, `\endR` | valign | node emitting the math node (horizontal mode; vertical: a paragraph begins); reversed when shipped | done (gap 7: LR problem reports, displays) |
| `\botmarks`, `\firstmarks`, `\topmarks`, `\splitbotmarks`, `\splitfirstmarks` | top_bot_mark | step: scans the class; rewrites the input with `Key::Mark(which, class)` (a read), defined by an output routine's run | approx: empty outside a routine until the fold runs routines |
| `\clubpenalties`, `\displaywidowpenalties`, `\interlinepenalties`, `\widowpenalties` | set_shape | `Key::PenaltiesArray(k)`, read by line breaking | done |
| `\currentgrouplevel`, `\currentgrouptype` | last_item | a read of `RState.groups` | done |
| `\currentifbranch`, `\currentiflevel`, `\currentiftype` | last_item | a read of `RState.conds` | done |
| `\detokenize`, `\unexpanded` | the | step: the balanced text, shown as characters or as it is; not expanded again inside `\edef`/`\message`/`\write` | done |
| `\eTeXrevision` | convert | step: `.6` | done |
| `\eTeXversion` | last_item | 2 | done |
| `\everyeof` | assign_toks | `Key::TokPar(Everyeof)`, read at a rewritten text's end | done |
| `\fontchardp`, `\fontcharht`, `\fontcharic`, `\fontcharwd` | last_item | a read of the font value | done |
| `\glueexpr`, `\muexpr`, `\numexpr`, `\dimexpr` | last_item | the expression grammar (§1520–1544), a pure function of the tokens and the quantities it reads; "Arithmetic overflow" makes it zero | done |
| `\glueshrink`, `\glueshrinkorder`, `\gluestretch`, `\gluestretchorder` | last_item | scans glue | done |
| `\gluetomu`, `\mutoglue` | last_item | scans glue, changes its level | done |
| `\ifcsname` | if_test | conditional; reads the name's meaning, defines nothing | done |
| `\ifdefined` | if_test | conditional; reads the meaning of the next token | done |
| `\iffontchar` | if_test | conditional; reads the font value | done |
| `\interactionmode` | set_page_int | `Key::Interaction`, read and assigned (global) | done |
| `\lastlinefit`, `\predisplaydirection`, `\savinghyphcodes`, `\savingvdiscards`, `\TeXXeTstate`, `\tracingassigns`, `\tracinggroups`, `\tracingifs`, `\tracingnesting`, `\tracingscantokens` | assign_int | `Key::IntPar(p)` | done (`\lastlinefit`, `\TeXXeTstate` read; others: gap 10) |
| `\lastnodetype` | last_item | the read of material (Conflict 13) | done (inside a piece; gap 2) |
| `\marks` | mark | node emitting `Item::Marks(n, …)` (class 0: `Item::Mark`) | done |
| `\middle` | left_right | closes and opens again the `\left` group (gap 8) | done |
| `\pagediscards`, `\splitdiscards` | un_vbox | the page fold's discards (gap 5) | approx: empty |
| `\parshapedimen`, `\parshapeindent`, `\parshapelength` | last_item | a read of `Key::Parshape` | done |
| `\protected` | prefix | `RState.protected` to the definition; `Macro.protected` | done |
| `\readline` | read_to_cs | a prefixed node: a line of `Key::ReadStream(n)` (IO), its tokens by `readline_toks` | done |
| `\scantokens` | input | step: rewrites the input with the shown text, read again (`RState.texts`) | done (no "File ended" check: gap 1) |
| `\showgroups`, `\showifs`, `\showtokens` | xray | log text and the error's context, the line a placeholder (gap 3) | done (file-level context) |
| `\unless` | expand_after | step (a node at the top, as conditionals are): the boolean conditional after it, negated | done |

## TRIP (agent F)

`tests/trip.rs` runs Knuth's TRIP test (`bench/trip/`, `bench/trip.sh
fetch`) as one session (`session.rs`). **The gate is the document only**
(the user's decision, 2026-10-10). `trip.typ` is DVItype's listing of our
`trip.dvi`. It must equal Knuth's, except for the header and the glue
rounding that TeX Live's `trip.diffs` allows, and it must be the same at
the five schedules. Until it is, `REACHED` in the test is a ratchet: the
first differing line may only move down.

**Not a gate** (written for navigation, no effort spent on their form):
`tripin.log`, `trip.log`, `trip.fot`. That covers:
- error messages, their context lines and help text;
- the output of `\show` and `\showthe`;
- the tracing family (`\tracingcommands`, `\tracingmacros`,
  `\tracingrestores`, `\tracingparagraphs`, `\tracingpages`);
- the statistics at `\dump` and at the end, and the history value;
- the labels on input levels in an error's context;
- printer limits, beyond the line lengths the terminal needs.

**Sessions.**
- A job starts at a line that begins with `graph::JOB`. It is a node of
  its own:
  - it takes the `**` line from the terminal and logs the banner;
  - it defines the date parameters and runs `\everyjob`.
- `Key::LineBase` and the page fold (`Item::JobStart`) start again at
  each job.
- `\dump` defines `Key::Format`, which the next job's banner names.
- The terminal is `Key::ReadStream(16)` (`Build::set_terminal`) and the
  interaction is `Key::Interaction` (`Build::set_interaction`). An error
  reads its answer from the terminal (tex.web 82–93): delete, insert,
  change the mode, or `X`/`E` (which goes through `Item::Halt`).
- `print::Limits` holds TRIP's compiled sizes (`error_line` and the
  others), including `dvi_buf_size` (800), which the DVI writer's
  half-buffer rule reads.
- The memory bound is A's `Options::main_memory`.

**Result** (2026-10-10): `trip.typ` is Knuth's, all 16 pages, at the
five schedules. The two remaining raw differences are the glue rounding
`trip.diffs` allows (`down4 639342177`, `y4`/`y0 203921756`).
- Pass 1 is no longer cut at `\dump`: A's `Key::Ended` ends job 1, and a
  job start defines it 0 again (a new run of TeX). The layout skips the
  rest of an ended job up to the next job's first piece.
- `tripos.tex` still goes through the session's fixpoint over written
  files. B's gap 1 below (a file written and read back in one run) is the
  same core gap.
- A's `Item::PageSoFar` (tex.web 1245) is sent at any level, not only at
  level 0: inside a group, box, paragraph or formula, through the node's
  `para` stream, as `Item::Halt` is (not inside an output routine).
  `\showlonglists` sets `\pagegoal` inside `{{...}}`.

**Fixed here** (each one changed TRIP's DVI or what came after it):
- `\fontdimen` growing the last font loaded (B's `Key::LastFont` and
  `Key::FontParams`, merged), counted by math's parameter checks too
  (§1195);
- `check_shrinkage` in line breaking, and `prev_p` after a
  discretionary;
- §1120 Illegal math `\discretionary`;
- §1241: `Improper \setbox` reads the register and the `=` first;
- `\outer`: `check_outer_validity` (§336–339) with a `scanner_status`
  per node (`Exec::scanning`). `\noexpand`, `\string`, `\meaning`, `\ifx`
  and `\ifdefined` read as `normal`. A frozen `\fi` (`\0fi`) and a frozen
  `\relax` (`\0relax`) were added beside `\0cr`.
  A scanned name reads only whether its meaning is `\outer`
  (`graph::Field::Outer` on its `Read`, AGENTS_BRIEF §1). `Read::same`
  compares only that, so a kept node stays kept. A piece records the read
  as `Val::Outer(bool)`, which a lean run keeps too, and `Val::seen_as`
  compares it at the commit. `build::outer_still` does not mark a piece
  whose every read of the name was of that kind and still sees the same.
  So redefining a macro that is only scanned somewhere (a definition, an
  argument, skipped text) re-runs none of the scanning nodes, and making
  it `\outer` re-runs them (`tests/flow.rs`,
  `a_scanned_name_reads_only_whether_it_is_outer`).
- §1094: `\par` in an alignment entry whose braces are still open does
  `off_save`;
- §378: `\input` while a file name is scanned inserts `\relax`;
- §362: `\endinput` before `\input` ends the new file's first line
  (`force_eof` belongs to the next line end, in whatever file);
- §352/355: a `^^`-reduced superscript can begin another `^^`;
- §442: a backquoted brace does not count in an alignment's balance;
- §288 `prepare_mag` and `Key::MagSet`. The postamble's magnification is
  the first page's.
- §641: Huge page cannot be shipped out;
- `Tok::Param(n, c)`: a parameter text keeps its character, and a body
  shows `match_chr` (§294).
- tex.web 392/397: a delimited argument matches the token its delimiter
  expects next before a `\par` ends the call (`\a#1\par` with `\par`
  next);
- tex.web 368: an `\input` expanded by `\expandafter` is read in the
  piece, not as a file-piece node (the token put back is read first).

**e-TRIP** (`tests/etrip.rs`, `bench/etrip/`: TeX Live's
`etexdir/etrip`, part 2 of `etripman.tex`). Two jobs: `*etrip` (extended
mode) dumps, `&etrip etrip` runs, at TRIP's sizes. Gate: `etrip.typ` (3
pages), byte-identical but for the header, at the five schedules: passes.
The logs are diagnostics. Fixed for it:
- tex.web 362: a file or `\scantokens` pseudo file that ends in the middle
  of what is being scanned ends it as an `\outer` name would, with "File
  ended while scanning ..." or "Incomplete \if" and the `\fi`, `}`,
  `\par` or `\cr}` inserted (`Exec::outer_recover`).
- tex.web 1371: a `\write`'s text is read between a `{` and a `}`, both
  read, so an alignment's brace count stays as it was (an
  `\immediate\write` inside a `\valign` entry).
- etex.ch: `\dump` turns `\TeXXeTstate` off in the format.
- `phitex-flow --trip DIR` runs e-TRIP when DIR holds `etrip.tex`.

The log still differs where a group or conditional spans a blank line
(item 14 above; etrip.tex lines 147–181). The DVI does not depend on it.

**Approximations left** (not met by TRIP's DVI):
- `\endtemplate` is not yet an `\outer` name for `check_outer`.
- `prepare_mag` runs at a `\shipout` only while no magnification is
  fixed. TeX runs it at the first page only.

## Fuzz findings (agent F)

**The gate.** `phitex-flow --fuzz SEED COUNT [SEEDS] [--threads N,..]
[--doc NAME] [--out DIR] [--no-over] [--strict-over] [--timeout SECS]`
(`src/fuzz.rs`, `tests/fuzz.rs`): from a seed, COUNT random edits in
sequence on a document of the corpus (the plain documents story, pages,
display, table, hyphens; "big", every body four times, 48 pages; TRIP's
session, edits from pass 2's line 100). Edits: a character, a word, a
blank line in or out, a paragraph split or joined, two paragraphs
swapped, `\hsize` `\parindent` `\baselineskip` `\tolerance` changed, a
use or redefinition of `\fz`, an `\iffz` toggled or its arm edited, a
span grouped, `\input fzsub` added or removed and the file edited, a
`\footnote`/`\mark`/`\insert`, an edit inside an alignment or a display.
After each edit, `Build::update` at a random schedule (1 or 8 threads,
waiting or predicted reads, priority or random order; lean, not lean, or
either at random by seed) must give a cold build's DVI and written
files. The log is not compared, since the bar is the DVI/PDF and the
feedback files. A failure is written as a reproduction (`--out`, the text before,
the edit, both schedules, the run's history) that `--fuzz-replay FILE`
runs again: the last edit alone, then, if that passes, the seed again to
its step. A summary line says "(a no-op edit fails too)" when the update
fails whatever the edit. `PHITEX_FUZZ_DUMP=DIR` writes both graphs, both
logs and both DVI files.

**At scale** (`scripts/flow-fuzz.sh [SEEDS] [OUT]`, for the cluster): SEEDS
seeds (5000 by default) of 6 edits at 1 thread, then SEEDS/5 other seeds
at 8 threads (random schedules). Chunks of 100 seeds run `JOBS` at a
time, under `scripts/sandbox`. It writes `OUT/summary.txt` (passed and
failed by document) and `OUT/classes.txt` (`phitex-flow --fuzz-classify
REPRO...`: F1, F2?, F6 for the open findings by how they fail, else
`edit`, `noop`, `history`, `over`, `panic` or `timeout` with the
document, the edit's kind and where the log first differs). One
reproduction per new class goes into `OUT/min/`, cut down by
`scripts/flow-fuzz-min.py` (delta debugging over the paragraphs of the
text; the edit's paragraph stays). `phitex-flow --fuzz-orders DOC FILE A
B [DIR]` compares two cold builds of a text in two orders (`priority`,
`random:N`; `-` is the document's own text). It was the oracle for F6.
`LATEX=1 scripts/flow-fuzz.sh [SEEDS] [OUT]`: the LaTeX documents
instead, SEEDS (30) seeds of each of B's ten, one chunk a document, on a
format made once (`OUT/latex.fmt`). DOCS="trip pages ..." runs those
plain documents only, each by `--doc`. A seed that times out ends its
process (exit 3, its thread cannot be stopped and held memory: job
7846's OOM). The chunk resumes after it.

**Over-invalidation** (seeds that keep their nodes; `src/fuzz.rs`
`over_invalidation`): a piece whose text the edit changed, a new piece, or
one entered in a predecessor's state may run all its nodes; any other
piece is matched by its id with the graph before the edit, its changed
names are those it reads from before it with another value (or with a
value the dump shows only in part that a node run defines), and it may
run nothing before its first node reading one. `--strict-over`: the
design's bound, only the readers of changed names outside the edited
pieces, on the sum (hook H2 below would make it node by node).
`fuzz::rerun_reasons` (`phitex-flow --fuzz-reruns BEFORE AFTER` on two
`--dump` graphs; `reruns.txt` beside every `PHITEX_FUZZ_DUMP` pair; the
first line of it in an over-invalidation's failure) says why each piece
run again ran. For a piece the build before had (marked when entered in
a predecessor's state), it gives the first read from before it that
changed: the node, the name, the
value it read before and the value reaching it now, each with its
defining node (`piece id N nM`, the last node of piece N defining the
name; `initial`; `the format`). A piece with no changed read is marked
"(over-invalidated)". Values the dump shows only in part are marked when
they read the same.

**TRIP edited** (`tests/fuzz.rs` `edited_trip_is_tex`): TeX (the system's
`tex`, which gives Knuth's `trip.typ` on the unedited file) is the oracle
of edited `trip.tex`s: our session's DVItype listing equals TeX's (4 seeds
of 3 edits by default, `PHITEX_FUZZ_TRIP=N`): passes.

**Local run** (500 seeds of 6 edits, 1 thread): 426 passed, 73 failed,
1 over-invalidated. The 73: 71 TRIP seeds (F1: every TRIP seed fails at
its first edit, and a no-op edit fails the same), hyphens seed 102 (F2),
big seed 236 (F6; cut down, its text also gives F4). The
over-invalidation: story seed 161 (F5).
`scripts/flow-fuzz.sh 1000` (1000 seeds at 1 thread, 200 at 8): 852 and
168 passed. Classes: 170 F1 (TRIP); 4 F2 (hyphens 102, 802, 970 with
history, 1187 a split); 1 F6 (big 236); 1 F6 by its cause (big 1153);
4 over-invalidations, all F5 (story 161 and 1127, pages 659, hyphens
830). Nothing new beyond F1–F6.

**Open findings** (`tests/fuzz/open/*.repro` must still fail; a fixed one
moves to `tests/fuzz/fixed/`, which must pass):

- **F1 (fixed, A). A second update after an output routine that holds
  inserts differs.** Cause (F's root cause): before deciding whether a
  fire holds, the fold read the last build's routine export at that same
  fire position (`\global\holdinginserts0`, made by the routine that
  had not run yet this build). Fixed: at a fire position still predicted
  (not yet retracted or made again by this build) the fold reads what
  reaches the node, not its own last definitions there
  (`read_at_with(.., at_node)`; `BuildFold::fired`).
  `tests/fuzz/fixed/f1-output-second-update.repro` (7 lines,
  no-op edit): `\holdinginserts1`, a group with an `\output` that sets
  `\global\holdinginserts0`, `\unvbox255\penalty\outputpenalty` and ships
  a `\vsplit`, an `\insert100`, a `\vbox spread 1000pt{}` and a
  `\vadjust{\penalty-5000}`. A cold build ships 4 pages (TeX's 4); the
  same text updated again ships 3, no node running again. In TRIP
  every update after the first differed (`--fuzz 6 1 1 --threads 1`).
  *Cause* (traced): the cold build fires three times at node (0,15).
  The first fire reads `\holdinginserts` = 1 and holds the inserts. The
  routine's `\global\holdinginserts0` goes into the names index at
  (0,15), so fires 2 and 3 read 0. Then the default fire at (0,24). In
  the second update the first fire at (0,15) is a predicted position. The
  page builder's hold test (page.rs, §1018: `env.read(Holdinginserts)`)
  goes through `BuildFold::read` and `read_at`, i.e.
  `names.reaching(key, (k, node+1))`. That includes the last build's
  fold definitions at (0,15), which are not retracted yet: `pass()` only
  drops positions before the scan's, and `fire()` retracts only after
  the hold test. So it reads 0 and does not hold. `find_memo` then finds
  the last build's no-hold run (reads nothing that changed), and one
  fire is lost. One `Pos` holds what all three fires at (0,15) defined.
- **F2. A kept node's error context shows the line as it was** (fixed,
  `logs.rs` only: `tests/fuzz/fixed/f2-error-context-line.repro`, an `X`
  inserted after the error on its line;
  `tests/fuzz/fixed/f2-error-context-edit-before.repro`, a `\mark{b} `
  inserted before the erring node). The capture is in `graph/diag.rs`
  (`show_context`): the node's log holds `X{before}CUT{after}`, the
  line around the error taken from the piece's text when the node ran.
  The node keeps its run while its reads are the same, but the rest of
  its line is other nodes' text, so an edit there changes nothing it
  read. The fix is in `logs::resolve`, when the log is gathered
  (`line_now`). The node's own text, from where its predecessor stopped
  to where it stopped, is unchanged. The error's place in it is the one
  split that `before` ends with and `after` begins with. From there the
  line is read again in the current text. If the split is not found or
  not unique (a pseudo file's line), the captured line is kept. Hyphens
  seeds 102, 802, 970 and 1187 pass now. Done without a core change: a
  placeholder carrying the error's offset from `diag.rs` would make it
  exact, if A wants it.
- **F3 (fixed, A, 2c217c3: `tests/fuzz/fixed/`). After an update of the
  empty text, no page is shipped again.** Fixed with the round changes of
  the \input-as-pieces slice: a round's page matching compares the pages
  with the runs that made them (the last build's, `Build::page_runs`),
  not with the previous round's.
  `tests/fuzz/fixed/f3-update-after-empty.repro`: `Build::new`,
  `update("")`, then `update("\hsize=100pt\vsize=100pt \font\x=cmr10 \x
  A\par\end")`: no page (a cold build ships one). Every corpus document:
  0 of its pages (`tests/design.rs` `cold_equals_edit` reported it). The
  cold path is the edit path by construction (one `update`); the empty
  build leaves state the next update reads.
  *Cause* (traced): a round that restructures leaves the next round
  comparing the wrong builds. In `Build::update`, a round that returns
  `None` (a run changed the sequence) puts back the last build's pages,
  orgs and marks ("the pages to compare with are the last build's"). But
  the next round's `olds`, and so `old_runs`, are the pieces of the round
  that just ran, not the last build's. `advance_match` finds every piece
  `same_material` (`Arc::ptr_eq(o, run)`: the same run) and, under "the
  same material throughout: the same pages", takes the last build's
  pages. After the empty text those are none. A smaller case:
  `tests/fuzz/fixed/f3-restructured-update-reuses-pages.repro`,
  `\count1=1 \hsize=100pt` + `A\par\end` in one piece, the `1` edited to
  `0`. The edited piece has no old `expect`, and its run ends the job
  (`got.ends`), so round 1 restructures. Round 2 reuses the last build's
  page `[0.1]`, where a cold build ships `[0]`. Any edit in the piece
  holding `\end`/`\bye` restructures the same way.
- **F4 (fixed, A, with F1). A second update loses the last page's last
  line.** The same cause as F1: the fold's first fire at a predicted
  position read the last build's routine definitions there; F1's fix
  fixes it (without it, it fails still).
  `tests/fuzz/fixed/f4-second-update-last-line.repro` (32 lines cut from
  big's text by delta debugging, no-op edit, both builds 1 thread in
  priority): an `\outer\def` whose body never closes, undefined
  `\alloc@`-style macros, plain's `\footnote` with its `\vfootnote`
  cut, a `\headline` with `\firstmark`, `\vsize=5in`. The cold build
  ends page 2 with the word "which"; the same text updated again ends
  page 2 without it. Like F1, nothing was edited.
- **F5 (fixed, A). A new definer with an equal value re-runs a macro's
  reader.** Fixed in two places. (1) `reached` asks F's predicate: a
  settled reader whose every read of the name has the value it read
  (`Run::values_read`, lean runs keep their reads' values now) is not
  marked, its edge moves to the definer reaching it now (`move_edge`).
  (2) That alone is not enough: the piece that no longer defines the name
  runs first, and the new definer has not run yet, so the reader does see
  another value then and is marked. So when a marked piece is chosen to
  run, its dirty names are looked at again (`still_dirty`): those whose
  reads have their values again are dropped (edges moved), and if none is
  left and it is entered as it was, its run is kept without running
  (`keep_again`). Correct whatever the order: the edge's source is
  always the definer reaching it at that moment, so a later change there
  marks it again. `tests/flow.rs`
  `a_new_definer_of_the_same_value_reruns_no_reader` (a split moving
  `\def\fz` and `\hsize` into new pieces runs the two new pieces only,
  lean or not, Suspend and Speculate). F's repro passed already on this
  tree (the matcher of F6 runs the split pieces as new); moved to
  `fixed/` with F3's smaller repro and F6's.
  Was:
  Story seed 161 step 2 (`--fuzz 161 2 1 --threads 1`): a swap moves
  `\def\fz{lazy}` into a new piece (id 134 for 133). Piece "\medskip
  \noindent{\bf EXERCISE 1.1}" runs 29 of its 77 nodes, from n48, the
  node before the first reading `\fz`; every read from before it has the
  value it had (`\fz`, `\vsize`, `\escapechar` now from piece 134, the
  same values), and n1, reading `\vsize` from the new piece too, is kept.
  A macro call's reading node compares the definer, not the meaning.
  It is not only macros: pages seed 659 step 5 (a swap) runs the
  `\bye` piece again, 5 of its 6 nodes, whose only changed read is
  `\vsize` = 14208860 now from piece 134 instead of 133. Hyphens seed
  830 step 5 (a split) runs 2 of 3 nodes of `\tolerance=1000`, whose
  only changed read is `\hsize` = 14208860 from piece 137 instead of
  133. Story seed 1127 (8 threads) is the swap again. So some reads
  compare the definer's piece, not the value.
  `tests/fuzz/fixed/f5-new-definer-equal-value.repro` (cut down from
  hyphens 830): `\vsize=2in \hsize=3in \tolerance=1000` split into two
  pieces before `\tolerance`, so `\hsize` now comes from a new piece with
  the same value. The `\tolerance` piece runs 2 of its 3 nodes again.
  *Cause* (traced): not `Read::same`, which compares reads inside a
  piece's chain. Marking between pieces is by position, not by value.
  When a piece finishes, `changed_names(old_run, run)` compares its
  exports with the run it replaces. A definition that moves to another
  piece with the same value counts twice: the edited piece no longer
  exports `\hsize` (changed), and the new piece, with no old run, exports
  it (changed). For each, `reached` (build.rs) marks every reader of the
  name whose edge source is at or before the changing place
  (`src_at_or_before(src, m)`), and `mark` makes it dirty. It never asks
  whether the value now reaching that reader is the one it read. The
  wrong predicate: `reached` marks reader `r` of `key` when
  `src_at_or_before(&sh.seq, src, m)`; it should also require
  `!read_val.seen_as(&names.reaching(key, (rpos, node)).1)`, where
  `read_val` is the value `r`'s run recorded for its first read of
  `key`. When the value is the same, move the edge's source to the new
  definer and do not mark. The trace (repro above): the update runs piece 1
  with `changed=[Hsize]` (marks piece 3), then the new piece 2 with
  `changed=[Hsize]` (marks piece 3 again); piece 3 reads `\hsize` =
  3in both times.
- **F6 (fixed, A). Big: the order of a cold build changes its output.**
  Cause (F's root cause): a restructure left pieces without runs, the
  next layout's `Matcher` indexed only pieces with runs, and its
  equal-text fallback gave a piece another place's run (the second of two
  equal paragraphs, which had read `\hsize=100pt`), kept as `Done` with
  its definer now after it. Fixed: every last build's piece is indexed by
  its text's `Arc` (a piece keeps its own place, run or not); an equal
  text is taken as its own only right after the last piece taken (a text
  cut again near an edit), and another place's run is never kept as it is
  — the piece runs from it, every kept node's reads looked at again
  (`Spec::moved`). Restructures now also wait for the runs to settle.
  `tests/order.rs`: big at 1 worker random:400 and 8 workers random:7, and
  F's six-piece repro at 40 orders.
  Was: At 1
  worker, waiting reads, `Order::Random(400)` gives another DVI and log
  than priority order, every time (`tests/design.rs`
  `the_same_output_at_any_w` reports it); at 8 workers,
  `Order::Random(7)` does about one run in eight. Big seed 236 (`--fuzz
  236 1 1 --threads 1`) is it: its first build ran in that order. The
  smaller documents do not show it.
  `tests/fuzz/fixed/f6-order-moves-a-run.repro`, 6 pieces: `\hsize=300pt`;
  paragraph P; `\hsize=100pt`; P again (the same text); `\input fzsub`;
  `\end`. `phitex-flow --fuzz-orders story FILE random:9 priority` says
  "differs": in random order the first P is broken at 100pt.
  *Cause* (traced): the `\input` piece restructures the sequence after 4
  of the 6 pieces are done. The first P has not run, the second has. The
  next round's layout matches the new pieces with the old ones.
  `Matcher::new` indexes only old pieces that have a run, so the first P
  does not find its own old piece by pointer. `Matcher::find` falls back
  to equal text and takes old piece 3 (the second P, which read
  `\hsize` = 100pt from piece 2). That run is kept as Done at position 1:
  its definer is now after it, and its read is not checked again there.
  Its `Para.lb.hsize` is 100pt, and the lines come from it. Big shows the
  same: 30 pieces were still unrun at the restructure, and part 0's story
  piece takes part 1's copy (`\hsize=3in`, from part 0's hyphens). The
  pieces' graphs are the same in both orders (the dumps differ only in
  piece ids; the lean regeneration shows the right reads). The page fed
  at the first fire differs.
  *Fix wanted*: index old pieces without a run by pointer too, so each
  piece keeps its own place. A run that the text fallback moves to
  another place has its reads checked by value there, or is not kept.
  Not only in random orders: big seed 1153 step 6 (removing an `\input
  fzsub`, 8 threads; it fails at 1 thread in priority order too). The
  first build, in priority order, restructures at the `\input` with 164
  of 170 pieces done. Part 1's "Mr.~Drofnats" piece has not run yet and
  takes part 2's run by equal text (old 152 for new 143). That build is
  already wrong, the same way as its cold build. The update without the
  `\input` keeps the wrong run, and the cold build of the edited text
  (no restructure) does not have it.

**LaTeX documents** (`fuzz::Kind::Latex`, `fuzz::latex_corpus`,
`phitex-flow --fuzz ... --latex`). A LaTeX document is its own job,
`&latex NAME`, on the saved format, as `tests/latex_docs.rs` builds them
(`fuzz::latex_format`: `latex.ini`'s job as job `latex`, `Build::format`,
made once per process and shared by the corpus;
`PHITEX_FUZZ_LATEX_FMT=FILE` keeps it in FILE across processes). It runs
in extended mode and batch mode, with the directories that B's
documents' `.dirs` and `tests/latex/dirs.txt` give. The corpus
(`fuzz::LATEX_DOCS`) is `tests/fuzz/latex/tiny.tex` plus B's ten
documents, named `latex-NAME`. The harness was made on tiny: sections,
labels and refs, a list, an equation, a tabular, a figure,
`\newcommand`, `ifthen`. The edits are made in the document's text
(`fuzz::edit_latex`):
- a word; an inserted paragraph; a `\section` title;
- a `\label` moved to another paragraph; a `\ref`'s target changed;
- `\emph`/`\textbf` wrapped round a word or removed;
- an `\item` added or removed; an equation changed; a tabular cell;
- a figure or table moved to another paragraph;
- `\usepackage` added or removed (ifthen, calc, array, verbatim);
- the class option toggled (none, 11pt, 12pt, twocolumn);
- the `\newcommand` body; a comment line; a blank line inside or outside
  an environment.
Each update is compared with a cold build (at 8 workers). A seed that
times out names its step: its reproduction holds the text and edit it
hung on. Seeds that
keep their nodes check over-invalidation, as for plain: the graph now
holds only the document's nodes, and the format's definitions sit before
piece 0.

`tests/fuzz.rs` `latex_edits_equal_cold_and_latex`: `PHITEX_FUZZ_LATEX`
seeds (1) of one edit on tiny, compared with the cold build and with
`latex` itself on its first run (DVI with the date masked, `.aux`). Locally, 3 seeds of 2 edits at 8
threads (random schedules) passed, and so did seed 0 against `latex`.

**300 LaTeX seeds** (job 7878: `LATEX=1 JOBS=6 TIMEOUT=180
scripts/flow-fuzz.sh 30`, 30 seeds of 6 edits on each of B's ten
documents, at 1 or 8 threads at random, on the saved format; snapshot
89f3a41 = 96bdfcc with the timeout-resume and OUT fixes, before
A's W2 and keep_again fixes): 229 passed, 0 over-invalidated, 71 failed.
- **Over-invalidation: none.** The check runs on the seeds that keep
  their nodes (a third), now that the graph holds only the document's
  nodes.
- **69 timeouts** (180 s a seed): gfx 30 of 30, rules 12, titlepage 7,
  packages 7, math 7, tables 2, ams 2, sections 1, figures 1; layout
  none. gfx fails at step 2–4 whatever the edit or schedule, so it may
  be slowness (W3: gfx 40 s through the CLI) rather than a hang; the
  others are mostly blank lines in or out. To be told apart: each
  replayed alone with a longer limit.
- **2 DVI mismatches**, the update against our cold build: ams seed 24
  step 3 (the class option toggled) and rules seed 10 step 3 (an `\item`
  removed). The log first differs at a `Package:` line (amsmath,
  caption).
- The repros are in `~/code/flinner/partex-phitex-runs/cmd-7878/fuzz-latex/latex/`.
The budgets table gets LaTeX rows (`phitex-flow --budgets REPS latex`
for all of them, `latex-NAME` for one; `tests/budgets.rs` runs tiny once).
The edits are word, line, page-break, the `\newcommand` body (def) and
`\parskip` (param), at 8 workers, lean, on the saved format.

LaTeX budgets, job 7816 (accl node, `-c 10`, commit b137ad8 = 1e84df9
with this slice): 5 repetitions, median ms (median steps). Cold is the
document's cold build. "-": the edit has no place in that document (no
`\newcommand`; sections has no word of five letters in a paragraph the
edit picks).

| document | cold ms | word | line | page-break | param | def |
|---|---|---|---|---|---|---|
| tiny | 378 | 102 (486) | 102 (870) | 406 (29289) | 92 (3265) | 122 (3207) |
| sections | 315 | - | 197 (964) | 460 (30772) | 105 (3049) | - |
| math | 622 | 260 (630) | 290 (2588) | 508 (22187) | 293 (1866) | - |
| tables | 723 | 281 (478) | 335 (1681) | 522 (21994) | 336 (2352) | - |
| titlepage | 1564 | 941 (2872) | 1274 (4407) | 1097 (26608) | 1203 (6498) | - |
| packages | 506 | 120 (408) | 119 (1916) | 545 (26250) | 121 (2931) | - |
| ams | 2224 | 911 (2368) | 962 (9527) | 1733 (55298) | 1043 (9097) | - |
| gfx | 678 | 169 (420) | 166 (483) | 729 (31100) | 142 (788) | - |
| layout | 5307 | 2624 (490) | 3098 (2458) | 3549 (60257) | 3327 (3416) | - |

- **figures and rules hang.** Each ran into the 1500 s limit: figures on
  page-break (after word 313 ms and line 324 ms, max), rules on line
  (after word 1421 ms, cold 3507 ms). A says it is the same bug as W2
  (the fixpoint's second run). Repro: `phitex-flow --budgets 1
  latex-figures`, `latex-rules`.
- Every row misses the brief's budgets: a word is 100–2600 ms, not
  ≤ 50 ms, and page-break reruns the whole document's nodes (21994 to
  60257 steps), about a cold build.
- The time is not in the steps. A word edit runs 400–3000 steps
  (median), yet its longest phase is `run`: layout runs 490 steps in
  2.5 s of `run` (pieces: 11). Not traced yet: the next profile, for P
  or A.
- Each edit kind's maximum steps (17000–58000) is a whole-document
  rerun in one of the five repetitions. The median is the edit's own
  cost.


**Edit budgets** (`src/budgets.rs`, `phitex-flow --budgets [REPS]
[DOC]`, `scripts/flow-budgets.sh`, `tests/budgets.rs`, informational).
Documents: the plain ones; big (48 pages); bigout (152 pages, big's body
twelve times with a `\mark` per part, through plain's output routine,
headlines and footnotes). Each at 1 and 8 workers (predicted reads),
lean and not. Five edits, each made 5 times on the document's text and
taken back unmeasured:
- word: a word of five letters or more, replaced by one as long;
- line: a line of words inserted in a paragraph in the middle;
- page-break: a six-line paragraph inserted near the start;
- def: `\fz` defined again, in a variant with `\fz` at the start of
  every paragraph;
- param: `\parindent` at the start of the body.
The table gives the median and maximum time, steps (nodes run), pieces
run, the join's time and the longest phase, and flags each miss of the
brief's budgets (word ≤ 50 ms and ≤ 300 steps, line ≤ 50 ms, page-break
≤ 100 ms, any edit ≤ 25% of a cold build).

First local table (5 repetitions, median ms):

| | big lean | big nodes | bigout lean | bigout nodes |
|---|---|---|---|---|
| word, 1 worker | 7.9 (10 steps) | 63.4 | 20.1 | 273 |
| line | 7.8 | 64.2 | 20.0 | 186 |
| page-break | 12.0 | 64.5 | 23.4 | 209 |
| def | 12.4 | 66.6 | 31.3 | 199 |
| param | 140 | 133 | 550 | 430 |
| cold | 135 | 160 | 360 | 400 |

- **B2 (fixed, A). Every edit scans every page, lean or not.** Fixed as
  wanted: a fancy fold has restart points too. After a fire whose
  routine was a memo hit and put nothing back (no held-over insertions,
  no list), the routine's last page is one; so is a default-output page
  of a fancy fold with nothing held over. The point carries the fold's
  state besides the contributions (`page::FoldState`: the marks,
  `\deadcycles`; `PageOrg::fold`); `Scan::resumed` starts with it, and
  convergence asks for the same restart point and the same state.
  Matching (`advance_match`) is on for fancy folds. A cold build has no
  memo hits, so its routine pages are not restart points; the first
  edit's are.
  Was: **Every edit scans every page, lean or not** (traced: the root of
  both). A word edit in the middle of big makes the scan feed all 1850
  items from piece 0 and fire all 48 output routines (memo hits), in both
  modes. Nothing is skipped: no `advance_match` match, no `resume`, no
  convergence. The predicate: `matching: !fancy && (...)` (build.rs, the
  round's `State`) turns matching off whenever the document has an
  `\output` routine. Every plain document has one (`\plainoutput`). And
  in page.rs `let resume = if fancy { None } ...` gives no page a restart
  point, so `converge` never finds `o.resume == Some(r)`. With an output
  routine the scan is O(pages) for any edit. Lean bigout, word edit
  (scan 16 ms): `find_memo` 152 × 53 µs = 8 ms (`m.page == rt.page.b` and
  the reads' `seen_as`), the fires' installs and retractions 2–3 ms, the
  page builder's feeding of about 5500 items about 5 ms. Big is the same
  in a third of the size. *Wanted*: a restart point for a fancy fold. The
  page builder's state after a fire whose routine's run is the last
  build's (a memo hit, same `\box255`, same exports), at the first item
  after the call, is what a fold started there with nothing would have.
  So the resume rule can treat a memo-hit fire like a default one: resume
  at the last such page before the first changed piece, and converge at
  the first page after the last changed piece. That makes the scan
  O(changed pages).
- **B1 (fixed, A). With pieces keeping their nodes the same scan costs 15 times
  more** (traced). Fixed: `changed_at` looks up each name's last reader
  in a per-run table made at the first fire in the run (`Run::last_readers`,
  `State::last_readers`), not a walk over the later nodes' reads. 66 ms against 4.5 ms on big, 188 ms against 16 ms on
  bigout, for the same items fed and the same 152 memo hits. On bigout,
  `install_fold_defs` takes 89 ms (lean: 2 ms), the retraction of the
  predicted definitions inside `fire` about 93 ms, `find_memo` 6.5 ms.
  Both go through `refresh_exports` and `changed_at` (build.rs). For each
  key a fire defines between a piece's nodes (`\outputpenalty`,
  `Page(0..8)`, the marks, `\box255`...), `changed_at` calls
  `run.reads_after(key, pos.1)`. The predicate: `Run::reads_after`
  (graph.rs:1702) is `(i+1..nodes.len()).any(|j| nodes[j].reads.iter()
  .any(|r| r.key == *key))` for a run with its nodes, a walk over every
  later node's reads. A lean run has a binary search over its tables.
  The fires sit in pieces of thousands of nodes, so each fire costs
  O(nodes × reads). *Wanted*: an index per run of the nodes reading each
  key (sorted node lists), so `reads_after` is a binary search as for a
  lean run. Then B2's restart point removes most fires altogether.
- `param` (every paragraph reflowed) is a cold build's cost or more:
  about 100% of cold, 140% on bigout lean.
- `word` misses the 300 steps where the word is part of a definition or
  an alignment: in pages it is inside `\para`'s body (1858 steps), in
  table inside the `\halign` (430). There the kind is really "def".
- The one-page documents miss the 25% rule on any edit (cold ≈ 20–30 ms,
  an edit 3–7 ms).

**F's F7 (fixed, A). An `\input` added by an edit never completes**
(bisected by F to 2c217c3; 1203 of 5000 cluster seeds).
`tests/fuzz/fixed/f7-input-added-in-update.repro`: the update printed
`(./fzsub.tex` and stopped. Cause: the edited piece's run ran again from
its old chain, and a kept node could be the run's end at a `Kind::Input`
(or `Kind::Cut`) whose file was laid out as pieces or not by the last
layout, so the new layout's decision was never asked again. Fixed in
29b6919 (a node ending a run at a cut or an `\input` is never kept:
`run_piece`'s `par_ok`). `tests/fuzz/fixed/f7-input-added-hang.repro`
(big seed 565): a deadlock under Suspend at one thread. A run waiting for
a definition of piece d, d not started and not enterable yet (its entry
pieces not run), parked; but its thread was the only one, running it
nested in the runs it had leapfrogged. Now it runs the first piece before
d that can be entered (`reach_now`), as an undefined name's wait already
did.

**B's F7 (fixed, A). A fire between a piece's nodes ran the piece again
from its first reader.** Each time the output routine fires it defines
names between the piece's nodes (`\vsize`, `\count0`, `\@colht` at
`(k,i)`). `reached` marked every reader of those names in piece k (its
edge is its first reader's, before the fire or not), and a lean piece has
no nodes to keep: the piece ran again whole at every fire (titlepage, one
piece of five pages, 78k commands for its 20k). Now (1) dirty names are
positional (`Dirty`: a name, the first node it may reach): a definition at
`(k,i)` marks piece k from node `i + 1` (`changed_at`, `mark_from`), and
`run_piece` takes only the readers from there as dirty; `reached` leaves
the piece's own nodes to `changed_at`. (2) A piece keeps its nodes until
the scan has passed it (`PState::lean_later`), so a fire runs only the
nodes after it. titlepage 78k -> 25k commands at one thread (sections
35k -> 28k); `tests/flow.rs`
`a_fire_between_a_pieces_nodes_reruns_only_the_nodes_after_it` (12 pages
in one piece, every line reading `\count0`: 5629 -> 752 commands of
433). Still 0.5–0.8 s cold per document: the per-command cost, not
reruns.

**T1 (open, A; was called F8 before F's F8). An edit of TRIP that never
ends.** Found once F1 let TRIP's updates be checked:
`tests/fuzz/hang/t1-trip-edit-hangs.repro`
(TRIP seed 1 step 3, `--fuzz 1 3 1 --doc trip`): a character of
`\hbox to 23pt{} % output now` replaced (`23st`). The update runs a
piece again and again (a worker in `run_piece`, the round waiting), at
any schedule (1 thread, Suspend, priority too). Pre-existing: 72ee98e
hangs the same. Not in `open/` (its replay does not end); a timeout's
reproduction names its step now (`fuzz::run_seed_at`).

*T1's trip timeouts are mostly TeX's own.* Job 7978 checked 7899's 108
trip timeouts (5000 plain seeds on 86941d0, 120 s a seed) with
`scripts/flow-fuzz-tex.py`: the system's `tex` on TRIP's two passes,
before and after the edit, a run counted only when tex wrote its files.
It also ran `phitex-flow --fuzz-cold FILE before|after` on each.
- 89 (and both of 7853's originals): tex loops on pass 2 after the edit,
  so this is TRIP edited into a loop, not a bug. flow-fuzz.sh now
  classes these "tex-loops trip".
- 20: our cold build of the text after the edit loops while tex ends on
  every pass. All 20 are `swap` edits, so the bug is in a cold build
  with no update (owner A), e.g. `cmd-7899/fuzz-out/j1/fuzz-trip-398-2.repro`.
- 1: both cold builds end.
The two cut repros named below were cut before the cold builds were
checked (A found that their text loops before the edit); the
minimizer now requires the cold builds to end, or, with FLOW_MIN_TEX,
cuts a cold loop of ours while tex still ends.

*T1 on plain documents too.* Fuzz subset 7853 ran 300 seeds at 1 thread and 60 at 8 on each of trip,
pages and table, with a 120 s limit per seed, on a50fa8a. 43 timeouts:
42 trip and 1 pages; table had none. The log is not compared, so the 9
log-only failures do not count. By edit kind the trip timeouts are 21
swap, 8 char-delete, 6 char-replace, 5 char-insert and 2 blank-delete,
at any schedule. A reports that trip fails differently run to run on the
same tree, and is bisecting. Three were cut down (job 7869,
`flow-fuzz-min.py`, while `--fuzz-replay FILE --alone` still runs past
45 s). They are in `tests/fuzz/hang/`, not replayed by the tests:
- `t1-pages-blank-insert.repro`, pages seed 134 step 4: 3 paragraphs,
  1587 bytes, a blank line inserted inside the second paragraph.
  *Fixed* (A; in `tests/fuzz/fixed/`): the scan waited for a piece
  whose run `keep_again` had kept (every mark read its value again)
  with 8 of its 18 paragraphs left without lines by an earlier round's
  restructure, and nothing queued them (the setup queues the unbroken
  paragraphs only of pieces Done when it starts); the stall detector
  names the scan's piece and its paragraphs without lines now.
  Invariant: a piece Done has lines for every paragraph, or their
  breaks are queued (a kept run leaves the queue as a fresh run would).
- `t1-trip-swap.repro` (trip seed 29 step 2, Suspend, priority, nodes
  kept) and `t1-trip-char-delete.repro` (trip seed 12 step 5, lean):
  2 and 3 of trip.tex's units.

F's report of it (F7):
- **F7 (fixed, A: above). An `\input` added by an edit stops the update at the file**
  (a regression on the gate head, from A's 2c217c3, "\input anywhere
  the reader is on the piece's own text is pieces"; flow-trip has not
  merged it and passes). Seen in the cluster run of 5000 seeds (job 7722,
  gate head 394eeab, before the F2 fix): 1203 failures of the classes
  "edit DOC input-add | dvi: ./fzsub.tex" on every plain document, and
  the one "timeout big" (seed 565). They come from the core, not the
  environment: the second file is in memory (`Build::set_file`, found
  before any directory), and 394eeab gives them here too. The update
  prints `(./fzsub.tex`, and then neither the file's text nor the `)`
  nor the pages after it. Bisected with `phitex-flow DOC --edit 'B
  second=>\input fzsub<newline>B second' --check`: 59c78f1 gives
  "edit == cold: yes", 2c217c3 and 394eeab give "NO". Regression
  repros in `tests/fuzz/fixed/` (they pass here and must keep passing;
  replayed with a 300 s limit, a hang counts as a failure):
  `f7-input-added-in-update.repro` (four lines and a two-piece
  `fzsub.tex`, `\input fzsub` added before the second paragraph) and
  `f7-input-added-hang.repro` (big, seed 565's first edit: on 394eeab
  the update never ends). B's bac0a76 does not contain 2c217c3, and both
  repros pass on it.

**F9 (fixed by A, as F8 above: `run_piece`'s `line_ok`). A kept node in
an edited piece reads its old line.** The log is not compared any more (the bar is the DVI/PDF and the
written files), so F9 fails no seed. Its repro is kept, not replayed, in
`tests/fuzz/log-only/`, for when a line number reaches a written file
(an `.aux` `\write` of `\the\inputlineno` would). Found by the LaTeX fuzz: tiny seed 2, with `\usepackage{ifthen}`
removed, gives `LaTeX Warning: Reference ... on input line 7` where a
cold build says 6. The cut-down repro is
`tests/fuzz/log-only/f9-kept-node-line-in-piece.repro`. Four lines and a
blank one, nodes kept: `\count2=2` is removed from the line above
`\message{[\the\inputlineno]}`, in the same piece. The update prints
`[4]`; a cold build prints `[3]`.

*Cause*: `input_line_no` (graph/io.rs) is `Key::LineBase` plus
`line_in(text, pos)`, the line within the piece. Only the first part is
a read. When an edit inside the piece removes or adds a line above a
node, that node can still be matched: its text and reads are the same,
so it keeps its run and the line it computed then. `cond_line` (the
`\if` line of `\showifs` and of the end-of-file warning) has the same
shape.
The keeping is in `run_piece` (graph.rs). An old node after the edit
(`text_ok`: `inp_pos >= e.old_end`) whose reads `reread` the same is
kept. Its `Key::LineBase` read is the same, because the base is the
piece's, not the node's.

*Wanted*: the node's line in the piece counts as one of its reads. For
example, a node that computed a line (`input_line_no`, `cond_line`)
carries a flag, and `text_ok` fails for it when the edit changed the
number of `\r` before it (`Edit` gives the old and new spans). The F2 fix in `logs.rs` covers
only the error context.

In LaTeX it is common: in a local run of 30 seeds on sections, 9
failed and all 9 were F9. Edits that add or remove a line above
(`\usepackage`, a comment line) move every later
`on input line N` of the piece, and pieces are long in LaTeX. 4 of
the 9 show it on the last edit alone; 5 show it only through the
seed's history.

The cluster run's other classes: 827 F1 (TRIP), 27 "timeout trip" (F1's
document, at 8 threads under load), and about 12 "F2?" on hyphens. The F2
fix (e1f2aa0) is not in 394eeab.

**Design tests** (`tests/design.rs`, the brief's 1–6; 7 waits for
`Key::File` by writes):
1. `a_segment_start_reads_zero_state`: `Entry` is `Start`, `File(usize)`,
   `Rest(usize, Option<usize>)` only; `Entry::Start` is entered in
   `RState::start()`, made from nothing; `RState` holds no name's value;
   every piece of the corpus is entered so. Passes.
2. `no_dominant_thread`: big, 8 workers, best of three: predicted reads
   16–18% for the busiest thread (asserted ≤ 25%); waiting reads 16–28%
   (shown, not asserted: it depends on how the machine runs the threads).
3. `cold_equals_edit`: one public function makes an `Output`
   (`Build::update`; `fuzz::cold` wraps it), and it does not branch on a
   first run; an update after the empty text equals a cold build: F3.
4. `a_blank_line_above_any_definition_reruns_no_reader`: above every
   piece holding a definition (the bodies, and plain.tex in one document),
   at most one piece runs, no rerun, a cold build's output. Passes.
5. `the_same_output_at_any_w`: every corpus document at 1 and 8 workers,
   both reads, priority and random orders: one DVI, log and set of files.
   Big differs in random orders: F6 (reported; any other difference
   fails).
6. `an_edit_to_the_untaken_arm_runs_nothing_past_it`: the φ at `\fi`;
   an edit to the arm not taken runs one piece and at most two nodes.
   Passes.

**Hooks wanted (A):**
- **H1** `Build::check_entries(&self) -> Result<(), String>`: for every
  piece that has run, its run's entry equals `entry_state` now, and an
  `Entry::Start` piece's equals `RState::start()`. Test 1 then checks
  built graphs, not only the source.
- **H2** The dump marks the nodes run in this build (`  n5* …`). The
  fuzz then checks the brief's bound node by node: outside the edited
  pieces a node runs only if it reads a changed name or a value a node run
  defined.
- **H3** The dump shows a token list or macro whole, or with a hash of the
  whole (now cut at 24 tokens; a box by its dimensions, a stream by its
  position): such values count as changed whenever a node run defines
  them.

## LaTeX's format: `latex.ltx` through the graph (agent B)

`tests/latex.rs` builds TeX Live's `latex.ltx` (and the 60 files it
reads: `texsys.cfg`, expl3, the font definitions, `hyphen.cfg` and the
patterns, the Unicode data) and compares the log with `etex -ini -etex
latex.ltx` (TeX Live's `etex` is pdfTeX writing DVI) up to `\dump`'s
` (preloaded format=…)` line, at the five schedules, without errors.
The input directories are the ones the oracle read from (`-recorder`'s
`.fls`, `tests/latex/dirs.txt`). The CLI takes `--etex`, `--input-dir DIR`
(repeatable) and `--jobname NAME` to run the same build by hand.

**pdfTeX's names.** In extended mode the graph is the oracle's engine:
pdfTeX 1.40.29 in DVI mode. Its 152 primitives are named
(`prim::PDFTEX_PRIMITIVES`). What a format reads is made:

| pdfTeX | graph |
|---|---|
| the 36 integer and 13 dimension parameters, 4 token parameters | `IntPar`/`DimenPar`/`TokPar`, with pdfTeX's initial values (`\pdfcompresslevel` 9, `\pdfdecimaldigits` 3, `\pdfminorversion` 4, `\pdfhorigin` 1in, `\pdfpxdimen` 1/72.27in…) |
| `\pdftexversion`, `\pdftexrevision`, `\pdftexbanner`, `\pdfshellescape`, `\pdflastxpos`… (14 read-only integers) | `Prim::PdfItem(k)`: a constant (`PDF_ITEMS`) |
| `\pdfstrcmp`, `\pdfescapestring`/`name`/`hex`, `\pdfunescapehex`, `\pdffilesize`, `\expanded` | steps (convert): the expanded text, shown, transformed (`expand::pdf_string`); `\pdffilesize` reads `Key::File` |
| `\ifincsname`, `\ifpdfprimitive`, `\ifpdfabsnum`, `\ifpdfabsdim` | conditionals; `\ifincsname` reads the step's `\csname` depth |
| `\quitvmode` | a node: `\indent`'s path only in vertical mode |
| `\pdfglyphtounicode` | a node scanning its two texts (a PDF-only table: DVI ignores it) |
| the other PDF-only primitives (`\pdfobj`, `\pdfliteral`…) | `Prim::PdfUnimpl`: an error if executed ("… is not supported yet"; a format only tests and `\let`s them) |
| `\lastnamedcs`, `\ifcondition`… (LuaTeX's) | undefined, as in pdfTeX |

**Language gaps the format found (fixed).**
- *A definition made while the closing command is fetched* (`\begingroup
  \expandafter\expandafter\expandafter\endgroup\expandafter\ifx\csname
  directlua\endcsname\relax`, expl3's idiom): the `\csname`'s `\relax` is
  defined inside the group, by the node whose command closes it. A
  `Close` node's group is the outer one, so it outlived the group
  (LaTeX then took LuaTeX's branch). Now `Exec::closes` (every close) lets
  the node's local definitions made before the close end with the group.
- *`\fontdimen` growth past 65535* (expl3's intarrays, `\fontdimen
  65536\font`): `Key::FontDimen`'s index is a `u32`.
- *web2c's `\openout j = `name'.` line* in the log (extended mode).

**The reader state's size (per node).** The format is 3.13M nodes in
one piece; at first 12.1 GB peak (3.9 KB a node). Three changes of
representation, the states unchanged:
- `RState.pending` is a `Pending`: the top list held in the state (a
  token read changes only it), the lists below a chain of cells shared
  from node to node (was a `Vec` copied into every node's state).
- A macro call reads the body where it is (`Macro.body: Arc<[Tok]>`, the
  definition's), its parameter tokens pushing their arguments when met
  (tex.web 357's `out_param`; was a copy of the body with the arguments
  substituted, kept by every state it was pending in).
- A node's read of a name defined in the piece shares the previous equal
  read (`Scratch::inside`: same defining node, scope, value).
Now 4.65 GB at `-j 1` and 4.69 GB at `-j 8` (1.5 KB a node).

**The cost of expansion.** latex.ltx: 45.0M expansion steps (29.0M of
them macro calls) in 3.13M nodes, the reader handing out 467M tokens (TeX
pays about 10–15 ns a token, the graph 125 ns at first). Changes, the
states, nodes and steps unchanged in meaning:
- *Arguments in one go* (`Exec::bulk_undelimited`, `bulk_delimited`): an
  argument all in the list the reader is in is taken by a [`Cursor`] over
  that list (a body's parameters read as their arguments), as a part of
  the list (`Arg`: zero copies) when it is one; else token by token as
  before. The reader now hands out 228M tokens.
- *An argument is read inside its body's list* (`Level::sub`), not as a
  list of its own pushed above it (tex.web 357 makes it an input level).
- *Names read before in the piece* (`Known`, now by the name's number
  in `Scratch::cs`): the read of the definition reaching the next node while
  no node after it defines the name, a local one while no group opens or
  closes, one from before the piece within its range of nodes; a run of
  fewer than 256 nodes keeps none. Chain lookups: 42.6M → 3.9M.
- *A name read again in the node* (now a stamp by number), and
  no copy of a macro's handle per call.
Cycles (user, `-j 1`): 161G → 123G; time 59 s → 45 s; 0.76M → 1.0M
steps/s. big2 (10k steps): reading unchanged within noise (+1.5%
instructions).

*Conflict with the brief (§3, "memoised expansion is required").* With
the body read in place, a call's expansion is one push of (the macro's
body, the arguments): there is no substituted body left to memoise, and a
memo keyed by the arguments needs them scanned first, which is the cost.
What could be memoised is the argument scan keyed by (the macro, the raw
input after it); it hits only on re-runs, and re-runs already keep nodes.
The memo's purpose is met by in-place bodies and arguments as parts of
lists. (Carried to the brief by the coordinator.)

*Names by number* (`build::CsTable`, one per build, kept from update
to update): `Tok::Cs` and `Key::Cs` hold a control sequence's number, a
per-build alias of its name (two are equal exactly when the names are;
never a version). A token is 8 bytes and copies no counted handle; a key
of a name hashes and compares as one word; text shows a name through the
table (`val::Shown`). Per piece, `Scratch::cs` is a table by number: the
node's read of the name (a stamp: the first read in a node no longer
searches the node's reads) and the read the piece keeps of it (`Known`).
The table is the build's, not the process's (`tests/shapes.rs`: no
process-wide state). latex.ltx `-j 1`: 135G → 92G cycles, 198G → 171G
instructions, about 35 s; 1.3M steps/s, 13M tokens/s; peak RSS 3.8 GB.

*The expansion profile now* (latex.ltx, `-j 1`, perf, inclusive): macro
calls (`expand` → `call`, argument scans included) 45%; integer scanning
(`scan_int`, `\numexpr`) 32%; `\expandafter` 25%; `\string`-like
conversions 16%; `\edef`/`\write` expanded texts 15%; conditionals 14%;
`\the`/token registers 14%; name lookups (`find`) 10%; `\csname` 9%; the
input lists (`Pending` pop) 8%; definitions 6%. By self time: `expand` 11%,
`Cursor::next` 10%, `next_raw0` 6%, `Pending::pop` 5%, the allocator 4%,
`next_x_m` 3%, `Cursor::arg` 3%, `next_raw` 3%, `Chain::resolve` 3%,
`Key::eq` 3%. What remains is reading itself (a token at a time through
the input lists) and the lists' cells (an allocation per macro call); a
meaning is still cloned per lookup (a macro's handle).

*The Unicode data block* (expl3-code.tex's `\ior_str_map_inline` reading
the Unicode data into intarrays, about 2.9M of latex.ltx's 3.1M commands,
one serial chain). Its piece exports 171,120 names: 157,260 of them
`Key::FontDimen` (the intarrays' entries, 157,753 definitions), 17,671
control sequences (619,065 definitions), 45 counts; `Key::ReadStream` is
defined 45,533 times (a line each), the stream advancing by offset over
the file's shared bytes (no copy). Installing the exports is about 2% of
the run, dropping the replaced run's 3M nodes at the commit 6.5% (on the
critical path). Profile (perf, frame pointers, inclusive): `next_x_m`
63%, `expand` 45%, `scan_int_ext` 32%, `\if` tests 26%, expanded texts
26%, `\expandafter` 26%, definitions 24%, `next_raw` 20%, conversions
19%, `scan_expr` 19%; by self time `expand` 11%, `Cursor::next` 10%,
`next_raw0` 6%, `Pending::pop` 4%, the allocator 6%, `outer_of` 3%,
`Cursor::arg` 3%, `Chain::resolve` 2%. *The `\outer` check of each
token scanned* (`Exec::outer_of`, `Field::Outer`) went through
`find_slow` (the node's reads searched one by one, a key hashed, a new
read made per name and node): 13.5% of the run. Now a name's is found by
its number as `find_name` does, and the piece's kept read (`Known`) holds
its outer-ness read, made once. latex.ltx `-j 1`: 119.4G → 109.2G cycles,
212G → 191G instructions (45 s → 42 s on a loaded machine).

**Gaps left for the core (A), with repros.**
1. *A file written and read back in one run.* `texsys.cfg` writes
   `texsys.aux` with `\immediate\openout`/`\write`/`\closeout`, then
   `\openin`s it. `Key::File` is answered by the build (`Env::file`) before
   any write is replayed, so the read sees the disk: the test places the
   recorded `texsys.aux` first in the input directories. Repro:
   `\immediate\openout1=t.aux \immediate\write1{x}\immediate\closeout1
   \openin1=t.aux \ifeof1 \message{no}\else\message{yes}\fi \end` (TeX:
   yes; the graph: no, without a `t.aux` on disk). Proposal: an
   `\immediate` write's file is a value the writing node defines
   (`Key::File(name)` → the bytes written so far), the reads after it in
   the order reading that, not the `Env`.
2. *(done: re-cuts)* *The cut is blind to catcodes.* `latex.ltx` line 160:
   `\catcode`\%=12 \def\@percentchar{%}\catcode`\%=14`; the cut's brace
   count reads `%` as a comment, stays at depth 1 to the end of the file,
   and `latex.ltx` is one piece (94 blank lines). Repro: those three
   lines, then two paragraphs with a blank line between: one piece.
   Now a reader that reaches a blank line at depth 0 under the real
   catcodes (canonical state, `RState::cut_point`; at a line start after a
   blank line, the rest not blank, outside an `\input`'s text) ends its
   run there (`Kind::Cut`); the layout re-cuts the rest of the piece's
   text and the pieces after the cut run with the start state
   (`Build::recut`, cached by text so the pieces keep their runs).
   `latex.ltx` is 19 pieces; the first (lines 1–1234) still does 3.0M of
   the 3.1M commands, `expl3-code.tex` read inside a macro's argument
   (line 1147, gap 3).
3. *`\input`'ed files run inside the calling piece* where `\input` is
   not a command of its own at a piece's top level: in `latex.ltx` every
   `\input` is inside a macro or a group (`\@input`, `\InputIfFileExists`,
   expl3.ltx's), so expl3-code.tex's 36k lines, the font definitions and
   the patterns are read by the one piece, and the build is sequential
   (one thread did 100%).
4. *(done: live words)* *The main memory counts what was ever defined.* `graph::words_of`
   sums every node's definitions (a macro's body, a box, a token list)
   over the run, kept or not; TeX's `mem` holds only what is live (a
   macro defined again frees the old body). LaTeX's format passes 5M
   words this way (`tests/latex.rs` runs with `main_memory` 2^32). Repro:
   `\count1=0 \def\l{\def\a{<1000 x>}\advance\count1 1 \ifnum\count1<10000
   \expandafter\l\fi}\l \message{\the\count1}\end`: TeX 10000, the graph
   "TeX capacity exceeded, sorry [main memory size=5000000]".

## LaTeX documents (agent B)

`tests/latex_docs.rs`: six documents (`tests/latex/docs/`: sections and
lists, base math, tabular and a table float, figures and `\twocolumn`,
a title page with `\tableofcontents` over five pages, `ifthen`/`calc`/
`array`/`verbatim`) against TeX Live's `latex` (pdfTeX in DVI mode, the
`latex` format) on each one's first run, no `.aux` present. Our side is
one build, a session of two jobs: `latex.ini` (the format: pdftexconfig,
`latex.ltx`, `\dump`) and `&latex NAME`, with the directories `latex
-recorder` read from. Compared: the DVI file (its dated comment masked)
and the files the document's job writes (`.aux`, `.toc`), at one thread,
eight, and eight speculating in a random order. The CLI runs the same
session: `--before latex.ini --terminal latex.ini --terminal '&latex
NAME' --font-dir DIR`.

All six pass at the three schedules (the `.aux` read-back below aside).
Cold, the format included: about 46 s each at `-j 1` and at `-j 8` (the
format's first piece is 99% of the work); `latex` takes 0.19 s with its
dumped format, 10.8 s to make the format (`pdftex -ini -etex latex.ini`)
on the same machine. Now the documents run on the saved format, at five
schedules (next section): 0.7–2 s each.

**Language gaps the documents found (fixed).**
- *`\lastskip` at a piece's start* (`\addvspace` after a list, a
  section after a blank line): the main vertical list's last item is
  another piece's. Now `Key::MainTail` (Conflict 13).
- *`\prevdepth` after a paragraph of the main vertical list*: its lines
  are broken by the page fold, so the reader state did not have the last
  line's depth (`\the\prevdepth` read -1000pt; LaTeX's floats save and
  restore it around their `\vbox{}`: no interline glue after a float).
  Now the reader state says `page::MAIN_DEPTH` there and at a piece's
  start, and `\prevdepth` reads `Key::MainDepth`: defined globally by
  each node at the outer level that sets it, the paragraph itself
  (`Val::Para`) when its lines set it, the depth made when read (line
  breaking is a function of the paragraph). After an output routine the
  main list's is the one from where the routine fired (`pop_nest`), not
  `ignore_depth`.
- *A `\write` on a page an output routine shipped* was expanded a second
  time, at the page builder's call with the definitions there
  (`Page::builder`: only pages the builder ships itself, TeX's default
  output, have their whatsits done by the fold). Repro:
  `\output={\begingroup\let\x\relax\shipout\box255\endgroup}
  \def\x{\iffalse} Text\write-1{[\x a]}\par\end` ("Incomplete \if").
- *A φ's read from before its `\if`* was entered in the run's reads at
  the `\fi`; a global definition in the arm of a name also defined
  between the piece's nodes (its place in the names index exact) then
  answered it, the read was stale at every commit and the piece ran
  forever (`twocolumn` with a `figure*`; `\addvspace` at `-j 8`). Now it
  is entered where the `\if` began (`graph::cond_at`), and the old
  run's entries are kept only before the first such place of the nodes
  run again.

**Gaps left (core, A).**
1. *The `.aux` file read back at `\end{document}`* (`\@@input\jobname.aux`
   after `\immediate\closeout\@mainaux`): gap 1 of LaTeX's format, a
   file written and read in one run. Not found, then "End of file on
   the terminal!" ends the job (batch mode), after the last page and the
   `.aux` file; the `.toc` lines that read writes (`\@writefile`) are not
   written. The test allows exactly these two errors and an empty `.toc`
   while the first is there. Repro: any of the documents.
2. *`\prevgraf` after a paragraph of the main vertical list* is not the
   lines' count (the same deferral as `\prevdepth`; no document reads it).
3. *`Key::MainTail` after a removal* in a node of its own is none, not the
   item before (Conflict 13).

## LaTeX packages (agent B)

Four more documents in `tests/latex_docs.rs`, on the saved format, at the
five schedules, against `latex` in DVI mode on the first run: `ams`
(amsmath and amssymb: `align`, `gather`, `multline`, `cases`, `\text`,
`\DeclareMathOperator`, the matrices, `\binom`, `\dfrac`), `gfx`
(graphicx and color with the dvips driver: `\includegraphics` of an EPS
by its bounding box, scaled and turned, `\scalebox`, `\rotatebox`,
`\reflectbox`, `\resizebox`, `\textcolor`, `\colorbox`, `\fcolorbox`, a
coloured paragraph: the `\special`s), `layout` (babel english, geometry
on A5, fancyhdr, enumitem, three pages), `rules` (booktabs, caption,
enumerate). A document's `NAME-*` files (the EPS) sit beside it and are
copied for `latex`; the graph reads them from the documents' directory.

Gap found and fixed: *an alignment entry's leaders are glue*
(`align::totals`, §796's `hpack`): booktabs' `\cmidrule` is a spanned
`\omit` entry holding `\leaders\hrule\hfill`; its stretch was not
counted, the entry's glue was not set and the rule was not drawn.

Not yet: hyperref (the `.aux` fixpoint — format gap 1 — and its many
`\special`s/`\pdf…` commands), microtype (pdfTeX's font expansion and
protrusion, PDF mode only).

## LaTeX corpus, round 3 (agent B)

Five more documents, on the saved format, five schedules, identical with
`latex` (DVI, written files): `tikz` (pgf/TikZ: paths, nodes, a circle, a
fill, a `matrix of nodes`), `ltmc` (longtable over a page break's worth
of rows, multicol), `lst` (listings with numbers and a frame,
`\lstinline`, fancyvrb's `Verbatim`), `bib` (a given `bib.bbl` read by
`\bibliography`; makeidx's `\index` writing `bib.idx`, compared as a
written file), `href` (hyperref with the dvips driver: `\url`, `\href`,
`\ref` with anchors; it gets through on the first run without the `.aux`
read-back, writing `href.aux` and `href.out`). No language gap in their
output.

*TikZ is the expansion-heaviest document* (P measures with it): cold, `-j
1`, on the format (fastdev): 132 s, 1,127,514 commands run for 225,506
(five runs of its one piece: the reruns from a routine's definitions,
F7). Profile (perf, frame pointers): 62% is `Chain::resolve` and
`Chain::scope_of` finding a definition inside its node by scanning the
node's definitions (`Key::eq`, `Key::word`). The φ nodes are large: of
267,608 definitions in the run, 164,646 are φs; the largest φ nodes hold
9,828, 7,666, 6,986 and 4,069 (each twice: nested `\fi`s, the outer one
taking every φ of the inner arm again), and every later read of a name
whose definition is in one of them scans it. For P (name lookup): find a
definition in its node by index (the node's def position in `defs_of`),
not by scanning. Fixed on the language side: `pop_cond` collected the
arm's keys with `Vec::contains` (quadratic in the keys an arm defines):
now a set (164 s → 132 s).

## The format: definitions before the first piece (agent B)

AGENTS_BRIEF §6.1: "known at t=0: the format, a set of definitions with
known versions". `Build::format` makes it after a build of the format's
job (`latex.ini`: pdftexconfig, `\pdfoutput=0`, `latex.ltx`, `\dump`):
each name's definition reaching the end of the source (the pieces'
exports in order), of the names TeX's `store_fmt_file` keeps
(`format::saved`): control sequences and active characters with their
meanings, codes, parameters, registers (boxes included), fonts and their
parameters, patterns and exceptions, the interaction mode, the e-TeX mode,
`format_ident`. Left out, as a run's and not a format's: files and
streams, the job's name, the page builder's state and marks, `mag_set`,
whether the job ended, the main vertical list. Also kept: the fonts'
numbering (the order the job loaded them, `Run::fonts` piece by piece: a
DVI file's font numbers; the build's own fonts are numbered after them).

`format::encode` writes it as our own file (`src/format.rs`, no crate):
a magic line, the control sequences' names (tokens and keys refer to
them by number in the file; `decode` gives each its number in the reading
build's `CsTable`), the fonts (the TFM bytes and the size: loaded again as
`\font` loads them), the numbering, the definitions. The order is the
names' own, so the same definitions give the same bytes
(`format_reads_back`).

`Build::set_format` gives a build the format: the names index answers a
read of a name no piece defined before (the place before piece 0) with the
format's definition, as it answers IniTeX's initial values
(`Names::initial`; the by-name table lives in the index, `FormatDefs`).
No piece starts with anything copied in, and a value carries no place: a
definition from the format equals the same definition made by running
`latex.ltx`. CLI: `--dump-fmt FILE` after a build (`phitex-flow latex.ini
--etex --jobname latex --terminal latex.ini ... --dump-fmt latex.fmt`),
`--fmt FILE` before one (with `--terminal '&latex NAME'`; a `--terminal`
line makes the text a job, `graph::jobs`).

Held by `tests/latex_docs.rs`: the six documents on the format at five
schedules are `latex`'s (DVI, `.aux`); `format_is_the_prelude`: a document
on the format is byte for byte the session `latex.ini`'s job + the
document (DVI, written files, the document job's log, errors) at five
schedules; `format_edit_is_cold`: an edit (a word, an inserted paragraph)
on a format-based build equals its cold build.

Numbers (release, this machine, the CLI): the format is 4.13 MB
(pdfTeX's `latex.fmt` 2.33 MB), made in 0.2 s after the 45 s build, read
in about 55 ms. The documents cold on it, `-j 1` / `-j 8`: sections 0.71
/ 0.74 s, math 0.97 / 1.00, tables 1.06 / 1.17, figures 0.98 / 1.01,
titlepage 2.04 / 1.97, packages 0.73 / 0.80 (`latex`: 0.19 s each, its
format read included). What remains is the documents' own work: about
11 µs a command (the expansion's cost, above) and the reruns below; one
to three pieces each, so eight threads have little to share.

Gaps:
- *(done)* *The command line's `-interaction` at a job's start.* web2c
  sets `interaction` from `-interaction` after loading a format (tex.ch,
  `interaction_option`); `latex.ltx` dumps `\errorstopmode`, so `latex
  -interaction=batchmode NAME` runs in batch mode. Now
  `Build::set_interaction_option` (CLI `--interaction MODE`, which sets
  the mode before any definition too) is defined at each job's start
  (`Env::interaction_option`), in a session and on a format alike; the
  format keeps the dumped mode (equal to the session). None until set:
  TRIP's second run (`tex` without `-interaction`) keeps its format's.
- *Reruns from definitions between a piece's nodes (core, A).* On the
  format a document's own cost shows: `titlepage` (one piece, five
  pages) runs its 20k commands ten times (204k), `sections` twice. Each
  page the output routine fires defines names between the piece's nodes
  (`\vsize`, `\count0`, LaTeX's `\@colht`…); every node of the piece
  reading such a name from before the piece is dirty, also the ones
  before the definition's place, so the piece runs again from its first
  reader, once per round. The session had the same reruns (hidden in the
  format's 45 s).

## Single-thread cost (agent P)

Why one thread of the graph is slower than TeX, measured (2026-10-10,
release build, `-j 1`, `perf stat -e cycles:u,instructions:u` inside
`scripts/sandbox`; the machine is shared, so cycles move by 10-20% from
run to run and instructions are the steadier figure).

**Against TeX on this machine.**

| document | graph | TeX | cycles |
|---|---|---|---|
| `latex.ltx` as a format (`etex -ini -etex`) | 41.5 s, 107.8G cycles, 190.8G instructions | 10.8 s, 28.8G, 71.2G | 3.75x |
| big2 (IniTeX prelude, 616 kB of text; `tex -ini`) | 0.98 s, 2.54G, 4.36G | 0.19 s, 0.38G, 0.65G | 6.7x |
| sections on the saved format (`latex`) | 0.68 s, 1.47G | 0.165 s, 0.45G | 13x the document's own work |
| titlepage on the saved format | 2.24 s, 5.42G | 0.14 s, 0.44G | 50x the document's own work |

`etex -ini` takes 10.8 s on `latex.ltx` here, not about 1 s: the expl3
Unicode-data block is as slow in TeX. `latex` spends 0.34G of its
0.38-0.45G cycles loading its format (a bare `\stop`), so a document's
own work is 40-110M cycles in TeX. The documents' other factor is
commands run per command: 1.9x (sections), 4x (math, figures), 5x
(tables), 9x (titlepage) — the reruns from definitions between a piece's
nodes (LaTeX documents, gaps). Per command run the graph costs about 6x
TeX; on `latex.ltx` 34k cycles a command against 9.2k.

**Per token.** `latex.ltx` hands out 228M tokens (2.8M of them made
from characters, the rest from rewritten lists; arguments taken in one
go are not counted): 177 ns a token, `etex` 47 ns over the same count.
A token is a plain value (`Tok` holds no counted handle); about 0.5
counted-handle increments a token come from the lists: a macro's
meaning cloned per call (31M), the body and argument handles, the
input lists' cells.

**Counts** (temporary counters, not in the code).

| | `latex.ltx` | big2 |
|---|---|---|
| nodes | 3.13M | 199k |
| reads a node | 15.6 | 9.6 |
| definitions a node | 0.30 | 0.04 |
| allocations a node | 71 | 9.2 |
| bytes allocated a node | 6.7 KB | 3.4 KB |
| macro calls / expansion steps | 29.0M / 32.5M | 8k / 12k |
| input-list cells (one allocation each) | 52.7M | 102k |
| argument arrays (`Arc<[Arg]>`) | 20.7M | 4k |
| one-token lists put back (`Exec::back`) | 26.0M | 94k |
| outer-ness reads (one a token scanned in an argument or definition) | 88.9M | 2k |
| reads through `Known` / `Chain::resolve` | 44.7M / 4.3M | 0 / 1.9M |

In big2 every read goes `Chain::resolve` (a miss in `defs_of`), then
the run's `Outside` map (80% hits), the rest to the names index:
`Known` is off in pieces under 256 nodes (they average 83).

**By phase** (perf -g on a frame-pointer build, each sample under the
innermost phase on its stack: the allocator, hashing and counted
handles are charged to the phase that called them).

| phase | `latex.ltx` | big2 |
|---|---|---|
| the input lists and argument scanning (`next_raw0`, `Pending`, `Cursor`, `call`) | 46% | 2% |
| name lookup (`find`, `resolve_shared`, `Chain::resolve`, `Outside`, the names index) | 20% | 39% |
| expansion primitives (`\expandafter`, `\csname`, conversions, `\the`, conditionals) | 17% | 1% |
| node recording (`Exec::new`/`finish`, the chain's indices, dropping nodes) | 7% | 10% |
| a piece's end (exports, sorting its reads) | 2.5% | 9.5% |
| install (validation, the readers map) | 2.4% | 10% |
| commands (material, assignments) | 1% | 9.5% |
| line breaking, the page fold, DVI | 0% | 12% |
| tokenizing characters | 1.2% | 2% |

Also: dropping a lean piece's 3.1M nodes is 5.9% of `latex.ltx`; the
growth of `Vec<Tok>`s (realloc) about 4%. On titlepage a node's
definitions scanned key by key (`Chain::resolve`, `scope_of`: `Key::eq`
16% inclusive) and the walk of TeX Live's whole TFM tree at the first
`\font` (`tfm::Index`, 0.28 s) stand out; on B's tikz document the
first is 62%.

**Changes** (each its own commit; results unchanged, the before and
after on the same base):

1. *A node's definition of a name found by index.* `Chain::defs_of`
   holds, per key, each defining node once with the index of its last
   definition of the key (the one `Chain::resolve` took by scanning the
   node's definitions backwards, `Key::eq` each); `resolve`,
   `scope_of`, `Run::leaned` and the routine's `after_close` read it.
   A φ at the `\fi` of a long arm defines every name the arm changed,
   and each later read reaching it scanned them all. Instructions
   (before → after): tikz 759G → 207G (149 s → 40 s), titlepage 8.41G →
   7.22G, sections 2.72G → 2.54G, `latex.ltx` 190.8G → 189.2G, big2
   unchanged; DVIs identical.

## Parallelism (agent S, 2026-10-10)

Measured on accl (64 cores, 2 hardware threads each; a job's `-c N` is N
hardware threads), release build, cold builds, `Reads::Suspend` (the
CLI's cold build). `PHITEX_SCHED=FILE` writes the scheduler's events of
every round (each run's setup, reading, install validation, lock wait and
hold, nodes and outcome; parks; idle waits; picks with the pieces ready;
the settled runs that change the sequence; every wait for the scheduler's
lock); `perf-in/sched.py FILE` reports them per round.

**Hardware first.** Measure pinned to physical cores
(`perf-in/hw.sh`: `sched_setaffinity`, one hardware thread per core or
two), with N independent `-j 1` processes side by side as the control
(job 7919, harrier): eight big2 processes read 772 → 790 ms each, so the
hardware alone costs nothing up to eight cores; unpinned on a busy node
with siblings (job 7914) the same build's reading looked 3.7x dearer. A
thread count above the physical cores buys nothing today: big2 at `-j 8`
on 4 cores with their siblings takes 183 ms (round 1), on 8 cores 194 ms.
The CLI's default thread count should be the physical cores, not the
logical CPUs.

**The first wall: the scheduler's lock (big2).** big2 (2401 pieces of 83
commands), pinned, round 1: 758.6 / 429 / 227 / 194 ms at `-j` 1 / 2 / 4
/ 8; the whole cold build 930-966 / 606 / 378 / 351 ms, the DVI the same
at every `-j`. Reading summed over the threads grows only 649 → 839 ms;
what grows is the one `Mutex<State>`: installs hold it 51.6 / 58.9 /
60.7 / 115.7 ms in all (21 → 48 µs an install as its lines move between
cores), at `-j 8` 60% of the round's wall, and threads wait for it 31.8%
of their time (15% before an install, 16% taking it back after a task).
Of the time held, the edges (`readers`, one insert per distinct name a
run read: 382k reads, 2401 installs) are 87%: 45.9 of 52.7 ms at `-j 1`,
91.6 of 109 ms at `-j 8` (job 7925); validation again 0.2, exports 3.2,
marks 1.8, entries 1.6 ms at `-j 1`.

**The edges out of the lock** (edaa404; A reviewed): the readers index
is sharded by name (`Readers`, 64 shards), a run's edges go in after its
reads are validated and before the scheduler's lock, each shard locked
once (`run_and_install` states the order and why no mark is lost;
`edges_outside_the_lock_lose_no_mark`). big2 pinned, before → after (job
7935, three runs each, the DVI the same): `-j 8` cold 355-368 → 303-307
ms, round 1 192-202 → 142-144 ms, waits for the lock 32-34% → 4.6-4.9%
of the threads' time; `-j 2`, `-j 4` the same walls, waits 4.5 → 0.7%, 8
→ 1.1%. Known cost to revisit: `-j 1` is 2.5-4% slower (923-930 → 946-968
ms), the inserts sorted by shard and the shard locks.

**LaTeX documents: rounds, the body's chain, per-page reruns** (job
7930, pinned, `Reads::Suspend`; `perf-in/biglatex.tex` is sections.tex's
body 40 times, 130 pieces). Walls `-j 1` / `-j 8`: sections 694 / 728
ms, titlepage 833 / 910, biglatex 5659 / 3992; six rounds each. A round's
barrier is cheap (layout 0.1-5.7 ms); what it lets run is not. biglatex
by round (`-j 1` / `-j 8`, ms): R1 1360 / 1613: `\documentclass` (a file
read) changed the sequence at 5.9 ms, but the round went on until every
piece settled, so the body ran on the layout before its class was read
and was thrown away (at `-j 8` serially too: parks 85%); R2 50 / 61 the
class's pieces; R3 1513 / 1351 the body on the right layout (at `-j 8`
parks 68%, 1.44 readers at once: each section defines names the next
reads, counters and `\@currentlabel`, a chain under `Suspend`); R4 1888 /
438 the output routine's definitions between nodes, every later piece
run again once per page (piece 129: 23-26 runs, 23 pages); R5-R6 676 /
479. Fix of R1's waste (A reviewed): a final settled run that changes
the sequence ends the round at once (`settle`, b06939f); one that is not
final yet (a fold read unchecked, its entry not holding) holds the pieces
after it, their runs stopped (`State::due_at`, de0d20cd). Measured by P
at `-j 1` (flow 67d83701 + these): tikz 58,619 → 2,487 ms (238k commands
run for 226k, 905 runs, 2 reruns), c0 54,735 → 1,397 ms, the DVIs
`latex`'s. Format builds no longer run a piece to 2.4M commands and 50
GB on a layout about to change. What remains for tikz: 65 rounds, one per
file it discovers (rounds 1-64 2-90 ms each), and the 65th, past the cap
of 64, reads every further file inline (519 ms).

**`latex.ltx` is a true chain.** 31.2 s at `-j 1`, 31.1 s at `-j 8`
(job 7914). Of its 19-20 rounds one holds 27.3 s, and in it one piece
holds 96.2% of the commands (2.9M): expl3-code.tex's Unicode-data block
(`\ior_str_map_inline`), read inside a macro's argument. Each iteration
reads the next line of the stream (`Key::ReadStream`, defined again per
line) and defines intarray entries the next ones read: a dependency
chain, not a granularity problem: no cut, file piece or scheduler policy
shortens it (AGENTS_BRIEF §6.5: the build is as long as its true chain).
Amdahl's serial fraction from that round alone is at least 0.88. The
format is built once and saved (`--dump-fmt`); documents start from it,
so this is left as is.

## Found by the CLI (agent W)

- **W1 (open, A): a LaTeX document in nonstop mode with no `.aux` never
  ends.** `phitex build --output-format dvi titlepage.tex` (the CLI's
  build then ran in `\nonstopmode` over the `latex` format: `Build::
  set_interaction_option(Some(1))`, terminal `&latex titlepage.tex`, the
  document of `tests/latex/docs/`, no `titlepage.aux` anywhere) ran over
  25 minutes without ending on flow 9fa4ccf (cluster job 7802, test
  `build_to_the_fixpoint`); the same document in batch mode (`latex
  -interaction=batchmode`) ends in under a second with the `.aux` gap's
  two errors. Suspected: the missing `.aux` at `\enddocument` in nonstop
  mode (TeX: `*** (job aborted, file error in nonstop mode)`). The CLI
  now builds in batch mode by default; its tests run every build under
  `timeout 600`.

- **W2 (open, A/B): the fixpoint's second run of a LaTeX document never
  ends.** `phitex build --output-format dvi titlepage.tex` in batch mode
  (cluster job 7826, test `build_to_the_fixpoint`, flow 45ffd43): run 1
  is the `latex` run that passes (0.84 s, the `.aux` gap's errors); then
  the CLI gives the build the files run 1 wrote (`Build::set_file` of
  `titlepage.aux` and `titlepage.toc`) and calls `update` with the same
  source: that update did not end within 600 s (killed by the test's
  `timeout`). The CLI's loop is `crates/phitex-cli/src/session.rs`,
  `Session::fixpoint`; B's `Fixpoint` replaces it when on flow.
- **W3 (open, A): `gfx` takes 40 s through the CLI** (`latex
  -interaction=batchmode gfx.tex`, job 7826; the other documents 0.7–1.6
  s, all on the cached format), its DVI right. The CLI's input
  directories are the document's, then one directory of links standing
  for TEXINPUTS.

## Tag numbers

Several tag namespaces must stay free of collisions. `Key::word` puts the
kind of key in the bits above 40, and `Hash` and `Eq` compare that word,
so two kinds with one tag are one key. The format file (`format.rs`) has
one-byte tags for keys, meanings, items, whatsits and values. The tests
check the tags without upkeep:
- `val.rs` `tag_tests`: `every_key` lists one key of each kind.
  `key_index` is a `match` without wildcard, so a new `Key` variant does
  not compile until it has an index, and `every_key_once` fails until it
  is listed. `key_word_tags_are_distinct` then checks the word tags.
- `format.rs` `tag_tests` does the same for meanings and whatsits
  (`every_meaning`, `every_whatsit`). It checks that the bytes the
  writer gives each key, meaning and whatsit are distinct per namespace.
To add a variant, give it the next free tag, then its index and sample
in these lists. `cargo test -p phitex-flow --lib tag_tests` runs them.

## Memory (agent A, 2026-10-10)

`latex.ltx` as a format at `-j 1` (`--cut-lines 200`), peak RSS: bac0a76
5.39 GB, 2c217c3 4.46, 021c65e 5.92, their merge 1ae75fa 7.82, head
36b0c50 6.85. `PHITEX_CENSUS=1` prints what each run holds
(`Run::census`): nodes, reads, distinct `Read`s, and the distinct ones
that are copies of another (same name, source, scope, conditional and
field: a read not shared).

| | reads held | distinct `Read`s | of the expl3 piece (2.9M nodes) | copies | RSS |
|---|---|---|---|---|---|
| 2c217c3 | 51.6M | 1.41M | 0.76M | | 4.46 GB |
| 1ae75fa | 51.3M | 25.9M | 23.9M | | 7.82 GB |
| 36b0c50 | 51.3M | 20.0M | 18.5M | 18.4M | 6.85 GB |
| shared outer-ness (this commit) | 51.3M | 1.70M | 1.05M | 79K | 3.75 GB |

The merge brought F's reads of outer-ness alone (`Field::Outer`): a
name scanned where `\outer` is forbidden reads only that, a copy of the
read with the field changed; a later whole read of it in the same node
(`Exec::whole`, the scanned argument expanded) made another copy, with
the field back. Every node scanning and then expanding a name held two
`Read`s of its own instead of the one all its readers share (the
`Known` cache made each outer copy once only for a name the piece
kept). Now `Exec::outer_copy` makes one outer copy per read in a run
(`Scratch::outer_of`), and `whole` gives back the read it was copied
from (`Scratch::whole_of`). Cold build 41.6 s -> 35.1 s.
`tests/flow.rs` `a_scanned_and_expanded_name_is_one_read`: 400 nodes
scanning and expanding `\x` hold 40 distinct reads (439 without
`whole`'s lookup).

## The inventory

### (i) Expandable and input-rewriting: steps — 40

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\botmark` | top_bot_mark | step: rewrites the input with `Key::Mark(which, 0)` (a read), defined by an output routine's run (`graph/output.rs`) | approx: empty outside a routine until the fold runs routines |
| `\csname` | cs_name | step: builds the name, rewrites the input with it; an undefined name is defined `\relax` (local) by the node the step is in | done |
| `\else` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\endcsname` | end_cs_name | outside `\csname`: an error node | done |
| `\endinput` | input | step: the innermost `\input` file ends with its current line (`Infile.endinput`); in pieces the core drops its later pieces (IO) | done (in an `\input` file; in the main file an error) |
| `\expandafter` | expand_after | step: expands the token after next once, rewrites the input | done |
| `\fi` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\firstmark` | top_bot_mark | step: rewrites the input with `Key::Mark(which, 0)` (a read), defined by an output routine's run (`graph/output.rs`) | approx: empty outside a routine until the fold runs routines |
| `\fontname` | convert | step: the font's name and size (tex.web 472) | done |
| `\if` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifcase` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifcat` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifdim` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifeof` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ; the test reads `Key::ReadStream(n)` | done |
| `\iffalse` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifhbox` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifhmode` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifinner` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifmmode` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifnum` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifodd` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\iftrue` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifvbox` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifvmode` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifvoid` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifx` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\input` | input | at the top a node whose result is the file's text (`Key::File` read; its out state's new `RState::texts` entry, `TextKind::File`); inside scanning a step; read in the piece now, cut into pieces by the core (IO) | done (single-piece emulation) |
| `\jobname` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens; the name a read of `Key::Job` | done |
| `\lowercase` | case_shift | node (not expandable): reads lccode/uccode (`get`) of each character and rewrites the input with the changed text; defines nothing | done |
| `\meaning` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\noexpand` | no_expand | step: marks the next token not-to-expand once (`Tok::DontExpand` in the rewritten input) | done |
| `\number` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\or` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\romannumeral` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\splitbotmark` | top_bot_mark | step: rewrites the input with class 0's last mark in `Key::SplitMarks`, defined (globally) by each `\vsplit` | done |
| `\splitfirstmark` | top_bot_mark | step: rewrites the input with class 0's first mark in `Key::SplitMarks`, defined (globally) by each `\vsplit` | done |
| `\string` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\the` | the | step: reads the internal quantity (`get`) and rewrites the input with its tokens (unexpanded inside `\edef`/`\message`) | done |
| `\topmark` | top_bot_mark | step: rewrites the input with `Key::Mark(which, 0)` (a read), defined by an output routine's run (`graph/output.rs`) | approx: empty outside a routine until the fold runs routines |
| `\uppercase` | case_shift | node (not expandable): reads lccode/uccode (`get`) of each character and rewrites the input with the changed text; defines nothing | done |

### (ii) Assignments and scoping: nodes defining Keys — 151

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\abovedisplayshortskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\abovedisplayskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\adjdemerits` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\advance` | advance | node: reads the register/parameter (`get`), defines it anew | done |
| `\afterassignment` | after_assignment | node putting the token in RState.after_assignment; the next assignment node rewrites the input with it | done |
| `\aftergroup` | after_group | node appending the token to the innermost RState.groups entry; the Close node rewrites the input with them | done |
| `\baselineskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\begingroup` | begin_group | `Kind::Open` node (RState.groups gets SemiSimple); scoping is the reaching definitions | done |
| `\belowdisplayshortskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\belowdisplayskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\binoppenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\boxmaxdepth` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\brokenpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\catcode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\chardef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\clubpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\count` | register | node defining `Key::Count/Dimen/Skip/Muskip(n)`; as a value, a read of it | done |
| `\countdef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\day` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\def` | def | node defining `Key::Cs(name)` = `Meaning::Macro` (long/outer flags from RState); `\edef`/`\xdef` expand the body as steps of this node | done |
| `\defaulthyphenchar` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\defaultskewchar` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\delcode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\delimiterfactor` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\delimitershortfall` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\dimen` | register | node defining `Key::Count/Dimen/Skip/Muskip(n)`; as a value, a read of it | done |
| `\dimendef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\displayindent` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\displaywidowpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\displaywidth` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\divide` | divide | node: reads, defines (truncating) | done |
| `\doublehyphendemerits` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\edef` | def | node defining `Key::Cs(name)` = `Meaning::Macro` (long/outer flags from RState); `\edef`/`\xdef` expand the body as steps of this node | done |
| `\emergencystretch` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\endgroup` | end_group | `Kind::Close` node, defines nothing; inside a `{` group: "Missing } inserted" and the input rewritten with `}\endgroup` | done |
| `\endlinechar` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\errhelp` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\errorcontextlines` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\escapechar` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\everycr` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\everydisplay` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\everyhbox` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\everyjob` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\everymath` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\everypar` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\everyvbox` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\exhyphenpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\fam` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\finalhyphendemerits` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\floatingpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\font` | def_font | prefixed node: `\font\f=name [at|scaled]` reads the TFM (`Env::font_file`, a pure function of the name): the font is a value whose identity is (name, size); at its first load (`Key::FontLoaded`) it defines global `Key::FontDimen(key,n)`, `HyphenChar`, `SkewChar`; defines `\f` = `Meaning::Font` and `Key::FontIdent(key)` (tex.web 1257–1260) | done |
| `\fontdimen` | assign_font_dimen | `\fontdimen n f`: `Key::FontDimen(font key, n)`, always global | done (no extension of the last font) |
| `\futurelet` | let | node defining `Key::Cs(name)` = the meaning read (`get`); `\futurelet` rewrites the input with the two tokens | done |
| `\gdef` | def | node defining `Key::Cs(name)` = `Meaning::Macro` (long/outer flags from RState); `\edef`/`\xdef` expand the body as steps of this node | done |
| `\global` | prefix | node setting RState.global/long/outer for the next node (which must be an assignment) | done |
| `\globaldefs` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\hangafter` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\hangindent` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\hbadness` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\hfuzz` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\hoffset` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\holdinginserts` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\hsize` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\hyphenation` | hyph_data | node defining `Key::Exceptions(lang)` globally (reads `\language`, the old value, the `\lccode`s); read by line breaking (see Hyphenation) | done |
| `\hyphenchar` | assign_font_int | `Key::HyphenChar(font key)`, always global | done |
| `\hyphenpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\interlinepenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\language` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\lccode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\lefthyphenmin` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\leftskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\let` | let | node defining `Key::Cs(name)` = the meaning read (`get`); `\futurelet` rewrites the input with the two tokens | done |
| `\linepenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\lineskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\lineskiplimit` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\long` | prefix | node setting RState.global/long/outer for the next node (which must be an assignment) | done |
| `\looseness` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\mag` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\mathchardef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\mathcode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\mathsurround` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\maxdeadcycles` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\maxdepth` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\medmuskip` | assign_mu_glue | node defining `Key::GluePar(p)` (Val::Glue in mu) | done |
| `\month` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\multiply` | multiply | node: reads, defines (Arithmetic overflow: no definition) | done |
| `\muskip` | register | node defining `Key::Count/Dimen/Skip/Muskip(n)`; as a value, a read of it | done |
| `\muskipdef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\newlinechar` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\nulldelimiterspace` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\nullfont` | set_font | initial meaning `Meaning::Font(null font)`; selecting it defines `Key::Font` | done |
| `\outer` | prefix | node setting RState.global/long/outer for the next node (which must be an assignment) | done |
| `\output` | assign_toks | node defining `Key::TokPar(p)` (Val::Toks); `\output` wrapped in braces; `=\toksN` copies | done |
| `\outputpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\overfullrule` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\parfillskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\parindent` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\parshape` | set_shape | node defining `Key::Parshape` (Val::Parshape) | done |
| `\parskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\patterns` | hyph_data | node defining `Key::Patterns(lang)` globally (reads `\language`, the old value, the `\lccode`s); always accepted (see Hyphenation) | done |
| `\pausing` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\postdisplaypenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\predisplaypenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\predisplaysize` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\pretolerance` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\prevdepth` | set_aux | `\spacefactor`/`\prevdepth`: fields of RState (the list being built), assigned and read there, never Keys; `\global` has no effect (as in TeX) | partial (see Conflicts) |
| `\prevgraf` | set_prev_graf | RState.prev_graf; after a paragraph it is a read of the paragraph's lines (see Conflicts) | partial |
| `\relpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\righthyphenmin` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\rightskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\scriptfont` | def_family | prefixed node defining `Key::FamFont(size, fam)` = `Val::Font`; as a font identifier, a read of it | done |
| `\scriptscriptfont` | def_family | prefixed node defining `Key::FamFont(size, fam)` = `Val::Font`; as a font identifier, a read of it | done |
| `\scriptspace` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\sfcode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\showboxbreadth` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\showboxdepth` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\skewchar` | assign_font_int | `Key::SkewChar(font key)`, always global | done |
| `\skip` | register | node defining `Key::Count/Dimen/Skip/Muskip(n)`; as a value, a read of it | done |
| `\skipdef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\spacefactor` | set_aux | `\spacefactor`/`\prevdepth`: fields of RState (the list being built), assigned and read there, never Keys; `\global` has no effect (as in TeX) | partial (see Conflicts) |
| `\spaceskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\splitmaxdepth` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\splittopskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\tabskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\textfont` | def_family | prefixed node defining `Key::FamFont(size, fam)` = `Val::Font`; as a font identifier, a read of it | done |
| `\thickmuskip` | assign_mu_glue | node defining `Key::GluePar(p)` (Val::Glue in mu) | done |
| `\thinmuskip` | assign_mu_glue | node defining `Key::GluePar(p)` (Val::Glue in mu) | done |
| `\time` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\toks` | toks_register | node defining `Key::Toks(n)` | done |
| `\toksdef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\tolerance` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\topskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\tracingcommands` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracinglostchars` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingmacros` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingonline` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingoutput` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingpages` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingparagraphs` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingrestores` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\tracingstats` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\uccode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\uchyph` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\vbadness` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\vfuzz` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\voffset` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\vsize` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\widowpenalty` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\xdef` | def | node defining `Key::Cs(name)` = `Meaning::Macro` (long/outer flags from RState); `\edef`/`\xdef` expand the body as steps of this node | done |
| `\xspaceskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\year` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |

### (iii) Material: nodes emitting items — 73

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\-` | discretionary | node emitting `Item::Disc` with the font's `\hyphenchar` as its pre-break list (tex.web 1117) | done |
| `\/` | ital_corr | node emitting the italic correction kern of the current list's last item (`last_item`: the one read of material, Conflict 13) | done |
| `\above` | above | node emitting `MItem::Above{thickness}` (tex.web 1181): the list's gatherer makes the numerator of the list so far; a second one in a list is "Ambiguous" (the node reads its list) | done |
| `\abovewithdelims` | above | as `\above`, with the two delimiters scanned (`\delcode`s read) | done |
| `\accent` | accent | node emitting the accent (boxed and shifted if need be) between kerns `(for accent)`, and the character after its assignments (§1123–1125; the remaining material) | done |
| `\atop` | above | node emitting `MItem::Above` with thickness 0 | done |
| `\atopwithdelims` | above | as `\atop`, with the two delimiters | done |
| `\char` | char_num | `\char n`: the main loop from that character (tex.web 1038) | done |
| `\cr` | car_ret | an entry's end, read with `align_state` 0 (`RState.al`): its v template goes in (a step of the reading node, §789); its `\endtemplate` node closes the entry's group, the next gathers the entry, and `fin_row`'s node the row (§791–§799); elsewhere `align_error` with TeX's log (§1127) | done |
| `\crcr` | car_ret | as `\cr`; ignored by `align_peek` and in a preamble's end (§785) | done |
| `\delimiter` | delim_num | in math, `\delimiter n`: a math character `n div "1000` (tex.web 1160); outside math, a `$` is missing | done |
| `\discretionary` | discretionary | three box-like groups (`BoxKind::Disc`): each `}` is a `Close` node, the packaging node keeps the part and opens the next; the last emits `Item::Disc` and the replacement (tex.web 1117–1121) | done |
| `\displaylimits` | limit_switch | node emitting `MItem::Limits(normal)`, applied to the list's last noad by its gatherer; the node reads its list to complain if that is not an operator (tex.web 1159) | done |
| `\displaystyle` | math_style | node emitting `MItem::Style(0)` | done |
| `\eqno` | eq_no | in a display, opens the equation number's formula (a math group, tex.web 1142); its packaging node keeps the box in the display's group record and closes the display (`\0displayend`) | done |
| `\halign` | halign | an `Open` node (the alignment's group; `to`/`spread`); the next reads the preamble once — a value in the group's entry (`AlignSpec`: templates, tabskip glue, loop) — and opens the entries' group; entries and rows are gathered by the nodes after their closes (unset items); `fin_align` (§800–§812) is the node after the alignment's close, a pure function of the rows and `\noalign` material its nodes emitted. In a paragraph it ends it (§1094); in `$$…$$` a display alignment in the display's math group (§1145, §1206: the list's depth the vertical list's before the display, rows shifted by `\displayindent`, kept in the group record as `MathKind::DisplayAlign` until `$$`; Conflict 26) | done |
| `\hfil` | hskip | node emitting glue (tex.web 1060) | done |
| `\hfill` | hskip | node emitting glue (tex.web 1060) | done |
| `\hfilneg` | hskip | node emitting glue (tex.web 1060) | done |
| `\hrule` | hrule | node emitting `Item::Rule` (and `Item::PrevDepth(ignore_depth)`); in horizontal mode `\par` goes in first | done |
| `\hskip` | hskip | node emitting glue (tex.web 1060); in vertical mode a paragraph begins first | done |
| `\hss` | hskip | node emitting glue (tex.web 1060) | done |
| `\ignorespaces` | ignore_spaces | node reading (with expansion) past spaces; the next token is the next node's command | done |
| `\indent` | start_par | `new_graf`: node emitting `\parskip` and `Item::BuildPage` (vertical) and the indent box (horizontal), `\everypar` rewrites the input | done |
| `\kern` | kern | node emitting an explicit kern | done |
| `\left` | left_right | node reading the delimiter, opening a math group and emitting the left noad into it (tex.web 1191) | done |
| `\leqno` | eq_no | as `\eqno`, on the left | done |
| `\limits` | limit_switch | node emitting `MItem::Limits(limits)` (as `\displaylimits`) | done |
| `\mathaccent` | math_accent | node emitting an accent noad whose nucleus `scan_math` gives (a character, or a subformula's group opened by the next node) | done |
| `\mathbin` | math_comp | node emitting a bin noad, nucleus from `scan_math` (tex.web 1158) | done |
| `\mathchar` | math_char_num | node emitting the noad of a 15-bit math code (`set_math_char`, tex.web 1155: class, family, `\fam` for a variable one) | done |
| `\mathchoice` | math_choice | four math groups in turn (`\0choicenext` between them, as a discretionary's parts); the last packaging node emits `MItem::Choice` | done |
| `\mathclose` | math_comp | node emitting a close noad, nucleus from `scan_math` | done |
| `\mathinner` | math_comp | node emitting an inner noad, nucleus from `scan_math` | done |
| `\mathop` | math_comp | node emitting an op noad, nucleus from `scan_math` | done |
| `\mathopen` | math_comp | node emitting an open noad, nucleus from `scan_math` | done |
| `\mathord` | math_comp | node emitting an ord noad, nucleus from `scan_math` | done |
| `\mathpunct` | math_comp | node emitting a punct noad, nucleus from `scan_math` | done |
| `\mathrel` | math_comp | node emitting a rel noad, nucleus from `scan_math` | done |
| `\mkern` | mkern | node emitting a kern of subtype `mu_glue` (made explicit by `mlist_to_hlist`) | done |
| `\mskip` | mskip | node emitting glue of subtype `mu_glue` | done |
| `\noalign` | no_align | after a `\cr` (`align_peek`): an `Open` node (its group), its material emitted into the alignment's list; its `}` a `Close` node; elsewhere "Misplaced \noalign" (§1129) | done |
| `\noboundary` | no_boundary | node affecting ligature boundaries | not yet |
| `\noindent` | start_par | `new_graf` without the indent box | done |
| `\nolimits` | limit_switch | node emitting `MItem::Limits(nolimits)` (as `\displaylimits`) | done |
| `\nonscript` | non_script | node emitting zero glue of subtype `cond_math_glue` (tex.web 1171) | done |
| `\omit` | omit | an entry's first token (`init_col`, §788): no u template, the v template `\endtemplate` alone (`RState.al.omit`); elsewhere "Misplaced \omit" | done |
| `\over` | above | node emitting `MItem::Above` with the default thickness (`default_code`, made `\fontdimen8` of family 3 by `mlist_to_hlist`) | done |
| `\overline` | math_comp | node emitting an over noad, nucleus from `scan_math` | done |
| `\overwithdelims` | above | as `\over`, with the two delimiters | done |
| `\par` | par_end | `Kind::Par` node: the paragraph gathered from the items its nodes emitted (`Para`: list + `LineParams`); outer level: lines a separate task, inside a box broken here (tex.web 1096) | done |
| `\penalty` | break_penalty | node emitting a penalty; in vertical mode also `Item::BuildPage` (tex.web 1103) | done |
| `\radical` | radical | node reading the 24-bit delimiter, emitting a radical noad whose nucleus `scan_math` gives | done |
| `\relax` | relax | node with no result | done |
| `\right` | left_right | `Close` node of the `\left` group: reads the delimiter inside the group; its packaging node gathers the list with the right noad and emits an inner noad | done |
| `\scriptscriptstyle` | math_style | node emitting `MItem::Style(6)` | done |
| `\scriptstyle` | math_style | node emitting `MItem::Style(4)` | done |
| `\setlanguage` | extension | node emitting `Item::Lang` and setting `clang` (horizontal mode only, tex.web 1377) | done |
| `\span` | tab_mark | in a preamble, expands the next token (§782); in an entry, ends it without closing it: the entry goes on in the next column (§791, span widths at `fin_align`, §798, §803) | done |
| `\textstyle` | math_style | node emitting `MItem::Style(2)` | done |
| `\underline` | math_comp | node emitting an under noad, nucleus from `scan_math` | done |
| `\unkern` | remove_item | node emitting `Item::Remove(1)`, applied by the list's gatherer (tex.web 1105) | done |
| `\unpenalty` | remove_item | node emitting `Item::Remove(0)`, applied by the list's gatherer | done |
| `\unskip` | remove_item | node emitting `Item::Remove(2)`, applied by the list's gatherer | done |
| `\valign` | valign | as `\halign`, rotated: entries vertical (`vpackage(natural, 0)`), rows `vpack`ed into the paragraph (§799, §804, §811); in vertical mode it begins a paragraph; in math a `$` is missing (§1046) | done |
| `\vcenter` | vcenter | a box group (`BoxKind::Vcenter`); its packaging node `vpack`s the list (with the box report) and emits a vcenter noad (tex.web 1167–1168) | done |
| `\vfil` | vskip | node emitting glue (vertical mode) | done |
| `\vfill` | vskip | node emitting glue (vertical mode) | done |
| `\vfilneg` | vskip | node emitting glue (vertical mode) | done |
| `\vrule` | vrule | node emitting `Item::Rule`; in vertical mode a paragraph begins first | done |
| `\vskip` | vskip | node emitting glue (vertical mode); in horizontal mode `\par` goes in first | done |
| `\vss` | vskip | node emitting glue (vertical mode) | done |
| `\␣` | ex_space | node emitting interword glue (`\spaceskip` or the font's, tex.web 1041) | done |

### (iv) Layout and output — 43

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\badness` | last_item | a read of the last box packing's badness (the packager's history: Conflict 20) | approx: 0 |
| `\box` | make_box | reads `Key::Box(n)`; `box_end` (append with interline glue, `\setbox`, `\shipout`); voids the register at the scope of the definition it replaces (Conflict 7) | done (voiding: Conflict 7) |
| `\cleaders` | leader_ship | `scan_box` (a box or a rule), then the glue after it: `Item::Leaders` (the remaining material) | done |
| `\copy` | make_box | reads `Key::Box(n)`; `box_end` | done |
| `\deadcycles` | set_page_int | `Key::Page(8)`, `Key::Page(9)`: read; assigned at the level of the definition replaced (§1246); an output routine's run defines them, `\shipout` makes `\deadcycles` 0 | approx: outside a routine, 0 until the fold defines them |
| `\dp` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\dump` | stop | `\end`, and the log's "Beginning to dump" lines: the format is the set of definitions reaching this point (no file) | done (the log up to the format's name) |
| `\end` | stop | `its_all_over`: node emitting `Item::End` (the page fold appends `\hbox to\hsize{}\vfill\penalty-2^30` and builds pages until empty, tex.web 1054); `final_cleanup`'s lines after the main file's ` )` (§1335); reading stops | done (later pieces run but are left out: Conflict 21, IO) |
| `\hbox` | make_box | a `Kind::Open` node opening a box group (`BoxGroup`: kind, context, `to`/`spread`, the enclosing list's mode, space factor, prev depth); its `}` is a `Close` node; the packaging node (`Kind::Par`) gathers the items emitted inside and packs them (hpack, tex.web 649) with `\hbadness`/`\hfuzz`/`\overfullrule` read after the group, diagnostics into the log | done |
| `\ht` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\insert` | insert | a group whose packaging node emits `Item::Ins` (with `\splittopskip`, `\splitmaxdepth`, `\floatingpenalty` read inside it); moved out of lines and adjusted hboxes (§655) | partial (the item; the page fold's handling is A/B's) |
| `\insertpenalties` | set_page_int | `Key::Page(8)`, `Key::Page(9)`: read; assigned at the level of the definition replaced (§1246); an output routine's run defines them, `\shipout` makes `\deadcycles` 0 | approx: outside a routine, 0 until the fold defines them |
| `\lastbox` | make_box | the current list's last box (Conflict 13's read of material) is the node's value, and an `Item::Remove(3)` its gatherer applies | done |
| `\lastkern` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\lastpenalty` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\lastskip` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\leaders` | leader_ship | `scan_box` (a box or a rule), then the glue after it: `Item::Leaders` (the remaining material) | done |
| `\lower` | vmove | `scan_box` with a shift (horizontal mode) | done |
| `\mark` | mark | node emitting `Item::Mark` (expanded text); marks move out of an `\hbox` in vertical mode and out of lines; `\topmark`... not yet | done (no mark classes) |
| `\moveleft` | hmove | `scan_box` with a shift (vertical mode) | done |
| `\moveright` | hmove | `scan_box` with a shift (vertical mode) | done |
| `\pagedepth` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pagefilllstretch` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pagefillstretch` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pagefilstretch` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pagegoal` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pageshrink` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pagestretch` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\pagetotal` | set_page_dimen | `Key::Page(0..=7)`: read; assigned at the level of the definition replaced (§1245); an output routine's run defines them inside its group | approx: outside a routine, an empty page's |
| `\raise` | vmove | `scan_box` with a shift (horizontal mode) | done |
| `\setbox` | set_box | prefixed node: `scan_box` with context `Set(n, global)`; the packaging node defines `Key::Box(n)` = `Val::Box` | done |
| `\shipout` | leader_ship | `scan_box`; `box_end` emits `Item::Shipout` (in the outer list's order, through paragraphs and boxes) with `\count0`–`9`, offsets, `\mag`, date read there; its `[n]` in the node's log | done |
| `\special` | extension | whatsit item holding the expanded text; `xxx` in the DVI file where shipping meets it (IO) | done |
| `\unhbox` | un_hbox | node emitting a box register's list (reads `Key::Box(n)`, voids it: Conflict 7) | done |
| `\unhcopy` | un_hbox | node emitting a box register's list | done |
| `\unvbox` | un_vbox | as `\unhbox`, vertical | done |
| `\unvcopy` | un_vbox | as `\unhbox`, vertical | done |
| `\vadjust` | vadjust | a group whose packaging node emits `Item::Adjust`, its material moved after the line or adjusted hbox holding it (§655) | done |
| `\vbox` | make_box | as `\hbox`, vpack (tex.web 668) with `\boxmaxdepth` read inside the group; a paragraph open at its `}` ends first | done |
| `\vsplit` | make_box | a node: `split::vsplit` of the register's box value (pure), the rest left in the register (Conflict 7), `Key::SplitMarks` defined | done |
| `\vtop` | make_box | as `\vbox`, height and depth readjusted (tex.web 1087) | done |
| `\wd` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\xleaders` | leader_ship | `scan_box` (a box or a rule), then the glue after it: `Item::Leaders` (the remaining material) | done |

### (v) IO and diagnostics — 18

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\batchmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\closein` | in_stream | a node defining `Key::ReadStream(n)` closed (global) | done |
| `\closeout` | extension | a whatsit (or with `\immediate` an event in the node's log), done when its page is shipped: the log's gatherer replays the streams (IO) | done |
| `\errmessage` | message | node whose result is a string (`Node.log`), the expanded text shown; `\errmessage` an error | done |
| `\errorstopmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\immediate` | extension | prefix for `\write`/`\openout`/`\closeout`: the extension's event is this node's (IO) | done |
| `\inputlineno` | last_item | a read of `Key::LineBase` (the lines before the piece, answered by the `Env`) plus the line within the piece; in a file read in the piece, its line there (IO) | done |
| `\message` | message | node whose result is a string (`Node.log`), the expanded text shown; `\errmessage` an error | done |
| `\nonstopmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\openin` | in_stream | a node reading `Key::File` and defining `Key::ReadStream(n)` = `Val::Stream` (global) | done |
| `\openout` | extension | a whatsit (or with `\immediate` an event in the node's log), done when its page is shipped (IO) | done |
| `\read` | read_to_cs | a prefixed node: reads `Key::ReadStream(n)`, tokenizes its next line(s), defines `\x` and the stream advanced (IO) | done |
| `\scrollmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\show` | xray | diagnostic: `> \x=meaning.` in the log, an error context | not yet |
| `\showbox` | xray | node whose result is log text (`> \boxN=`, the box, "OK" and its context; line numbers as placeholders, Conflict 16) | done |
| `\showlists` | xray | node reading the open lists (the one read of material): log text as `show_activities`, the current page not shown (Conflict 16) | partial: the current page |
| `\showthe` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\write` | extension | with `\immediate`: an event in the node's log, its text expanded in no mode; deferred: a whatsit whose text is expanded where its page is shipped (IO) | done |

## pdfTeX's PDF mode (agent Q)

`\pdfoutput>0` at a page's ship: the page's value is its content
stream (`pdf/content.rs`, made in the ship task where the DVI body is
made), and the PDF file is a fold over the pages in document order
(`pdf/fold.rs`): object numbering, object streams, fonts at the end
(map file, encodings, Type 1 subsetting copied from partex's writet1),
page tree, catalog, `/Info`, cross-reference table or stream. Streams
are compressed by a copy of zlib's deflate (`pdf/zlib.rs`, the
library pdfTeX links: equal to the system zlib's output). The date is a
given of the build (`Build::set_pdf_date`, `SOURCE_DATE_EPOCH`, 0 until
set). Checked by `tests/pdf.rs` against `pdftex -ini -etex`.

Found and fixed in `prim.rs` (B's table, two entries): `\pdfomitprocset`
and `\pdfomitinfodict` (pdfTeX 1.40.29's integer parameters 34, 35) were
missing (154 primitives now), and IniTeX's `\pdfhorigin`/`\pdfvorigin`
are 4736287sp (1truein as pdfTeX rounds it), not 4736286.

**State (Q1).** `tests/pdf.rs`: eight `-ini` documents (one font
uncompressed and compressed, `\mag`, object streams and a two-level
page tree over 42 pages, rules and leaders and four fonts, fonts mixed
in a line, a page made again) and plain TeX's ten documents of
`tests/plain.rs` (story, pages, display, table, hyphens, each with
plain's `\output` and TeX's default) are byte-identical with
`pdftex -ini -etex` at six schedules (1 thread, 8, 8 speculating, three
random orders).

**State (Q2).** `\pdfglyphtounicode` is a global entry
(`Key::GlyphUnicode(name)`: a code or a string of hexadecimal digits;
the empty name marks that the table exists), kept by the format as
pdfTeX dumps its table; with `\pdfgentounicode` positive at the end the
fold writes each font's ToUnicode CMap before its dictionary
(`pdf/font/unicode.rs`, tounicode.c), reading the entries reaching the
end of the job. LaTeX's `latex.ltx` loads `glyphtounicode.tex` and sets
`\pdfgentounicode=1`, so every pdflatex document has the maps.
`tests/pdflatex_docs.rs`: the ten documents of `tests/latex_docs.rs`
on the saved format of `pdflatex.ini`'s job are byte-identical with
`pdflatex` (first run, `SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1`,
batch mode; the `.aux` read-back gap of "LaTeX documents" stands) at
five schedules: the PDF file and the files written. Found on the way: a
page made where it is shipped numbers the fonts loaded after the pages
before it by its own guess (as the DVI writer does, `extra`); the fold
now checks every font's number against the job's and makes the page
again when one differs (as the DVI join does).

**pdfTeX work split** (modules another agent can take in parallel; each
is a pure function or a fold step, no state shared with the others):

| part | where | interface |
|---|---|---|
| numbers, zlib, MD5 | `pdf/num.rs`, `pdf/zlib.rs`, `pdf/md5.rs` | `num::Out` (pdfTeX's printing), `zlib::compress(data, level)`, `md5::md5` — done |
| a page's content stream | `pdf/content.rs` | `content::page(&Page, &Ship, &Numbers) -> PageBody`: the stream, `fonts` (font, number, glyphs), `events` (what makes objects, in stream order), guesses (`ff`, `fixed`). New whatsits (`\pdfliteral`, `\pdfcolorstack`, `\pdfsetmatrix`/`\pdfsave`/`\pdfrestore`, `\pdfrefxform`, `\pdfrefximage`, links, annotations, dests) are items in the page's list (`Item::Whatsit` variants made by their nodes) that the walk turns into stream text and `Event`s; position-dependent results (`\pdfsavepos`'s x, y; a link's rectangles split at line breaks) are made here, in the page's value |
| the fold | `pdf/fold.rs` | `fold::assemble(pages, bodies, numbers, &End, &mut dyn Files) -> Written`; each `Event` gets a branch in `F::ship` that numbers its object as pdfTeX's `pdf_create_obj` does and writes it where pdfTeX does (`pdf_ship_out`'s "Write out resource lists", "Write out pending PDF marks", "Finish the PDF file"); document-level items (`\pdfobj`, `\pdfoutline`, `\pdfdest` names, `\pdfcatalog`, `\pdfinfo`, `\pdfnames`, `\pdftrailer`) arrive in position order as items of the material and are fold steps |
| fonts | `pdf/font/` (`map.rs`, `enc.rs`, `t1.rs`), the font part of `fold.rs` | the map file (`map::Map`), encodings, Type 1 subsetting (`t1::writet1`), `fold::F::{do_pdf_font, write_fontdescriptor, write_enc, write_fontdictionary}`. To come: ToUnicode (`\pdfgentounicode`, `\pdfglyphtounicode`'s table as a document-level value), virtual fonts (`do_vf_packet` in the content walk), PK fonts (Type 3), TrueType/OpenType, `\pdfmapfile`/`\pdfmapline` (document-level items the fold applies before the first font is initialized), `\pdffontattr` |
| images | `pdf/image/` (new) | `\pdfximage` is a node whose value is the image's description (a pure function of the file's bytes, the page asked for, the box); its dimensions are known at the node (pdfTeX reads the file there); `\pdfrefximage` is a whatsit; the fold writes the XObject (PNG through `zlib`, JPEG as it is, PDF pages through a PDF reader copied from partex-engine's `pdfread.rs`/partex-core's `epdf.rs`) at the first page using it |
| links, dests, outlines | `pdf/link.rs` (new) + fold | link and annotation rectangles in `content.rs` (pdfTeX's `append_link`, `end_link`: per line box at its nesting level), dest names sorted and the name tree in the fold (`sort_dest_names`, "Output name tree"), outlines as document-level items in position order |
| reads of the fold | core (A: Conflict 12's `ReadRef`) | `\pdflastobj`, `\pdflastxform`, `\pdflastximage`, `\pdflastannot`, `\pdflastlink`, `\pdflastxpos`/`\pdflastypos`: the fold's counters at the node's position |

## Font expansion and character protrusion (agent B)

pdfTeX's `\pdfadjustspacing`, `\pdfprotrudechars`, their codes and
fonts, copied from pdftex.web as pure functions (`src/micro.rs`) and
values:

- **Codes** (`\efcode`, `\lpcode`, `\rpcode`, `\knbscode`, `\stbscode`,
  `\shbscode`, `\knbccode`, `\knaccode`: `Mat::FontInt` with pdfTeX's
  code numbers, `micro::LP`…): one value per font and code,
  `Key::FontCode(code, font)`, the font's 256-entry table
  (`Val::Penalties`; empty: the defaults, `\efcode` 1000, the others 0),
  global as pdfTeX keeps them beside the font; an assignment is a node
  reading the table and defining the new one (clamped as `set_*_code`).
  A table per font, not a key per character: a paragraph's `\par` node
  reads three keys a font, not three a character, and a document sets
  them once, at a font's first use (microtype's `\MT@setupfont`).
  `\pdfnoligatures f` is `Key::FontCode(6, f)` = 1, read by the main loop
  (its lig/kern program is not looked at; the left boundary's still is,
  as `set_no_ligatures` clears only the characters' tags).
- **`\pdffontexpand f stretch shrink step [autoexpand]`**:
  `Key::FontExpand(f)` = `micro::Expand` (`read_expand_font`'s checks;
  given twice, the same parameters or pdfTeX's fatal error). The fonts
  expanded by the limits are loaded there (`Key::FontLoaded`: numbered
  as pdfTeX's `get_expand_font` numbers them); the others are numbered
  where a page uses them (`dvi::number`'s `extra`).
- **An expanded font is a value**: `Font::expanded(base, e)`
  (`auto_expand_font`: widths, italic corrections, kerns scaled by
  `round_xn_over_d(x, 1000+e, 1000)`; named `name+e`; identity `base.key`
  and `e`), `Font::derived = Derived { base, kind: Expanded(e) }`. Made
  by the line breaker where `hpack(…, subst_ex_font)` substitutes a
  character's font (`do_subst_font`), once per paragraph (`micro::Cache`).
- **The `\par` node reads them** only with `\pdfadjustspacing` or
  `\pdfprotrudechars` positive (`LineParams::micro`: `None` otherwise, so
  DVI documents without them read nothing more): for the paragraph's
  fonts (its characters', its discretionaries', its hboxes'), the
  expansion parameters, `\efcode`, `\lpcode`, `\rpcode` tables and the
  quad (`\fontdimen6` now); `check_expand_pars`' one step and one pair
  of limits a paragraph is checked there.
- **Line breaking** (`linebreak.rs`): the widths are eight (the font
  stretch and shrink, `cur_active_width[7..8]`, with
  `\pdfadjustspacing>1`: `micro::Micro::widths_at`, a character's
  `char_stretch`/`char_shrink`, a font kern's `kern_stretch`/
  `kern_shrink` between two characters of its font); `try_break`'s
  shortfall gets `total_pw` (`\pdfprotrudechars>1`) and the font
  stretch/shrink adjustment (`\pdfadjustspacing>1`), as pdftex.web
  orders them. `post_line_break` puts the margin kerns where pdfTeX does
  (`\pdfprotrudechars>0`: the right one after the item before the break,
  the protruding character found back from it with `find_protchar_right`,
  or a discretionary's pre-break list's last; the left one first) and
  packs the line with `micro::Micro::hpack_line` (`\pdfadjustspacing>0`:
  `cal_expand_ratio`, then `subst_ex_font` with the ratio, kerns and
  margin kerns redone as pdfTeX redoes them).
- **The main loop**: `\pdfprependkern`/`\pdfappendkern` kerns
  (`get_auto_kern`: before the first character — no left boundary then —,
  between two characters instead of their lig/kern program, before a
  ligature, around `\-`'s hyphen), `Item::Kern { sub: 3 }` (shown
  `(for \pdfprependkern/\pdfappendkern)`); `\pdfadjustinterwordglue`:
  every space is the normal space, widened by the character before's
  `\knbscode`… (`adjust_interword_glue`; it reads the current list's
  tail, so a space node with it positive runs again with its paragraph).
- **`\pdfcopyfont\cs=f`**: `Font::copied` (a font of its own: key
  `f.key copyN`, parameters as `f`'s are now, its own codes);
  **`\letterspacefont\cs=f n [nolig]`**: `Font::letterspaced` (widths
  plus `n` thousandths of the quad, named `name+nls`); both counted per
  base font (`Key::FontCopies`) for their identity, loaded (numbered,
  `font_ptr`) as `\font` loads a font. `\pdffontattr f {…}`:
  `Key::FontAttr(f)`, PDF mode only (pdfTeX's fatal error in DVI mode).
- `\leftmarginkern n`, `\rightmarginkern n` (`Conv`): the box's margin
  kern past what protrusion skips and the skip glue. `\unhbox` drops
  margin kerns (pdftex.web 1110).

**For Q (the PDF content stream).** Two things reach a page:

1. `Item::MarginKern { w, right, font, c }`: everywhere a kern of width
   `w` (`cur_h += w`, as `hlist_out`'s `margin_kern_node, kern_node`
   case); `font`/`c` are the protruding character (`margin_char`), for
   nothing in the stream. DVI and `pdf/content.rs` move right by `w`
   (the one line changed in `content.rs`).
2. `Item::Char(f, c)` / `Item::Lig` whose `f.expanded_from()` is
   `Some((base, e))`: set the base font's glyph `c` with the text matrix
   scaled horizontally by `1000+e` thousandths (pdftex.web's
   `pdf_new_Tm_a`/`get_font_auto_expand_ratio`, the `Tm` operator when
   `e` changes: pdftex.web 16285–16300); the PDF font object is the
   base's (`pdf_font_blink`), never one for `name+e`. The advance is the
   expanded width (`f.glyph(c).width`). A letter-spaced font
   (`DerivedKind::LetterSpaced { e, .. }`) is a virtual font over
   `base`: each character is the base's, moved right by
   `round_xn_over_d(quad, e, 2000)` before and after (pdftex.web's
   `letter_space_font` packets, `vf_replace_z`'s fix-word rounding).

**Held by**: `tests/etex.rs`' `micro` (PDF mode, no page shipped: the
codes and their `\the`, copies and letter-spaced fonts, `\pdfnoligatures`,
protrusion at `\pdfprotrudechars` 1 and 2, `\leftmarginkern`/
`\rightmarginkern`, expansion at `\pdfadjustspacing` 1 and 2 and with
protrusion, an `\hbox to`, the auto kerns, `\pdfadjustinterwordglue`;
`\showbox`'s lines against `etex -ini -etex`, i.e. pdfTeX) at five
schedules; `tests/latex_docs.rs`' `micro` (microtype in DVI mode:
protrusion; DVI and `.aux` against `latex`, first run and fixpoint).
`tests/pdflatex/docs/micro.tex` (expansion and protrusion, lmodern) and
`microls.tex` (`\textls`, tracking, spacing, kerning) are for the
pdflatex harness (`tests/pdflatex_docs.rs`, Q), not in the tree yet:
their PDF needs (2) above.

Gaps:
- *Expanded TFM files.* A font given `\pdffontexpand` without
  `autoexpand` names `name+e.tfm` files pdfTeX loads (`load_expand_font`
  → `read_font_info`); here every font is expanded as `autoexpand` makes
  it. microtype always gives `autoexpand` with pdfTeX.
- *`\tagcode` and `\pdfnoligatures` outside the main loop.* Both clear
  tags (`Key::FontCode(5, f)`, the bits cleared per character; read by
  the main loop and `\the`); the hyphenation pass's reconstitution and
  math (lig/kern, charlists, extensible recipes) look at the font as
  loaded.
- *The margin kerns' variations* in `try_break` (`cal_margin_kern_var`)
  are taken as zero: an autoexpanded font's protrusion is its base's
  (same quad, same codes), so pdfTeX's are zero too.
- *A derived font in a saved format*: `format::decode` refuses it (the
  font is written as its file's bytes).
- *`\pdfmatch`, `\pdflastmatch`* (`src/ere.rs`, POSIX extended
  expressions, backtracking; the last match `Key::LastMatch`, global):
  the whole match is leftmost-longest as POSIX's; a group's submatch is
  the first path's to it, not POSIX's subexpression rules. A bad
  expression gives -1 without pdfTeX's warning in the log.
- *An error's context at a macro's end (for P).* `\def\m{\setbox0\hbox{}\showbox0 }
  \m`: TeX's `! OK.` context shows the macro's level (`\m ...\showbox
  0 `) above `l.N \m`; the graph shows `...` there: the macro's level,
  read to its end, is gone before the error is shown (TeX pops it at the
  next `get_next`). Found by `tests/etex/micro.tex`, which now puts
  `\showbox0` after its macro.
- *`\partokenname` in the reader (for P).* `Key::ParToken` (global) is
  the paragraph token where material.rs, align.rs and output.rs insert
  one (`\vskip`-like commands in horizontal mode; with
  `\partokencontext` 1 or 2 the box, `\insert`, `\vadjust`, `\output`,
  `\noalign` and `\valign` entry ends: partoken.ch). Still `\par`: the
  blank line's token (graph.rs `get_next`, `TState::N`), the runaway
  checks of a non-`\long` macro's argument (`is_par`, and the token a
  runaway `use` backs up). Those are the reader's, and want the token in
  the reader state, defined where `\partokenname` runs.
- *`\pdfinsertht n` for classes 0–9.* `Key::Page(n)` is the page builder's
  quantities for 0–9, so those classes read 0pt; 10–254 are the page's
  insertion records' heights (`Page::page_quantity`, validated as the
  other page quantities).
- *`\pdfprimitive` where a value is scanned* (pdftex.web's "Reset
  `cur_tok` for unexpandable primitives" in `scan_something_internal`):
  not done; as a command and an expansion step it is.
- *A box's glue when the expansion ratio rounds to zero*: pdfTeX returns
  the first pass's box without setting its glue (uninitialized fields);
  here its glue is normal.

## The cross-run fixpoint (agent B)

AGENTS_BRIEF §7. Run k+1 is the same build updated again. The files run
k wrote are given to it as a new version of those files
(`Build::set_file_bytes`; files run k no longer writes are removed): an
edit of run k. Nothing is copied into a state. It stops when a run
writes byte for byte what the run before wrote, or after five runs
(oscillation: `Runs::stable` false, `changed` names the files).

API (`src/fixpoint.rs`): `Fixpoint::run(&mut build, src, opts) -> Run
{ out, changed, stable }` is one run. The CLI runs bibtex/makeindex
between runs and gives their output with `set_file`; a changed `.bbl`
means one more run whatever `stable` says. `fixpoint(b, src, opts, max)`
is the plain loop. Dev binary: `phitex-flow --runs N` prints each run's
changed files.

Oracle (`tests/latex_docs.rs`, `fix_*`): `latex` run until what it
writes is stable (`NAME.fix.dvi`, `.fix.writes`, `.fix.runs`). The last
run's DVI and written files must be latex's at five schedules, with
latex's number of runs (format gap 1, the same-run read-back of the
`.aux`, is closed: "Reads of folds").

Run 2 against run 1, commands run at one thread, on 9fa4ccf (job 7796):

| document | run 1 | run 2 | files run 1 changed |
|---|---|---|---|
| sections | 35684 | 8741 | sections.aux |
| math | 21786 | 4553 | math.aux |
| tables | 21590 | 4365 | tables.aux |
| ams | 70064 | 13584 | ams.aux |
| gfx | 79195 | 4305 | gfx.aux |
| layout | 77985 | 10209 | layout.aux |
| packages | 30361 | 5358 | packages.aux |
| ltmc | 31599 | 11077 | ltmc.aux |
| lst | 62491 | 21588 | lst.aux |
| bib | 22762 | 5056 | bib.idx, bib.aux |
| href | 222359 | 9372 | href.aux, href.out |
| micro (job 7824) | 150555 | 30644 | micro.aux |

Every document converges in latex's two runs. Before 9fa4ccf (A's F5:
pieces mark their readers by position), run 2 re-ran every piece
(sections: 21155 commands against 20942 for the whole document). Now it
re-runs 5–25% of run 1. What remains is the pieces reading the `.aux`
at `\begin{document}` and the nodes after a changed read. F's
over-invalidation dump was not needed: no piece re-runs wholesale.

**Hang (W2, A's; fixed in 87a1d3d, the tests run again).** Run 2 of
`titlepage`, `figures` and `rules` did not finish: 7796 made no progress for an hour on their first schedule
(threads 1). W sees the same on titlepage (W2). Smallest repro:
`tests/latex/docs/titlepage.tex` (466 bytes) on the saved format. Run 1
as `fix_at` runs it, `set_file_bytes` of run 1's `titlepage.aux` and
`titlepage.toc`, then `update` again: it never returned. The
`fix_*` tests give a fixpoint 600 s (`FIX_TIMEOUT`, in a thread of its
own) and fail instead of hanging.

**The CLI** (`phitex-cli` `Session::fixpoint`) runs each run as
`Fixpoint::run_reading`: a written file not given by a run before is
compared with what the run read as it (the file on disk from an earlier
job), so a first run whose `.aux` is the one on disk is stable; BibTeX
and makeindex run between runs and a changed `.bbl` or `.ind` is one
more run.
