# CORE: the command set of TeX on the graph

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
    stale value is possible: a gap). Gaps: on the main vertical list after a
    paragraph's lines, or at a piece's start, the last item is another
    task's (lines) or another piece's — read as none; TeX's `\lastpenalty`
    of a penalty the page builder has just set to 10000 is not seen.
    `\lastbox` (a removal whose value the node needs): done, the same read
    and an `Item::Remove(3)`.
14. **Groups and conditionals across a piece boundary** (a blank line inside
    `\begingroup…\endgroup` or an `\if…\fi`; `\obeylines` or a changed
    `\endlinechar`, where a blank line is not a `\par`): the cut assumes the
    start state at every blank line. Proposal (brief §6.3): a piece whose
    predecessor's `End` state is not the start state is re-cut — it runs on
    from that state. Now: "Missing } inserted at the end of a piece" /
    "Incomplete \if".
15. **`\mag` and `true` units** — TeX freezes `\mag` at its first use
    (`mag_set`, a hidden global). Proposal: `Key::MagSet`, defined globally
    by the first node using it. Now: `\mag` is read, not frozen.
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
- Two paths (done). An `\input` that is a command of its own at a piece's
  top level (vertical mode, no group, box or paragraph open, no text
  rewritten) ends its node's run and its file's pieces join the sequence
  (`Build::layout`, entry edges, `\endinput` dropping the later pieces):
  they run in parallel like the main file's. Anywhere else (inside a
  group, a box, a paragraph, a macro's expansion, another `\input` read
  in the piece) the file is read in the piece as a text on the
  `\scantokens` stack (`TextKind::File`, `Exec::start_input`). Files are
  `Key::File(name)` = `Val::Bytes` answered by the build (`Given`: the
  files given in memory with `Build::set_file`, then the input
  directories); `Key::LineBase` is the lines of its own file before a
  piece of the sequence.

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

**Stopgaps in the harness:**
- *Waiting on A's `Key::Ended`:* pass 1's text is cut after its `\next`
  (= `\dump`) line, in `tests/trip.rs` and in `phitex-flow --trip`. The
  core still runs the pieces after an `\end`/`\dump`. In IniTeX's state
  they loop forever at line 206 (`\everypar`'s `\gobble` eats the `$`
  that is inserted).
- *Waiting on A's same-run write/read:* `tripos.tex`, written by
  `\write10` and read back in the same run, goes through the session's
  fixpoint over written files (`Session::run`, up to 3 runs) (B's gap 1 below,
  the same core gap).

**Gaps for A:**
1. **The fold ignores a document's `\pagegoal=` and the other page
   dimensions** (tex.web 1245: `page_so_far[k]` is assigned at once, at
   the command, against the page as the last `build_page` left it). This
   is the last thing between TRIP and Knuth's `trip.typ`:
   - Pages 10 and 15 are packed to 16383.99998pt, not 10000pt.
     `\showlonglists` sets `\pagegoal=10000pt`.
   - Repro, against `tex -ini`:

         \catcode`\{=1 \catcode`\}=2
         \vsize=100pt \hsize=50pt \maxdepth=2pt \output={\showbox255 \shipout\box255}
         \hrule height 5pt\penalty0
         \pagegoal=50pt
         \vfil\penalty-10000
         \end

     TeX gives `\vbox(50.0+0.0)x0.0, glue set 45.0fil`; we give 100pt.
   - Without the `\penalty0` the page is still empty at the assignment.
     TeX then sets the goal back to `\vsize` when the first box arrives
     (§987).
   - Proposal: the assignment emits an item that the fold applies to its
     page state on receipt, not queued behind the contributions (like
     `Item::PrevDepth`).
2. **The pieces after `\end`/`\dump` still run** (`Key::Ended`, above).

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

**Approximations left** (not met by TRIP's DVI):
- `\endtemplate` is not yet an `\outer` name for `check_outer`.
- `prepare_mag` runs at a `\shipout` only while no magnification is
  fixed. TeX runs it at the first page only.
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
2. *The cut is blind to catcodes.* `latex.ltx` line 160:
   `\catcode`\%=12 \def\@percentchar{%}\catcode`\%=14`; the cut's brace
   count reads `%` as a comment, stays at depth 1 to the end of the file,
   and `latex.ltx` is one piece (94 blank lines). Repro: those three
   lines, then two paragraphs with a blank line between: one piece.
3. *`\input`'ed files run inside the calling piece* where `\input` is
   not a command of its own at a piece's top level: in `latex.ltx` every
   `\input` is inside a macro or a group (`\@input`, `\InputIfFileExists`,
   expl3.ltx's), so expl3-code.tex's 36k lines, the font definitions and
   the patterns are read by the one piece, and the build is sequential
   (one thread did 100%).
4. *The main memory counts what was ever defined.* `graph::words_of`
   sums every node's definitions (a macro's body, a box, a token list)
   over the run, kept or not; TeX's `mem` holds only what is live (a
   macro defined again frees the old body). LaTeX's format passes 5M
   words this way (`tests/latex.rs` runs with `main_memory` 2^32). Repro:
   `\count1=0 \def\l{\def\a{<1000 x>}\advance\count1 1 \ifnum\count1<10000
   \expandafter\l\fi}\l \message{\the\count1}\end`: TeX 10000, the graph
   "TeX capacity exceeded, sorry [main memory size=5000000]".

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
