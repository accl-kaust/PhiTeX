# Primitive catalogue

Every primitive of TeX82 (`tex.web`), e-TeX (`etex.ch`) and pdfTeX
(`pdftex.web`) as a node spec on `phitex-reduce`. The core knows fields,
scopes, claims and unfolds, nothing of TeX; `phitex-tex` gives each
primitive a spec in those terms. Where a primitive needs something the
core does not have, the row says so in **hazards** and the primitive is
listed again under **Findings**. Nothing here is a design proposal.

Citations: `§N` is a section of `tex.web`; `pdfTeX §N` is a section of
`pdftex.web` (TeX Live's, which contains e-TeX: e-TeX primitives are cited
by their `pdftex.web` section, since `etex.ch` is a change file without
stable section numbers). `primitive(...)` lines are cited for the
primitive's registration, implementation sections for its behaviour.

## Field model

The eqtb map (§220–§251), named as field families. A field is what a node
reads or defines; a *family* is a name space indexed by a key.

| Family | Key | Holds | eqtb region |
|---|---|---|---|
| `cs[name]` | control-sequence name (or active character) | meaning: primitive, macro (prefixes, parameter text, body), `\let` copy, `chardef` value, register shorthand, font identifier, `undefined` | region 2, §222 |
| `cat[c]` | character code | catcode 0..15 | region 4, §230 |
| `lc[c]`, `uc[c]`, `sf[c]`, `mathcode[c]`, `delcode[c]` | character code | the code | region 4/5, §230, §236 |
| `int.p` | parameter name | integer (`\pretolerance` … ; e-TeX and pdfTeX add theirs) | region 5, §236–§238 |
| `dim.p` | parameter name | dimension (`\hsize` …) | region 6, §247–§248 |
| `glue.p`, `mu.p` | parameter name | glue spec (`\baselineskip` …, `\thinmuskip` …) | region 3, §224–§226 |
| `toks.p` | parameter name | token list (`\output`, `\everypar`, … `\everyeof`, pdfTeX's `\pdfpageattr` …) | region 3, §230 |
| `count[n]`, `dimen[n]`, `skip[n]`, `muskip[n]`, `toks[n]`, `box[n]` | register number (0..255; 0..32767 with e-TeX, pdfTeX §1809) | the register | regions 3–6; sparse arrays pdfTeX §1815 |
| `curfont` | — | the current font identifier | region 4, §230 `cur_font_loc` |
| `fam[size][k]` | size 0..2, family 0..15 | font identifier (`\textfont` …) | region 4, §230 `math_font_base` |
| `font[f].*` | font identifier | loaded metrics (static after `\font`), `hyphenchar`, `skewchar`, `fontdimen[k]`; pdfTeX adds `lpcode`, `rpcode`, `efcode`, `tagcode`, `knbscode`, `stbscode`, `shbscode`, `knbccode`, `knaccode`, `noligatures`, expansion spec, font attributes | §549–§576, writable parts §578, §1253 |
| `parshape` | — | the paragraph shape list | region 3, §230 `par_shape_loc` |
| `penalties.p` | e-TeX `\interlinepenalties` … | penalty array | pdfTeX §248 `etex_pen_base` |
| `hyph` | language | exceptions and patterns (global, `\patterns` INITEX only) | §934, §960 |
| `interaction` | — | the interaction mode | §73, §1265 |
| `stream[n]` | 0..15 | open output file (`\openout`) | §1341, §1350 |
| `infile[n]` | 0..15 | open input file and its position (`\openin`, `\read`) | §480 |
| `pdf.*` | — | pdfTeX's document-wide state: object numbers, `\pdflastobj` …, catalog, info, names, trailer, outlines, destinations, color stacks, map lines, glyph-to-unicode table, timer, random seed | pdfTeX §1544 ff. |

Things that are not fields, because the core has no such notion; a row
that needs one says so in **hazards** and the primitive goes to
**Findings**:

- **MODE**: the current mode (vertical, horizontal, math, their inner
  variants; §211). Many primitives dispatch on it (§1045 ff.).
- **LIST**: the current list being built (`head`, `tail`, `prev_depth`,
  `space_factor`, `prev_graf`, the LR state; §212–§213).
- **PAGE**: the page builder's state (`page_so_far`, `page_contents`,
  `dead_cycles`, `insert_penalties`, the contribution list, held-over
  inserts, the discards; §980–§994).
- **MARKS**: the five current marks per class, written by `fire_up`
  §1012 and `vsplit` §977.
- **ALIGN**: the alignment stack (`cur_align`, `align_state`, templates;
  §770–§777).
- **IF**: the condition stack (`cond_ptr`, `if_limit`; §489).
- **INPUT**: the input stack (`\inputlineno`, `\endinput`, `\scantokens`
  pseudo-files, `\read` from a file whose position advances; §300–§331).
- **TERM**: the terminal (§71, `\read16`, `\pausing`, error recovery).
- **DEFERRED**: actions parked for later: `\afterassignment` (§1268),
  `\aftergroup` (§1271), `\immediate`-less `\write` (§1375), the output
  routine (§1025), `\pdfliteral shipout` (pdfTeX §1534).

## Conventions

Column meanings and abbreviations:

- **reads**: the fields read; `cat[*]` when the claim is tokenised with
  the current catcodes, `cs[*]` when the names it meets are looked up.
- **defines**: `L` local to the current group, `G` always global, `L/G`
  local unless prefixed `\global` or `\globaldefs>0`, `G<0` means
  `\globaldefs<0` forces local (§1214). `S`: the set of defined fields is
  static (known from the text); `V`: it depends on a value (a register
  number, a computed name, a count).
- **claim**: how much following text is consumed. `K` Known from text
  (given catcodes and the meanings of the names in it), `P` Predicted
  (knowable under the stated assumption, usually "the number or keyword is
  literal and the token after it is not a macro that expands to more of
  it"), `Pd` Pending (value-dependent: expansion decides the extent).
  Numbers, dimensions and glue are scanned by `scan_int` §440,
  `scan_dimen` §448, `scan_glue` §461 through `get_x_token`, so every
  such claim is at best `P`; an `<optional equals>` §405 and one optional
  space after a number are part of the claim. `<balanced text>` is
  `scan_toks` §473 (`K` given catcodes). `<general text>` is `scan_toks`
  after expanding to the left brace (`P`: the left brace may come from a
  macro).
- **unfolds**: the text it splices in its place and what it depends on;
  `—` for unexpandable primitives.
- **contribution**: what it appends to the current list or writes to an
  output stream.
- **hazards**: everything that does not fit the field model, with the
  capital names above.

Parameters are catalogued one row per *kind* with every name listed; the
row applies to each name.

## TeX82

### Prefixes and assignments

All assignments go through `prefixed_command` §1211. A prefix (`\long`,
`\outer`, `\global`) claims the following assignment command, with
spaces and `\relax` between prefixes skipped (§1212); `\global` turns
`L` into `G` unless `\globaldefs<0`, and `\globaldefs>0` turns every
`L` into `G` (§1214). After any assignment the `\afterassignment` token
is inserted (§1269), see **DEFERRED**.

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\global` | §1208, §1211 | `int.globaldefs` | none itself; makes the prefixed assignment `G` | — | `K`: one assignment command (prefixes, spaces, `\relax` in between, §1212); the command's own claim follows | — | — | reads `int.globaldefs` (a value) to decide scope: the *scope* of the definition is value-dependent, not only its content. Non-assignment after a prefix is an error (§1213) |
| `\long` | §1208 | — | — | — | `K`: one `\def`-family command (other prefixed commands error) | — | — | the prefix becomes part of the macro's meaning (§1213 `long_call`) |
| `\outer` | §1208 | — | — | — | `K` as `\long` | — | — | an `\outer` macro's name is illegal inside claims (`check_outer_validity` §336): every later claim that contains the name reads `cs[name]` to know it is `\outer`. A claim's extent thus depends on meanings, not only catcodes |
| `\let` | §1219, §1221 | `cs[rhs]` (meaning copied) | `cs[lhs]` `L/G`, `S` | — | `K`: `<cs>` `<optional =>` one token (one space after `=` skipped) | — | — | the rhs token is *not* expanded; if rhs is a character token the lhs becomes that character (§1221). Static only when lhs is a literal cs token: lhs via `\csname` in a macro is `V` after unfolding |
| `\futurelet` | §1219, §1221 | `cs[t2]` | `cs[lhs]` `L/G`, `S` | — | `K`: `<cs>` then two tokens, both re-inserted (peek) | re-emits `t1 t2` (the two tokens it claimed) | — | peeks: the claim is also an unfold of the same text; `t2` is looked at unexpanded |
| `\def`, `\gdef`, `\edef`, `\xdef` | §1208, §1218 | `cat[*]` for the body; `\edef`/`\xdef` also every field the body's expansion reads (`cs[*]`, registers via `\the`, …) | `cs[name]` `L/G` (`\gdef`, `\xdef`: `G` unless `\globaldefs<0`), `S` | — | `K` for `\def`/`\gdef`: `<cs>` `<parameter text>` up to `{`, `<balanced text>` (§473–§476); `K` extent for `\edef`/`\xdef` too (the braces are matched on the unexpanded text as `scan_toks(true,true)` expands *inside* the braces but `#` and brace matching use the expanded result: a macro expanding to an unbalanced brace changes the extent, so `P` with "no macro in the body yields an unmatched brace") | — | — | `\edef` body is a reduction result: `\the\toks` and `\noexpand` inside stop expansion (§478); `\edef` of `\outer`-containing text is an error. Macro parameter `#1`…`#9` and `##` are structural |
| `\chardef`, `\mathchardef` | §1222, §1224 | — | `cs[name]` `L/G` as `char_given`/`math_given`, `S` (name static, value is a number) | — | `P`: `<cs>` `<optional =>` `<number>` (8-bit, 15-bit) | — | — | the name is first set to `\relax` (§1224) so that `\chardef\x=\x` works: two definitions of the same field in one node |
| `\countdef`, `\dimendef`, `\skipdef`, `\muskipdef`, `\toksdef` | §1222, §1224 | — | `cs[name]` `L/G` as a register shorthand, `S` name; the bound register number is a value | — | `P`: `<cs>` `<optional =>` `<8-bit number>` (e-TeX: register number up to 32767, pdfTeX §1811) | — | — | later uses of `\x` are reads/defines of `count[n]` with `n` from this value: a `V` dependency one step removed |
| `\read` | §265, §1225, §482 | `infile[n]` position, `cat[*]`, `int.endlinechar`; `n<0` or closed: **TERM** | `cs[name]` `L/G`, `S`; also advances `infile[n]` (the file position is a field defined by every `\read` of it) | — | `P`: `<number>` `to` `<cs>`; the *lines* claimed are from the file, as many as make the braces balance (§483–§486), `Pd` on the file content | — | — | **INPUT**/**TERM**: reads lines from a file whose position is state advanced by every `\read` in document order (§485); from stream 16 or a closed stream reads the terminal (§484). `\read` to `\relax`-ed names: the definition target is a cs token (`get_r_token` §1215) |
| `\font` | §265, §1257 | `cs[*]` for the file name, `font[*]` (reuse of an already-loaded font §1260), the TFM file | `cs[name]` `L/G` as `set_font` with a new font identifier `f`; `font[f].*` (metrics, `G`: font memory is never local, §1257) | — | `P`: `<cs>` `<optional =>` `<file name>` then optional `at <dimen>` or `scaled <number>` (§1258); file name ends at a space or non-letter (§526) | — | — | the font identifier `f` is allocated by load order (`font_ptr` §1260): a global counter, so the *identity* of the defined field depends on how many `\font`s ran before (pdfTeX relaxes this with `\pdfcopyfont`). The name is first set to `\nullfont` (§1257) |
| `\fontdimen` | §265, §1253, §578 | `font[f].params` | `font[f].fontdimen[k]` `G` (font memory, §578; `\global` irrelevant), `V` (`k`, `f` are values) | — | `P`: `<number>` `<font>` `<optional =>` `<dimen>` | — | — | may *grow* the font's parameter array if `f` is the last font loaded (§580): the field set depends on load order |
| `\hyphenchar`, `\skewchar` | §1254, §1253 | — | `font[f].hyphenchar`/`skewchar` `G` (§1253 ignores `\global`), `V` (`f` a value) | — | `P`: `<font>` `<optional =>` `<number>` | — | — | global even without `\global`, cannot be undone by a group |
| `\textfont`, `\scriptfont`, `\scriptscriptfont` | §1230, §1234 | — | `fam[size][k]` `L/G`, `V` (`k` a number) | — | `P`: `<4-bit number>` `<optional =>` `<font>` | — | — | — |
| `\nullfont` | §553 | — | `curfont` `L/G` when used as a font identifier in `\nullfont` as a command (§1217 `set_font`) | — | `K`: none | — | — | as a `<font>` operand it is a value, not a command |
| `\catcode`, `\mathcode`, `\lccode`, `\uccode`, `\sfcode`, `\delcode` | §1230, §1232 | — | `cat[c]`, `mathcode[c]`, `lc[c]`, `uc[c]`, `sf[c]`, `delcode[c]` `L/G`, `V` (`c` a number) | — | `P`: `<8-bit number>` `<optional =>` `<number>` | — | — | `cat[c]` is read by *every* later tokenisation: a `\catcode` with a non-literal `c` or value makes all following claims `Pd` until the value is known |
| `\setbox` | §265, §1241 | — | `box[n]` `L/G`, `V` (`n` a number); with `\global` the box is `G` (§1241 encodes it as `n+256`) | the box's own group (`\hbox{…}` opens `hbox_group` etc.) | `P`: `<8-bit number>` `<optional =>` `<box>`; the `<box>` claim is that of `\hbox`/`\vbox`/`\box`/`\copy`/`\lastbox`/`\vsplit` | — | none (the box is stored, not appended) | `\lastbox`/`\vsplit` as the rhs read **LIST**/`box[m]`; `set_box_allowed` is false inside `\accent` and display `\halign` (§1241): a **MODE**-like flag |
| `\parshape` | §265, §1248 | — | `parshape` `L/G`, `S` (one field, variable size) | — | `P`: `<number>` then that many pairs of `<dimen>`: the *count* of operands is a value (`Pd` unless the number is literal) | — | — | claim length is value-dependent |
| `\prevgraf` | §265, §1244 | **LIST** (`nest[p].pg_field` of the innermost non-inner-math level) | **LIST** `prev_graf` | — | `P`: `<optional =>` `<number>` | — | — | **LIST**: writes the paragraph's line count used by `\clubpenalty` logic; not a field |
| `\spacefactor`, `\prevdepth` | §416, §1243 | **MODE** (`cur_chr<>abs(mode)` is an error) | **LIST** `space_factor` or `prev_depth` | — | `P`: `<optional =>` `<number>` or `<dimen>` | — | — | **MODE**, **LIST** |
| `\deadcycles`, `\insertpenalties` | §416, §1246 | — | **PAGE** `dead_cycles`/`insert_penalties` | — | `P`: `<optional =>` `<number>` | — | — | **PAGE** |
| `\pagegoal`, `\pagetotal`, `\pagestretch`, `\pagefilstretch`, `\pagefillstretch`, `\pagefilllstretch`, `\pageshrink`, `\pagedepth` | §983, §1245 | — | **PAGE** `page_so_far[c]` | — | `P`: `<optional =>` `<dimen>` | — | — | **PAGE** (writing is meaningful only inside `\output`, §1245) |
| `\wd`, `\ht`, `\dp` | §416, §1247 | — | `box[n].wd/ht/dp` `G` (changes the node in place: §1247 says nothing about groups; a box is a shared object), `V` | — | `P`: `<8-bit number>` `<optional =>` `<dimen>` | — | — | mutates a stored box, not a slot: `\setbox` copies by reference (`\copy` copies the node) |
| `\hyphenation` | §1250, §934 | `cat[*]`, `lc[*]`, `int.language` | `hyph` exceptions `G` (§934: the exception table is never local), `V` (keyed by the words and by `\language`) | — | `K`: `<general text>` (`P` on the brace) | — | — | always global |
| `\patterns` | §1250, §960 | `cat[*]`, `lc[*]`, `int.language` | `hyph` patterns `G`; only in INITEX before the first paragraph (§960 `trie_not_ready`) | — | `K`: `<general text>` | — | — | an error after the trie is packed: order-dependent on the first `\par` in the whole job |
| `\batchmode`, `\nonstopmode`, `\scrollmode`, `\errorstopmode` | §1262, §1265 | — | `interaction` `G` (§1264 ignores `\global`; e-TeX's `\interactionmode` is the readable form) | — | `K`: none | — | — | **TERM**: affects how errors are reported, not the document |
| `\afterassignment` | §265, §1268 | — | — | — | `K`: one token (saved as `after_token`) | — | — | **DEFERRED**: the token is inserted after the *next* assignment, whichever it is, even inside a later `\setbox`'s box (§1279 ff. in `box_end` for `\setbox`). Only one is kept: a second `\afterassignment` overwrites |
| `\aftergroup` | §265, §1271 | — | — | saves one token on the save stack to be inserted when the current group ends (§280 `save_for_after`) | `K`: one token | — | — | **DEFERRED**: re-emits text at the group's close, in order of saving; the group close is another node |
| `\advance`, `\multiply`, `\divide` | §265, §1235, §1236, §1238 | the register (`count[n]`, `dimen[n]`, `skip[n]`, `muskip[n]`, or an `int.p`/`dim.p`/`glue.p` named by its primitive) | the same register `L/G`, `V` (which register is decided by the next token: `\count n`, a `\countdef` shorthand, or a parameter name) | — | `P`: `<register>` optional `by` `<number or dimen or glue>` | — | — | an overflow is an error, not a value (§1236) |

### Parameters

Each parameter name is both a readable internal quantity (§413) and an
assignment command (§1224, §1228). The row applies to every listed name.

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| integer parameters: `\pretolerance` `\tolerance` `\linepenalty` `\hyphenpenalty` `\exhyphenpenalty` `\clubpenalty` `\widowpenalty` `\displaywidowpenalty` `\brokenpenalty` `\binoppenalty` `\relpenalty` `\predisplaypenalty` `\postdisplaypenalty` `\interlinepenalty` `\doublehyphendemerits` `\finalhyphendemerits` `\adjdemerits` `\mag` `\delimiterfactor` `\looseness` `\time` `\day` `\month` `\year` `\showboxbreadth` `\showboxdepth` `\hbadness` `\vbadness` `\pausing` `\tracingonline` `\tracingmacros` `\tracingstats` `\tracingparagraphs` `\tracingpages` `\tracingoutput` `\tracinglostchars` `\tracingcommands` `\tracingrestores` `\uchyph` `\outputpenalty` `\maxdeadcycles` `\hangafter` `\floatingpenalty` `\globaldefs` `\fam` `\escapechar` `\defaulthyphenchar` `\defaultskewchar` `\endlinechar` `\newlinechar` `\language` `\lefthyphenmin` `\righthyphenmin` `\holdinginserts` `\errorcontextlines` (55) | §238, §1228 | — | `int.p` `L/G`, `S` | — | `P`: `<optional =>` `<number>` | — | — | `\globaldefs` changes the *scope* of every later definition (§1214); `\endlinechar` and `\escapechar` change how *lines* are tokenised and how `\string` prints: both are read by every later claim and unfold. `\pausing` is **TERM** (§363). `\mag` is checked at the first `\shipout` and fixed thereafter (§288 `prepare_mag`). `\tracing*` and `\show*` write the log only. `\hangafter`, `\looseness` are reset by `normal_paragraph` §1070 at every `\par` — an implicit `L` definition by `\par` |
| dimension parameters: `\parindent` `\mathsurround` `\lineskiplimit` `\hsize` `\vsize` `\maxdepth` `\splitmaxdepth` `\boxmaxdepth` `\hfuzz` `\vfuzz` `\delimitershortfall` `\nulldelimiterspace` `\scriptspace` `\predisplaysize` `\displaywidth` `\displayindent` `\overfullrule` `\hangindent` `\hoffset` `\voffset` `\emergencystretch` (21) | §248, §1228 | — | `dim.p` `L/G`, `S` | — | `P`: `<optional =>` `<dimen>` | — | — | `\hangindent` is reset by `\par` (§1070); `\predisplaysize`, `\displaywidth`, `\displayindent` are defined *by* `$$` from **LIST** (§1145–§1146): the display start is a node that defines three fields from the current paragraph's lines |
| glue parameters: `\lineskip` `\baselineskip` `\parskip` `\abovedisplayskip` `\belowdisplayskip` `\abovedisplayshortskip` `\belowdisplayshortskip` `\leftskip` `\rightskip` `\topskip` `\splittopskip` `\tabskip` `\spaceskip` `\xspaceskip` `\parfillskip` (15); muglue: `\thinmuskip` `\medmuskip` `\thickmuskip` (3) | §226, §1228 | — | `glue.p`/`mu.p` `L/G`, `S` | — | `P`: `<optional =>` `<glue>` or `<muglue>` | — | — | `\tabskip` is read *during* `\halign` preamble scanning at each `&` (§778): the value in force at each column boundary |
| token parameters: `\output` `\everypar` `\everymath` `\everydisplay` `\everyhbox` `\everyvbox` `\everyjob` `\everycr` `\errhelp` (9) | §230, §1226 | `toks[n]` or `toks.q` when the rhs is `\toks n` or another token parameter (§1227) | `toks.p` `L/G`, `S` | — | `K`/`P`: `<optional =>` then either `<general text>` or one `\toks`/token-parameter token (§1226 skips blanks, `\relax`, and expands macros to find the brace: `P`) | — | — | `\output` is wrapped in braces (§1226) and *run* by the page builder (§1025): **DEFERRED**, **PAGE**. `\everypar` … are unfolded by `\par`-start, `$`, `\hbox{`, `\vbox{`, `\cr` (§1091, §1139, §1083, §799): those nodes read `toks.everyX` and unfold it. `\errhelp` is **TERM** |

### Registers

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\count`, `\dimen`, `\skip`, `\muskip` | §411, §1224, §1236 | as an operand (`\the\count n`, `\ifnum\count n…`): `count[n]`… | as a command: `count[n]`, `dimen[n]`, `skip[n]`, `muskip[n]` `L/G`, `V` (`n` a number) | — | `P`: `<8-bit number>` `<optional =>` `<number or dimen or glue or muglue>` | — | — | the same token is a read or a define depending on position: after `\the`/in a `<number>` it reads, at statement level it defines. `trap_zero_glue` §1229 normalises `0pt plus 0pt minus 0pt` |
| `\toks` | §265, §1226 | as operand: `toks[n]`; as command: the rhs `toks[m]`/`toks.q` if given by reference | `toks[n]` `L/G`, `V` | — | `P`: `<8-bit number>` `<optional =>` `<general text>` or one `\toks m`/token-parameter token | — | — | `\toks n=\toks m` copies by reference (§1227) |
| `\box`, `\copy` | §1071, §1079 | `box[n]` | `\box`: `box[n]` := void "at the same level" (§1079: the current binding is overwritten in place, no save-stack entry), `V`; `\copy`: none | — | `P`: `<8-bit number>` | — | appends the box to the current list (`box_end` §1075), or hands it to `\setbox`, `\shipout`, `\leaders`, `\raise`… (the `box_context`) | `\box` is a define-and-read whose scope is "the slot as currently visible" (§1079, e-TeX `change_box` pdfTeX §1079): not `L`, not `G`, mutation of the visible binding; **MODE**: a vbox in horizontal mode starts a paragraph (§1076), an hbox in vertical mode is a line, in math mode it is wrapped in a noad (§1076) |
| `\lastbox` | §1071, §1080 | **LIST** (the tail of the current list) | **LIST**: removes the tail box | — | `K`: none | — | removes rather than adds; result goes to the `box_context` | **LIST**; illegal in math mode and on the main vertical list (§1080) |
| `\vsplit` | §1071, §1082, §977 | `box[n]`, `dim.splitmaxdepth`, `glue.splittopskip`, **MARKS** (sets `split_first_mark`, `split_bot_mark` §979) | `box[n]` (remainder, in place as `\box`), **MARKS** `split_*_mark` | — | `P`: `<8-bit number>` `to` `<dimen>` | — | the split-off box goes to the `box_context` | **MARKS** written; `\vsplit` is a line-break-like computation over the box's vlist (pure given the box) |
| `\wd`, `\ht`, `\dp` as operands | §416, §420 | `box[n].wd/ht/dp` (void box: 0) | — | — | `P`: `<8-bit number>` | — | — | see the assignment row |

### Macros, expansion, `\csname`

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| macro call (not a primitive; the meaning `\def` left in `cs[name]`) | §389–§401 | `cs[name]`, `cat[*]` for the argument text; `cs[*]` of the argument's tokens only for `\outer` checks and for `\par` in a non-`\long` macro (§392) | — | — | `K`: the parameter text decides: undelimited `#n` takes one token or one balanced group, delimited `#n` takes up to the delimiter tokens (matching on *token identity*: cs name and char+catcode, §392–§397); braces stripped per §400. `Pd` if a delimiter or argument comes from expansion of an earlier operand — it does not: `macro_call` uses `get_token`, never expands, so the extent is `K` given catcodes | the body with `#n` replaced by the arguments | — | `\par` in an argument of a non-`\long` macro is an error (§396: `cs[par]` meaning compared, by *name* `par`, not meaning — `\let\endgraf\par` does not trigger it); `\outer` in an argument is an error |
| `\relax` | §265, §1045 | — | — | — | `K`: none | — | none | stops a `<number>`/`<glue>`/keyword scan; the meaning `\csname` gives an undefined name (§372). TeX inserts a frozen `\relax` itself (§379 `insert_relax`) when `\input` appears inside a file name (§378) and when `\or`/`\else`/`\fi` is met while the condition is still being scanned, e.g. `\ifnum1=1\fi` (§510) |
| `\expandafter` | §265, §368 | `cs[t2]` | — | — | `K`: two tokens; the second is expanded once | `t1` followed by the one-step expansion of `t2` (if `t2` is unexpandable, `t1 t2`) | — | the result's extent depends on `t2`'s meaning and its own claim: `t2 = \csname` claims to `\endcsname`, `t2 = \if…` claims to its `\fi` |
| `\noexpand` | §265, §367, §369 | — | — | — | `K`: one token | the token, marked `dont_expand` so the *next* reader sees it as `\relax` (frozen) if it was expandable (§358) | — | the mark lives in the input stack, one token deep (§369 `frozen_dont_expand`): a reader that is not the immediate next consumer sees the plain token. In `\edef` and `\write` it means "copy without expanding" (§478) |
| `\csname` … `\endcsname` | §265, §372 | `cs[*]` of every token between (expanded fully, §372), `cat[*]`; `cs[result]` | `cs[result]` := `\relax`, `L` (§372 `eq_define(cur_cs,relax,256)`: undone at group end), only if the name was undefined; `V`: the name is computed | — | `Pd`: everything up to the matching `\endcsname` after full expansion; the `\endcsname` may itself be produced by expansion | the control-sequence token for the computed name | — | a value-dependent *define* of a field whose *identity* is a value: the hazard for the def-use graph. Non-character tokens remaining after expansion are an error (§373) |
| `\endcsname` | §265, §1134 | — | — | — | `K`: none | — | — | alone (outside `\csname`) it is an error (`cs_error` §1135); it is the delimiter of `\csname`'s claim, found by meaning (§372 compares `cur_cmd=end_cs_name`, so `\let\x\endcsname` also ends it) |
| `\string` | §468, §470, §472 | `int.escapechar`; the one claimed token is taken unexpanded (§470 `get_token`) | — | — | `K`: one token (unexpanded) | the name of the token as characters of catcode 12 (a space gets 10, §464 `str_toks`), with `\escapechar` prepended for a cs | — | depends on `int.escapechar` at *this* point; an active character prints as itself |
| `\number` | §468, §471 | the `<number>`'s reads | — | — | `P`: `<number>` | the decimal digits as catcode 12 | — | — |
| `\romannumeral` | §468, §471 | the `<number>`'s reads | — | — | `P`: `<number>` | lowercase roman numeral as catcode 12 (empty for `<=0`) | — | — |
| `\meaning` | §468, §471, §296 | `cs[t]` or `cat` of a char token; `int.escapechar` | — | — | `K`: one token | the printed meaning (§296 `print_meaning`) as catcode-12 text | — | the printed form of a macro includes its parameter text and body, of a `\chardef` its value: a read of the whole meaning; of a primitive, its name |
| `\fontname` | §468, §471 | `font[f].name, area, size` | — | — | `P`: `<font>` | the file name, with ` at <size>` if scaled (§1261) | — | — |
| `\jobname` | §468, §471 | the job name (fixed by the first `\input`/first line, §532) | — | — | `K`: none | the job name | — | `job_name` is set by the *first* `\input` or `\openout`/`\jobname` (§532 `open_log_file`): a document-level constant fixed at the first use |
| `\the` | §265, §465, §413 | the internal quantity named by the next token(s): `int.p`, `dim.p`, `glue.p`, `mu.p`, `toks.p`, `count[n]`…, `box[n].wd`…, `font[f].*`, `cat[c]`…, `curfont`, `fam[s][k]`, `parshape` (length), `\lastpenalty`-family: **LIST**, `\pagegoal`-family: **PAGE**, `\spacefactor`/`\prevdepth`: **LIST**, `\inputlineno`: **INPUT**, `\badness`: last `hpack`/`vpack` result (§660, §676: **LIST**-adjacent global), `\deadcycles`/`\insertpenalties`: **PAGE** | — | — | `P`: one token naming the quantity, plus that quantity's own operand (`\count` `<8-bit number>`, `\fontdimen` `<number>` `<font>`, …) | the value as catcode-12 text (§465); for `\the\toks n`/`\the\everypar` the token list itself, unexpandable in `\edef` (§478) | — | the hazards of what it reads. `\the\font` gives the font's cs token (§465) |
| `\ignorespaces` | §265, §1045 | `cs[*]` of the tokens it skips (expands them) | — | — | `Pd`: all following tokens that expand to spaces (§406 `get_x_token` until non-blank) | — | none | claims *through* expansion |
| `\uppercase`, `\lowercase` | §1286, §1288 | `uc[c]`/`lc[c]` for each character token in the text; `cat[*]` for the braces | — | — | `P`: `<general text>` (`scan_toks(false,false)`) | the text with character codes replaced (catcodes kept), re-inserted (§1288 `back_list`) | — | re-emits text that is then *re-read*: the result's meaning depends on fields of the re-read |
| `\input` | §376, §378, §537 | the file (by name and search path), `cat[*]` for the name, `int.endlinechar` | **INPUT**: pushes a file level; sets `job_name` if unset (§532) | — | `P`: `<file name>` (ends at space or a non-letter/non-other token, §526; expands while scanning §526 — `Pd` if a macro is in the name) | the file's lines, tokenised with the catcodes *in force when each line is read* | — | **INPUT**: `\endinput` and `\inputlineno` refer to this level; the file's text is spliced *lazily*: the catcodes that tokenise line k are those after lines 1..k-1 have run. `name_in_progress` (§378) makes an `\input` inside a file name insert `\relax` instead |
| `\endinput` | §376, §378, §362 | **INPUT** | **INPUT**: `force_eof` | — | `K`: none | — | — | **INPUT**: stops the *current file level* after the current line (§362); its effect depends on which file the token came from, not where it is in the text |
| `\inputlineno` | §416, §424 | **INPUT** `line` | — | — | `K`: none | (operand) the line number of the innermost file | — | **INPUT** |
| `\topmark`, `\firstmark`, `\botmark`, `\splitfirstmark`, `\splitbotmark` | §384, §386 | **MARKS** `cur_mark[c]` | — | — | `K`: none | the mark text (possibly empty) | — | **MARKS**: written by `fire_up` §1012 (`\topmark` := previous `\botmark`, `\firstmark`/`\botmark` from the marks on the page being shipped) and `vsplit` §979: reading them is reading the page builder's result for the page containing this token's expansion — which page that is depends on everything before |

