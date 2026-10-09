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
| (ii) assignments, scopes | 151 | 146 | 3 | 2 |
| (iii) material | 73 | 28 | 0 | 45 |
| (iv) layout and output | 43 | 31 | 12 | 0 |
| (v) IO and diagnostics | 18 | 14 | 0 | 4 |
| all | 325 | 256 | 18 | 51 |

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
| `cond_ptr`, `if_limit` | `RState.conds`: per open conditional, the node that opened it, which ends may come, its test |
| `\global`/`\long`/`\outer` prefixes | `RState.global/long/outer`, set by the prefix node, consumed by the assignment node |
| `after_token` (`\afterassignment`) | `RState.after_assignment` |
| mode, `space_factor`, `prev_depth`, `prev_graf` | `RState.mode`, `RState.space_factor`, `RState.prev_depth`, `RState.prev_graf` |
| `end_line_char` as the line was read | `RState.eol` (read when a line is begun, as TeX's `input_ln` does) |
| the input stack, `back_input`, `ins_list` | `RState.pending` (rewritten input; `Tok::DontExpand` is `\noexpand`'s mark) |
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
   are a field of the position, never in a version.
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
   outside the group. Proposal (exact): a definition kind "at the level of
   the definition it replaces": the node defines `Key::Box(n)` with the
   scope of the reaching definition it read; `Chain::resolve` then treats
   it as made in that definition's group. Now (`Exec::change_box`): the
   definition is global when the replaced one was made at the outermost
   level (or globally), local when it was made in the current group —
   exact in both; for a definition made in an enclosing group between
   those it is local (approximation: the change is undone one group too
   early).
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
    the output routine's nodes (19). Now: an empty page's values, empty
    marks, `\outputpenalty` not defined.
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
16. **`\show`, `\showthe`, error contexts** — need the line-number field;
    not implemented. `\message` and `\immediate\write` are the conformance
    oracle meanwhile.
17. **The DVI file is a fold**: TeX writes a font's definition where it is
    first used, reuses `w`/`x`/`y`/`z` movements only within what its
    half-buffer (`dvi_buf_size` 16384) still holds (`dvi_gone`), points each
    `bop` at the page before, and writes the postamble's fonts last-loaded
    first. Every page's bytes depend on the pages before, so `dvi::assemble`
    writes the whole file in one sequential pass over the pages (cheap next
    to building them); `dvi::page_body` is empty. Proposal for incremental
    shipping: a page's bytes as a function of (page, the fonts defined
    before it, `dvi_gone` at its start) — equal inputs, equal bytes.
18. **`\output`** (proposal only, not implemented): the page fold, firing,
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
    (done, tex.web 1023).
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
23. **Hyphenation** (patterns, `\hyphenation`) is not implemented: line
    breaking's second pass does not hyphenate (exact for `-ini` documents
    without patterns). `\noboundary` is not yet; `\accent`, `\vadjust`,
    `\insert` (the item), leaders, `\special` and deferred `\write`s are
    done (IO, the remaining material).
24. Small departures, documented: `\if…`'s inserted `\relax` (a `\fi` met
    while the test is read) is the name `\relax`, not a frozen one; a
    macro's parameter shows as `#n` whatever its parameter character; a
    `^^` reduction is not reduced again.

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
| `\read n to \x` | a prefixed node (§1225, §482–486): reads `Key::ReadStream(m)`, tokenizes the next line with the codes and `\endlinechar` in force (the line is a source the reader enters above the pending input — `RState::files`, a `\read` line — and leaves at its end), more lines while braces are open, the rest of a line after an unmatched `}` dropped; defines `\x` (a macro without parameters) and the stream anew, advanced (a new value); at the file's end the stream is closed and the line is empty (a `\par`), with "File ended within \read" if braces were open; a non-control-sequence target is "Missing control sequence inserted" (and `\inaccessible` defined); stream 16 or a closed one is the terminal: an error (batch mode) |
| `\ifeof n` | a read of `Key::ReadStream(n)`: closed? |
| `write_open[j]`, `write_file[j]` | none while commands run: events, replayed by the log's gatherer (`logs::replay`) in document order |
| `\immediate\openout/\write/\closeout` | the node's log holds an event (`logs::io_event`: kind, stream, payload); a `\write`'s text is expanded in the node (§1370, no mode, `\newlinechar` a line break, unprintables `^^`) |
| `\openout`, `\write`, `\closeout`, `\special` | whatsit items (`Item::Whatsit`) in the current list, in any mode; a `\write` holds its tokens unexpanded, a `\special` its expanded text |
| `ship_out`'s `out_what` (§1374) | an explicit `\shipout`: its node does the box's whatsits (DVI order, none inside leaders) — its `\write`s expanded with the definitions reaching it — between its `[n` and `]`. A page the page builder ships: at the join, `graph::page_io` expands its `\write`s with the definitions reaching the node whose `BuildPage` (or `End`) shipped it (`logs::page_places`: the call's node from the log fragments' marks; a pure function of the box, the place and those definitions: a candidate for a task and memo) |
| the files written | `Output::writes`: (name, contents) per file, in the order first opened; `\openout` adds `.tex` to a name without extension (web2c's `\openout j = `name'.` log line is off for `tex`: texmf.cnf `log_openout.tex = f`); a `\write` to a closed stream, 16 or 17 is a log line |
| `\special` in the DVI file | `xxx1`/`xxx4` after `synch_h`, `synch_v` (§1368) |

**`\input` (decided design, TeX side complete; the core's part is A's).**
`\input` met at the top is a node of its own (`next_x_m(top)` returns it,
as a conditional): it scans the name (§526), reads `Key::File` (the
kpathsea rule above), logs ` (./name` (§537), and its **result is the
file's text in its out state**: `node.out.files` gains an entry
`Infile { text, pos: 0, name, endinput: false, under, tstate, eol }` —
`text` the file's lines (`graph::file_text`: web2c's `input_ln`, trailing
spaces dropped, each line ending in `\r`, an empty file one empty line),
`under` the input that was pending at the call (read after the file),
`tstate`/`eol` the caller's tokenizer state, restored after it. Met while
another command scans, `\input` is a step of that node (the same entry).

*Now (single-piece emulation, exact TeX semantics):* the reader reads the
entry's text in the same piece, above the piece's text (`Exec::src`); at
its end it logs `)`, pops it and goes on with `under` and the rest of the
calling line. So the first file line and the rest of the calling line
carry the predecessor's state, as decided; nothing is cut.
*Interface for the core (A):* for a node `i` of piece `k` whose command
is `\input` (`Kind::Other`, `cmd` meaning `Prim::Input`) and whose
`out.files` has one more entry than its input, at `pos: 0`:
1. cut the entry's `text` with `graph::cut` into pieces F1..Fm, inserted
   after node `i` of piece `k` in the order;
2. F1 runs with entry state `node.out` *minus that entry*, with `pending`
   empty, `tstate: N`, `eol: EOL_UNREAD`; F2..Fm keep the constant start
   state (their cuts stay canonical);
3. the piece R holding the rest of the calling line runs from Fm's `End`
   state with `pending = under ++ Fm's pending`, `tstate`/`eol` restored,
   `pos` = `node.out.pos`, and its log begins with `)`;
4. `Key::LineBase` answered per piece is the piece's line offset *in its
   own file* (F1: 0).
With that, `RState::files` holds only `\read` lines and `\input`s met
inside another command's scanning (those stay in-piece: a node cannot
span pieces).
*`\endinput`* (§378): now the innermost file's `endinput` flag; the file
ends when the reader passes the current line's end. In pieces: proposal —
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
task).

## The remaining material (agent E)

| TeX | graph |
|---|---|
| `\leaders`, `\cleaders`, `\xleaders` (§1078) | `scan_box` with `BoxContext::Leaders(100..102)` (a box, or a rule: §1084); the glue after it (`\hskip`… in horizontal, `\vskip`… in vertical lists, else "Leaders not followed by proper glue") becomes `Item::Leaders{g, sub, b, zero}`: glue everywhere glue is (`Item::glue`, `Item::is_glue`: line breaking, the page builder, `\unskip`, `\lastskip`, `vert_break`, `prune_page_top`, §816's last glue), plus the box's height and depth in `hpack` (§656) and its width in `vpack` (§671); shipped as tex.web §626–628, §635–637 (aligned, centred, spread; rule leaders); shown as §190 |
| `\accent` (§1123–1125) | a node: the accent character; `do_assignments` (§1270: prefixed assignments in this node, `\setbox` refused); the next character in the font then current; kerns `(for accent)` by both fonts' slants (`\fontdimen1`) and the accent font's x-height (`\fontdimen5`, read as Keys), the accent boxed and shifted when the heights differ |
| `\lastbox` (§1080–1081) | the one read of material (Conflict 13): the current list's last box, unless a discretionary replaces it, is the node's value (unshifted) and an `Item::Remove(3)` its gatherer applies; on the main vertical list TeX's list is the contributions since the last page-builder call (none: "You can't use `\lastbox' in vertical mode") |
| `\vsplit n to h` (§977–979) | `split::vsplit`, a pure function of the box's list, `h`, `\splitmaxdepth`, `\splittopskip` (`vert_break` §970–976, `prune_page_top` §968); the part split off packed to `h` (reports with `\vbadness`, `\vfuzz`), the rest packed naturally into the register at the scope of the definition it replaces (Conflict 7); `Key::SplitMark(0/1)` (`\splitfirstmark`, `\splitbotmark`) defined globally by every `\vsplit`, read by those steps |
| `\vadjust`, `\insert n` (§1097–1100) | a group like a box's (`BoxKind::Insert(n, _)`, 255 for `\vadjust`), `normal_paragraph` inside; its `}` reads `\splittopskip`, `\splitmaxdepth`, `\floatingpenalty` inside the group; the packaging node emits `Item::Adjust(list)` / `Item::Ins{n, height+depth, depth, split_top, float_cost, list}` (reads nothing else: no register of the class); `hpack` with an adjust list moves `Ins`, `Mark` and `Adjust`'s material out (§655): after a paragraph's line (`linebreak`'s `Line.adjust`, no change there) and after an `\hbox` appended in vertical mode; on the main vertical list the page builder is called (§1100) |
| `\topmark`, `\firstmark`, `\botmark` | not wired: B's marks value is not on `flow`; they stay empty (approx) |

**Approximations (material).** The page fold does not handle insertions
(A/B: an `Item::Ins` on the main vertical list is contributed to the page
and ignored there; TeX moves it to `\box n` and its size into
`\pagegoal`); `\lastbox` on the main vertical list sees the items
emitted since the last page-builder call in its piece (TeX's
contributions: exact but for a kern the page builder left there, §1000).

## The inventory

### (i) Expandable and input-rewriting: steps — 40

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\botmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
| `\csname` | cs_name | step: builds the name, rewrites the input with it; an undefined name is defined `\relax` (local) by the node the step is in | done |
| `\else` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\endcsname` | end_cs_name | outside `\csname`: an error node | done |
| `\endinput` | input | step: the innermost `\input` file ends with its current line (`Infile.endinput`); in pieces the core drops its later pieces (IO) | done (in an `\input` file; in the main file an error) |
| `\expandafter` | expand_after | step: expands the token after next once, rewrites the input | done |
| `\fi` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\firstmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
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
| `\input` | input | at the top a node whose result is the file's text (`Key::File` read; its out state's new `RState::files` entry); inside scanning a step; read in the piece now, cut into pieces by the core (IO) | done (single-piece emulation) |
| `\jobname` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens; the name a read of `Key::Job` | done |
| `\lowercase` | case_shift | node (not expandable): reads lccode/uccode (`get`) of each character and rewrites the input with the changed text; defines nothing | done |
| `\meaning` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\noexpand` | no_expand | step: marks the next token not-to-expand once (`Tok::DontExpand` in the rewritten input) | done |
| `\number` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\or` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\romannumeral` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\splitbotmark` | top_bot_mark | step: rewrites the input with `Key::SplitMark(1)`, defined (globally) by each `\vsplit` | done |
| `\splitfirstmark` | top_bot_mark | step: rewrites the input with `Key::SplitMark(0)`, defined (globally) by each `\vsplit` | done |
| `\string` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\the` | the | step: reads the internal quantity (`get`) and rewrites the input with its tokens (unexpanded inside `\edef`/`\message`) | done |
| `\topmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
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
| `\hyphenation` | hyph_data | `\patterns` (IniTeX only) and `\hyphenation`: global definitions of a language's hyphenation tables, `Key::Patterns(lang)`/`Key::Exceptions(lang)`, read by line breaking | not yet |
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
| `\patterns` | hyph_data | `\patterns` (IniTeX only) and `\hyphenation`: global definitions of a language's hyphenation tables, `Key::Patterns(lang)`/`Key::Exceptions(lang)`, read by line breaking | not yet |
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
| `\above` | above | generalized fraction | not yet |
| `\abovewithdelims` | above | generalized fraction | not yet |
| `\accent` | accent | node emitting the accent (boxed and shifted if need be) between kerns `(for accent)`, and the character after its assignments (§1123–1125; the remaining material) | done |
| `\atop` | above | generalized fraction | not yet |
| `\atopwithdelims` | above | generalized fraction | not yet |
| `\char` | char_num | `\char n`: the main loop from that character (tex.web 1038) | done |
| `\cr` | car_ret | alignment row end | not yet |
| `\crcr` | car_ret | alignment row end | not yet |
| `\delimiter` | delim_num | math delimiter | not yet |
| `\discretionary` | discretionary | three box-like groups (`BoxKind::Disc`): each `}` is a `Close` node, the packaging node keeps the part and opens the next; the last emits `Item::Disc` and the replacement (tex.web 1117–1121) | done |
| `\displaylimits` | limit_switch | limits flag on the previous noad | not yet |
| `\displaystyle` | math_style | math style item | not yet |
| `\eqno` | eq_no | equation number | not yet |
| `\halign` | halign | alignment: nodes for the template and rows; the alignment is a node whose operands are the rows' material | not yet |
| `\hfil` | hskip | node emitting glue (tex.web 1060) | done |
| `\hfill` | hskip | node emitting glue (tex.web 1060) | done |
| `\hfilneg` | hskip | node emitting glue (tex.web 1060) | done |
| `\hrule` | hrule | node emitting `Item::Rule` (and `Item::PrevDepth(ignore_depth)`); in horizontal mode `\par` goes in first | done |
| `\hskip` | hskip | node emitting glue (tex.web 1060); in vertical mode a paragraph begins first | done |
| `\hss` | hskip | node emitting glue (tex.web 1060) | done |
| `\ignorespaces` | ignore_spaces | node reading (with expansion) past spaces; the next token is the next node's command | done |
| `\indent` | start_par | `new_graf`: node emitting `\parskip` and `Item::BuildPage` (vertical) and the indent box (horizontal), `\everypar` rewrites the input | done |
| `\kern` | kern | node emitting an explicit kern | done |
| `\left` | left_right | math delimiters (a group) | not yet |
| `\leqno` | eq_no | equation number | not yet |
| `\limits` | limit_switch | limits flag on the previous noad | not yet |
| `\mathaccent` | math_accent | math accent | not yet |
| `\mathbin` | math_comp | math noad | not yet |
| `\mathchar` | math_char_num | math character | not yet |
| `\mathchoice` | math_choice | math choice (four groups) | not yet |
| `\mathclose` | math_comp | math noad | not yet |
| `\mathinner` | math_comp | math noad | not yet |
| `\mathop` | math_comp | math noad | not yet |
| `\mathopen` | math_comp | math noad | not yet |
| `\mathord` | math_comp | math noad | not yet |
| `\mathpunct` | math_comp | math noad | not yet |
| `\mathrel` | math_comp | math noad | not yet |
| `\mkern` | mkern | node emitting a math kern | not yet |
| `\mskip` | mskip | node emitting math glue | not yet |
| `\noalign` | no_align | alignment | not yet |
| `\noboundary` | no_boundary | node affecting ligature boundaries | not yet |
| `\noindent` | start_par | `new_graf` without the indent box | done |
| `\nolimits` | limit_switch | limits flag on the previous noad | not yet |
| `\nonscript` | non_script | math glue cancel | not yet |
| `\omit` | omit | alignment | not yet |
| `\over` | above | generalized fraction | not yet |
| `\overline` | math_comp | math noad | not yet |
| `\overwithdelims` | above | generalized fraction | not yet |
| `\par` | par_end | `Kind::Par` node: the paragraph gathered from the items its nodes emitted (`Para`: list + `LineParams`); outer level: lines a separate task, inside a box broken here (tex.web 1096) | done |
| `\penalty` | break_penalty | node emitting a penalty; in vertical mode also `Item::BuildPage` (tex.web 1103) | done |
| `\radical` | radical | math radical | not yet |
| `\relax` | relax | node with no result | done |
| `\right` | left_right | math delimiters (a group) | not yet |
| `\scriptscriptstyle` | math_style | math style item | not yet |
| `\scriptstyle` | math_style | math style item | not yet |
| `\setlanguage` | extension | whatsit: material | not yet |
| `\span` | tab_mark | alignment cell end | not yet |
| `\textstyle` | math_style | math style item | not yet |
| `\underline` | math_comp | math noad | not yet |
| `\unkern` | remove_item | node emitting `Item::Remove(1)`, applied by the list's gatherer (tex.web 1105) | done |
| `\unpenalty` | remove_item | node emitting `Item::Remove(0)`, applied by the list's gatherer | done |
| `\unskip` | remove_item | node emitting `Item::Remove(2)`, applied by the list's gatherer | done |
| `\valign` | valign | alignment, vertical | not yet |
| `\vcenter` | vcenter | math box | not yet |
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
| `\deadcycles` | set_page_int | `\deadcycles`, `\insertpenalties`: page builder / output state, a read of the page scan (see Conflicts) | approx: 0 |
| `\dp` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\dump` | stop | `\end`, and the log's "Beginning to dump" lines: the format is the set of definitions reaching this point (no file) | done (the log up to the format's name) |
| `\end` | stop | `its_all_over`: node emitting `Item::End` (the page fold appends `\hbox to\hsize{}\vfill\penalty-2^30` and builds pages until empty, tex.web 1054); `final_cleanup`'s lines after the main file's ` )` (§1335); reading stops | done (later pieces run but are left out: Conflict 21, IO) |
| `\hbox` | make_box | a `Kind::Open` node opening a box group (`BoxGroup`: kind, context, `to`/`spread`, the enclosing list's mode, space factor, prev depth); its `}` is a `Close` node; the packaging node (`Kind::Par`) gathers the items emitted inside and packs them (hpack, tex.web 649) with `\hbadness`/`\hfuzz`/`\overfullrule` read after the group, diagnostics into the log | done |
| `\ht` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\insert` | insert | a group whose packaging node emits `Item::Ins` (with `\splittopskip`, `\splitmaxdepth`, `\floatingpenalty` read inside it); moved out of lines and adjusted hboxes (§655) | partial (the item; the page fold's handling is A/B's) |
| `\insertpenalties` | set_page_int | `\deadcycles`, `\insertpenalties`: page builder / output state, a read of the page scan (see Conflicts) | approx: 0 |
| `\lastbox` | make_box | the current list's last box (Conflict 13's read of material) is the node's value, and an `Item::Remove(3)` its gatherer applies | done |
| `\lastkern` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\lastpenalty` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\lastskip` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\leaders` | leader_ship | `scan_box` (a box or a rule), then the glue after it: `Item::Leaders` (the remaining material) | done |
| `\lower` | vmove | `scan_box` with a shift (horizontal mode) | done |
| `\mark` | mark | node emitting `Item::Mark` (expanded text); marks move out of an `\hbox` in vertical mode and out of lines; `\topmark`... not yet | done (no mark classes) |
| `\moveleft` | hmove | `scan_box` with a shift (vertical mode) | done |
| `\moveright` | hmove | `scan_box` with a shift (vertical mode) | done |
| `\pagedepth` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagefilllstretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagefillstretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagefilstretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagegoal` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pageshrink` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagestretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagetotal` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
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
| `\vsplit` | make_box | a node: `split::vsplit` of the register's box value (pure), the rest left in the register (Conflict 7), `Key::SplitMark` defined | done |
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
| `\showbox` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\showlists` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\showthe` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\write` | extension | with `\immediate`: an event in the node's log, its text expanded in no mode; deferred: a whatsit whose text is expanded where its page is shipped (IO) | done |
