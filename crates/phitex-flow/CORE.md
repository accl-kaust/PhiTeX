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
| (i) expandable, input rewriting | 40 | 33 | 5 | 2 |
| (ii) assignments, scopes | 151 | 146 | 3 | 2 |
| (iii) material | 73 | 63 | 0 | 10 |
| (iv) layout and output | 43 | 23 | 12 | 8 |
| (v) IO and diagnostics | 18 | 6 | 2 | 10 |
| all | 325 | 271 | 22 | 32 |

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
   each `\read` (which also defines the macro). Not implemented; `\ifeof`
   is true (no stream is ever open), which is exact while `\openin` is
   absent.
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
    `\lastbox` (a removal whose value the node needs) is not yet.
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
    `(\end occurred inside a group…)` lines are not written.
22. **The DVI comment and the date**: TeX Live's `tex` takes the
    preamble's comment date from the clock (it ignores
    `SOURCE_DATE_EPOCH`), the graph from `\year`… (1970.01.01:0000
    initially). `tests/dvi.rs` masks the comment's bytes (15 to 41) when it
    compares files; everything else is compared byte for byte.
23. **Hyphenation**: the tables and `hyphenate` are pure functions over
    values (`hyph.rs`, see Hyphenation below); `\patterns` and
    `\hyphenation` are not yet nodes and line breaking's second pass does
    not hyphenate (exact for `-ini` documents without patterns). `\accent`, `\noboundary`, `\vadjust`, `\insert`,
    leaders, `\special` and non-immediate `\write` are not yet.
24. Small departures, documented: `\if…`'s inserted `\relax` (a `\fi` met
    while the test is read) is the name `\relax`, not a frozen one; a
    macro's parameter shows as `#n` whatever its parameter character; a
    `^^` reduction is not reduced again.

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
- Not yet: `\accent` in math (and outside), display alignments
  (`$$\halign…$$`, with the alignments), `\vadjust` and `\insert` in
  formulas, `\showlists`/`\showbox` of noads. Approximations: the check
  for enough symbol and extension font parameters (§1195) counts the
  parameters the font was loaded with (TeX also counts those `\fontdimen`
  added to the last loaded font); `off_save` in math (a `$` or `\right`
  closing the wrong group) inserts a `}` only.

## Hyphenation

Knuth's algorithm as pure functions (`hyph.rs`, tex.web §891–965), checked
by `tests/hyph.rs`: `hyphen.tex`'s patterns and exceptions, 368 words
(plus `\uchyph`, minima and a later `\hyphenation`) break exactly where
`tex -ini`'s `\showhyphens` breaks them.

| TeX | graph |
|---|---|
| the trie of one language (§920–958) | `Key::Patterns(lang)` → `Val::Patterns(hyph::Patterns)`: an immutable `Arc` value, each pattern's letters to its digits; it answers as the packed trie does (§923–924) |
| the exception dictionary (§926–941) | `Key::Exceptions(lang)` → `Val::Exceptions(hyph::Exceptions)`, one per language (TeX keys its one table by word + `cur_lang`: the same thing) |
| `\patterns{…}` | a node reading `\language` (`IntPar`, made 0–255 by `hyph::cur_lang`), the old `Key::Patterns(lang)` and the `\lccode` of each character it reads; it defines `Key::Patterns(lang)` = `old.add(toks)` **globally** (TeX's tables have no save stack); its errors are the node's |
| `\hyphenation{…}` | likewise: reads `\language`, `Key::Exceptions(lang)`, the `\lccode`s; defines `Key::Exceptions(lang)` globally (a word given again replaces the old one) |
| `\language`, `\lefthyphenmin`, `\righthyphenmin`, `\uchyph` | `IntPar`s; a paragraph takes language and minima where it starts (§1091, `norm_min`) and where `\language` changes in it (a language whatsit, §1376), `\uchyph` where it ends (§891) |
| `\hyphenchar` | `Key::HyphenChar(font key)` (done), read by the paragraph for each font its words use (§891: outside 0–255, no word of that font is hyphenated) |
| `trie_not_ready` (§891, §960) | open: TeX packs the trie at the first paragraph whose line breaking reaches the second pass, and `\patterns` after that is "Too late for \patterns" — whether that happened is a read of earlier paragraphs' line breaking. Proposal: accept `\patterns` always (different from TeX only after that error) |

The integration (after the merge) is in the line-break task: its second
pass (§894–903) finds the words — after glue, letters of one font with
nonzero `\lccode`, at most 63, followed by glue, a penalty, an insertion,
a mark, an adjust, a whatsit or an explicit kern (§896–899) — and calls
`hyphenate` with the paragraph's `Patterns`/`Exceptions` (reads of
`Key::Patterns(lang)`, `Key::Exceptions(lang)` at the paragraph's end, so
an edit to the tables re-runs exactly the paragraphs that reached the
second pass), memoised per word by (the two versions, the word, the
minima). Reconstitution (§903–918) turns the positions into
discretionaries with the font's ligatures and kerns; it can drop a
position (§916 builds a post-break text without hyphens: `raffish` with
both minima 1 gives `raf-fish` in cmr10 though `hyphenate` allows
`raf-f-ish`), so the oracle for that step is the line breaker's own
`tests/dvi.rs` documents.

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

`\deadcycles` and `\insertpenalties` (set_page_int) are fold state, not
`Key`s: an assignment to them in the body emits an item the fold applies
in order (as `\prevdepth`'s `Item::PrevDepth` does now); in a routine run
it changes the run's own copy, returned to the fold at the run's end. A
read of them, of the marks, of `\pagegoal`… in the body is a read of the
fold's state at the reading node's position (Conflict 12).

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

## The inventory

### (i) Expandable and input-rewriting: steps — 40

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\botmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
| `\csname` | cs_name | step: builds the name, rewrites the input with it; an undefined name is defined `\relax` (local) by the node the step is in | done |
| `\else` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\endcsname` | end_cs_name | outside `\csname`: an error node | done |
| `\endinput` | input | a call into a named source: the file is a value whose text becomes pieces — DESIGN CONVERSATION (see Conflicts) | not yet |
| `\expandafter` | expand_after | step: expands the token after next once, rewrites the input | done |
| `\fi` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\firstmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
| `\fontname` | convert | step: the font's name and size (tex.web 472) | done |
| `\if` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifcase` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifcat` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifdim` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifeof` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done (no stream is open) |
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
| `\input` | input | a call into a named source: the file is a value whose text becomes pieces — DESIGN CONVERSATION (see Conflicts) | not yet |
| `\jobname` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done (constant `texput`) |
| `\lowercase` | case_shift | node (not expandable): reads lccode/uccode (`get`) of each character and rewrites the input with the changed text; defines nothing | done |
| `\meaning` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\noexpand` | no_expand | step: marks the next token not-to-expand once (`Tok::DontExpand` in the rewritten input) | done |
| `\number` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\or` | fi_or_else | ends the taken arm (skips the rest to `\fi`): the closing node defines each key the arm defined as φ(after, before) | done |
| `\romannumeral` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | done |
| `\splitbotmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
| `\splitfirstmark` | top_bot_mark | step: rewrites the input with a mark of the page builder's last output — reads the page scan's result (see Conflicts); empty until marks exist | approx: empty |
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
| `\hyphenation` | hyph_data | `\patterns` (IniTeX only) and `\hyphenation`: global definitions of a language's hyphenation tables, `Key::Patterns(lang)`/`Key::Exceptions(lang)`, read by line breaking (see Hyphenation) | not yet: the tables as values and `hyphenate` are done (`hyph.rs`, `tests/hyph.rs`); the nodes and the line breaker's pass come after the merge |
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
| `\patterns` | hyph_data | `\patterns` (IniTeX only) and `\hyphenation`: global definitions of a language's hyphenation tables, `Key::Patterns(lang)`/`Key::Exceptions(lang)`, read by line breaking (see Hyphenation) | not yet: the tables as values and `hyphenate` are done (`hyph.rs`, `tests/hyph.rs`); the nodes and the line breaker's pass come after the merge |
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
| `\accent` | accent | node emitting an accented character | not yet |
| `\atop` | above | node emitting `MItem::Above` with thickness 0 | done |
| `\atopwithdelims` | above | as `\atop`, with the two delimiters | done |
| `\char` | char_num | `\char n`: the main loop from that character (tex.web 1038) | done |
| `\cr` | car_ret | alignment row end | not yet |
| `\crcr` | car_ret | alignment row end | not yet |
| `\delimiter` | delim_num | in math, `\delimiter n`: a math character `n div "1000` (tex.web 1160); outside math, a `$` is missing | done |
| `\discretionary` | discretionary | three box-like groups (`BoxKind::Disc`): each `}` is a `Close` node, the packaging node keeps the part and opens the next; the last emits `Item::Disc` and the replacement (tex.web 1117–1121) | done |
| `\displaylimits` | limit_switch | node emitting `MItem::Limits(normal)`, applied to the list's last noad by its gatherer; the node reads its list to complain if that is not an operator (tex.web 1159) | done |
| `\displaystyle` | math_style | node emitting `MItem::Style(0)` | done |
| `\eqno` | eq_no | in a display, opens the equation number's formula (a math group, tex.web 1142); its packaging node keeps the box in the display's group record and closes the display (`\0displayend`) | done |
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
| `\noalign` | no_align | alignment | not yet |
| `\noboundary` | no_boundary | node affecting ligature boundaries | not yet |
| `\noindent` | start_par | `new_graf` without the indent box | done |
| `\nolimits` | limit_switch | node emitting `MItem::Limits(nolimits)` (as `\displaylimits`) | done |
| `\nonscript` | non_script | node emitting zero glue of subtype `cond_math_glue` (tex.web 1171) | done |
| `\omit` | omit | alignment | not yet |
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
| `\setlanguage` | extension | whatsit: material | not yet |
| `\span` | tab_mark | alignment cell end | not yet |
| `\textstyle` | math_style | node emitting `MItem::Style(2)` | done |
| `\underline` | math_comp | node emitting an under noad, nucleus from `scan_math` | done |
| `\unkern` | remove_item | node emitting `Item::Remove(1)`, applied by the list's gatherer (tex.web 1105) | done |
| `\unpenalty` | remove_item | node emitting `Item::Remove(0)`, applied by the list's gatherer | done |
| `\unskip` | remove_item | node emitting `Item::Remove(2)`, applied by the list's gatherer | done |
| `\valign` | valign | alignment, vertical | not yet |
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
| `\cleaders` | leader_ship | `\shipout`: a node whose operand is a box value and whose result is a page; leaders: material | not yet |
| `\copy` | make_box | reads `Key::Box(n)`; `box_end` | done |
| `\deadcycles` | set_page_int | `\deadcycles`, `\insertpenalties`: page builder / output state, a read of the page scan (see Conflicts) | approx: 0 |
| `\dp` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\dump` | stop | the end of a format: the format is the set of definitions reaching this point | no-op |
| `\end` | stop | `its_all_over`: node emitting `Item::End` (the page fold appends `\hbox to\hsize{}\vfill\penalty-2^30` and builds pages until empty, tex.web 1054); reading stops | done (later pieces still run: Conflict 21) |
| `\hbox` | make_box | a `Kind::Open` node opening a box group (`BoxGroup`: kind, context, `to`/`spread`, the enclosing list's mode, space factor, prev depth); its `}` is a `Close` node; the packaging node (`Kind::Par`) gathers the items emitted inside and packs them (hpack, tex.web 649) with `\hbadness`/`\hfuzz`/`\overfullrule` read after the group, diagnostics into the log | done |
| `\ht` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\insert` | insert | node emitting an insertion (material in a group); the page scan reads its size | not yet |
| `\insertpenalties` | set_page_int | `\deadcycles`, `\insertpenalties`: page builder / output state, a read of the page scan (see Conflicts) | approx: 0 |
| `\lastbox` | make_box | removes the last box of the current list: a removal whose value the node needs — Conflict 13 | not yet |
| `\lastkern` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\lastpenalty` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\lastskip` | last_item | the current list's last item (`last_item`, Conflict 13) | done (inside a piece) |
| `\leaders` | leader_ship | `\shipout`: a node whose operand is a box value and whose result is a page; leaders: material | not yet |
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
| `\special` | extension | whatsit: material reaching `\shipout` | not yet |
| `\unhbox` | un_hbox | node emitting a box register's list (reads `Key::Box(n)`, voids it: Conflict 7) | done |
| `\unhcopy` | un_hbox | node emitting a box register's list | done |
| `\unvbox` | un_vbox | as `\unhbox`, vertical | done |
| `\unvcopy` | un_vbox | as `\unhbox`, vertical | done |
| `\vadjust` | vadjust | node emitting adjust material in a group | not yet |
| `\vbox` | make_box | as `\hbox`, vpack (tex.web 668) with `\boxmaxdepth` read inside the group; a paragraph open at its `}` ends first | done |
| `\vsplit` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\vtop` | make_box | as `\vbox`, height and depth readjusted (tex.web 1087) | done |
| `\wd` | set_box_dimen | `\wd n`...: a read of `Key::Box(n)`'s dimensions; assignment defines a new box value at the scope of the replaced definition (Conflict 7) | done |
| `\xleaders` | leader_ship | `\shipout`: a node whose operand is a box value and whose result is a page; leaders: material | not yet |

### (v) IO and diagnostics — 18

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\batchmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\closein` | in_stream | `\openin`/`\closein`: a node defining `Key::ReadStream(n)` = the opened file value (see Conflicts) | not yet |
| `\closeout` | extension | a write stream: a node defining `Key::WriteStream(n)`; deferred ones are whatsits run at `\shipout` | not yet |
| `\errmessage` | message | node whose result is a string (`Node.log`), the expanded text shown; `\errmessage` an error | done |
| `\errorstopmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\immediate` | extension | prefix for `\write`/`\openout`/`\closeout`: the extension runs in this node | with `\write` |
| `\inputlineno` | last_item | a read of the line-number field of the position (never part of a version) | not yet (error, 0) |
| `\message` | message | node whose result is a string (`Node.log`), the expanded text shown; `\errmessage` an error | done |
| `\nonstopmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\openin` | in_stream | `\openin`/`\closein`: a node defining `Key::ReadStream(n)` = the opened file value (see Conflicts) | not yet |
| `\openout` | extension | a write stream: a node defining `Key::WriteStream(n)`; deferred ones are whatsits run at `\shipout` | not yet |
| `\read` | read_to_cs | `\read n to \x`: a node defining `\x` from the next line of a stream — a stream is a source read by position (see Conflicts) | not yet |
| `\scrollmode` | set_interaction | node with no result (interaction changes only what reaches the terminal) | done (no-op) |
| `\show` | xray | diagnostic: `> \x=meaning.` in the log, an error context | not yet |
| `\showbox` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\showlists` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\showthe` | xray | diagnostic node whose result is log text (`> ...`) and an error context — needs the line number field | not yet |
| `\write` | extension | with `\immediate`: a node whose result is the written line (`Node.log`, ending in a newline), its text expanded in no mode; deferred: a whatsit expanded at `\shipout` | `\immediate` only |
