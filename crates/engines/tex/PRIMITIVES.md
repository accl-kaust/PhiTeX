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

### Conditionals

A conditional is one node: it claims from the `\if…` token to its matching
`\fi` (`pass_text` §494 skips with `get_next`, *without* expansion, counting
`if_test` and `fi_or_else` by *meaning*: `cs[*]` of every cs in the skipped
text is read to find the end), evaluates its test, and unfolds the chosen
branch. The branch text is then read again by the following nodes, so the
unfold is "the text of the taken branch, unexpanded". The condition
stack (**IF**, §489) exists so that `\else`/`\fi` met *while reading the
taken branch* know which conditional they close (§510): in the node
model that is the conditional's own claim boundary, not state. `\or`,
`\else`, `\fi` are therefore markers, not nodes, except where they are
met early (§510 `insert_relax`, see `\relax`).

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\if` | §487, §506 | `cs[*]`/`cat` of the two compared tokens, after expansion (§506: `\noexpand`ed or undefined cs count as char 256, cat 0); `cs[*]` of the skipped text | — | — | `Pd`: two expanded tokens, then the branch text to `\fi` (branch boundaries `K` given meanings) | taken branch | — | the test needs *meanings* of the operands (expandable ones are expanded: `Pd`) |
| `\ifcat` | §487, §506 | as `\if` | — | — | as `\if` | taken branch | — | as `\if` |
| `\ifnum` | §487, §503 | the two `<number>`s' reads, the relation token | — | — | `P`: `<number>` `<` or `=` or `>` `<number>`, then branches | taken branch | — | — |
| `\ifdim` | §487, §503 | two `<dimen>`s | — | — | `P` | taken branch | — | — |
| `\ifodd` | §487, §504 | `<number>` | — | — | `P` | taken branch | — | — |
| `\ifvmode`, `\ifhmode`, `\ifmmode`, `\ifinner` | §487, §501 | **MODE** | — | — | `K`: branches | taken branch | — | **MODE**: the taken branch depends on the mode at this point, which is the result of everything before (`\par`, `$`, boxes) |
| `\ifvoid`, `\ifhbox`, `\ifvbox` | §487, §505 | `box[n]` (its type) | — | — | `P`: `<8-bit number>`, branches | taken branch | — | — |
| `\ifx` | §487, §507 | `cs[t1]`, `cs[t2]` (meanings compared, macros by text and prefixes) | — | — | `K`: two tokens (unexpanded), branches | taken branch | — | — |
| `\ifeof` | §487, §505 | `infile[n]` open or not | — | — | `P`: `<4-bit number>`, branches | taken branch | — | **INPUT**: a stream is "eof" also after `\read` has consumed its last line (§486) |
| `\iftrue`, `\iffalse` | §487, §505 | — | — | — | `K`: branches | the then-branch / the else-branch | — | — |
| `\ifcase` | §487, §509 | `<number>` | — | — | `P`: `<number>`, then branches separated by `\or` | the n-th branch (or the `\else` one) | — | — |
| `\fi`, `\or`, `\else` | §491, §510 | **IF** (`if_limit`) when met outside a conditional's claim | — | — | `K`: none | — | — | **IF**: unmatched ones are errors (§510); matched ones are consumed by the conditional's claim. Inside a `<number>` scan the `\fi` that ends the taken branch is met *during* scanning of the operand: the operand's claim crosses the branch boundary (that is why `\ifnum\count0=1 \fi` needs the space) |

### Grouping

Group codes are §269. Every group close runs `unsave` §281 (restores
`L` definitions) and inserts the `\aftergroup` tokens (§280).

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `{` (catcode 1, `left_brace`) | §1063 | — | — | opens `simple_group` (in math mode: `math_group` with a new noad list §1150) | `K`: none (the matching `}` is a separate node) | — | in math mode: a sub-formula noad | **MODE**: in math it also pushes a list (**LIST**) |
| `}` (catcode 2, `right_brace`) | §1068 | the group's code | undoes the group's `L` definitions | closes the current group; what it *does* depends on the group code: `simple_group` nothing; box groups package the list (§1085); `insert_group` builds an insert/adjust node (§1100); `output_group` ends the output routine (§1026); `disc_group` counts a discretionary part (§1119); `math_choice_group` (§1174); `math_group` finishes a noad (§1186); `semi_simple_group`, `math_shift_group`, `math_left_group`: errors (§1069) | `K`: none | `\aftergroup` tokens | the packaged box, insert, adjust, noad … to the enclosing list | **DEFERRED** (`\aftergroup`), **LIST** (packaging the list built inside), **PAGE** (`\output`'s end puts `\box255` back: §1026 `build_page`) |
| `\begingroup` | §265, §1063 | — | — | opens `semi_simple_group` | `K`: none | — | — | — |
| `\endgroup` | §265, §1063 | — | undoes `L` | closes `semi_simple_group` (error if the current group is another kind, §1065 `off_save` inserts what is missing) | `K`: none | `\aftergroup` tokens | — | **DEFERRED** |
| `\aftergroup` | see assignments | | | | | | | |

### Boxes

Box constructors open a group whose close packages the list built
inside (§1083–§1087). `box_end` §1075 decides what happens to the result
by the *context*: append (then **MODE** matters), `\setbox`, `\shipout`,
`\leaders`, `\raise`/`\lower`/`\moveleft`/`\moveright`.

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\hbox` | §1071, §1083, §1085 | `toks.everyhbox` (unfolded after the `{`); `hpack` §649: `font[*]` of its content, `dim.hfuzz`, `int.hbadness`, `dim.overfullrule` | — | opens `hbox_group` (`adjusted_hbox_group` in vertical mode, §1083: the `\vadjust`/`\insert` material migrates) | `P`: `<box specification>` (`to <dimen>` or `spread <dimen>`, §645) then `{`; the `{` may come from expansion (§403); the content is claimed by the group, not by `\hbox` | `toks.everyhbox` after the brace | the packaged hbox, per `box_end`: in vertical mode a line (plus baselineskip glue: reads **LIST** `prev_depth`), in horizontal mode a box (sets `space_factor:=1000`), in math a noad (§1076) | **MODE** (which group, what `box_end` does), **LIST** (`prev_depth`, `space_factor`), the list inside is **LIST** for its own content |
| `\vbox`, `\vtop` | §1071, §1083, §1086 | `toks.everyvbox`, `dim.boxmaxdepth` (§1086 `vpackage`), `int.vbadness`, `dim.vfuzz`; `\vtop` recomputes height/depth (§1087) | — | opens `vbox_group`/`vtop_group`; inside, `normal_paragraph` §1070 resets `\looseness`, `\hangindent`, `\hangafter`, `\parshape` (`L` definitions inside the group) | as `\hbox` | `toks.everyvbox` | the vbox per `box_end` §1076: in vertical mode `append_to_vlist` (interline glue from **LIST** `prev_depth`) then `build_page`; in horizontal mode a box with `space_factor:=1000`; in math a noad | **MODE**, **LIST** (`prev_depth:=ignore_depth` inside, §1083) |
| `\box`, `\copy`, `\lastbox`, `\vsplit` | see registers | | | | | | | |
| `\setbox` | see assignments | | | | | | | |
| `\shipout` | §1071, §1073, §1075, §638 | the box; `int.mag`, `dim.hoffset`, `dim.voffset`, `count[0..9]` (printed as the page number, §638), `int.tracingoutput`, `font[*]` of everything in the box; `\write`/`\special`/`\openout`/`\closeout` whatsits inside are *executed now* (§1366–§1374: `\write` expands its text at shipout) | `pdf.*`/DVI output position; `stream[n]` via whatsits; `total_pages` | — | `P`: `<box>` (the claim of the following box constructor) | — | the page to the DVI/PDF; whatsit side effects to streams | **DEFERRED**: `\write` text is expanded with the fields in force *at shipout*, not where written (§1370 `write_out`: "expand the token list"), and `\openout`/`\closeout` happen in page order (§1374). Can be used in any mode, inside a box being built. `dead_cycles:=0` (**PAGE**) |
| `\leaders`, `\cleaders`, `\xleaders` | §1071, §1073, §1078 | — | — | — | `P`: `<box or rule>` then `<glue>` (`\hskip`/`\vskip`/`\hfil`… §1078: the glue command decides which) | — | a glue node carrying the box/rule as leaders | **MODE**: the glue must be vertical in vmode, horizontal in hmode (§1078, §1094 `head_for_vmode` on mismatch: in hmode `\vskip`-leaders end the paragraph) |
| `\moveleft`, `\moveright` | §1071, §1073 | — | — | — | `P`: `<dimen>` `<box>` | — | the box shifted, vertical mode only (§1073 `vmode+vmove`) | **MODE**: error in other modes (§1048) |
| `\raise`, `\lower` | §1071, §1073 | — | — | — | `P`: `<dimen>` `<box>` | — | the box shifted, horizontal or math mode (§1073) | **MODE** |
| `\hrule` | §265, §1056 | — | — | — | `P`: `<rule specification>`: any order of `width`/`height`/`depth` `<dimen>` (§463), ends at the first non-keyword (expands to find keywords: `Pd` strictly) | — | a rule node, vertical mode only; in horizontal mode `head_for_vmode` (§1094): ends the paragraph first | **MODE**: `\hrule` in a paragraph is "`\par` then `\hrule`" (§1095: the `\hrule` token is re-read after `\par`) |
| `\vrule` | §265, §1056 | — | — | — | as `\hrule` | — | a rule node in horizontal or math mode (`space_factor:=1000` §1056: **LIST**); in vertical mode starts a paragraph (§1090) | **MODE**, **LIST** |
| `\unhbox`, `\unhcopy` | §1107, §1110 | `box[n]` (must be an hbox or void) | `\unhbox`: `box[n]` := void at the same level (§1110) | — | `P`: `<8-bit number>` | — | the box's *list* spliced into the current list (horizontal/math mode; in vertical mode starts a paragraph §1090); `space_factor` untouched | **LIST**: splicing a list, the appended nodes keep their glue/penalty identity (`\unskip` can then remove the last). In math mode the list must be an hlist (§1110) |
| `\unvbox`, `\unvcopy` | §1107, §1110 | `box[n]` (vbox or void) | `\unvbox`: `box[n]` := void | — | `P`: `<8-bit number>` | — | the vlist spliced into the current vertical list; in horizontal mode `head_for_vmode` ends the paragraph (§1094); in math mode an error (§1110) | **MODE**, **LIST**; on the main vertical list the spliced nodes reach the page builder (§1110 `build_page` is *not* called here; it is called by the next `\par`/box: **PAGE** timing) |

### Glue, kerns, penalties, list surgery

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\hskip`, `\hfil`, `\hfill`, `\hss`, `\hfilneg` | §1058, §1060 | `\hskip`: the `<glue>`'s reads (`skip[n]`, `glue.p` if given by name) | — | — | `\hskip`: `P` `<glue>`; the others `K`: none | — | a glue node, horizontal or math mode; in vertical mode starts a paragraph (§1090) | **MODE** |
| `\vskip`, `\vfil`, `\vfill`, `\vss`, `\vfilneg` | §1058, §1060 | as above | — | — | as above | — | a glue node, vertical mode; in horizontal mode `head_for_vmode` (§1094): ends the paragraph, then the glue goes to the vertical list; error in math (§1046) | **MODE** |
| `\mskip` | §1058, §1060 | `<muglue>` reads (`muskip[n]`, `mu.p`) | — | — | `P`: `<muglue>` | — | a glue node (`mu_glue` subtype), math mode only | **MODE**; converted to real glue in `mlist_to_hlist` using `fam[cur_size][2].fontdimen[6]` (§716): a read made at formula end |
| `\kern` | §1058, §1061 | `<dimen>` | — | — | `P`: `<dimen>` | — | a kern node (`explicit`), any mode | — |
| `\mkern` | §1058, §1061 | `<mudimen>` | — | — | `P`: `<mudimen>` | — | a `mu_glue` kern, math mode only | **MODE** |
| `\penalty` | §265, §1102, §1103 | `<number>` | — | — | `P`: `<number>` | — | a penalty node; on the main vertical list it runs the page builder (§1103 `build_page`) | **PAGE**: `\penalty` in vertical mode triggers `build_page`, which may fire `\output` — the node's *side effect* is the entire page builder step |
| `\unpenalty`, `\unkern`, `\unskip` | §1107, §1104, §1105 | **LIST** (tail type) | **LIST**: removes the tail if it is of that type | — | `K`: none | — | removal | **LIST**; on the main vertical list with an empty contribution list it is an error ("can't take things from the current page", §1106) — the main vlist's tail has already gone to **PAGE** |
| `\lastpenalty`, `\lastkern`, `\lastskip` | §416, §424 | **LIST** tail (0 or `zero_glue` if the tail is not of that type; on an empty main vlist `last_glue`/`last_penalty`/`last_kern` kept by the page builder §991, **PAGE**) | — | — | `K`: none | (operand) | — | **LIST**, **PAGE** |
| `\badness` | §416, §424 | `last_badness` set by the last `hpack`/`vpack` (§660, §667, §675, §678) | — | — | `K`: none | (operand) | — | a global written by every box packaging, including those the page builder and `\vsplit` do internally: not attributable to a field |
| `\indent`, `\noindent` | §1088, §1091, §1093 | `dim.parindent`, `toks.everypar`, **MODE** | **LIST**: in vertical mode starts a paragraph: pushes a horizontal list with `prev_graf:=0`, `space_factor:=1000`, `\language` noted (§1091); `int.language` read; `glue.parskip` appended to the vlist if not at the top (§1091) | — | `K`: none | `toks.everypar` | an indent box (`\indent`) in horizontal mode; in math an empty box of `\parindent` width | **MODE**, **LIST**, **PAGE** (`build_page` runs when the `\parskip` is appended §1091) |
| `\par` | §334, §1094, §1096, §1070 | **MODE**; in horizontal mode: everything `line_break` §815 reads: `dim.hsize`, `parshape`, `dim.hangindent`, `int.hangafter`, `int.pretolerance`, `int.tolerance`, `glue.leftskip`, `glue.rightskip`, `glue.parfillskip`, `int.looseness`, `dim.emergencystretch`, penalties and demerits, `int.adjdemerits`, `int.*hyphenmin`, `int.uchyph`, `hyph` for the languages of the words, `lc[*]`, `font[*]` of the characters, `int.interlinepenalty`, `int.clubpenalty`, `int.widowpenalty`, `int.brokenpenalty`, `glue.baselineskip`, `glue.lineskip`, `dim.lineskiplimit` (when the lines are appended, §679), `int.tracingparagraphs`; `\prevgraf` of the enclosing level (display math) | `normal_paragraph` §1070: `int.looseness:=0`, `dim.hangindent:=0`, `int.hangafter:=1`, `parshape:=null`, `L` (through `eq_word_define`: local to the current group); **LIST** `prev_graf`, `prev_depth` | — | `K`: none | — | in horizontal mode: the paragraph's lines (hboxes, interline glue, penalties, migrated inserts/adjusts/marks) appended to the enclosing vertical list; then `build_page` (§1096) | **LIST**: consumes the whole current horizontal list; **MODE**: in vertical mode just `normal_paragraph`; in math mode an error that ends the formula (§1047 `insert_dollar_sign`, §1046); **PAGE**: runs the page builder. The macro `\par` is looked up by *name* in `macro_call` §396 and `\outer` checks (§339), and the `\par` *token* is inserted by TeX at blank lines (§347) and by `\vskip` in hmode (§1095) — those inserted tokens take the *current meaning* of `cs[par]` |
| `\noboundary` | §265, §1045, §1030 | — | — | — | `K`: none | — | nothing; suppresses the boundary-character ligature/kern of the adjacent word (§1038–§1039: it is a `no_boundary` command that `main_control` sees between characters) | an effect on the *neighbouring* character nodes' ligature building: the character node after it reads this |
| `\accent` | §265, §1122, §1123 | `curfont`, `font[f].*` (accent and accentee metrics, `fontdimen[1]` slant, `fontdimen[5]` x-height) | — | — | `P`: `<8-bit number>` then optional font assignments (`do_assignments` §1270) then one character (or `\char`) | — | an hbox/kern/char combination in horizontal mode (§1123–§1125); `space_factor:=1000`; in vertical mode starts a paragraph; in math mode `math_ac` (§1165) | **MODE**; `set_box_allowed:=false` during the assignments (§1123) |
| `\/` | §265, §1112, §1113 | **LIST** tail (a char or ligature), `font[f].italic_correction[c]`, in math mode nothing | — | — | `K`: none | — | a kern node (`explicit`) of the italic correction of the tail character; `space_factor:=1000` | **LIST**: reads the previous node |
| `\ ` (control space) | §265, §1030, §1041 | `curfont`, `glue.spaceskip`, `font[f].fontdimen[2,3,4]` | — | — | `K`: none | — | a glue node (`\spaceskip` or the font's space, `space_factor` ignored §1041); in vertical mode starts a paragraph | **MODE** |
| `\-` | §1114, §1117 | `curfont`, `font[f].hyphenchar` | — | — | `K`: none | — | a discretionary node with the hyphen char as pre-break (`\hyphenchar` of `curfont`, §1117); horizontal or math mode | **MODE** |
| `\discretionary` | §1114, §1117–§1121 | `cat[*]` for the three brace groups | — | opens `disc_group` three times (one per part), each closed by `}` (§1119); `space_factor:=1000` inside (§1117 `push_nest`) | `K`: three `{…}` groups; `P` because each `{` is found by `scan_left_brace` §403 (expands) | — | a discretionary node (pre-break, post-break, replace count) | **LIST**: the parts are built as lists; replace text longer than 255 or containing non-char/box/rule/kern nodes is an error (§1121) |
| `\insert` | §265, §1097, §1099, §1100 | `<8-bit number>`; at close: `dimen[n]`, `skip[n]`, `count[n]`, `dim.splitmaxdepth`, `int.floatingpenalty`, **LIST** `prev_depth` (`ignore_depth` inside) | — | opens `insert_group` (§1099), `normal_paragraph` resets inside; close builds the insert node (§1100) | `P`: `<8-bit number>` `{`; 255 is an error | — | an `ins_node` to the current list (any mode; in a paragraph it migrates to the enclosing vlist at line break, §655) | **MODE**-neutral, but the node's reading of `count[n]`, `skip[n]`, `dimen[n]` happens in the *page builder* (§1008–§1010), with the values in force when the page is built |
| `\vadjust` | §265, §1097, §1099, §1100 | — | — | opens `insert_group` with box 255 (`\vadjust` = `\insert255`-like, §1099) | `K`: `{` (`P`) | — | an `adjust_node` in horizontal/math mode; vertical mode: error (§1098) | **MODE** |
| `\mark` | §265, §1097, §1101 | the text's expansion (`scan_toks(false,true)`: `cs[*]`, registers via `\the`…) | **MARKS** (later, when the page builder/`\vsplit` meets the node) | — | `P`: `<general text>` (expanded) | — | a `mark_node` to the current list (any mode); migrates out of paragraph lines (§889) and out of an `\hbox` built in vertical mode (§1085, §655 `adjust_tail`) | **MARKS** |

### Math

Math mode is entered by `$` (§1137 `init_math`) and left by `$`
(§1193–§1200 `after_math`), which converts the noad list to an hlist
(`mlist_to_hlist` §726) reading `fam[size][k]`, `font[*]` of those,
`int.delimiterfactor`, `dim.delimitershortfall`, `dim.nulldelimiterspace`,
`dim.scriptspace`, `mu.thinmuskip`/`medmuskip`/`thickmuskip`,
`int.binoppenalty`, `int.relpenalty`, `dim.mathsurround`; a display also
reads `dim.displaywidth`, `dim.displayindent`, `dim.predisplaysize`
(defined by `$$` from the paragraph so far, §1145–§1146), the display
skips, `int.predisplaypenalty`, `int.postdisplaypenalty`, and `\prevgraf`.
Everything below is "math mode only" unless said otherwise (`\accent`
has its math meaning here too, §1164).

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `$` (catcode 3) | §1137–§1139, §1193–§1200 | `toks.everymath`/`toks.everydisplay`, `cat` of the next token (`$$` is two `$`: the second is peeked §1138), **LIST** (a display takes the paragraph so far: `line_break(display_widow_penalty)` §1145 runs the paragraph builder *now*), **MODE** | `dim.predisplaysize`, `dim.displaywidth`, `dim.displayindent` `L` inside the display (§1145–§1146 via `eq_word_define`); at the end `\prevgraf` += 3 (§1199) | opens `math_shift_group` (§1136 `push_math`), closed by the matching `$` (§1194) | the formula's text is claimed by the group | `toks.everymath` or `toks.everydisplay` | at close: an inline formula as math nodes + hlist (`\mathsurround` kerns), or a display: the paragraph's lines so far, penalties, display skips, the centred box, and the paragraph *continues* after (§1200 `resume_after_display`) | **MODE**, **LIST**: a display splits the paragraph in two at this point: the preceding text is line-broken here, so `$$` is a `\par`-like consumer of **LIST**; in vertical mode `$` starts a paragraph (§1090) |
| `\eqno`, `\leqno` | §1141, §1140, §1142 | **MODE** (`privileged` §1051: display math only, not inner) | — | opens a new `math_shift_group` for the equation number (§1142) | the number's text to the closing `$$` | — | the equation number box placed by `after_math` (§1204) | **MODE** |
| `\mathchar` | §265, §1154 | `<15-bit number>`; later `fam[size][k]` | — | — | `P` | — | a noad with that math char | — |
| `\char` in math | §1154 | `<8-bit number>`, `mathcode[c]`, `int.fam` | — | — | `P` | — | a noad per `mathcode[c]` (§1155: `"8000` makes it active — re-read as an active character) | `mathcode "8000` turns a value into a *token*: the claim becomes an unfold of the active character `cs[c]` |
| `\delimiter` | §265, §1154, §1160 | `<27-bit number>` | — | — | `P` | — | a noad with a delimiter field (§1160) | — |
| `\mathord`, `\mathop`, `\mathbin`, `\mathrel`, `\mathopen`, `\mathclose`, `\mathpunct`, `\mathinner`, `\underline`, `\overline` | §1156, §1158, §1151 | `cat`; the `<math field>` (§1151 `scan_math`: one char with `mathcode`, a `\mathchar`, or a `{` group) | — | `{`-fields open `math_group` (§1153) | `P`: one `<math field>`; a brace group's extent is claimed by the group | — | a noad of that class whose nucleus is the field | — |
| `\mathaccent` | §265, §1164, §1165 | `<15-bit number>`, then a `<math field>` | — | as above | `P` | — | an `accent_noad` | — |
| `\radical` | §265, §1162, §1163 | `<27-bit number>` (delimiter), then a `<math field>` | — | as above | `P` | — | a `radical_noad` | — |
| `\mathchoice` | §265, §1171, §1172–§1174 | `cat[*]` | — | opens `math_choice_group` four times (§1172, §1174 counts via `saved(-1)`) | `P`: four `{…}` groups | — | a `choice_node`; the style used is decided in `mlist_to_hlist` (§731) | the group close counts which of the four it is: group-close behaviour depends on a saved counter |
| `\displaylimits`, `\limits`, `\nolimits` | §1156, §1158, §1159 | **LIST** tail (must be an `op_noad`) | **LIST**: sets the tail's subtype | — | `K`: none | — | modifies the preceding noad | **LIST** |
| `\nonscript` | §265, §1171 | — | — | — | `K`: none | — | a zero glue with `cond_math_glue` subtype; removed in script styles by `mlist_to_hlist` (§732) | — |
| `\displaystyle`, `\textstyle`, `\scriptstyle`, `\scriptscriptstyle` | §1169, §1171 | — | — | — | `K`: none | — | a `style_node` | — |
| `\above`, `\over`, `\atop`, `\abovewithdelims`, `\overwithdelims`, `\atopwithdelims` | §1178, §1180, §1181 | `<dimen>` (`\above*`), two `<27-bit number>`s (`*withdelims`); **LIST**: the *whole current noad list* becomes the numerator (§1181 `incompleat_noad`) | **LIST**: the list so far is moved into the fraction's numerator | — | `P`: operands; the denominator is the rest of the list up to the group end | — | a `fraction_noad` holding the list before it; the list after it is the denominator (§1185 `fin_mlist`) | **LIST**: an infix operator over the entire enclosing math list: a second `\over` in the same list is an error (§1183) |
| `\left`, `\right` | §1188, §1190, §1191 | `<27-bit number>`; **MODE**/group (`\right` needs `math_left_group` §1192) | — | `\left` opens `math_left_group` (§1191 `push_math`); `\right` closes it and makes an `inner_noad` of the list (§1191, §1185) | `P`: `<delimiter>`; `\left`'s inner text claimed by the group | — | an `inner_noad` wrapping `\left … \right` | **LIST**; `\right` without `\left` is an error (`off_save`) |
| `\vcenter` | §265, §1167, §1168 | `<box specification>`; the close `vpack`s (§1168) | — | opens `vcenter_group` (§1167), `normal_paragraph` inside, `prev_depth:=ignore_depth` | `P`: `<box specification>` `{` | — | a `vcenter_noad` (§1168) | — |
| `^`, `_` (catcodes 7, 8) | §1175–§1177 | **LIST** tail (the noad to attach to; a new empty one if none), the `<math field>` | **LIST** | `{`-fields open `math_group` | `P`: one `<math field>` | — | sets the tail noad's `supscr`/`subscr` (§1176; double superscript is an error) | **LIST** |
| `\fam` | parameters | `int.fam` is read by every math character whose `mathcode` has family 7 (§1155 `cur_fam`) | | | | | | an integer parameter whose *read* is distributed over every math character of class 7 |

### Alignments

`\halign`/`\valign` (`init_align` §774) claim a preamble up to the first
`\cr`, then rows. The body is *not* claimed as text: TeX reads the body
token by token, and at every `&`/`\cr`/`\span` (recognised by `align_state`
= 0, i.e. at brace depth 0 *of the alignment*, §342, §789) inserts the
template's `u`/`v` parts into the input (§788–§791 `insert_the_v_template`
via `endv`). The column boundaries are therefore decided by *tokens of
catcode 4 and `\cr` at alignment brace depth 0*, found while reading the
body with expansion (an `&` can come from a macro). The whole alignment
is one node family that cannot be split per cell on text alone.

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\halign` | §265, §1130, §774–§812 | `<box specification>`, `glue.tabskip` *at each `&` of the preamble* (§778 `cur_loop`; `\tabskip` assignments inside the preamble take effect per column), `toks.everycr` (§799 at every `\cr`, and once after the preamble §774), `cat[*]`, `cs[*]` (the preamble is read *unexpanded* except `\span` and `\tabskip`-assignments §782, §783); **MODE** (vertical or display math §1130; in horizontal mode `head_for_vmode` ends the paragraph); for the body each cell is an hbox: `hpack` reads | — | opens `align_group` (§774), each cell is its own `align_group` with `u`/`v` templates (§787, §791); `\noalign` opens `no_align_group`; the preamble's `\tabskip` assignments are `L/G` *definitions during the claim* (§783 `do_assignments`-like via `scan_glue` and `eq_define`) | `Pd`: `<box specification>` `{` `<preamble>` `\cr` then `<rows>` `}`; the preamble ends at the first `\cr` (`K` given meanings: `\cr` is matched by meaning `car_ret` §779, `\span` by meaning); rows end at `}` at `align_state=0` (§1131: body braces are balanced); the cell boundaries (`&`, `\cr`, `\span`) are found by *meaning and align_state* while reading with expansion (§342: `&` from a macro counts; `\omit` and `\noalign` are looked for by *meaning* after expansion §785 `align_peek`) | at each cell start: the template's `u` part (unless `\omit`), at each cell end: the `v` part (§789 `insert_the_v_template`); `toks.everycr` at every `\cr` | the finished alignment: rows of unset boxes becoming hboxes (`\halign`) or vboxes (`\valign`) with tabskip glue between, appended to the enclosing list (§812 `fin_align`); a display `\halign` also gets the display penalties and skips (§1206) | **ALIGN**: `align_state`, `cur_align`, the template lists, the unset-node widths (`fin_col` §796 accumulates per-column maxima across rows: the column width of row 1 depends on row n: a fixpoint over the whole alignment); **MODE**; `\cr` in the body is recognised only when `align_state=0`, so a `\cr` inside a cell's brace group does not count, but one from a macro at depth 0 does; `\span` in the body merges cells (§789–§791 reads the next template parts). The `u`/`v` templates are *re-read* in each cell: a template with `\global\advance\count0` runs once per cell. A `\noalign` body is a vertical (or horizontal for `\valign`) list between rows (§785, §1133) |
| `\valign` | §265, §1130 | as `\halign` with hlists and vlists swapped; horizontal mode only (in vertical mode starts a paragraph §1090) | | | | | columns as vboxes | **ALIGN**, **MODE** |
| `\noalign` | §265, §785, §1133 | **ALIGN** (only legal right after `\cr`, §785 `align_peek`) | — | opens `no_align_group` (§785), closed by `}` (§1133) | `K`: `{` `<balanced text>` `}` | — | its list (vertical for `\halign`) goes between rows | **ALIGN**: elsewhere an error ("Misplaced \noalign", §1129) |
| `\omit` | §265, §789, §1129 | **ALIGN** (legal only at a cell start, §789 `align_peek`/§785) | — | — | `K`: none | suppresses the `u` template of this cell (the `v` part too, §789 uses `omit_template`) | — | **ALIGN** |
| `\span` | §780, §782, §789, §791 | in the preamble: `cs[next]` — expands the next token once (§782); in the body: **ALIGN** | — | — | `K`: none itself | in the preamble: the expansion of the next token; in the body: the cell continues into the next column (§791 `init_span`) | — | **ALIGN** |
| `\cr`, `\crcr` | §780, §785, §791, §799 | **ALIGN**; `toks.everycr` | — | ends the cell's `align_group` and the row (§799); `\crcr` is ignored right after `\cr`/`\noalign` (§785) | `K`: none | `toks.everycr` | the row (an unset box) | **ALIGN**: outside an alignment an error (§1128 "Misplaced \cr") |
| `&` (catcode 4) and `\span` in the body | §789, §1128 | **ALIGN** (`align_state=0`) | — | ends a cell (`align_group`) and starts the next | `K`: none | the `v` template, then the next `u` template | an unset box for the cell (§796 `fin_col`) | **ALIGN**: outside an alignment or at brace depth not 0 it is an error/ordinary character (§1128); `\tabskip` glue *between* cells uses the preamble's values (§795) |
| `\endtemplate` (`frozen_endv`, internal, §375, §1131) | | | | | | | | the token TeX inserts to end a `v` template; not a user primitive (`\outer`-like, §375); mentioned because its meaning is looked up where `\outer` is forbidden |

### Paragraph and page builders

The paragraph builder is `\par`'s (and `$$`'s) contribution, see above.
The page builder (`build_page` §994) runs after every box, glue,
penalty, insert, mark, kern, whatsit appended to the *main* vertical list
(§1058–§1103 call it), moves nodes from the contribution list to the
page, and when a break is chosen (`fire_up` §1012) sets `\box255`,
`\outputpenalty`, the marks, the insert boxes `box[n]`, `\insertpenalties`,
and runs `\output` (`toks.output`) in an `output_group` with
`mode:=-vmode` (§1025). None of its inputs is a field of the node that
triggers it; see **Findings**.

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\output` (the routine, when run) | §1025–§1028, §230 | `toks.output`, **PAGE**: `box[255]` (`G` set by `fire_up` §1017), `int.outputpenalty` (`G` §1013), **MARKS**, `box[n]` of inserts (§1018–§1021), `int.holdinginserts`, `int.maxdeadcycles`, `int.vsize`, `dim.maxdepth`, `glue.topskip` | **PAGE**: `box[255]` must be void at the end (§1028); `dead_cycles` incremented (§1025); `\insertpenalties` := number of held-over inserts | the routine runs in `output_group` (§1025), closed by its `}` (§1026) | the braces around the routine (§1226) | `toks.output` is read as input (§1025 `begin_token_list(output_routine,output_text)`) | whatever the routine ships out; the leftover contribution list and held inserts return to the contribution list (§1026) | **DEFERRED**, **PAGE**, **MARKS**, **MODE** (`-vmode`): the output routine is the clearest case of a text whose reads bind to definitions from an unknown point in the document (the page break) |
| `\pagegoal` … `\pagedepth` as operands | §983, §421 | **PAGE** `page_so_far` (`\pagegoal` is `max_dimen` while the page is empty, §421) | — | — | `K`: none | (operand) | — | **PAGE** |
| `\deadcycles`, `\insertpenalties` as operands | §416, §419 | **PAGE** | — | — | `K` | (operand) | — | **PAGE**: `\insertpenalties` is the running sum of insert penalties during page building, the held-over count inside `\output` (§419, §1014) |
| `\holdinginserts`, `\vsize`, `\maxdepth`, `\topskip`, `\maxdeadcycles`, `\outputpenalty` | parameters | read by the page builder at the moment it runs, not at the node that set them | | | | | | the *time of reading* is the page builder's, not any text position |

### Output streams, files, terminal, interaction

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\openout` | §1344, §1351, §1374 | `<4-bit number>`, `<file name>` (scanned now, §1351); at shipout opens the file; `job_name` | `stream[n]` (at shipout, or now with `\immediate`) | — | `P`: `<4-bit number>` `<optional =>` `<file name>` | — | an `open_node` whatsit (any mode) | **DEFERRED**: happens when the page is shipped (§1374), in page order; **MODE**-neutral but as a whatsit in a paragraph it is a node of the hlist and goes with its line |
| `\write` | §1344, §1352, §1370 | `<number>`; the text is stored *unexpanded* (§1352 `scan_toks(false,false)`) and expanded at shipout (§1371: `cs[*]`, registers, `\the`… as bound *then*); `int.newlinechar`; `stream[n]` open or not (closed or out of range: the log/terminal §1370) | nothing (appends to a stream) | — | `P`: `<number>` `<general text>` | — | a `write_node` whatsit; at shipout, one line to `stream[n]` | **DEFERRED**: the archetype of a text whose reads bind at shipout time, after the page break is known. The expansion at shipout happens in a group-less context with `\outer` checks off (§1371) and `\noexpand` honoured. `\write` to a negative stream goes to the log; 16+ to terminal and log |
| `\closeout` | §1344, §1353, §1374 | `<4-bit number>` | `stream[n]` closed (at shipout) | — | `P` | — | a `close_node` whatsit | **DEFERRED** |
| `\immediate` | §1344, §1375 | — | — | — | `K`: one following `\openout`/`\write`/`\closeout` (anything else: nothing, the token is re-read §1375) | — | performs the stream operation *now* instead of appending a whatsit; `\write`'s text is expanded now | the only way a `\write` binds its reads at the text position |
| `\special` | §1344, §1354, §1368 | the text expanded *now* (`scan_toks(false,true)`) | — | — | `P`: `<general text>` | — | a `special_node` whatsit, copied into the DVI/PDF at shipout (§1368; pdfTeX §1615 writes it as a PDF literal of the "page" kind unless it starts with `pdf:` keywords) | its *position* on the page is the shipout's business; the text is fixed here |
| `\setlanguage` | §1344, §1377, §1376 | `<number>`, `int.lefthyphenmin`, `int.righthyphenmin`, **MODE** (horizontal only, §1377) | — | — | `P`: `<number>` | — | a `language_node` whatsit (§1376) read by the hyphenator for the words after it (§891 `cur_lang`) | **MODE**; a character node appended when `\language` differs from the last `language_node` makes TeX insert a `language_node` itself (§1034 `fix_language`): an implicit whatsit between characters |
| `\openin` | §1272, §1275 | `<4-bit number>`, `<file name>`, the file system | `infile[n]` (opened at position 0) `G` (not undone by groups) | — | `P`: `<4-bit number>` `<optional =>` `<file name>` | — | — | **INPUT**: a stream is global mutable state; a missing file leaves it closed and `\ifeof` true |
| `\closein` | §1272, §1275 | — | `infile[n]` closed | — | `P`: `<4-bit number>` | — | — | **INPUT** |
| `\message`, `\errmessage` | §1277, §1279 | the text expanded now; `toks.errhelp` (`\errmessage`), `int.newlinechar`, `interaction` | — | — | `P`: `<general text>` | — | text to the terminal and log | **TERM**: `\errmessage` is an error with recovery (§1283); neither affects the PDF |
| `\show`, `\showbox`, `\showthe`, `\showlists` | §1291, §1293–§1298 | `\show`: one token's meaning (`cs[t]`); `\showbox`: `box[n]`, `int.showboxbreadth`, `int.showboxdepth`; `\showthe`: as `\the`; `\showlists`: **LIST** (all nesting levels, §218) and **PAGE** (§986) | — | — | `\show`: `K` one token; `\showbox`: `P` `<8-bit number>`; `\showthe`: as `\the`; `\showlists`: `K` none | — | log/terminal output; in `errorstopmode` an interaction (§1293 `error`) | **TERM**, **LIST**, **PAGE** (`\showlists` prints the page builder's state) |
| `\end` | §1052, §1054 | **PAGE** (`page_head=page_tail`, `dead_cycles`), **LIST** (`head=tail`), `int.maxdeadcycles`, `box[255]` | **PAGE** | — | `K`: none | in vertical mode with a non-empty page or list: appends `\hbox{}\vfill\penalty-2^30` and runs the page builder until the page is empty, then ends | the final pages | **PAGE**, **MODE** (horizontal mode: `head_for_vmode` ends the paragraph first §1094); `its_all_over` §1054 loops: `\end` may run `\output` several times |
| `\dump` | §1052, §1335, §1302 | everything (the whole eqtb, fonts, hyphenation) | the format file | — | `K`: none | — | — | INITEX only (§1335), outside a group only (§1304); not part of a document build: the format is `\input`-like preloaded state |

## e-TeX

Everything here exists only in extended mode (`eTeX_mode`, entered by
the `*` on the first line or a format dumped in it; pdfTeX §1649 ff.).
e-TeX also widens the register families to 0..32767 (sparse arrays,
pdfTeX §1809–§1841: `count[n]`… with `n>255` are the same fields as
far as this catalogue is concerned, `L/G` through `sa_def`/`gsa_def`
pdfTeX §1839–§1840) and adds mark *classes* (`\marks n`).

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| `\eTeXversion`, `\eTeXrevision` | pdfTeX §1649, §1651 | constants | — | — | `K`: none | (`\eTeXrevision` is a convert: the text `.6`) | — | — |
| `\everyeof` | pdfTeX §1657, §384 | — | `toks.everyeof` `L/G`, `S` | — | as a token parameter | — | — | unfolded by the *end of every file or pseudo-file* (§384, unless `\endinput` forced it or the file is `\scantokens`' of `\edef`…): the file-end node reads `toks.everyeof` |
| `\tracingassigns`, `\tracinggroups`, `\tracingifs`, `\tracingscantokens`, `\tracingnesting` | pdfTeX §1657 | — | `int.p` `L/G`, `S` | — | `P` | — | — | log only |
| `\predisplaydirection` | pdfTeX §1657, §254 | — | `int.p` `L/G`; defined by `$$` from the paragraph's LR state (**LIST**) in TeX--XeT mode | — | `P` | — | — | **LIST** when TeX--XeT is on |
| `\lastlinefit` | pdfTeX §1657, §1843 | read by `line_break` for the last line | `int.p` `L/G` | — | `P` | — | — | — |
| `\savingvdiscards` | pdfTeX §1657, §1176, §1145 | read by the page builder and `\vsplit` when they discard glue/kern/penalty at a break: the discarded nodes go to **PAGE** `page_disc`/`split_disc` | `int.p` `L/G` | — | `P` | — | — | **PAGE** |
| `\savinghyphcodes` | pdfTeX §1657, §1068 | read by `\patterns` (stores `lc[*]` with the patterns) and by the hyphenator (`set_hyph_index`) | `int.p` `L/G` | — | `P` | — | — | makes `\patterns` snapshot the `lc` table into `hyph`: a bulk read of `lc[*]` at `\patterns` time |
| `\currentgrouplevel`, `\currentgrouptype` | pdfTeX §1663, §1665 | the save stack's depth and the current group code | — | — | `K` | (operand) | — | reads the *group nesting* at this point: the scope depth is a value. In the def-use model the enclosing scopes are known structurally from the text, so this is a structural read, not state; `\currentgrouptype` needs the group *kind* (`\hbox` vs `\begingroup` …) |
| `\currentiflevel`, `\currentiftype`, `\currentifbranch` | pdfTeX §1666–§1667 | **IF** (`cond_ptr`, `cur_if`, `if_limit`) | — | — | `K` | (operand) | — | **IF**: how many conditionals are *open while this token is read*, including ones whose branch is being read: a property of the enclosing conditionals' claims |
| `\fontcharwd`, `\fontcharht`, `\fontchardp`, `\fontcharic` | pdfTeX §1669, §1671 | `font[f].metrics[c]` | — | — | `P`: `<font>` `<8-bit number>` | (operand) | — | — |
| `\parshapelength`, `\parshapeindent`, `\parshapedimen` | pdfTeX §1672, §1674 | `parshape` | — | — | `P`: `<number>` | (operand) | — | — |
| `\showgroups`, `\showifs` | pdfTeX §1675, §1677, §1689, §1691 | the save stack (group kinds and the line each began on: **INPUT**), **IF** | — | — | `K` | — | log | **TERM**, **INPUT**, **IF** |
| `\showtokens` | pdfTeX §1684, §1471 | the `<general text>` | — | — | `P`: `<general text>` | — | log | **TERM** |
| `\unexpanded` | pdfTeX §1686, §493 | the `<general text>` (not expanded) | — | — | `P`: `<general text>` | the text itself, protected from expansion in `\edef`/`\write`/`\message` the way `\the\toks` is (§478); outside those contexts it is simply the text | — | the "protect in `\edef`" behaviour means its unfold differs by *who reads it*: an `\edef` body reader vs the main reader |
| `\detokenize` | pdfTeX §1686, §493 | the `<general text>`, `int.escapechar` | — | — | `P`: `<general text>` | the text as catcode-12/10 characters (`token_show` then `str_toks`) | — | — |
| `\interactionmode` | pdfTeX §1692, §1424, §1443 | `interaction` | as a command: `interaction` `G` (§1443 `new_interaction`, unaffected by groups) | — | `P`: `<optional =>` `<number>` | (operand: 0..3) | — | **TERM** |
| `\middle` | pdfTeX §1697, §1369 | **LIST**: must be inside a `math_left_group` | — | — | `P`: `<delimiter>` | — | a `middle_noad` in the `\left…\right` list | **LIST** |
| `\TeXXeTstate` | pdfTeX §1701, §1700 | — | `int.p` `L/G` (`eTeX_state` register) | — | `P` | — | — | read by `hpack` (§823), `post_line_break`, `ship_out`, and by `\beginL`… (which are errors when off, §1703): changes the meaning of every hlist built after it |
| `\beginL`, `\endL`, `\beginR`, `\endR` | pdfTeX §1701, §1703 | `int.TeXXeTstate`; horizontal mode (**MODE**) | **LIST** `LR_ptr` (the LR stack of the current list) | — | `K`: none | — | a `math_node` of subtype `L_code`/`R_code` (`begin`/`end`); unmatched ones are fixed up by `hpack` and `post_line_break` (`LR_problems`, §1705) | **LIST**: the LR stack is a running state of the list; `\endL` without `\beginL` is repaired at packaging time |
| `\scantokens` | pdfTeX §1747, §1753–§1757 | the `<general text>` (unexpanded), `int.newlinechar` (line splitting §1754), then `cat[*]` *at re-read time*, `int.endlinechar` (each line gets one), `toks.everyeof` at its end (§384), `int.tracingscantokens` | **INPUT**: pushes a pseudo-file level | — | `P`: `<general text>` | the text, re-tokenised line by line as if from a file (`\endlinechar` appended to each line, catcodes re-applied) | — | **INPUT**: the text is turned into *characters* and read back through the tokeniser: catcodes changed inside the text affect its own later lines; `\endinput`, `\inputlineno` apply to the pseudo-file; `\outer` and `\par` checks apply as for a file |
| `\readline` | pdfTeX §1759, §508 | `infile[n]` (one line); no catcodes: every character gets catcode 12, spaces 10 (§508 for `j=1`) | `cs[name]` `L/G`, `S`; advances `infile[n]` | — | `P`: `<number>` `to` `<cs>`; exactly one line (no brace balancing) | — | — | **INPUT**/**TERM** as `\read` |
| `\unless` | pdfTeX §1762, §524 | `cs[next]` must be an `\if…` other than `\ifcase` (§524) | — | — | `K`: one `\if…` token, then that conditional's claim | the conditional with its test negated | — | — |
| `\ifdefined` | pdfTeX §1762, §1766 | `cs[t]` (defined or not, after one `get_next` *without* expansion) | — | — | `K`: one token, branches | taken branch | — | a `\csname`-made `\relax` counts as defined (§372) |
| `\ifcsname` | pdfTeX §1762, §1767 | `cs[*]` of the expanded tokens to `\endcsname`; `cs[name]` exists or not (*no* definition, unlike `\csname`) | — | — | `Pd`: to `\endcsname` | taken branch | — | the lookup happens without creating the name: `\ifcsname x\endcsname` is false before `\csname x\endcsname` and true after it (which defined `\relax`): reads the `V`-keyed field |
| `\iffontchar` | pdfTeX §1762, §1769 | `font[f].metrics[c]` exists | — | — | `P`: `<font>` `<number>` | taken branch | — | — |
| `\protected` | pdfTeX §1770, §1772 | — | prefix: the macro gets `protected_token` first in its body | — | `K`: one `\def`-family command | — | — | a `\protected` macro is *not* expanded in `\edef`, `\write`, `\message`, alignment preamble scanning (`get_x_or_protected` §1772, used in §1226 and in `\edef` §478): its unfold depends on who reads it, as `\unexpanded` |
| `\numexpr`, `\dimexpr`, `\glueexpr`, `\muexpr` | pdfTeX §1778, §1780, §1782–§1799 | the operands' reads | — | — | `Pd`: an expression of operands, `+ - * /` and parentheses, terminated by the first non-expression token or `\relax` (consumed, §1782); every token is reached through `get_x_token`, so the extent depends on expansion | (operand) | — | the terminator `\relax` is swallowed; the result is rounded per §1790 |
| `\gluestretchorder`, `\glueshrinkorder`, `\gluestretch`, `\glueshrink` | pdfTeX §1801, §1803 | the `<glue>` | — | — | `P`: `<glue>` | (operand) | — | — |
| `\mutoglue`, `\gluetomu` | pdfTeX §1805, §1807 | the `<muglue>`/`<glue>` | — | — | `P` | (operand) | — | — |
| `\marks` | pdfTeX §1809, §1279 | `<number>` (class), the text expanded now | **MARKS** class `n` later | — | `P`: `<number>` `<general text>` | — | a `mark_node` with a class (`\marks0` = `\mark`) | **MARKS**, `V` class |
| `\topmarks`, `\firstmarks`, `\botmarks`, `\splitfirstmarks`, `\splitbotmarks` | pdfTeX §1809, §412 | **MARKS** `cur_mark` of class `n` (sparse array for `n>0`) | — | — | `P`: `<number>` | the mark text | — | **MARKS**, `V` class |
| `\pagediscards`, `\splitdiscards` | pdfTeX §1861, §1288 | **PAGE** `page_disc`/`split_disc` (the nodes discarded at the last page/split break when `\savingvdiscards>0`) | **PAGE**: emptied | — | `K`: none | — | the discarded nodes appended to the current vertical list | **PAGE**, **MODE** (vertical only, as `\unvbox`) |
| `\interlinepenalties`, `\clubpenalties`, `\widowpenalties`, `\displaywidowpenalties` | pdfTeX §1864, §1067 | as operand: `penalties.p[k]`; read by `post_line_break` | `penalties.p` `L/G`, `S` (variable-size like `\parshape`) | — | `P`: `<number>` then that many `<number>`s (`Pd` on the count) | (operand: `\interlinepenalties k`) | — | claim length is value-dependent |
| register widening (`\count`, `\dimen`, `\skip`, `\muskip`, `\toks`, `\box`, `\setbox`, `\wd`…, `\vsplit`, `\unvbox`…, `\ifvoid`…, `\countdef`… with `n` up to 32767) | pdfTeX §1811, §1815–§1841 | | sparse `count[n]`… `L/G` via `sa_def`/`gsa_def`, saved/restored by `sa_save`/`sa_restore` (§1837, §1841) | | `P`: `<register number>` (`scan_register_num` §1811) | | | the number's *range* is a value read from `eTeX_mode`: `\count300` is an error in compatibility mode |

## pdfTeX

pdfTeX's primitives fall into: parameters (read mostly at shipout or at
the first page, pdfTeX §680–§684, §747: `\pdfoutput`, `\pdfcompresslevel`,
`\pdfobjcompresslevel`, `\pdfdecimaldigits`, `\pdfminorversion`,
`\pdfmajorversion`, `\pdfdraftmode`, `\pdfgentounicode`, `\pdfpkresolution`,
`\pdfgamma`, `\pdfimagegamma`, `\pdfimagehicolor`, `\pdfimageapplygamma`,
`\pdfinclusioncopyfonts` are *frozen* at the first `\shipout`/object
write: `fixed_*`), font extensions, whatsits (`do_extension` pdfTeX §1528:
nodes that act at shipout), immediate object writers, expandable
helpers and `last_item` queries that read `pdf.*` document state.

| Primitive | § | reads | defines | scope effects | claim | unfolds | contribution | hazards |
|---|---|---|---|---|---|---|---|---|
| integer parameters: `\pdfoutput` `\pdfcompresslevel` `\pdfobjcompresslevel` `\pdfdecimaldigits` `\pdfmovechars` `\pdfimageresolution` `\pdfpkresolution` `\pdfuniqueresname` `\pdfoptionpdfminorversion` `\pdfoptionalwaysusepdfpagebox` `\pdfoptionpdfinclusionerrorlevel` `\pdfmajorversion` `\pdfminorversion` `\pdfforcepagebox` `\pdfpagebox` `\pdfinclusionerrorlevel` `\pdfgamma` `\pdfimagegamma` `\pdfimagehicolor` `\pdfimageapplygamma` `\pdfadjustspacing` `\pdfprotrudechars` `\pdftracingfonts` `\pdfadjustinterwordglue` `\pdfprependkern` `\pdfappendkern` `\pdfgentounicode` `\pdfdraftmode` `\pdfinclusioncopyfonts` `\pdfsuppresswarningdupdest` `\pdfsuppresswarningdupmap` `\pdfsuppresswarningpagegroup` `\pdfinfoomitdate` `\pdfsuppressptexinfo` `\pdfomitcharset` `\pdfomitinfodict` `\pdfomitprocset` `\pdfptexuseunderscore` (38, pdfTeX §256) and `\ignoreprimitiveerror` (pdfTeX §1657) | pdfTeX §254, §256, §1657 | — | `int.p` `L/G`, `S` | — | `P`: `<optional =>` `<number>` | — | — | `\pdfoutput` decides DVI vs PDF and is frozen at the first shipout (§747, `check_pdfoutput` §1537: pdf-only primitives error in DVI mode); the `fixed_*` group is frozen at the first page (§683): a later change is an error, so these are document constants defined by their *first-page* value. `\pdfadjustspacing`, `\pdfprotrudechars`, `\pdfadjustinterwordglue`, `\pdfprependkern`, `\pdfappendkern` are read by `line_break`/`hpack`/the main loop for each paragraph and box (§1234, §729) |
| dimension parameters: `\pdfhorigin` `\pdfvorigin` `\pdfpagewidth` `\pdfpageheight` `\pdflinkmargin` `\pdfdestmargin` `\pdfthreadmargin` `\pdffirstlineheight` `\pdflastlinedepth` `\pdfeachlineheight` `\pdfeachlinedepth` `\pdfignoreddimen` `\pdfpxdimen` (13) | pdfTeX §266 | — | `dim.p` `L/G`, `S` | — | `P` | — | — | `\pdfpagewidth`/`\pdfpageheight`/`\pdfhorigin`/`\pdfvorigin` are read at each `\shipout` (§644 `cur_page_width`); `\pdfpxdimen` is read by the `px` unit in every `<dimen>` scan (§480): a parameter read inside *claims* |
| token parameters: `\pdfpagesattr` `\pdfpageattr` `\pdfpageresources` `\pdfpkmode` (4) | pdfTeX §248 | — | `toks.p` `L/G`, `S` | — | as token parameters | — | — | read at shipout (`\pdfpageattr`, `\pdfpageresources`, §763, §769) and at document end (`\pdfpagesattr`, §803), written verbatim as token text (`pdf_print_toks_ln`), not expanded |
| `\pdftexversion`, `\pdftexrevision`, `\pdftexbanner` | pdfTeX §442, §494 | constants | — | — | `K` | revision/banner as text | — | — |
| `\pdflastobj`, `\pdflastxform`, `\pdflastximage`, `\pdflastximagepages`, `\pdflastximagecolordepth`, `\pdflastannot`, `\pdflastlink`, `\pdfretval` | pdfTeX §442, §450 | `pdf.last_*`: the object number allocated by the *most recent* `\pdfobj`/`\pdfxform`/`\pdfximage`/`\pdfannot`/`\pdfstartlink` *in document order* (set when the primitive is scanned, §1544, §1548, §1552, §1558, §1560) | — | — | `K` | (operand) | — | a global "last value" register written by every allocation: a def-use chain in text order whose definitions are the allocating nodes; object *numbers* themselves are allocation-order values (PhiTeX's SSA modes relax object numbering) |
| `\pdflastxpos`, `\pdflastypos` | pdfTeX §442, §450, §1621 | `pdf.last_x_pos`/`y_pos`: set when a `\pdfsavepos` whatsit is *shipped out* (§1621) | — | — | `K` | (operand) | — | **DEFERRED**: the value exists only after the page containing the `\pdfsavepos` has been shipped; reading it in the same page is reading the previous `\pdfsavepos`'s result. Typical use is `\write` at shipout, which is after the position is known |
| `\pdfelapsedtime`, `\pdfrandomseed`, `\pdfshellescape` | pdfTeX §442, §450 | the clock, the random seed, the shell-escape setting | — | — | `K` | (operand) | — | non-reproducible (`\pdfelapsedtime`); `\pdfshellescape` is a run constant |
| `\pdfuniformdeviate`, `\pdfnormaldeviate` | pdfTeX §494, §497, §125 | the random state (`randoms[]`, advanced by each call) | the random state | — | `P`: `<number>` (`\pdfuniformdeviate`) / `K` | the number as text | — | a global sequence state advanced by every read in document order; seeded by `\pdfsetrandomseed` or the time |
| `\pdfsetrandomseed`, `\pdfresettimer` | pdfTeX §1524, §1528 | `<number>` | the random state / the timer | — | `P` / `K` | — | nothing (immediate) | global run state |
| `\expanded` | pdfTeX §494, §497 | the `<general text>` fully expanded (`\protected` and `\unexpanded` honoured as in `\edef`) | — | — | `P`: `<general text>` | the expanded text | — | an `\edef` without a name: the unfold is a reduction result |
| `\pdfstrcmp` | pdfTeX §494, §497 | two `<general text>`s, expanded | — | — | `P`: two `<general text>` | `-1`, `0`, `1` | — | — |
| `\pdfescapestring`, `\pdfescapename`, `\pdfescapehex`, `\pdfunescapehex` | pdfTeX §494, §497 | one `<general text>`, expanded | — | — | `P` | the escaped text | — | — |
| `\pdfmdfivesum`, `\pdffilesize`, `\pdffilemoddate`, `\pdffiledump` | pdfTeX §494, §497 | `\pdfmdfivesum file {name}`/`\pdffilesize{name}`…: the file system; `\pdfmdfivesum{text}` (no `file`): the text; `\pdffiledump offset <n> length <n> {name}` | — | — | `P`: optional keywords and `<number>`s, then `<general text>` | the result as text | — | reads files outside the document (impure; restricted by `openin_any`) |
| `\pdfcreationdate` | pdfTeX §494, §497 | the clock at the *start* of the run (`start_time`), or `SOURCE_DATE_EPOCH` | — | — | `K` | the date string | — | a run constant |
| `\pdfmatch`, `\pdflastmatch` | pdfTeX §494, §497 | `\pdfmatch [icase] [subcount <n>] {pattern}{text}`: both expanded; `\pdflastmatch <n>`: the result of the most recent `\pdfmatch` in document order | `pdf.match_results` | — | `P` | `-1`/`0`/`1`; `\pdflastmatch n`: `pos->text` | — | a "last result" register like `\pdflastobj`, written by expansion |
| `\pdffontname`, `\pdffontobjnum`, `\pdffontsize` | pdfTeX §494, §497 | `font[f]`; `\pdffontname`/`\pdffontobjnum`: `pdf.font_objects` (the font's PDF resource name/object, *allocated on first use* in the output, §693 `pdf_init_font`) | `pdf.font_objects` (allocates if not yet, §497) | — | `P`: `<font>` | `F<n>`/`<n>`/the size | — | allocation-order value; reading it allocates (a read with a side effect) |
| `\pdfpageref` | pdfTeX §494, §497 | `pdf.page_objects[n]` (allocates the page's object number if not yet) | `pdf.page_objects[n]` | — | `P`: `<number>` | the object number | — | allocation on read |
| `\pdfxformname` | pdfTeX §494, §497 | `pdf.xform[n]` | — | — | `P`: `<number>` | the resource name number | — | — |
| `\pdfcolorstackinit` | pdfTeX §494, §497 | `[page] [direct] {<initial text>}` | `pdf.colorstacks`: allocates a new stack (a run-global counter `colorstackused`) | — | `P`: keywords then `<general text>` | the new stack's number | — | allocation on expansion, in document order; errors after the first page (`\pdfcolorstackinit` must come before output starts) |
| `\pdfinsertht` | pdfTeX §494, §498 | **PAGE**: the height of the insert `box[n]` material *on the current page so far* (`height(page_ins_head)` entries) | — | — | `P`: `<number>` | the dimension as text | — | **PAGE** |
| `\pdfximagebbox` | pdfTeX §494, §497 | `pdf.ximage[n].bbox[k]` | — | — | `P`: two `<number>`s | the dimension as text | — | — |
| `\leftmarginkern`, `\rightmarginkern` | pdfTeX §494, §498 | `box[n]` (an hbox; its first/last `margin_kern_node` width) | — | — | `P`: `<8-bit number>` | the dimension as text | — | the margin kern exists only if `\pdfprotrudechars` was on when the line was broken |
| `\ifpdfprimitive` | pdfTeX §513, §527 | `cs[t]` compared with the *primitive* meaning of the same name (`prim_lookup` §281) | — | — | `K`: one token, branches | taken branch | — | reads a second, immutable table (the primitive table) |
| `\ifincsname` | pdfTeX §1762, §389 | **IF**-like flag `is_in_csname`: whether this token is being expanded *inside* a `\csname…\endcsname` or `\ifcsname` claim | — | — | `K`: branches | taken branch | — | reads the *reader's context*: which enclosing claim is consuming the token |
| `\ifpdfabsnum`, `\ifpdfabsdim` | pdfTeX §1762, §524 | two `<number>`s/`<dimen>`s (absolute values compared) | — | — | `P` | taken branch | — | — |
| `\pdfprimitive` | pdfTeX §287, §1388 | `cs[*]`: the next token's *name*, looked up in the primitive table (§281) | — | — | `K`: one token | the primitive with that name (even if `cs[name]` was redefined); an unexpandable one is executed in place, an expandable one expands | — | the primitive table is a second meaning table that no `\let`/`\def` touches |
| `\quitvmode` | pdfTeX §1266 | **MODE** | **LIST** (starts a paragraph in vertical mode like `\indent`; no-op in horizontal/math) | — | `K` | `toks.everypar` in vmode | an indent box in vmode | **MODE** |
| `\letterspacefont` | pdfTeX §287, §1388, §705 | `font[g]` (the base), the TFM is re-read | `cs[name]` `L/G` as a new font `f`; `font[f].*` `G` (allocates a font number) | — | `P`: `<cs>` `<font>` `<number>` | — | — | font allocation order, as `\font` |
| `\pdfcopyfont` | pdfTeX §287, §1388 | `font[g]` | `cs[name]` `L/G` as a new font `f` sharing `g`'s metrics; `font[f]` `G` | — | `P`: `<cs>` `<font>` | — | — | as `\font` |
| `\lpcode`, `\rpcode`, `\efcode`, `\tagcode`, `\knbscode`, `\stbscode`, `\shbscode`, `\knbccode`, `\knaccode` | pdfTeX §1432, §452, §1436 | as operand: `font[f].code[c]` | `font[f].lpcode[c]`… `G` (font memory; `\global` irrelevant), `V` (`f`, `c` values) | — | `P`: `<font>` `<8-bit number>` `<optional =>` `<number>` | (operand) | — | global, as `\hyphenchar`; read by `line_break` (protrusion) and `hpack` (expansion) for *every line that uses the font* |
| `\pdfnoligatures` | pdfTeX §1432 | — | `font[f].noligatures` `G` | — | `P`: `<font>` | — | — | global; read by the main loop's ligature building for every character in `f` |
| `\pdffontexpand` | pdfTeX §1524, §1528, §705 | `font[f]`; the expanded TFMs (`autoexpand` or the named ones) | `font[f].expand` (stretch, shrink, step, autoexpand) `G` | — | `P`: `<font>` three `<number>`s, optional `autoexpand` | — | nothing (immediate) | global font state read by `line_break` when `\pdfadjustspacing>0`; the font must not have been used yet (§705) |
| `\pdffontattr` | pdfTeX §1524, §1528 | `<font>`, `<general text>` | `font[f].attr` `G` | — | `P` | — | nothing | global, written to the font dictionary at document end |
| `\pdfincludechars` | pdfTeX §1524, §1587 | `<font>`, `<general text>` (characters) | `pdf.font_used[f][c]` (marks glyphs for embedding) | — | `P` | — | nothing | document-end state (which glyphs are embedded) |
| `\pdfmapfile`, `\pdfmapline` | pdfTeX §1524, §1528, §693 | `<general text>` (expanded) | `pdf.fontmap` `G` (the map is read when the first font is used in the output: §693 `pdfmaplinesp`) | — | `P`: `<general text>` | — | nothing (immediate) | global run state consumed at the first font use; later lines with `+`/`-`/`=` prefixes modify it |
| `\pdfglyphtounicode`, `\pdfnobuiltintounicode` | pdfTeX §1524, §1528 | `<general text>` ×2 / `<font>` | `pdf.glyph_unicode` `G` / `font[f].nobuiltin_tounicode` `G` | — | `P` | — | nothing | global, read at font embedding |
| `\pdfinfo`, `\pdfcatalog`, `\pdfnames`, `\pdftrailer` | pdfTeX §1524, §1528 | `<general text>` expanded (`scan_pdf_ext_toks`); `\pdfcatalog` also `openaction <action>` | `pdf.info`, `pdf.catalog`, `pdf.names`, `pdf.trailer` `G` (appended to, in document order) | — | `P`: `<general text>` (+ optional `openaction`) | — | nothing until document end (written once) | append-only global accumulators: document-end output is their concatenation in text order |
| `\pdftrailerid` | pdfTeX §1524, §1528, §814 | `<general text>` | `pdf.trailer_id` `G` | — | `P` | — | nothing | document-end |
| `\pdfobj` | pdfTeX §1524, §1544 | optional `reserveobjnum`; else optional `useobjnum <n>`, optional `stream [attr {…}]`, optional `file`, then `<general text>` (expanded now). `\pdfobj` is not a whatsit: the object is created when scanned (§1544) and *written* when a `\pdfrefobj` whatsit referring to it is shipped (§1603) or, with `\immediate`, now (§1623) | `pdf.objects[k]` (content), `pdf.last_obj:=k` `G` | — | `P`: keywords, `<number>`, `<general text>` | — | nothing on the list; the object in the PDF | object number `k` is allocation-order; its *writing* is deferred to the first `\pdfrefobj` shipped (§1603 `pdf_write_obj`) |
| `\pdfrefobj` | pdfTeX §1524, §1528, §1603 | `<number>` (`pdf.objects[k]` must exist) | — | — | `P` | — | a `pdf_refobj_node` whatsit; at shipout schedules the object for writing | **DEFERRED** writing |
| `\pdfxform` | pdfTeX §1524, §1548 | optional `attr {…}`, `resources {…}` (expanded), `<8-bit number>`: `box[n]` (must not be void) | `pdf.xform[k]` (takes the box: `box[n]` is voided at the same level by `change_box(null)` §1548, like `\box`), `pdf.last_xform:=k` | — | `P` | — | nothing (the form is written when `\pdfrefxform` is shipped or with `\immediate`) | the box's content is shipped *as an XObject*: `\write`/`\special` inside it are shipped then; `\pdfannot` inside an XForm is an error (§1630 `do_annot`) |
| `\pdfrefxform` | pdfTeX §1524, §1528, §1603 | `<number>` (`pdf.xform[k]`) | — | — | `P` | — | a `pdf_refxform_node` whatsit of the form's size (a box-like node in the list, any mode) | **DEFERRED** writing; it is a whatsit with width/height/depth (§1603), appended as-is in any mode, not a box |
| `\pdfximage` | pdfTeX §1524, §1552 | `<rule spec>` (width/height/depth), optional `attr {…}`, `named {…}` or `page <n>`, `colorspace <n>`, a `<pagebox>` keyword, then `<general text>` (file name, expanded); the image file (its bounding box, resolution, pages); `int.pdfimageresolution`, `int.pdfpagebox`, `int.pdfforcepagebox`, `int.pdfinclusionerrorlevel`, `int.pdfmajorversion`, `int.pdfminorversion`, `int.pdfimagehicolor`… | `pdf.ximage[k]`, `pdf.last_ximage:=k`, `pdf.last_ximage_pages`, `pdf.last_ximage_colordepth` | — | `P`: keywords and operands (the order is fixed) then `<general text>` | — | nothing until `\pdfrefximage` | reads an external file (its size decides the node's dimensions); allocation order |
| `\pdfrefximage` | pdfTeX §1524, §1528, §1603 | `<number>` (`pdf.ximage[k]`) | — | — | `P` | — | a `pdf_refximage_node` whatsit with the image's dimensions | **DEFERRED** writing (the image stream is written once, at the first page that uses it) |
| `\pdfannot` | pdfTeX §1524, §1558 | optional `reserveobjnum` / `useobjnum <n>`, `<rule spec>`, `<general text>` (expanded) | `pdf.last_annot:=k` | — | `P` | — | a `pdf_annot_node` whatsit; at shipout an annotation with the node's page position and the rule spec's size (§1630 `do_annot`) | position is a shipout result; `reserveobjnum` allocates without a node |
| `\pdfstartlink`, `\pdfendlink` | pdfTeX §1524, §1560, §1561, §1635 | **MODE** (horizontal or math: vertical mode is an error §1560); `\pdfstartlink`: `<rule spec>`, optional `attr {…}`, an `<action>` (`user {…}`, `goto` [`file {…}`] (`page <n> {…}` or `name {…}` or `num <n>`) [`newwindow`/`nonewwindow`], `thread` [`file`] (`name`/`num`)) with the texts expanded now | `pdf.last_link:=k` | — | `P` | — | `pdf_start_link_node`/`pdf_end_link_node` whatsits; at shipout the link rectangles are computed from the *lines* between them (§1635 `do_link`, `pdf_link_stack`: a link spanning a line break becomes several rectangles, one per line) | **MODE**; the link's geometry is a line-break result; `\pdfendlink` must be in the same box level as its start (link stack at shipout, §1635) |
| `\pdfoutline` | pdfTeX §1524, §1563 | optional `attr {…}`, an `<action>`, optional `count <n>`, `<general text>` (title, expanded) | `pdf.outlines` `G` (appended; the tree structure comes from the `count` values of *previous* outlines in document order, §1563) | — | `P` | — | nothing (immediate; written at document end) | the outline tree is a fold over all `\pdfoutline`s in text order |
| `\pdfdest` | pdfTeX §1524, §1565, §1637 | `num <n>` or `name {…}` (expanded), optional `struct <n>`, a destination type (`xyz [zoom <n>]`, `fit`, `fith`, `fitv`, `fitb`, `fitbh`, `fitbv`, `fitr <rule spec>`); `int.pdfsuppresswarningdupdest` | `pdf.dests[id]` at shipout (§1637 `do_dest`: the page and position) | — | `P` | — | a `pdf_dest_node` whatsit (any mode); at shipout the destination with its page position | **DEFERRED**: a destination's page/position is a shipout result; a duplicate is a warning at shipout |
| `\pdfthread`, `\pdfstartthread`, `\pdfendthread` | pdfTeX §1524, §1566, §1567, §1637 | `<rule spec>`, optional `attr {…}`, `num <n>`/`name {…}`; no mode check (§1567 `new_annot_whatsit`); start/end pair like links | `pdf.threads` at shipout (`append_bead` §1637) | — | `P` | — | whatsits; beads (article thread rectangles) computed at shipout from the lines between start and end | as links: shipout geometry |
| `\pdfsavepos` | pdfTeX §1524, §1528, §1621 | — | at shipout: `pdf.last_x_pos`, `pdf.last_y_pos` (§1621) | — | `K` | — | a `pdf_save_pos_node` whatsit (any mode) | **DEFERRED**: the only way to learn a position; its result is read by `\pdflastxpos` *after* the shipout, typically in a deferred `\write` |
| `\pdfsnaprefpoint`, `\pdfsnapy`, `\pdfsnapycomp` | pdfTeX §1524, §1528, §1145 | `\pdfsnapy`: `<glue>` (`new_snap_node`, negative is an error); `\pdfsnapycomp`: `<number>` (0..1000); `\pdfsnaprefpoint`: none | — | — | `P` | — | whatsits (vertical mode) read by the page builder/`vpack` for grid snapping (§1145) | **PAGE**-adjacent: they alter `vpack`'s interline arithmetic |
| `\pdfliteral` | pdfTeX §1524, §1534 | optional `shipout`, `direct` or `page`; `<general text>`: expanded *now* (`scan_toks(false,true)`) unless `shipout`, which stores it unexpanded and expands at shipout like `\write` (`latespecial_node`) | — | — | `P`: keyword then `<general text>` | — | a `special_node`/`latespecial_node` whatsit; at shipout the text goes into the page content stream (with a `q 1 0 0 1 x y cm`/`Q` wrapping unless `direct`/`page`) | **DEFERRED** for `shipout`; the `direct` form depends on the PDF *text state* left by the previous literal and by pdfTeX's own `BT/ET` (§727 `pdf_out_literal` ends the text block for non-direct literals): the output stream has state |
| `\pdfcolorstack` | pdfTeX §1524, §1539, §727 | `<number>` (stack id from `\pdfcolorstackinit`), `set`/`push`/`pop`/`current`, `<general text>` for `set`/`push` | at shipout: `pdf.colorstack[n]` | — | `P` | — | a `pdf_colorstack_node` whatsit; at shipout a literal, and the stack's top is re-emitted at every page start (`pdf_out_colorstack_startpage`) | **DEFERRED**: the stack's state at a page start is a fold over the whatsits shipped so far, in shipout order |
| `\pdfsetmatrix`, `\pdfsave`, `\pdfrestore` | pdfTeX §1524, §1528, §1603 | `\pdfsetmatrix`: `<general text>` (six numbers, expanded) | at shipout: the transformation matrix stack (`matrixused`, read by `set_rect_dimens` §1630 to transform link/annot rectangles) | — | `P` / `K` | — | whatsits; literals `cm`/`q`/`Q` at shipout | **DEFERRED**: a stack in shipout order; `\pdfrestore` must match a `\pdfsave` on the same page |
| `\pdfinterwordspaceon`, `\pdfinterwordspaceoff`, `\pdffakespace` | pdfTeX §1524, §1528 | — | — | — | `K` | — | whatsits toggling, at shipout, whether interword glue is written as a space glyph of `\pdfspacefont` (tagged PDF support) | shipout state in shipout order |
| `\pdfrunninglinkon`, `\pdfrunninglinkoff` | pdfTeX §1524, §1528 | — | — | — | `K` | — | whatsits toggling, at shipout, whether a link continues across lines (`do_link`) | shipout state |
| `\pdfspacefont` | pdfTeX §1524, §1528 | `<font>` | `pdf.space_font` `G` | — | `P` | — | nothing | global |
| `\special` in pdfTeX | pdfTeX §1524, §1615 | as TeX's; text beginning with `pdf:` keywords is treated as a literal | | | | | a `special_node` written by `pdf_special` §727 as a page literal | — |

