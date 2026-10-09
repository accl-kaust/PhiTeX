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
| (i) expandable, input rewriting | 40 | 32 | 6 | 2 |
| (ii) assignments, scopes | 151 | 138 | 5 | 8 |
| (iii) material | 73 | 7 | 0 | 66 |
| (iv) layout and output | 43 | 0 | 19 | 24 |
| (v) IO and diagnostics | 18 | 6 | 2 | 10 |
| all | 325 | 183 | 32 | 110 |

("done": tex.web's semantics for programs that do not typeset, checked
against `tex -ini` by `tests/core.rs`; "partial": the parts that fit, with
what is missing in the status column.)

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
| `\box` n | `Key::Box(n)` (`Val::Void` only, yet; later `Val::Box`: dimensions + contents, two fields) |
| `\parshape` | `Key::Parshape` (`Val::Parshape`) |
| current font | `Key::Font` (`Val::Font`) |
| `\fontdimen` n of a font | `Key::FontDimen(font name, n)` (always global) |
| `\hyphenchar`, `\skewchar`, `\textfont`… (later) | `Key::HyphenChar(font)`, `Key::SkewChar(font)`, `Key::FamFont(size, fam)` |
| `cur_group`, the save stack's group boundaries | `RState.groups` (kind, for mismatch errors; and the `\aftergroup` tokens of each open group) — scoping itself is `Chain::resolve` |
| `cond_ptr`, `if_limit` | `RState.conds`: per open conditional, the node that opened it, which ends may come, its test |
| `\global`/`\long`/`\outer` prefixes | `RState.global/long/outer`, set by the prefix node, consumed by the assignment node |
| `after_token` (`\afterassignment`) | `RState.after_assignment` |
| mode, `space_factor`, `prev_depth`, `prev_graf` | `RState.mode`, `RState.space_factor`, `RState.prev_depth`, `RState.prev_graf` |
| `end_line_char` as the line was read | `RState.eol` (read when a line is begun, as TeX's `input_ln` does) |
| the input stack, `back_input`, `ins_list` | `RState.pending` (rewritten input; `Tok::DontExpand` is `\noexpand`'s mark) |
| `\message`, `\immediate\write` output | `Node.log`, gathered in document order into `Output.log`; `print::log` lays it out as TeX's log |

The initial values (`val::initial`) are IniTeX's (tex.web 222-240), but for
what the minimal system's documents rely on: category codes of `{ } $ & # ^
~` as plain TeX sets them, and `\hsize`, `\vsize`, `\parindent`,
`\maxdepth`, `\baselineskip`, `\topskip`, `\tolerance`, `\linepenalty`.
`tests/core.rs` gives `tex -ini` the same prelude. Milestone 2 should make
them exactly IniTeX's (a format is a set of definitions; `tests/flow.rs`
then sets what it needs) — a decision for whoever owns `tests/flow.rs`.

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
   each `\read` (which also defines the macro). Not implemented; `\ifeof`
   is true (no stream is ever open), which is exact while `\openin` is
   absent.
7. **`\setbox`/`\box`/`\copy`/`\wd`** — a box register's value is material:
   `Val::Box` with two fields, dimensions and contents, so `\wd`/`\ifhbox`
   read the dimensions field only. Conflict: `\box n` (use) empties the
   register *without* respecting grouping (tex.web 1079: `box(n):=null` is
   not an `eq_define`): after `{\box0}` the register is void even if it was
   set outside the group. Proposal: a definition kind "at the level of the
   definition it replaces": the node defines `Key::Box(n)` = void with the
   scope of the reaching definition it read (local to that group, or
   global). Now: only void boxes exist (`\ifvoid` true, `\wd` 0pt).
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
   Now: assignments and reads work; characters and paragraphs do not update
   them yet (milestone 2: byte-identical spacing needs the space factor).
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
    material before it (the true dependency). Now: an empty page's values,
    empty marks.
13. **The current list's last item**: `\lastpenalty`, `\lastkern`,
    `\lastskip`, `\unskip`, `\unkern`, `\unpenalty`, `\/`. Proposal: the
    read is an operand naming the preceding node's emitted material; the
    removals emit a `Remove` item that the `Par` node (or a box's `Close`)
    applies when it gathers the list. Now: 0 in vertical mode (no command
    appends such items yet), an error in horizontal mode.
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
17. Small departures, documented: `\if…`'s inserted `\relax` (a `\fi` met
    while the test is read) is the name `\relax`, not a frozen one; a
    macro's parameter shows as `#n` whatever its parameter character; a
    `^^` reduction is not reduced again.

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
| `\fontname` | convert | step: reads its operand (a token, a number, `\escapechar`, catcodes of one-letter names) and rewrites the input with category-12 tokens | partial: `\font` only |
| `\if` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifcase` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifcat` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifdim` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifeof` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done (no stream is open) |
| `\iffalse` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifhbox` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done (void boxes only) |
| `\ifhmode` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifinner` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifmmode` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifnum` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifodd` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\iftrue` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifvbox` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done (void boxes only) |
| `\ifvmode` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done |
| `\ifvoid` | if_test | conditional: at the top a node (`Kind::If`, test operands = the reads it made); inside scanning a step of that node; only the taken arm is read; the end gets an explicit φ | done (void boxes only) |
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
| `\font` | def_font | `\font\x=name`: node defining `Key::Cs(x)` = a font identifier whose value is the TFM file read by name — a file read like `\input` (see Conflicts); as an identifier, the current font (`Key::Font`) | partial: identifier only |
| `\fontdimen` | assign_font_dimen | node defining `Key::FontDimen(font, n)` (always global); only `\font` (the current font) as identifier yet | partial |
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
| `\hyphenchar` | assign_font_int | node defining `Key::HyphenChar(font)`/`Key::SkewChar(font)` (always global) — needs font identifiers | not yet |
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
| `\nullfont` | set_font | the null font identifier: a node defining `Key::Font` | not yet |
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
| `\scriptfont` | def_family | node defining `Key::FamFont(size, fam)` — needs font identifiers | not yet |
| `\scriptscriptfont` | def_family | node defining `Key::FamFont(size, fam)` — needs font identifiers | not yet |
| `\scriptspace` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\sfcode` | def_code | node defining `Key::Cat/Mathcode/Lccode/Uccode/Sfcode/Delcode(c)` | done |
| `\showboxbreadth` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\showboxdepth` | assign_int | node defining `Key::IntPar(p)` (Val::Int); `\global` via RState.global; `\globaldefs` read by every assignment | done |
| `\skewchar` | assign_font_int | node defining `Key::HyphenChar(font)`/`Key::SkewChar(font)` (always global) — needs font identifiers | not yet |
| `\skip` | register | node defining `Key::Count/Dimen/Skip/Muskip(n)`; as a value, a read of it | done |
| `\skipdef` | shorthand_def | node defining `Key::Cs(name)`: first `\relax`, then `Meaning::CharGiven/MathCharGiven/CountRef/DimenRef/SkipRef/MuskipRef/ToksRef` | done |
| `\spacefactor` | set_aux | `\spacefactor`/`\prevdepth`: fields of RState (the list being built), assigned and read there, never Keys; `\global` has no effect (as in TeX) | partial (see Conflicts) |
| `\spaceskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\splitmaxdepth` | assign_dimen | node defining `Key::DimenPar(p)` (Val::Int, sp) | done |
| `\splittopskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\tabskip` | assign_glue | node defining `Key::GluePar(p)` (Val::Glue) | done |
| `\textfont` | def_family | node defining `Key::FamFont(size, fam)` — needs font identifiers | not yet |
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
| `\-` | discretionary | node emitting a discretionary (material in groups) | not yet |
| `\/` | ital_corr | node emitting the italic correction kern (reads the previous item) | not yet |
| `\above` | above | generalized fraction | not yet |
| `\abovewithdelims` | above | generalized fraction | not yet |
| `\accent` | accent | node emitting an accented character | not yet |
| `\atop` | above | generalized fraction | not yet |
| `\atopwithdelims` | above | generalized fraction | not yet |
| `\char` | char_num | `\char n`: node emitting the character | done |
| `\cr` | car_ret | alignment row end | not yet |
| `\crcr` | car_ret | alignment row end | not yet |
| `\delimiter` | delim_num | math delimiter | not yet |
| `\discretionary` | discretionary | node emitting a discretionary (material in groups) | not yet |
| `\displaylimits` | limit_switch | limits flag on the previous noad | not yet |
| `\displaystyle` | math_style | math style item | not yet |
| `\eqno` | eq_no | equation number | not yet |
| `\halign` | halign | alignment: nodes for the template and rows; the alignment is a node whose operands are the rows' material | not yet |
| `\hfil` | hskip | node emitting glue | not yet |
| `\hfill` | hskip | node emitting glue | not yet |
| `\hfilneg` | hskip | node emitting glue | not yet |
| `\hrule` | hrule | node emitting a rule | not yet |
| `\hskip` | hskip | node emitting glue | not yet |
| `\hss` | hskip | node emitting glue | not yet |
| `\ignorespaces` | ignore_spaces | node reading (with expansion) past spaces; the next token is the next node's command | done |
| `\indent` | start_par | node switching RState.mode to H, emitting the indent box | done |
| `\kern` | kern | node emitting a kern | not yet |
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
| `\noindent` | start_par | node switching RState.mode to H, emitting the indent box | done |
| `\nolimits` | limit_switch | limits flag on the previous noad | not yet |
| `\nonscript` | non_script | math glue cancel | not yet |
| `\omit` | omit | alignment | not yet |
| `\over` | above | generalized fraction | not yet |
| `\overline` | math_comp | math noad | not yet |
| `\overwithdelims` | above | generalized fraction | not yet |
| `\par` | par_end | `Kind::Par` node: operands = the material the paragraph's nodes emitted + parameters; result the paragraph | done |
| `\penalty` | break_penalty | node emitting a penalty | not yet |
| `\radical` | radical | math radical | not yet |
| `\relax` | relax | node with no result | done |
| `\right` | left_right | math delimiters (a group) | not yet |
| `\scriptscriptstyle` | math_style | math style item | not yet |
| `\scriptstyle` | math_style | math style item | not yet |
| `\setlanguage` | extension | whatsit: material | not yet |
| `\span` | tab_mark | alignment cell end | not yet |
| `\textstyle` | math_style | math style item | not yet |
| `\underline` | math_comp | math noad | not yet |
| `\unkern` | remove_item | `\unskip`...: a node whose result removes the previous node's last item — a read of the preceding emitted material | not yet |
| `\unpenalty` | remove_item | `\unskip`...: a node whose result removes the previous node's last item — a read of the preceding emitted material | not yet |
| `\unskip` | remove_item | `\unskip`...: a node whose result removes the previous node's last item — a read of the preceding emitted material | not yet |
| `\valign` | valign | alignment, vertical | not yet |
| `\vcenter` | vcenter | math box | not yet |
| `\vfil` | vskip | node emitting glue (vertical mode) | not yet |
| `\vfill` | vskip | node emitting glue (vertical mode) | not yet |
| `\vfilneg` | vskip | node emitting glue (vertical mode) | not yet |
| `\vrule` | vrule | node emitting a rule | not yet |
| `\vskip` | vskip | node emitting glue (vertical mode) | not yet |
| `\vss` | vskip | node emitting glue (vertical mode) | not yet |
| `\␣` | ex_space | node emitting interword glue | done |

### (iv) Layout and output — 43

| primitive | tex.web cmd | graph form | status |
|---|---|---|---|
| `\badness` | last_item | a read of the last box packing's badness | approx: 0 |
| `\box` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\cleaders` | leader_ship | `\shipout`: a node whose operand is a box value and whose result is a page; leaders: material | not yet |
| `\copy` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\deadcycles` | set_page_int | `\deadcycles`, `\insertpenalties`: page builder / output state, a read of the page scan (see Conflicts) | approx: 0 |
| `\dp` | set_box_dimen | `\wd n`...: a read of the box value's dimensions field (`Key::Box(n)`); assignment defines a new box value | partial: void boxes |
| `\dump` | stop | the end of a format: the format is the set of definitions reaching this point | no-op |
| `\end` | stop | the end of the document: the last page scan and `\output` | no-op |
| `\hbox` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\ht` | set_box_dimen | `\wd n`...: a read of the box value's dimensions field (`Key::Box(n)`); assignment defines a new box value | partial: void boxes |
| `\insert` | insert | node emitting an insertion (material in a group); the page scan reads its size | not yet |
| `\insertpenalties` | set_page_int | `\deadcycles`, `\insertpenalties`: page builder / output state, a read of the page scan (see Conflicts) | approx: 0 |
| `\lastbox` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\lastkern` | last_item | `\lastpenalty`... read the current list's last item: a read of the preceding node's emitted material | approx: 0 |
| `\lastpenalty` | last_item | `\lastpenalty`... read the current list's last item: a read of the preceding node's emitted material | approx: 0 |
| `\lastskip` | last_item | `\lastpenalty`... read the current list's last item: a read of the preceding node's emitted material | approx: 0 |
| `\leaders` | leader_ship | `\shipout`: a node whose operand is a box value and whose result is a page; leaders: material | not yet |
| `\lower` | vmove | box shifted, material | not yet |
| `\mark` | mark | node emitting a mark item; the page scan reads it into `\topmark`... | not yet |
| `\moveleft` | hmove | box shifted, material | not yet |
| `\moveright` | hmove | box shifted, material | not yet |
| `\pagedepth` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagefilllstretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagefillstretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagefilstretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagegoal` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pageshrink` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagestretch` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\pagetotal` | set_page_dimen | `\pagegoal`...: the page scan's state at this point — a read of the page scan (see Conflicts) | approx: empty page |
| `\raise` | vmove | box shifted, material | not yet |
| `\setbox` | set_box | `\setbox n=<box>`: node defining `Key::Box(n)` = Val::Box (material: dimensions + contents) — see Conflicts | not yet |
| `\shipout` | leader_ship | `\shipout`: a node whose operand is a box value and whose result is a page; leaders: material | not yet |
| `\special` | extension | whatsit: material reaching `\shipout` | not yet |
| `\unhbox` | un_hbox | node emitting a box register's contents (reads `Key::Box(n)`; `\unhbox` also defines it void) | not yet |
| `\unhcopy` | un_hbox | node emitting a box register's contents (reads `Key::Box(n)`; `\unhbox` also defines it void) | not yet |
| `\unvbox` | un_vbox | as `\unhbox`, vertical | not yet |
| `\unvcopy` | un_vbox | as `\unhbox`, vertical | not yet |
| `\vadjust` | vadjust | node emitting adjust material in a group | not yet |
| `\vbox` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\vsplit` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\vtop` | make_box | box construction: a node opening a group whose Close node packs the material emitted inside into a box value | not yet |
| `\wd` | set_box_dimen | `\wd n`...: a read of the box value's dimensions field (`Key::Box(n)`); assignment defines a new box value | partial: void boxes |
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
