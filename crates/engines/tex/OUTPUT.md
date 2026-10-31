# Output routines as native node specs

Companion to [PRIMITIVES.md](PRIMITIVES.md). The page builder (F3) is a
fold over the main vertical list; at each break it defines `box[255]`,
`int.outputpenalty`, the insert boxes and **MARKS**, and unfolds
`toks.output` in an `output_group` (§1012–§1028). The output node's
definitions bind "after page k's material", a position known only when
the fold reaches page k. Every read of a field the output node *may*
define therefore waits for the fold. The syntactic pass bounds plain's
`\output` from its text but not LaTeX's (143 names, 13 prefixes, 183
bound-breaking constructs). This file gives the **native specs**:
`phitex-tex` recognises a standard routine by hashing a fixed list of
meanings, and then uses the read and definition sets below instead of the
text-derived bound. Only the user hooks inside the routine are still
unfolded, each with its own hints.

Sources: `plain.tex` (TeX Live 2026, `\fmtversion` 3.1415926535);
`ltoutput.dtx` 2026-09-12 v1.5a; `ltshipout.dtx` 2026-03-12 v1.0q;
`ltmarks.dtx`, `lthooks.dtx`, `ltsockets.dtx`, `lttagging.dtx`,
`ltfloat.dtx`, `ltcounts.dtx`, `ltpageno.dtx`, `ltboxes.dtx`,
`ltdefns.dtx`, `ltcntrl.dtx`, `ltfssini.dtx`, `ltfsstrc.dtx`,
`ltfssbas.dtx` of the same kernel. A `\changes` entry cited as
"v1.4m 2025/02/14" is in the macro's own `macro` environment in
`ltoutput.dtx` unless another file is named.

## Conventions used here

The columns are PRIMITIVES.md's (reads, defines, unfolds, contribution,
hazards), with these additions:

- `reg[\name]`: read `cs[\name]` (a `\countdef`/`\dimendef`/`\skipdef`/
  `\chardef` box number) and the register it names. `box[\name]`
  includes the box's `\ht`/`\dp`/`\wd` and contents.
- `sw[\if@x]`: the switch's meaning (`\iftrue` or `\iffalse`, set by
  `\let`), i.e. `cs[\if@x]`.
- `val[\x]`: a macro read as data (a float list, a fraction, a style
  name), i.e. `cs[\x]` whose *value* matters, not a hashed meaning.
- **defines**: `L` is local to the group it is made in. Every `L` made
  directly in the routine dies when the `output_group` closes (§1026), so
  only `G` definitions are visible to the rest of the document. `G!`
  means *always* defined by every run of that sub-routine; `G?` means
  defined only under the stated condition.
- **void**: `\box n` and `\unvbox n` void the register *at the level of
  its current binding* (F13, §1079, §1110). The page builder's boxes
  (`box[255]`, insert boxes) are bound by `fire_up` (§1017–§1021) outside
  any group, so voiding them is a `G` definition.
- **re-contribution**: material appended to the main vertical list from
  inside the routine goes back to the contribution list (§1026) and is
  the head of what the fold continues with (design note, "feedback").
- **unfold (hook)**: user-supplied text the routine runs. It is not part
  of the recognised meaning. It is unfolded in place, in the scope the
  table states, with its own text-derived hints. A customised hook does
  not invalidate the native spec.
- `\shipout` of a box is the *shipout node* of F9: it contributes
  `shipped[k]`, reduces the `\write`/`\openout`/`\closeout`/
  `\pdfliteral shipout` texts found in the box (each an unfold (hook)
  with its own hints, bound at the shipout node, §1370–§1375), resolves
  the position-dependent pdfTeX whatsits (F9c), and sets `\deadcycles`
  to 0 (§638). It reads `dim.hoffset`, `dim.voffset`, `int.mag`
  (first-use constant, F10), `int.tracingoutput`, `int.showboxbreadth`,
  `int.showboxdepth`, `dim.pdfpagewidth`, `dim.pdfpageheight`,
  `dim.pdfhorigin`, `dim.pdfvorigin`, `toks.pdfpageattr`,
  `toks.pdfpageresources`, `int.pdfpagebox`, `int.pdfcompresslevel`,
  `int.pdfobjcompresslevel`, `int.pdfdecimaldigits`, `int.pdfimageresolution`,
  `int.pdfpkresolution`, `int.pdfgentounicode`, and the `pdf.*` and
  `font[f].*` fields of what the box uses (pdfTeX's `pdf_ship_out`; the
  page size is `\pdfpagewidth`/`\pdfpageheight` when nonzero, else the
  box size plus the origins). The table rows below list these once as
  **SHIP** instead of repeating them.

---

## 1. Plain TeX

`plain.tex` lines 1144–1214:

```tex
\output{\plainoutput}
\def\plainoutput{\shipout\vbox{\makeheadline\pagebody\makefootline}%
  \advancepageno
  \ifnum\outputpenalty>-\@MM \else\dosupereject\fi}
```

### 1.1 Sub-routines

| Macro | reads | defines | unfolds | contribution | hazards |
|---|---|---|---|---|---|
| `\output` (toks) | `toks.output` | — | `\plainoutput` (recognised) | — | runs in `output_group`, `mode=-vmode` (§1025) |
| `\plainoutput` | `int.outputpenalty`, `reg[\@MM]` | via callees | — | the shipped page (**SHIP**); on `\dosupereject` a re-contribution | **DEFERRED** (F9b): the page's `\write` texts |
| `\makeheadline` | `reg[\z@]`, `reg[\p@]`, `dim.hsize` (`\line`), `toks[\headline]`, `cs[\headline]` (a `\toksdef`) | `dim.prevdepth` (**LIST**, `\nointerlineskip`, inside the shipout `\vbox`) | `\the\headline` (hook) in the `\hbox to\hsize` | inside the shipped box | — |
| `\pagebody` | `dim.vsize`, `dim.maxdepth` | `dim.boxmaxdepth` `L` (inside the `\vbox to\vsize`) | — | inside the shipped box | — |
| `\pagecontents` | `box[\topins]`, `box[255]` (`\@cclv`), its `\dp`, `box[\footins]`, `skip[\footins]`, `sw[\ifr@ggedbottom]`, `cs[\topins]`, `cs[\footins]` (chardefs: 253 and 254 in plain, read not assumed) | `dimen[0]` (`\dimen@`) `L`; **void** `box[\topins]` `G?` (if not void), `box[255]` `G!`, `box[\footins]` `G?` (if not void) | `\footnoterule` (hook) when `box[\footins]` is not void | inside the shipped box | `box[255]` must be void at the routine's end (§1028): this is where it happens |
| `\makefootline` | `reg[\p@]`, `reg[\z@]`, `dim.hsize`, `toks[\footline]`, `cs[\footline]` | `glue.baselineskip` `L`, `dim.lineskiplimit` `L` (inside the shipout `\vbox`) | `\the\footline` (hook); plain's default `\hss\tenrm\folio\hss` reads `count[0]` **before** `\advancepageno` | inside the shipped box | — |
| `\advancepageno` | `count[0]` (`\pageno`), `reg[\z@]`, `reg[\m@ne]`, `reg[\@ne]` | `count[0]` `G!` (±1) | — | — | — |
| `\dosupereject` (only if `\outputpenalty ≤ -20000`) | `int.insertpenalties` (held-over count inside `\output`, §1014), `reg[\z@]`, `glue.topskip`, `dim.hsize` | — | — | re-contribution: `\hbox to\hsize{}`, `\kern-\topskip`, `\penalty10000`, `\vfill`, `\par\penalty-20000` (`\supereject`) | re-contribution makes the fold wait for this node (one more dead cycle, §1025) |

### 1.2 Totals for one run

Reads (values):

```
toks.output
box[255]
int.outputpenalty
int.insertpenalties
count[0]
dim.vsize
dim.maxdepth
dim.hsize
glue.topskip
dimen[\z@]
dimen[\p@]
count[\m@ne]
toks[\headline]
toks[\footline]
box[\topins]
box[\footins]
skip[\footins]
```

Reads (meanings; these are also the recognition hash in §1.3 unless
marked *read only*): 25 macro/register names and 32 primitive names.

```
\plainoutput \pagebody \makeheadline \makefootline \dosupereject
\pagecontents \advancepageno \line \nointerlineskip \supereject \nobreak
\z@ \p@ \m@ne \@ne \@M \@MM \@cclv \dimen@
\footins \topins \headline \footline \ifr@ggedbottom \footnoterule
\shipout \vbox \hbox \vskip \vss \vfill \vfil \kern \hrule \unvbox
\penalty \par \prevdepth \advance \global \ifnum \ifvoid \else \fi
\outputpenalty \insertpenalties \boxmaxdepth \maxdepth \vsize \hsize
\baselineskip \lineskiplimit \topskip \dp \dimen \skip \the
```

Plus **SHIP**. Total: **74** fields (17 values, 57 meanings) plus
**SHIP**.

Definitions: `G!` `count[0]`, `box[255]` (void); `G?` `box[\topins]`,
`box[\footins]` (void); **PAGE** `\deadcycles:=0` (§638); `L`
`dimen[0]`, `dim.boxmaxdepth`, `glue.baselineskip`, `dim.lineskiplimit`,
`dim.prevdepth` (all dead after the routine). **4 `G`, 5 `L`.**

Rest-of-document definition set: **`count[0]`** (`\pageno`, read by
`\folio` and by any later `\the\pageno`). The insert boxes are read
only by the next run of the routine and by `\ifvoid` tests nobody makes.
The plain routine never defines a mark, a macro or a parameter.

### 1.3 Recognition (plain)

The routine is "plain's" when all of these hold:

1. `toks.output` is exactly the one token `\plainoutput` (plain stores
   `{\plainoutput}`; the braces are the `<general text>` delimiters).
2. The meanings of these 19 names equal plain's: `\plainoutput`,
   `\pagebody`, `\makeheadline`, `\makefootline`, `\dosupereject`,
   `\pagecontents`, `\advancepageno`, `\line`, `\nointerlineskip`,
   `\supereject`, `\nobreak`, `\z@`, `\p@`, `\m@ne`, `\@ne`, `\@M`,
   `\@MM`, `\@cclv`, `\dimen@`.
   (`\@M` enters through `\nobreak`.)
3. The 32 primitive names in §1.2 still mean their primitives.
4. Kind checks, values read, not hashed: `\footins`, `\topins` are
   `\chardef`s (any number), `\headline`, `\footline` are `\toksdef`s,
   `\ifr@ggedbottom` is `\iftrue` or `\iffalse`.

Hooks, unfolded with their own hints: `\the\headline`, `\the\footline`
(and through it `\folio`), `\footnoterule`, the page's `\write` texts.
A document that changes `\headline` (e.g. `\nopagenumbers`, which sets
`\footline`) keeps the native spec.

Recognition hash list size: **1 token list + 19 macro/register meanings
+ 32 primitive meanings = 52**.

---

## 2. LaTeX (2026 kernel)

### 2.1 Structure

```
\output                         ltoutput "The LaTeX output routine", v1.0f 1993/12/05
├─ \outputpenalty < -10000:  \@specialoutput
│    ├─ > -10002 (=-10001, \clearpage):  \@doclearpage
│    ├─ < -10003 (=-10004, first penalty of a float):  save page in \@holdpg
│    └─ else (-10002/-10003, the float itself):  \@addtocurcol | \@addmarginpar
└─ else:  \@makecol  \@opcol  \@startcolumn  {\@opcol \@startcolumn while \if@fcolmade}
\@opcol  →  \@outputpage (onecolumn) | \@outputdblcol (twocolumn) ; \@floatplacement
\@outputpage  →  hooks, resets, \shipout\vbox{head, \@outputbox, foot}, \stepcounter{page}
\shipout (ltshipout)  →  shipout/before … \tex_shipout:D … shipout/after
```

The constants are `\@M`=10000, `\@Mi`=10001, `\@Mii`=10002,
`\@Miii`=10003, `\@Miv`=10004, `\@MM`=20000 (ltplain).

Common to every LaTeX sub-routine (not repeated in the tables):

- **K** (constants read): `reg[\z@]`, `reg[\p@]`, `reg[\m@ne]`,
  `reg[\@ne]`, `reg[\tw@]`, `reg[\thr@@]`, `reg[\sixt@@n]`,
  `reg[\@xxxii]`, `reg[\@M]`, `reg[\@Mi]`, `reg[\@Mii]`, `reg[\@Miii]`,
  `reg[\@Miv]`, `reg[\@MM]`, `reg[\@cclv]`, `reg[\maxdimen]`,
  `val[\@empty]`.
- **S** (scratch, written before read, always `L`): `box[\@tempboxa]`,
  `count[\@tempcnta]`, `count[\@tempcntb]`, `dimen[\@tempdima]`,
  `dimen[\@tempdimb]`, `skip[\@tempskipa]`, `sw[\if@tempswa]`,
  `cs[\reserved@a]`, `cs[\reserved@b]`, `cs[\@elt]` (`\let` to a
  per-list handler and back to `\relax`). These are never part of the
  rest-of-document set: no LaTeX code reads a scratch before writing it.
- **T** (trace): `\fl@trace` is `\@gobble` outside `fltrace`; messages
  (`\@latex@warning@no@line`, `\@latex@error`, `\@latex@info@no@line`)
  write to the log and terminal (**TERM**) and unfold `\thepage` (hook)
  in some texts.

### 2.2 `\output` (ltoutput, `\output`, v1.0f 1993/12/05; page-room test v1.0j 1993/12/17, v1.1t/u 1996)

| | |
|---|---|
| reads | `toks.output`, `int.outputpenalty`, `dimen[\@colroom]`, `glue.baselineskip` (the document's, in force at the break), `dim.textheight`, `sw[\if@fcolmade]` |
| defines | `cs[\par]` `L` (`\let\par\@@par`); `dim.vsize` `G!` (`\@colroom`, or `\maxdimen` after a `-10004` break) |
| calls | `\@specialoutput` or `\@makecol`, `\@opcol`, `\@startcolumn`, `\@whilesw` loop |
| unfolds | `\thepage` (hook) in the "contains only floats" warning |
| contribution | `\@emptycol` (`\vbox{}\penalty-10000`) when `\@colroom < 1.5\baselineskip` and `< \textheight`: re-contribution |
| hazards | **PAGE**: `\vsize` is read by the fold for the *next* page (§987 sets `page_goal` from it); it is the routine's main output to the fold |

### 2.3 `\@specialoutput` (v0.1c 1993/11/23, v1.0f 1993/12/06; v1.1i 1994/11/21 `\if@nobreak`)

| | |
|---|---|
| reads | `int.outputpenalty`, `box[255]`, `box[\@holdpg]`, `val[\@currlist]`, `box[\footins]`, `skip[\footins]`, `box[\@kludgeins]` (`\ifvbox`, `\wd`, `\ht`), `count[\@currbox]` (the float's type), `sw[\if@nobreak]`, `int.interlinepenalty`, `reg[\@pageht]`, `reg[\@pagedp]` |
| defines | `-10001`: see `\@doclearpage`. `-10004` and below: `box[\@holdpg]` `G!` (:= `\vbox{\unvbox255}`), **void** `box[255]` `G!`; **PAGE** `\deadcycles:=0` `G?` (only `< -20000`). `-10002`/`-10003`: `box[\@holdpg]` `G!` (the held page plus box 255, last box removed), then **void** `G!` by `\unvbox\@holdpg`; `dimen[\@pageht]`, `dimen[\@pagedp]` `L`; `cs[\@currbox]` `L`, `val[\@currlist]` `G!` (popped by `\@next`/`\@xnext`: `\gdef#4`); `box[\footins]`, `box[\@kludgeins]` **void** `G?` by `\@reinserts` (re-inserted) |
| calls | `\@doclearpage`, `\@next`, `\@reinserts`, `\@addtocurcol` (float, `\count\@currbox > 0`) or `\@addmarginpar` |
| unfolds | — |
| contribution | re-contribution: `\unvbox\@holdpg` (the page so far), `\insert\footins{\unvbox\footins}`, `\insert\@kludgeins{…}` (§1097), `\nobreak` or `\addpenalty\interlinepenalty` when `\outputpenalty<0`, plus what `\@addtocurcol`/`\@addmarginpar` add |
| hazards | every run re-contributes: the fold waits for it. A `-10004` run ships nothing; it is a dead cycle (§1025) |

### 2.4 `\@doclearpage` (v1.2n 2015/01/14 float order; v1.2l 2004/02/07 kludgeins pr/3528; v1.2b 2000/09/24)

| | |
|---|---|
| reads | `box[\footins]`, `box[\@kludgeins]`, `box[255]`, `val[\@toplist]`, `val[\@botlist]`, `val[\@deferlist]`, `val[\@currlist]`, `val[\@dbltoplist]`, `dimen[\@colht]`, `sw[\if@twocolumn]`, `sw[\if@firstcolumn]`, `sw[\if@fcolmade]`, `dim.textheight` |
| defines | footnotes present: **void** `box[255]` `G!` via `\setbox255\vbox{\box255\vfil}` (box 255 `L` then voided by `\@makecol`), then as `\@makecol`+`\@opcol`. No footnotes: **void** `box[\@kludgeins]` `G?`, **void** `box[255]` `G!` (`\vsplit` to 0pt and `\box255`; the `\vsplit` sets the split marks **MARKS** `G!`, §977); `val[\@deferlist]` `G!` (`\xdef`, top+bottom lists prepended), `val[\@toplist]` `G!`, `val[\@botlist]` `G!` (:= empty), `dimen[\@colroom]` `G!` (:= `\@colht`), `val[\@currlist]` `G?` (emptied with "Float(s) lost"), twocolumn and first column: `val[\@dbltoplist]` `G!` (:= empty), `dimen[\@colht]` `G!` (:= `\textheight`) |
| calls | `\@makefcolumn`, `\@opcol`, `\@whilesw`; twocolumn: `\@dblfloatplacement`, `\@outputpage`; `\@makecol`, `\@opcol`; `\clearpage` |
| unfolds | — |
| contribution | the `\vsplit`'s 0pt material is `\unvbox`ed (re-contribution of the marks/whatsits at the page top); `\clearpage` (re-contribution `\vbox{}`, `\write-1{}` whatsit, `\penalty-10001`) when floats remain or in the second column; `\vbox{}` |
| hazards | recursive through `\clearpage` → another `\@doclearpage` per float page: one fold step per page, all re-contributed |

### 2.5 `\@makecol` and the column box (v0.1c 1993/11/23, v1.0b 1993/11/29; rebuilt with hooks and sockets v1.4m 2025/02/14; bottom-skip fixes v1.4q 2025/03/20, v1.4r 2025/03/25, v1.4s 2025/03/29)

`\@makecol`:

| | |
|---|---|
| reads | `box[255]`, `val[\@freelist]`, `val[\@midlist]`, `box[\@kludgeins]`, `dimen[\@maxdepth]`, `cs[l__socket_build/column/outputbox_plug_str]` |
| defines | `box[\@outputbox]` `L` (`\setbox\@outputbox\box\@cclv`); **void** `box[255]` `G!`; `val[\@freelist]` `G!` (`\xdef`, the in-text floats returned); `val[\@midlist]` `G!` (:= empty); `dim.maxdepth` `G!` (:= `\@maxdepth`); `box[\@kludgeins]` **void** `G?` (`\@make@specialcolbox`) |
| unfolds | hooks `build/column/before`, `build/column/after` (via `\UseHook`, reading `cs[__hook build/column/before]` …); the socket plug `build/column/outputbox` (the kernel default `footnotes-floats-legacy` is recognised, see 2.13; another plug is a hook); `\UseTaggingSocket{build/column/outputbox}` (tagging off: nothing) |
| contribution | — |

`\@make@normalcolbox` (v1.4m): reads `dimen[\@colht]`, `box[\@outputbox]`; defines `box[\@outputbox]` `L`, `dimen[\@outputbox@depth]` `L` (inside the box); unfolds `\@texttop`, `\@textbottom` (hooks; `\relax` by default, `\vfil`-ish under `\raggedbottom`/`\flushbottom` set by classes).

`\@make@specialcolbox` (v1.4m, `\enlargethispage`): reads `box[\@kludgeins]` (`\wd`, `\ht`), `dimen[\@colht]`, `dim.pageshrink` (**PAGE**, the fold's value at this break), `box[\@outputbox]`, `dimen[\@outputbox@depth]`; defines `box[\@outputbox]` `L`, **void** `box[\@kludgeins]` `G!`; unfolds `\@textbottom` (hook).

`\@outputbox@removebskip` (v1.4m; v1.4q): reads `box[\@outputbox]`, `glue.lastskip` of it (via `\@outputbox@append`), `\gluestretchorder`; defines `cs[\@outputbox@reinsertbskip]` `G!` (`\relax`, or an `\xdef` that re-adds the removed skip); `box[\@outputbox]` `L`.

`\@outputbox@append` (v1.4m): reads `dimen[\@maxdepth]`, `box[\@outputbox]`; defines `box[\@outputbox]` `L`, `dim.boxmaxdepth` `L`, `dimen[\@outputbox@depth]` `L`; unfolds its argument (kernel text from the callers).

`\@outputbox@appendfil`, socket `build/column/baselineattach` (default plug `off`; plug `on` runs `\@backup@outputbox@depth`, which reads `dimen[\@outputbox@depth]`, `dim.maxdepth`, v1.4r 2025/03/25).

`\@outputbox@appendfootnotes` (v1.4m): reads `box[\footins]`, `skip[\footins]`, `cs[\pdfcolfoot@current]` (via `\csname`, `\relax` if undefined: pdfcol); defines **void** `box[\footins]` `G?` (if not void), `box[\@outputbox]` `L`; unfolds `\@makecol@handlesplitfootnotes` (hook, `\@empty` by default), socket `build/column/footnotes` (default plug: nothing), tagging socket `build/column/footins`, `\color@begingroup`, `\normalcolor`, `\footnoterule`, `\color@endgroup` (hooks: the `color` package and classes redefine them).

`\@outputbox@attachfloats` = `\@combinefloats` (v1.4m: "Macro got a new name"):
`\@outputbox@attachtopfloats` → `\@cflt` if `val[\@toplist]` is not empty,
`\@outputbox@attachbottomfloats` → `\@cflb` if `val[\@botlist]` is not empty
(v1.4s 2025/03/29 socket `baselineattach`).

`\@cflt`, `\@cflb` (v1.0h 1993/12/12): read `val[\@toplist]`/`val[\@botlist]`, every `box[\bx@…]` on the list, `glue[\floatsep]`, `glue[\textfloatsep]`, `dim.maxdepth`, `box[\@outputbox]`, `val[\@freelist]`; define **void** each listed `box[\bx@…]` `G!` (`\box #1` in `\@comflelt`), `box[\@outputbox]` `L`, `val[\@freelist]` `G!` (`\xdef`, the list returned), `val[\@toplist]`/`val[\@botlist]` `G!` (:= empty); unfold `\topfigrule`/`\botfigrule` (hooks, `\relax` by default).

`\@if@flushbottom@TF` reads `cs[\@textbottom]`; `\@if@footnotes@TF` reads `box[\footins]`; `\@if@bottomfloats@TF` reads `val[\@botlist]`. No definitions.

The default plug `footnotes-floats-legacy` (ltoutput, socket section, v1.4m; `\AssignSocketPlug` at line 2794) runs: baselineattach socket if footnotes or bottom floats, `\@outputbox@reinsertbskip` (reads and unfolds `cs[\@outputbox@reinsertbskip]`, kernel text from `\@outputbox@removebskip`), `\@outputbox@appendfootnotes`, `\@outputbox@attachfloats`. The other kernel plugs (`space-footnotes-floats`, `footnotes-space-floats`, `floats-space-footnotes`, `space-floats-footnotes`, `floats-footnotes`, `footnotes-floats`) call the same macros in another order and have the same read and definition sets; they are also recognised.

### 2.6 `\@opcol` (v1.0f 1993/12/05; marks v1.ih 2022/04/03)

| | |
|---|---|
| reads | `sw[\if@twocolumn]`, `box[\@outputbox]` (marks extraction) |
| defines | `dimen[\@mparbottom]` `G!` (:= 0), `dimen[\@textfloatsheight]` `G!` (:= 0); via `\@floatplacement` (ltfloat): `count[\@topnum]` `G!` (:= `\c@topnumber`), `dimen[\@toproom]` `G!` (:= `\topfraction\@colht`), `count[\@botnum]` `G!`, `dimen[\@botroom]` `G!`, `count[\@colnum]` `G!` (:= `\c@totalnumber`), `dimen[\@fpmin]` `L`; the mark structures (below) `G!` |
| reads via `\@floatplacement` | `count[\c@topnumber]`, `count[\c@bottomnumber]`, `count[\c@totalnumber]`, `val[\topfraction]`, `val[\bottomfraction]`, `val[\floatpagefraction]`, `dimen[\@colht]` |
| calls | `\@expl@@@mark@update@singlecol@structures@@` then `\@outputpage`; or `\@expl@@@mark@update@dblcol@structures@@` then `\@outputdblcol` |

Mark update (ltmarks, `\__mark_update_singlecol_structures:` /
`\__mark_update_dblcol_structures:`, "Interface with new mark mechanism"
2022/04/03): unpacks `box[\@outputbox]` into a scratch box, `\vsplit`s it
to `\maxdimen` (`\__mark_vbox_set_split_to_maxdimen:NN`) to obtain
`\splitfirstmarks`/`\splitbotmarks` per class, in a group with
`\splitmaxdepth`, `\vbadness`, `\vfuzz` `L`.

- reads: `cs[\g__mark_classes_seq]` (the declared classes),
  `cs[\c__mark_class_<c>_mark]` (each class's `\marks` number),
  `cs[\g__mark_<s>_<p>_<c>_tl]` for structures `s` ∈ {`page`,
  `previous-page`, `column`, `previous-column`, `first-column`,
  `last-column`}, positions `p` ∈ {`top`, `first`, `last`}, classes
  `c`; `sw[\if@firstcolumn]` (dblcol); **MARKS** split marks of its own
  `\vsplit` (local to it, F4).
- defines `G!`: `cs[\g__mark_<s>_<p>_<c>_tl]` for `s` ∈ {`page`,
  `previous-page`, `previous-column`, `column`, `first-column`,
  `last-column`} (singlecol; dblcol: `column`, `previous-column`,
  `first-column` or `last-column`, and on the second column `page`,
  `previous-page`), `cs[\g__mark_new_top_tl]`, `cs[\g__mark_tmp_tl]`.
  A prefix: `cs[\g__mark_*]`, `G`, keyed by the classes in
  `\g__mark_classes_seq` (bounded once that sequence is read).
- **MARKS** of TeX itself (`\topmark`, `\firstmark`, `\botmark`, used by
  `\leftmark`/`\rightmark`) are set by the fold (`fire_up` §1012), not
  by this node.

### 2.7 `\@outputdblcol` (ltoutput; v1.2n 2015/01/14 float order)

| | |
|---|---|
| reads | `sw[\if@firstcolumn]`, `box[\@outputbox]`, `box[\@leftcolumn]`, `dim.textwidth`, `dim.columnwidth`, `dim.columnseprule`, `sw[\if@fcolmade]` |
| defines | first column: `sw[\if@firstcolumn]` `G!` (false), `box[\@leftcolumn]` `G!` (`\global\setbox…\copy\@outputbox`). Second column: `sw[\if@firstcolumn]` `G!` (true), `box[\@outputbox]` `L`, **void** `box[\@leftcolumn]` `G!` |
| calls | `\@combinedblfloats`, `\@outputpage`, `\@dblfloatplacement`, `\@startdblcolumn`, `\@whilesw` |
| unfolds | `\normalcolor` (hook) around the column rule |

`\@combinedblfloats`: reads `val[\@dbltoplist]`, its `box[\bx@…]`, `glue[\dblfloatsep]`, `glue[\dbltextfloatsep]`, `count[\@dbltopnum]`, `dim.textheight`, `box[\@outputbox]`, `val[\@freelist]`; defines **void** each `box[\bx@…]` `G!`, `val[\@freelist]` `G!`, `val[\@dbltoplist]` `G!`, `box[\@outputbox]` `L`; unfolds `\dblfigrule` (hook).

`\@dblfloatplacement` (ltfloat): reads `count[\c@dbltopnumber]`, `val[\dbltopfraction]`, `val[\dblfloatpagefraction]`, `dimen[\@colht]`, `dim.textheight`, `skip[\@dblfptop]`, `skip[\@dblfpsep]`, `skip[\@dblfpbot]`; defines `count[\@dbltopnum]` `G!`, `dimen[\@dbltoproom]` `G!`, `dimen[\@textmin]` `L`, `dimen[\@fpmin]` `L`, `skip[\@fptop]`, `skip[\@fpsep]`, `skip[\@fpbot]` `L` (in a `\begingroup` in its callers).

### 2.8 `\@outputpage` (v1.0f 1993/12/05 … v1.1w 1996/09/21 `\@parboxrestore`; hooks and sockets v1.4m 2025/02/14; `\language` v1.3c 2017/03/10; catcodes v1.4o 2025/02/25, v1.4y 2025/06/07–08, v1.4z 2025/08/22, v1.5a 2026/09/13; `\protect` placement v1.4z 2025/08/22; `\label`/`\index` v1.4l 2025/01/21, v1.4m 2025/01/23)

The body is `\begingroup … \shipout\vbox{…}` with the `\endgroup`
re-emitted by `\aftergroup` from inside the box (§1271): every `L`
below dies at that `\endgroup`, before `\stepcounter{page}`.

| Step | reads | defines | unfolds | contribution |
|---|---|---|---|---|
| `\UseHook{build/page/before}` | `cs[__hook build/page/before]` | — | hook | — |
| `\language\document@default@language` | `cs[\document@default@language]` | `int.language` `L` | — | — |
| `\@resetactivechars` | — | active `cs[^^M]`, `cs[␣]` `L` (`\def`), active `cs[']` `L` (`\let` to `\active@math@prime`, read) | — | — |
| catcode resets | — | `cat[\]`, `cat[{]`, `cat[}]`, `cat[$]`, `cat[&]`, `cat[^^M]`, `cat[#]`, `cat[^]`, `cat[_]`, `cat[␣]`, `cat[^^I]`, `cat[<]`, `cat[>]`, `cat[*]`, `cat[.]`, `cat[-]`, `cat[/]`, `cat[[]`, `cat[]]`, ``cat[`]``, `cat[']`, `cat["]`, `cat[,]`, `cat[~]`, `cat[%]` `L` (25) | — | — |
| `\if@newlist` save | `sw[\if@newlist]` | `cs[\@@if@newlist]` `G!`, `sw[\if@newlist]` `G!` (false) | — | — |
| `\@parboxrestore` (ltboxes) | `dim.hsize`, `glue[\normallineskip]`, `dim[\normallineskiplimit]`, `glue[\normalbaselineskip]`, `glue[\@flushglue]`, `cs[\@@par]`, `cs[\@dischyph]`, `cs[\@acci]`, `cs[\@accii]`, `cs[\@acciii]`, `cs[\@normalcr]` | `L`: `sw[\if@nobreak]`, `sw[\if@noskipsec]`, `cs[\par]`, `cs[\-]`, `cs[\']`, ``cs[\`]``, `cs[\=]`, `cs[\\]`, `dim.parindent`, `glue.parskip`, `toks.everypar`, `dimen[\linewidth]`, `dimen[\@totalleftmargin]`, `glue.leftskip`, `glue.rightskip`, `skip[\@rightskip]`, `glue.parfillskip`, `glue.lineskip`, `dim.lineskiplimit`, `glue.baselineskip`; via `\sloppy`: `int.tolerance`, `dim.emergencystretch` | — | — |
| `\UseHook{build/page/reset}` | `cs[__hook build/page/reset]` | — | hook | — |
| `\let\protect\noexpand` | — | `cs[\protect]` `L` (in force for the `\write`s of this page, F9b) | — | — |
| `\shipout\vbox{` | see 2.9 | | | |
| `\set@typeset@protect`, `\aftergroup\endgroup`, `\aftergroup\set@typeset@protect` | `cs[\@typeset@protect]` | `cs[\protect]` `L`; two **DEFERRED** `\aftergroup` tokens (F9a) | — | — |
| `\if@specialpage` | `sw[\if@specialpage]`, `val[\@specialstyle]` | `sw[\if@specialpage]` `G?` (false, when set by `\thispagestyle`) | `\ps@<\@specialstyle>` (hook; a `\csname`, the name known from `val[\@specialstyle]`); it defines `\@oddhead` … `L` | — |
| `\if@twoside` | `sw[\if@twoside]`, `count[0]` | `cs[\@thehead]`, `cs[\@thefoot]`, `cs[\@themargin]` `L` (`\let` to `\@oddhead`/`\@evenhead` …; reads those meanings) | — | — |
| `\reset@font` = `\normalfont` (ltfssini; NFSS, 2.12) | see 2.12 | see 2.12 | hook `normalfont` | — |
| `\normalsize` | — | — | hook (class text: `\@setfontsize\normalsize\@xpt\@xiipt` etc.) | — |
| `\normalsfcodes` (ltfiles, `\frenchspacing`/`\nonfrenchspacing`/`\@empty`) | — | `sf[.]`, `sf[?]`, `sf[!]`, `sf[:]`, `sf[;]`, `sf[,]` `L` | — | — |
| `\let\label`, `\index`, `\glossary` | `cs[\@gobble@with@sphack@om]`, `cs[\@gobble@with@sphack@som]` | `cs[\label]`, `cs[\index]`, `cs[\glossary]` `L` | — | — |
| spacing | — | `glue.baselineskip`, `glue.lineskip`, `dim.lineskiplimit` `L` | — | — |
| `\@begindvi` | `cs[\@begindvi]`, `box[\@begindvibox]` | first page only: **void** `box[\@begindvibox]` (`\unvbox`, `G`), `cs[\@begindvi]` `G?` (:= `\@empty`) | — | the `\AtBeginDvi` material (legacy box) |
| page box | `dim.topmargin`, `dimen[\@themargin]` (oddside/evenside margin), `dim.headheight`, `dim.headsep`, `dim.textwidth`, `box[\@outputbox]`, `dim.footskip` | `box[\@tempboxa]` `L` (head, `\dp` := 0), **void** `box[\@outputbox]` (`\box`, `L` binding), `glue.baselineskip` `L` | `\pdfannot@link@off@@`/`@on@@` (`\csname pdfannot_link_off:\endcsname`: a hook, `\relax` unless the PDF management is loaded), tagging sockets `build/page/header`/`footer` (`c__socket_tagsupport/build/page/header_args_int`; tagging off: the second argument), `\color@hbox`, `\normalcolor`, `\color@endbox` (hooks), **`\@thehead`, `\@thefoot` (hooks: the page style's head and foot)** inside `\hbox to\textwidth` | the shipped page's content |
| after the box: `\global\let\if@newlist\@@if@newlist` | `cs[\@@if@newlist]` | `sw[\if@newlist]` `G!` (restored: same value as at entry) | — | — |
| `\global\@colht\textheight` | `dim.textheight` | `dimen[\@colht]` `G!` | — | — |
| `\stepcounter{page}` (ltcounts) | `cs[\c@page]` (`\@ifundefined`, a `\csname`), `count[0]`, `val[\cl@page]` | `count[0]` `G!` (+1); for each `\@elt{x}` in `\cl@page`: `count[\c@x]` `G?` (:= −1 then `\stepcounter{x}` → 0, recursively its own `\cl@x`) | — | — |
| `\UseHook{build/page/after}` | `cs[__hook build/page/after]` | — | hook | — |

`\@resetactivechars` (ltoutput, v1.0n 1994/04/30; v1.2f 1999/10/09 prime)
defines active characters `L`; their bodies unfold `\@activechar@info`
(log only) if executed.

### 2.9 `\shipout` (ltshipout v1.0q, `\__shipout_execute:` and `\__shipout_execute_main_cont:Nnnn`)

`\shipout` is `\__shipout_execute:`: it stores `\currentgrouplevel` in
`\l__shipout_group_level_tl`, then `\afterassignment` +
`\setbox\l_shipout_box`; when the box closes,
`\__shipout_execute_test_level:` puts `\__shipout_execute_cont:` after
the group (F9a). The continuation:

| Step | reads | defines | unfolds | contribution |
|---|---|---|---|---|
| set box | `int.currentgrouplevel` | `box[\l_shipout_box]` `L`, `cs[\l__shipout_group_level_tl]` `L`, `cs[\__shipout_saved_protect:]` `L`, `cs[\protect]` `L` | — | — |
| void box test | `box[\l_shipout_box]` | — | — | warning only |
| size | `box[\l_shipout_box]` | `dimen[\l_shipout_box_ht_dim]`, `…_dp_dim`, `…_wd_dim`, `…_ht_plus_dp_dim` `L` | — | — |
| `shipout/before` | `cs[__hook shipout/before]` | — | hook (`\AtBeginShipout`, `\AtBeginShipoutNext`, atbegshi-ltx/everyshi-ltx code); may discard (`\DiscardShipoutBox` sets `\g__shipout_discard_bool`) or change `\l_shipout_box` | — |
| counters | `int[\g_shipout_totalpages_int]`, `sw[\g__shipout_discard_bool]`, `int[\g_shipout_readonly_int]` | `int[\g_shipout_totalpages_int]` `G!` (+1, `\c@totalpages`); discarded: `sw[\g__shipout_discard_bool]` `G!` false, **PAGE** `\deadcycles:=0`; else `int[\g_shipout_readonly_int]` `G!` (+1, `\ReadonlyShipoutCounter`) | — | — |
| `\__shipout_run_firstpage_hook:` | `cs[__hook shipout/firstpage]` | first page: `cs[\__shipout_run_firstpage_hook:]`, `cs[\__shipout_add_firstpage_specials:]`, `cs[\__shipout_drop_firstpage_specials:]`, `cs[\__shipout_add_firstpage_material:Nn]` `G?`; `box[\l__shipout_firstpage_box]` `L` | hook `shipout/firstpage` (first page) | — |
| background/foreground | `cs[__hook shipout/background]`, `cs[__hook shipout/foreground]`, `cs[\@kernel@before@shipout@background]`, `cs[\@kernel@after@shipout@background]`, `cs[\pdfvariable]`, `cs[\pdfhorigin]`, `cs[\pdfvorigin]`, `dim.pdfhorigin`, `dim.pdfvorigin`, `int.vfuzz`, `int.vbadness` | `box[\l_shipout_box]` `L` (background/foreground boxes added), `cs[\c__shipout_horigin_tl]`, `cs[\c__shipout_vorigin_tl]`, `cs[\__shipout_init_page_origins:]` `G?` (first page with a non-empty hook), `dim.unitlength` `L`, `cs[\l__shipout_saved_badness_tl]` `L` | hooks `shipout/background`, `shipout/foreground` and the two kernel token lists (filled by packages: hooks) | — |
| first-page specials | `box[\l__shipout_firstpage_box]` | `box[\l__shipout_raw_box]` `L`, `cs[\__shipout_add_firstpage_specials:]` `G?` | — | — |
| last page | `cs[\@abspage@last]`, `cs[__hook shipout/lastpage]`, `cs[\@kernel@after@shipout@lastpage]` | `sw[\g__shipout_lastpage_handled_bool]` `G?` | hook `shipout/lastpage` (only on the page `\@abspage@last` from the `.aux`) | — |
| `shipout` hook | `cs[__hook shipout]` | — | hook | — |
| `\tex_shipout:D \box_use:N \l_shipout_box` | **SHIP**, `box[\l_shipout_box]` | `cs[\protect]` `L` (`\exp_not:N` during the `\write`s); **PAGE** `\deadcycles:=0` | the page's `\write`/`\pdfliteral shipout` texts (hooks, F9b) | **the shipped page** `shipped[k]` |
| after | — | `box[\l_shipout_box]` `L` (first page: raw box restored), `cs[\__shipout_drop_firstpage_specials:]` `G?`, `cs[\protect]` `L` | hook `shipout/after` | — |

`\pdfpagewidth`/`\pdfpageheight`: the 2026 kernel neither reads nor sets
them (no occurrence in `latex2e/base`). They are pdfTeX parameters read
by the primitive shipout (**SHIP**). `geometry` and `hyperref` set them
globally in the preamble or at `\begin{document}`; a later
`\newgeometry` is an ordinary `G` definition with a text position. The
routine does not define them.

### 2.10 `\@startcolumn`, `\@tryfcolumn`, `\@makefcolumn` (v1.0f 1993/12/05; v1.2d 1997/11/19; v1.2n 2015/01/14)

`\@startcolumn`: reads `dimen[\@colht]`, `val[\@deferlist]`; defines
`dimen[\@colroom]` `G!` (:= `\@colht`); calls `\@tryfcolumn\@deferlist`;
if no float column: `val[\@deferlist]` `G!` (:= empty) then each
deferred float through `\@scolelt` → `\@addtonextcol` (which re-`\@cons`
them: `val[\@deferlist]` `G!` again).

`\@startdblcolumn`: the same with `\@sdblcolelt` → `\@addtodblcol`.

`\@tryfcolumn #1` with `\@xtryfc`, `\@ytryfc`, `\@ztryfc`, `\@vtryfc`,
`\@wtryfc`, `\@bitor`, `\@xbitor`, `\@testfp`, `\@testwrongwidth`:

| | |
|---|---|
| reads | `val[#1]` (`\@deferlist`), every listed `box[\bx@…]` (`\ht`, `\dp`) and `count[\bx@…]` (type and placement bits), `dimen[\@colht]`, `dimen[\@fpmin]`, `skip[\@fptop]`, `skip[\@fpsep]`, `skip[\@fpbot]`, `cs[\f@depth]` (`\z@`, or `1sp` inside `\@startdblcolumn`), `count[\@currtype]`, `val[\@freelist]` |
| defines | `sw[\if@fcolmade]` `G!`; `val[\@trylist]` `G?`, `val[\@failedlist]` `G?`, `val[\@flsucceed]` `G?`, `val[\@flfail]` `G?` (only if the list is not empty); `sw[\if@test]` `G?` (`\@testtrue`/`\@testfalse` are `\global\let`, v1.1v 1996/07/26); `count[\@currtype]` `L`; on success: `box[\@outputbox]` `G!` (`\global\setbox`, the float page), **void** each succeeded `box[\bx@…]` `G!`, `val[#1]` `G!` (:= failed + rest), `val[\@freelist]` `G!` |
| contribution | — |

`\@makefcolumn` (forced float page, from `\@doclearpage`): the same with
`\@fpmin` := `-\maxdimen` and `\@testfp` := `\@gobble` (`L`).

### 2.11 Float placement: `\@addtocurcol`, `\@addtonextcol`, `\@addtodblcol`, `\@addtotoporbot`, `\@addtobot` (ltoutput "Float placement"; `\changes` v1.0f–v1.2n), and helpers

Helpers (ltoutput): `\@setfloattypecounts` (defines `count[\@currtype]`,
`count[\@fpstype]` `L` from `count[\@currbox]`), `\@getfpsbit`/
`\@boxfpsbit` (`count[\@tempcnta]` `L`), `\@testfp`, `\@flsetnum #1`
(`count[#1]` `L` := 1 when it is 0 and the float has `!`),
`\@flsettextmin` (`dimen[\@textmin]` `L` from `val[\textfraction]`,
`dimen[\@colht]`), `\@flcheckspace #1#2` (reads `dimen[\@colroom]`,
`dimen[#1]`, `val[#2]`, `skip[\textfloatsep]`, `skip[\floatsep]`,
`dimen[\saved@reqcolroom]`; defines `dimen[\@reqcolroom]` `L`,
`sw[\if@tempswa]` `L`), `\@flupdates #1#2#3` (defines `count[#1]` `G`,
`count[\@colnum]` `G`, `dimen[#2]` `G`, `dimen[\@colroom]` `G`,
`val[#3]` `G` via `\@cons`), `\@resethfps` (`count[\@currbox]` `G?` +2
when `h` alone cannot be honoured: the float's own count register),
`\@cons` (ltdefns: `\xdef`, `G`), `\@next`/`\@xnext` (`\def#3` `L`,
`\gdef#4` `G`).

| Macro | reads | defines | contribution |
|---|---|---|---|
| `\@addtocurcol` (from `\@specialoutput`) | `cs[\@currbox]`, `count[\@currbox]`, `box[\@currbox]` (`\ht`, `\dp`), `dimen[\@textfloatsheight]`, `dimen[\@pageht]`, `dimen[\@colroom]`, `dimen[\@colht]`, `count[\@colnum]`, `count[\@topnum]`, `dimen[\@toproom]`, `count[\@botnum]`, `dimen[\@botroom]`, `val[\@deferlist]`, `val[\@botlist]`, `val[\@midlist]`, `val[\@toplist]`, `val[\textfraction]`, `skip[\intextsep]`, `skip[\floatsep]`, `skip[\textfloatsep]`, `glue.parskip`, `int.interlinepenalty`, `int.outputpenalty`, `sw[\if@nobreak]`, `cs[\f@depth]` | `L`: `sw[\if@insert]`, `count[\@currtype]`, `count[\@fpstype]`, `dimen[\@textmin]`, `dimen[\@reqcolroom]`, `dimen[\saved@reqcolroom]`, `int.outputpenalty`, `toks.everypar`; `G?`: `count[\@colnum]`, `dimen[\@textfloatsheight]`, `val[\@midlist]` (placed here), `count[\@topnum]`/`dimen[\@toproom]`/`val[\@toplist]` (top), `count[\@botnum]`/`dimen[\@botroom]`/`val[\@botlist]` (bottom), `dimen[\@colroom]`, `val[\@deferlist]` (deferred), `count[\@currbox]` (`\@resethfps`), `sw[\if@test]`, **`sw[\if@nobreak]` `G?`** (`\@nobreakfalse` is `\global\let`, ltspace: only when a float is placed here while `\if@nobreak` is true) | placed here: re-contribution `\nobreak` or `\addpenalty\interlinepenalty`, `\vskip\intextsep`, `\box\@currbox` (**void** `box[\@currbox]` `G`), `\penalty\interlinepenalty`, `\vskip\intextsep`, `\vskip-\parskip` |
| `\@addtonextcol` (from `\@startcolumn`) | as above minus `\@pageht`, `\@textfloatsheight`, `\intextsep`, `\if@nobreak` | `G?`: top/bottom lists and counts via `\@addtotoporbot`, `val[\@deferlist]`; all in a `\begingroup`, so its `L` die there | — |
| `\@addtotoporbot`, `\@addtobot` | `count[\@topnum]`/`\@botnum`, `dimen[\@toproom]`/`\@botroom`, `val[\@toplist]`/`\@botlist`, `val[\@midlist]` | via `\@flupdates` (`G?`), `sw[\if@insert]` `L` | — |
| `\@addtodblcol` (from `\@startdblcolumn`) | `count[\@dbltopnum]`, `dimen[\@dbltoproom]`, `dimen[\@textmin]`, `val[\@dbltoplist]`, `val[\@deferlist]`, `skip[\dbltextfloatsep]`, `skip[\dblfloatsep]`, `box[\@currbox]`, `count[\@currbox]` | `G?`: `dimen[\@dbltoproom]`, `dimen[\@colht]`, `count[\@dbltopnum]`, `val[\@dbltoplist]`, `val[\@deferlist]` | — |

### 2.12 NFSS reset (`\reset@font` = `\normalfont`, ltfssini v3.2l 2025/09/15; `\selectfont`, ltfsstrc)

| | |
|---|---|
| reads | `cs[\encodingdefault]`, `cs[\familydefault]`, `cs[\seriesdefault]`, `cs[\shapedefault]`, `cs[\f@encoding]`, `cs[\f@family]`, `cs[\f@series]`, `cs[\f@shape]`, `cs[\f@size]`, `cs[\f@baselineskip]`, `cs[\f@linespread]`, `cs[\baselinestretch]`, `cs[\delayed@f@adjustment]`, `cs[\curr@fontshape]`, the shape table `cs[\<enc>/<fam>/<ser>/<sh>]` and font cache `cs[\<enc>/<fam>/<ser>/<sh>/<size>]` (`\csname`, keyed by the values just read), encoding switch `cs[\T@<enc>]`, `cs[\M@<enc>]`, `cs[\LastDeclaredEncoding]`, meta-family macros, the hooks `normalfont`, `selectfont` |
| defines | `L`: `cs[\f@encoding]`, `cs[\f@family]`, `cs[\f@series]`, `cs[\f@shape]`, `cs[\delayed@f@adjustment]`, meta family, `curfont`, `glue.baselineskip` (when the line spread differs); `G!`: `cs[\font@name]` (`\xdef` in `\selectfont`); `G?` first use of a shape or size anywhere in the document: `cs[\<enc>/<fam>/<ser>/<sh>/<size>]` (`\global\font`, F10 font number), `cs[\<enc>/<fam>/<ser>/<sh>]` (from a `.fd` file, `\input`: F15) |
| unfolds | hooks `normalfont`, `selectfont`; a `.fd` file on first use |
| hazards | every `G` here is a cache whose value does not depend on who defines it first (idempotent): a reader elsewhere gets the same meaning from its own `\selectfont`. `\font@name` is written before read by every `\selectfont`. Only the font *number* allocation order differs (F10, relaxed by the SSA modes) |

### 2.13 Shared small macros

`\@whilesw`, `\@iwhilesw` (ltcntrl), `\@next`, `\@xnext`, `\@cons`,
`\@nameuse` (ltdefns), `\hb@xt@` (ltdefns), `\@makeother` (ltmiscen),
`\@reinserts` (ltoutput, v1.0e 1993/12/01: reads `box[\footins]`,
`box[\@kludgeins]`; re-inserts them, voiding them `G`), `\@emptycol`
(v1.0n 1994/04/30), `\@addmarginpar` (ltoutput, marginpars: reads
`val[\@currlist]`, `box[\@marbox]`, `box[\@currbox]`, `sw[\if@twocolumn]`,
`sw[\if@firstcolumn]`, `sw[\if@mparswitch]`, `sw[\if@reversemargin]`,
`count[0]`, `dimen[\@mparbottom]`, `dimen[\@pageht]`, `dimen[\@pagedp]`,
`dim.marginparpush`, `dim.marginparsep`, `dim.marginparwidth`,
`dim.columnwidth`; defines `val[\@currlist]` `G!`, `val[\@freelist]`
`G!` (both boxes returned), `box[\@marbox]` `G!` (`\global\setbox`,
`\global\ht`/`\dp` := 0), `dimen[\@mparbottom]` `G!`; re-contributes
`\kern-\@pagedp`, `\nointerlineskip`, the marginpar `\hbox`, a strut
`\hbox`; unfolds `\thepage` in the "moved" warning).

### 2.14 Totals for one run of LaTeX's routine

Union over every branch (what a run *may* touch; a single run touches
the branch's subset given in the tables). Families with a computed key
are one entry with their key set.

Reads:

```
toks.output
box[255]
int.outputpenalty
dim.pageshrink
int.currentgrouplevel
count[0]
dim.vsize
dim.maxdepth
dim.hsize
dim.textheight
dim.textwidth
dim.columnwidth
dim.columnseprule
dim.topmargin
dim.headheight
dim.headsep
dim.footskip
dim.oddsidemargin
dim.evensidemargin
dim.marginparwidth
dim.marginparsep
dim.marginparpush
dim.pdfhorigin
dim.pdfvorigin
glue.baselineskip
glue.parskip
int.interlinepenalty
int.vfuzz
int.vbadness
box[\footins]
skip[\footins]
box[\@kludgeins]
box[\@holdpg]
box[\@outputbox]
box[\@leftcolumn]
box[\@begindvibox]
box[\@marbox]
box[\bx@*]
count[\bx@*]
count[\c@topnumber]
count[\c@bottomnumber]
count[\c@totalnumber]
count[\c@dbltopnumber]
count[\c@*] in \cl@page
dimen[\@colht]
dimen[\@colroom]
count[\@topnum]
dimen[\@toproom]
count[\@botnum]
dimen[\@botroom]
count[\@colnum]
count[\@dbltopnum]
dimen[\@dbltoproom]
dimen[\@textmin]
dimen[\@fpmin]
dimen[\@pageht]
dimen[\@pagedp]
dimen[\@mparbottom]
dimen[\@textfloatsheight]
dimen[\@maxdepth]
dimen[\@outputbox@depth]
skip[\floatsep]
skip[\textfloatsep]
skip[\intextsep]
skip[\dblfloatsep]
skip[\dbltextfloatsep]
skip[\@fptop]
skip[\@fpsep]
skip[\@fpbot]
skip[\@dblfptop]
skip[\@dblfpsep]
skip[\@dblfpbot]
glue[\normallineskip]
dim[\normallineskiplimit]
glue[\normalbaselineskip]
glue[\@flushglue]
val[\topfraction]
val[\bottomfraction]
val[\textfraction]
val[\floatpagefraction]
val[\dbltopfraction]
val[\dblfloatpagefraction]
val[\@freelist]
val[\@toplist]
val[\@botlist]
val[\@midlist]
val[\@currlist]
val[\@deferlist]
val[\@dbltoplist]
val[\@specialstyle]
val[\cl@page]
val[\@abspage@last]
val[\document@default@language]
sw[\if@twocolumn]
sw[\if@firstcolumn]
sw[\if@twoside]
sw[\if@specialpage]
sw[\if@fcolmade]
sw[\if@nobreak]
sw[\if@newlist]
sw[\if@mparswitch]
sw[\if@reversemargin]
cs[\@@if@newlist]
cs[\@begindvi]
cs[\@outputbox@reinsertbskip]
cs[\f@depth]
cs[\@oddhead]
cs[\@evenhead]
cs[\@oddfoot]
cs[\@evenfoot]
cs[\@typeset@protect]
cs[\@gobble@with@sphack@om]
cs[\@gobble@with@sphack@som]
cs[\@@par]
cs[\@dischyph]
cs[\@acci]
cs[\@accii]
cs[\@acciii]
cs[\@normalcr]
cs[\active@math@prime]
cs[\pdfcolfoot@current]
cs[\pdfvariable]
cs[\pdfhorigin]
cs[\pdfvorigin]
cs[__hook <name>] for 13 hooks
cs[l__socket_<name>_plug_str] for 3 sockets
cs[c__socket_tagsupport/<name>_args_int] for 4 tagging sockets
cs[\@kernel@before@shipout@background]
cs[\@kernel@after@shipout@background]
cs[\@kernel@after@shipout@lastpage]
int[\g_shipout_totalpages_int]
int[\g_shipout_readonly_int]
sw[\g__shipout_discard_bool]
cs[\__shipout_run_firstpage_hook:]
box[\l__shipout_firstpage_box]
cs[\g__mark_classes_seq]
cs[\c__mark_class_*_mark]
cs[\g__mark_*_tl]
NFSS: 14 named cs + 2 computed-key families (2.12)
SHIP
```

That is **140 entries**: named fields, plus key-bounded families
(`\bx@*`, `\cl@page` counters, `\g__mark_*`, the 13 hooks, 3 sockets, 4
tagging sockets, the NFSS block of 2.12 and **SHIP**, each counted
once). Add the **K** constants (17), the **S** scratch names (10) and
the recognised meanings of 2.16 (read as the hash).

Definitions, `G` (visible after the routine):

```
G!  dim.vsize
G!  dim.maxdepth
G!  count[0]                                   (\c@page)
G?  count[\c@*] in \cl@page                    (empty by default, ltpageno)
G!  box[255] (void)
G?  box[\footins] (void), box[\@kludgeins] (void)
G?  box[\@holdpg]
G?  box[\@outputbox]                           (\@vtryfc float page)
G?  box[\@leftcolumn]
G?  box[\@begindvibox] (void, first page)
G?  box[\@marbox]
G?  box[\bx@*] (void), count[\bx@*]            (\@resethfps)
G!  dimen[\@colht]
G!  dimen[\@colroom]
G!  count[\@topnum], dimen[\@toproom], count[\@botnum], dimen[\@botroom], count[\@colnum]
G?  count[\@dbltopnum], dimen[\@dbltoproom]
G!  dimen[\@mparbottom]
G!  dimen[\@textfloatsheight]
G!  val[\@freelist]
G!  val[\@midlist]
G?  val[\@toplist], val[\@botlist], val[\@dbltoplist]
G!  val[\@deferlist]
G?  val[\@currlist]
G?  val[\@trylist], val[\@failedlist], val[\@flsucceed], val[\@flfail]
G!  sw[\if@fcolmade]
G?  sw[\if@test]
G?  sw[\if@firstcolumn]
G?  sw[\if@specialpage]
G?  sw[\if@nobreak]
G!  sw[\if@newlist] (restored to its entry value), cs[\@@if@newlist]
G!  cs[\@outputbox@reinsertbskip]
G?  cs[\@begindvi]
G!  cs[\g__mark_*_tl] (structures x classes), cs[\g__mark_new_top_tl], cs[\g__mark_tmp_tl]
G!  int[\g_shipout_totalpages_int]
G?  int[\g_shipout_readonly_int], sw[\g__shipout_discard_bool]
G?  first page: cs[\__shipout_run_firstpage_hook:], cs[\__shipout_add_firstpage_specials:],
    cs[\__shipout_drop_firstpage_specials:], cs[\__shipout_add_firstpage_material:Nn],
    cs[\c__shipout_horigin_tl], cs[\c__shipout_vorigin_tl], cs[\__shipout_init_page_origins:]
G?  sw[\g__shipout_lastpage_handled_bool]
G!  cs[\font@name]; G? font cache cs[\<shape>/<size>], shape tables (2.12)
PAGE  \deadcycles := 0 (shipout, discard, \@specialoutput < -20000)
MARKS split marks (\@doclearpage's \vsplit)
```

That is **61 entries** (named fields plus the key-bounded families
`\cl@page` counters, `\bx@*` boxes and counts, `\g__mark_*`, the NFSS
font cache and shape tables). The `L` definitions die with the
`output_group` and are listed per table.

### 2.15 Rest-of-document read set (LaTeX)

Computed from the kernel's own uses: every `G` definition of 2.14
intersected with the fields read *outside* the output routine by kernel
code (`grep` of `latex2e/base/*.dtx` except `ltoutput.dtx` and
`latexrelease.dtx`, then the defining macro checked):

| Field | readers outside the routine | note |
|---|---|---|
| `count[0]` (`\c@page`) | `\cleardoublepage` (`\ifodd\c@page`), `\enddocument` (ltmiscen), makeidx's `\@mkidx`, ltshipout's lastpage code, `\thepage` anywhere in the body | the page counter |
| `count[\c@x]` for `\@elt{x}` in `\cl@page` | `\the\c@x`, `\thex`, `\stepcounter{x}` | empty in the kernel; `\@addtoreset{x}{page}` (chngcntr, perpage, footmisc `perpage`) adds to it: bounded by harvesting the literal `page` second arguments of `\@addtoreset`/`\counterwithin` |
| `val[\@freelist]` | `\@xfloat`, `\@xdblfloat`, `\marginpar`, `\@topnewpage` (`\@next\@currbox\@freelist`) | every float start |
| `val[\@currlist]` | `\end@float`, `\end@dblfloat`, `\@xympar` (`\@cons\@currlist`) | every float end |
| `box[\bx@*]`, `count[\bx@*]` | through `\@currbox`/`\@marbox`: `\@xfloat` (`\count\@currbox` from the placement), `\@endfloatbox` (`\global\setbox\@currbox`), `\@largefloatcheck` (`\ht\@currbox`), `\end@dblfloat` (`\global\dp`), `\@xdblfloat`, `\marginpar` (`\global\count\@marbox\m@ne`), `\@savemarbox`, `\@xympar`, `\@topnewpage`, `\ShowFloat` (`\csname bx@#1\endcsname`) | see section 4 |
| `count[\@dbltopnum]` | `\clearpage` (`\ifnum\@dbltopnum=\m@ne`) | every `\clearpage`, so every `\chapter` |
| `val[\@dbltoplist]`, `dimen[\@colht]` | `\@topnewpage` (`\twocolumn[…]`); `\@floatplacement`/`\@dblfloatplacement` from `\onecolumn`/`\twocolumn` read `\@colht` | layout switches only |
| `dim.vsize` | `\@enlargepage` (`\enlargethispage`) | also the fold itself (§987), which is not a node |
| `dim.maxdepth` | `\addpenalty` (ltspace, `\ifdim\prevdepth>\maxdepth`) | the value is restored to `\@maxdepth` every page: unchanged unless a package changed `\maxdepth` mid-document |
| `sw[\if@nobreak]` | `\addvspace`, `\addpenalty`, `\@afterheading`'s `\everypar`, `\newpage`, `\item`, `\@specialoutput` | `G?`: only when an in-text `h` float is placed by `\@addtocurcol` while `\if@nobreak` is true |
| `sw[\if@newlist]` | `\item`, `\endverbatim`, list code | the routine restores the value it read: the definition is the identity, and the native spec declares it *transparent* (a reader may bind past it) |
| `sw[\if@firstcolumn]` | makeidx `\@mkidx` (twocolumn index margin) | only in `\twocolumn` documents |
| `cs[\g__mark_*_tl]` | `\TopMark`, `\FirstMark`, `\LastMark`, `\IfMarksEqualTF` (ltmarks) in body text | usually only in page styles, i.e. inside the routine |
| `int[\g_shipout_totalpages_int]`, `int[\g_shipout_readonly_int]` | `\c@totalpages`, `\ReadonlyShipoutCounter`, ltproperties' `abspage`/`totalpages` when recorded immediately | rare in body text |

Not read outside the routine by any kernel code (they are only the next
run's inputs): `\@colroom`, `\@topnum`, `\@toproom`, `\@botnum`,
`\@botroom`, `\@colnum` (written, not read, by `\suppressfloats`),
`\@dbltoproom`, `\@mparbottom` (written by `\reversemarginpar`),
`\@textfloatsheight`, `\@toplist`, `\@botlist`, `\@midlist`,
`\@deferlist`, `\@trylist`, `\@failedlist`, `\@flsucceed`, `\@flfail`,
`\if@fcolmade`, `\if@test`, `\if@specialpage` (written by
`\thispagestyle`), `\@@if@newlist`, `\@outputbox@reinsertbskip`,
`\@begindvi`, `\@holdpg`, `\@leftcolumn`, `\@outputbox`, `\@kludgeins`,
`\font@name` (written before read), the shipout internals.

**Result.** The hypothesis holds, with additions. The fields that later
nodes actually bind to are these **15 entries**: `\c@page`, the `\cl@page`
counters, `\@freelist`, `\@currlist`, the `\bx@` boxes and counts,
`\@dbltopnum`, `\@dbltoplist`, `\@colht`, `\vsize`, `\maxdepth`,
`\if@nobreak`, `\if@newlist` (transparent), `\if@firstcolumn`, the
`\g__mark_` structures, and the two shipout counters. `\@colroom` is
*not* among them (only the next run reads it). Nothing that ordinary
body text reads in every paragraph is there except `\if@nobreak`, and
that one is defined only conditionally.

### 2.16 Recognition (LaTeX)

The routine is "LaTeX's" when `toks.output` and the meanings of every
name below equal the 2026 kernel's (a hash of each meaning; the set is
fixed per kernel version, and `latexrelease` rollbacks give other sets).

Token list: `toks.output` (ltoutput `\output {…}`).

Kernel macros (hashed, 101):

```
\@specialoutput \@doclearpage \@makecol \@make@normalcolbox \@make@specialcolbox
\@outputbox@removebskip \@outputbox@append \@outputbox@appendfil
\@outputbox@appendfootnotes \@outputbox@attachfloats \@outputbox@attachtopfloats
\@outputbox@attachbottomfloats \@combinefloats \@cflt \@cflb \@comflelt
\@comdblflelt \@combinedblfloats \@backup@outputbox@depth \@if@flushbottom@TF
\@if@footnotes@TF \@if@bottomfloats@TF \@opcol \@outputdblcol \@outputpage
\@resetactivechars \@activechar@info \@parboxrestore \@arrayparboxrestore \sloppy
\@reinserts \@emptycol \@startcolumn \@startdblcolumn \@tryfcolumn \@makefcolumn
\@xtryfc \@ytryfc \@ztryfc \@vtryfc \@wtryfc \@scolelt \@sdblcolelt
\@addtocurcol \@addtonextcol \@addtodblcol \@addtotoporbot \@addtobot
\@addmarginpar \@setfloattypecounts \@getfpsbit \@boxfpsbit \@testfp \@flsetnum
\@flsettextmin \@flcheckspace \@flupdates \@resethfps \@bitor \@xbitor
\@testtrue \@testfalse \@testwrongwidth \@floatplacement \@dblfloatplacement
\@whilesw \@iwhilesw \@next \@xnext \@cons \@nameuse \hb@xt@ \@makeother
\@nobreakfalse \nobreak \addpenalty \clearpage \newpage \stepcounter \addtocounter
\@stpelt \@ifundefined \set@typeset@protect \@latex@warning@no@line \@latex@error
\@latex@info@no@line \UseHook \UseSocket \UseTaggingSocket
\@expl@@@mark@update@singlecol@structures@@ \@expl@@@mark@update@dblcol@structures@@
\shipout \reset@font \normalfont \selectfont \fontencoding \set@current@meta@family
\pdfannot@link@on@@ \pdfannot@link@off@@ \@@write \fl@trace
```

expl3 internals they reach (hashed, 29): `\hook_use:n`,
`\__hook_preamble_hook:n`, `\__hook_use_initialized:n`,
`\socket_use:nw`, `\__mark_update_singlecol_structures:`,
`\__mark_update_dblcol_structures:`,
`\mark_update_structure_from_material:nn`,
`\__mark_extract_and_handle_marks:nn`, `\__mark_prepare_and_extract:nn`,
`\__mark_vbox_set_split_to_maxdimen:NN`,
`\__mark_update_structure_from_splitmarks:n`, `\mark_copy_structure:nn`,
`\mark_set_structure_to_err:n`, `\__shipout_execute:`,
`\__shipout_execute_test_level:`, `\__shipout_execute_cont:`,
`\__shipout_execute_main_cont:Nnnn`, `\__shipout_get_box_size:N`,
`\__shipout_add_background_box:n`, `\__shipout_add_foreground_box:n`,
`\__shipout_add_background_picture:n`,
`\__shipout_add_foreground_picture:n`, `\__shipout_picture_overlay:n`,
`\__shipout_init_page_origins:`, `\__shipout_run_firstpage_hook:`,
`\__shipout_add_firstpage_specials:`,
`\__shipout_drop_firstpage_specials:`, `\__shipout_finalize_box:`,
`\__shipout_debug:n`, and the four kernel socket plugs' macros
`\__socket_build/column/outputbox_plug_footnotes-floats-legacy:w` (or
one of the six other kernel plugs), `…/baselineattach_plug_off:w` or
`…_on:w`, `…/footnotes_plug_noop:w` (`\NewSocket` assigns `noop` to a 0-input socket, ltsockets) (the assigned plug names are read
from `l__socket_…_plug_str`; a non-kernel plug is a hook).

The `\selectfont` subtree (`\maybe@load@fontshape`, `\pickup@font`,
`\define@newfont`, `\extract@font`, `\size@update`, `\enc@update`,
`\set@fontsize`, `\merge@font@shape`, `\merge@font@series`, …) is the
NFSS module: it is recognised as a whole by its own hash list in the
NFSS spec (outside this file), and 2.12 gives its sets.

Register and constant names (hashed: their meanings are the allocated
`\countdef`/`\dimendef`/`\skipdef`/`\chardef`, 74): the **K** constants
(17), the **S** scratch names (10), and `\@colht`, `\@colroom`,
`\@topnum`, `\@toproom`, `\@botnum`, `\@botroom`, `\@colnum`,
`\@dbltopnum`, `\@dbltoproom`, `\@textmin`, `\@fpmin`, `\@pageht`,
`\@pagedp`, `\@mparbottom`, `\@textfloatsheight`, `\@currtype`,
`\@fpstype`, `\@reqcolroom`, `\saved@reqcolroom`, `\@maxdepth`,
`\@outputbox@depth`, `\@outputbox`, `\@leftcolumn`, `\@holdpg`,
`\@begindvibox`, `\@kludgeins`, `\footins`, `\c@page`, `\c@topnumber`,
`\c@bottomnumber`, `\c@totalnumber`, `\c@dbltopnumber`, `\floatsep`,
`\textfloatsep`, `\intextsep`, `\dblfloatsep`, `\dbltextfloatsep`,
`\@fptop`, `\@fpsep`, `\@fpbot`, `\@dblfptop`, `\@dblfpsep`,
`\@dblfpbot` (47).

The primitive names occurring in those bodies are checked too (that
they still mean their primitives); the implementation collects them from
the hashed bodies, so they are not listed here.

Recognition hash list size: **1 token list + 101 kernel macros + 29
expl3 internals + 3 socket plugs + 74 register/constant names = 208**,
plus the primitives in those bodies and the NFSS module's own list.

**Values read, not hashed** (they may change and the spec stays valid):
everything in 2.14's read list, notably the switches, the fractions,
`\topnumber` etc., the dimensions, and the float lists.

**User hooks** (unfolded with their own hints; customising them keeps
the native spec):

- page styles: `\@oddhead`, `\@evenhead`, `\@oddfoot`, `\@evenfoot`
  (through `\@thehead`/`\@thefoot`), `\ps@<\@specialstyle>`, and what
  they read (`\thepage`, `\leftmark`, `\rightmark`, `\TopMark`, …);
- `\thepage` in warnings and `\write` texts;
- hooks `build/page/before`, `build/page/reset`, `build/page/after`,
  `build/column/before`, `build/column/after`, `shipout/before`,
  `shipout`, `shipout/background`, `shipout/foreground`,
  `shipout/firstpage`, `shipout/lastpage`, `shipout/after`,
  `normalfont` (13), with `\@kernel@before@shipout@background`,
  `\@kernel@after@shipout@background`,
  `\@kernel@after@shipout@lastpage`;
- sockets with a non-kernel plug; tagging sockets
  `build/column/outputbox`, `build/column/footins`, `build/page/header`,
  `build/page/footer` when tagging is on;
- `\@texttop`, `\@textbottom`, `\topfigrule`, `\botfigrule`,
  `\dblfigrule`, `\footnoterule`, `\@makecol@handlesplitfootnotes`;
- colour: `\normalcolor`, `\color@begingroup`, `\color@endgroup`,
  `\color@hbox`, `\color@endbox`, `\pdfcolfoot@current`;
- `\normalsize` (class text), `\normalsfcodes` (one of three values),
  `\pdfannot_link_on:`/`off:` (PDF management);
- the `\write`, `\openout`, `\closeout`, `\pdfliteral shipout` texts in
  the shipped box (F9b), e.g. `\label`'s and `\index`'s, which read
  `\thepage` at shipout.

### 2.17 Packages

| Package | What it changes | Effect |
|---|---|---|
| fancyhdr | defines `\ps@fancy` etc., setting `\@oddhead`… (hooks), `\headrulewidth`… | native spec stays valid |
| geometry | sets `\textheight`, `\textwidth`, margins, `\headheight`…, `\pdfpagewidth`/`\pdfpageheight`; `\newgeometry` also `\@colht`, `\@colroom`, `\vsize` | values only: stays valid |
| hyperref | adds to `shipout/*` hooks and `\AtBeginDvi`; sets the page size; may redefine `\@begindvi` in old versions | hooks: stays valid. A redefined `\@begindvi` is read as a value (it is reset by `\@outputpage`), not hashed |
| atbegshi, everyshi | in the 2026 kernel these load `atbegshi-ltx`/`everyshi-ltx`, which map to the `shipout/*` hooks | stays valid. An old standalone version redefines `\shipout`: hash mismatch |
| eso-pic, pdfpages, background, tikzpagenodes | `shipout/background`/`foreground` hooks (pdfpages ships its pages as ordinary page material) | stays valid |
| multicol | inside `multicols` sets `\output` to its own routine (`toks.output` mismatch); restores LaTeX's after | inside the environment: fallback; outside: native |
| footmisc, flafter, fltrace, stfloats, dblfloatfix, nidanfloat, placeins (`\@doclearpage` wrap) | redefine `\@makecol`, `\@addtocurcol`, `\@addtonextcol`, `\@outputdblcol`, `\@doclearpage`, … | that macro's hash mismatches |
| tagpdf / `\DocumentMetadata{tagging=on}` | redefines `\UseTaggingSocket` plugs, PDF management redefines `\pdfannot_link_*` | sockets and links are hooks; `\UseTaggingSocket` itself is unchanged |
| color, xcolor | `\normalcolor`, `\color@*` | hooks: stays valid |

On a mismatch the spec degrades *per macro*, not per routine. The
recognised specs above are by name: each sub-routine's node spec holds
for its own body, and its callers name it. A replaced macro becomes an
unfold with its text-derived bound (the syntactic pass's
`may_define`) at its call sites; its callers keep their native specs.
If the replaced macro's text cannot be bounded, the reads that wait on
the fold are those of fields in its harvested prefixes and names, which
the engine reports (counter: reads waiting on the fold). A mismatch in
`toks.output` itself (multicol, a class with its own routine) means the
whole routine is unfolded with its text bound, as for any unknown
routine.

---

## 3. What stays unbounded natively

### 3.1 The float boxes

`\@freelist` is `\@elt\bx@A … \@elt\bx@R` then `\bx@S … \bx@Z`,
`\bx@AA … \bx@ZZ` (ltoutput, "Always use extended free list" v2.3m
2025/04/14; extended list v1.3a 2015/09): 52 names, each allocated by
`\newinsert` (`\let\@elt\newinsert` while the list is built, so each is
an insertion class: `box`, `count`, `dimen`, `skip` of the same number,
F10 allocation order). Prefix spec:

| Prefix | keys | family | locality |
|---|---|---|---|
| `cs[\bx@*]` | `A`…`Z`, `AA`…`ZZ` (52) | meaning: `\chardef` to the insert number; defined once at format time, never by the routine | static |
| `box[\bx@*]` | the 52 numbers | float contents | `G` (routine voids them, float code `\global\setbox`es them) |
| `count[\bx@*]` | the 52 numbers | float type and placement bits (`\@resethfps` adds 2) | `G` |

Which box a given float uses is a *value*: the head of `\@freelist`
when the float starts, i.e. the free list's state after the last output
run. The set of names is bounded (52); which name a float gets is not
known from text. Readers outside the routine that bind to these: only
float code, through `\@currbox`/`\@marbox`: `\@xfloat`, `\@floatboxreset`
(via `\@xfloat`), `\@endfloatbox`, `\@largefloatcheck`, `\end@float`,
`\end@dblfloat`, `\@xdblfloat`, `\@dblfloat`, `\marginpar`, `\@xmpar`,
`\@ympar`, `\@savemarbox`, `\@xympar`, `\@topnewpage`, `\ShowFloat`
(`\fl@ShowFloat`). Since every one of them first reads `\@freelist`
(which is in the rest-of-document set), the dependency is through
`\@freelist`; no float code reads a `\bx@` box it did not pop.

Plain's `\topins`/`\footins` and LaTeX's `\footins`/`\@kludgeins` are
fixed numbers (static `cs`), not a prefix.

### 3.2 Other residues

- **`\cl@page`** counters: the set is the value of `\cl@page` at the
  run; bounded by harvesting `\@addtoreset{…}{page}` and
  `\counterwithin{…}{page}` with literal arguments; a computed first
  argument breaks the bound for that counter only.
- **Hooks and page styles** (2.16): each unfolds with its own hints. An
  unboundable hook text (computed names in `shipout/before`, e.g.
  `\csname` on `\thepage`) degrades only the fields that hook may
  define.
- **`\write` texts** of each page (F9b): reduced at the shipout node;
  they define nothing (a `\write` text is expanded, not executed), so
  they add reads, never definitions.
- **NFSS caches** (2.12): `G` but idempotent; only F10's font numbering.
- **`\if@nobreak`**: the one rest-of-document read that body code makes
  all the time. It is defined only when an `h` float lands under a
  heading; a reader of `\if@nobreak` after a page break may bind to the
  routine. Precisely: it waits for the fold only if a float was pending
  (`\@currlist` not empty) at a break before it.
- **Re-contributions** (`\@specialoutput`, `\@doclearpage`,
  `\@emptycol`, `\@addtocurcol`, plain's `\dosupereject`): not fields;
  they are the head of the list the fold continues with. Every float
  costs two dead-cycle runs (`-10004`, then `-10002`/`-10003`) before
  the page continues: serial by nature, but cheap and local.

With these, the reads of later nodes that wait for page breaking are
those of the 15 entries in 2.15, `\if@nobreak` only after pending
floats. For plain, only `\pageno`.

## 4. The page builder over paragraph results

### 4.1 Today

The page builder (§980–§1028) is one fold over the main vertical list,
run once by a single `breaks` node after the text (`page.rs`,
`Run::breaks`). No paragraph region reads or writes page-builder state:
there is no page-state cell, and `build_page` (§994) is not a step of
the contributing nodes. Before 2026-10-16 every `build_page` point was a
step of a chain of page-state definitions threaded through the
contributing nodes; that chain and the per-point light steps are gone.

### 4.2 What a region contributes

A paragraph region (§5) builds its paragraph on its run-local nest and
contributes, to the accumulator `items`, the material that reaches the
list it began in, each entry tagged by that list's id (0 for the main
list), in document order:

- the paragraph's lines and the interline material, and the vertical
  material around them (boxes and rules, glue, kerns, penalties, marks,
  inserts, `\vadjust` material, whatsits, the delete markers of
  `\unskip`/`\unkern`/`\unpenalty`/`\lastbox` on that list); a box whose
  interline glue (§679) depends on the previous depth is a `T_VBOX`
  item, resolved by the list's reader from the depth before it, so a
  region never reads `prev_depth` from another node;
- a `T_PAGE_PARAMS` item `[tag, \vsize, \maxdepth, \topskip,
  \holdinginserts]` when the run changed one of those, so the fold uses
  the new values from that point on (§987, §1000, §1005);
- for `\end` (§1054), a `T_END` item `[tag, null box, fill glue,
  penalty −'10000000000]`: the material the page builder appends each
  time round until the page is empty.

`\prevgraf` of the list is a definition (`pg:<list>`). Parameters a
region reads are ordinary reads bound to their last definer.

### 4.2a Page steps

One `breaks` node after the text folds the whole main list into pages:
§980–§1008's arithmetic once over the list, with the parameters each
point's `T_PAGE_PARAMS` item gives, `T_END` re-appended (§1054) until
the page is empty. Each page fired is a `pagestep` child (counted in
`Stats::page_steps`, those with no output routine in
`Stats::page_steps_any`) whose argument is the page's material:
`\box255`, the insertions, the marks, the held-over insertions and
`\outputpenalty`. The step (`Run::page_step`) does §1012–§1021 and runs
the output routine inline on its own nest (`OutPage` opened at its start
§1025, `output_end` §1026 at its `}`), or ships the page (§1023).

What a routine leaves for the main list (§1026) or changes in the page
parameters is a loop-back, `pagefx:<j>`: the next round of `breaks`
reads it and refolds from that page on, to a fixpoint. Round one assumes
none.

### 4.3 The page fold

The fold carries tex.web's page state locally (`page_so_far`, the insert
state, the best break so far, the held contributions) through the
queue of items; nothing of it is a field. Its fast path: while the
page's total plus an item's natural height stays below the goal and the
item is no breakpoint, insert or mark, the item is added with no break
test. Otherwise §1000–§1010 run item by item.

### 4.4 Readers of the page state

`\pagegoal`, `\pagetotal` and the other §1245 dimensions,
`\outputpenalty` and `\insertpenalties` outside the output routine and
`\showlists` read from the text cannot see the fold, which runs after
the text: they answer the empty page's values (see `docs/TARGET_GRAPH.md`,
"Not yet"). Inside the output routine they read the step's own state.

### 4.5 The chain after

A region depends only on definitions (regime cells) of earlier nodes.
The critical path is the format's chain + one region + `breaks` + the
page steps chained through what their routines define globally
(`\count0`, the insert boxes; speculated by the engine) + the writer.

### 4.6 What must stay exact

- §987 fixes `page_goal` from `\vsize` and `page_max_depth` from
  `\maxdepth` when the first box or insert reaches an empty page: the
  fold uses the values of the last `T_PAGE_PARAMS` item before that
  point, never later ones.
- §1000's `\topskip` glue, §1001's discard of glue and penalties at the
  top of an empty page, §1009's insert placement and split (`\vsplit` of
  an insert box), and §1005's `\holdinginserts` are steps of the fold.
- `\lastskip`, `\lastpenalty`, `\lastkern`, `\lastbox` on the main list
  read the region's own contributions and the ones before it on that list.
- The output routine runs where tex.web runs it: in its page step, in an
  `output_group`, with `\box255`, the marks and the insert boxes defined
  as §1017 leaves them.

### 4.7 Conditions

1. One `pagestep` per page, never one per `build_page` point. The
   `chains` test asserts `height <= 6 + pages` per fixture.
2. `\vsize` or `\maxdepth` changed inside an output routine reach the
   next page through `pagefx:<j>` (second round); a test changes
   `\vsize` in `\output` and checks the next page against TeX.
3. An edit re-runs its region; `breaks` re-runs on any change of the
   main list; page steps re-run only where their material changed.

All outputs byte-identical; edits equal cold; at 1 and 4 threads.

## 5. Paragraph regions

A document node is a paragraph region: the text between two
paragraph-ending items at brace depth 0 and outside every conditional,
as the syntactic pass marks them (`hints::slices_of`). (Before
2026-10-16 a paragraph crossed nodes through anchors and paragraph
cells; those are gone.) The region's
run opens its paragraph (and any group, box, math list, insert or
alignment) as a level on its own semantic nest (`nest.rs`,
`Run::push_list`/`push_group`/`pop_level`) and closes it before it
ends, so nothing of a paragraph crosses to another node.

What a region leaves is its contributions to the list it began in, the
`\prevgraf` of that list (`pg:<list>`) and the definitions it makes at
its base level. A region the pass closed too early reads on into the
following text (claimed) until it is balanced: still correct, and serial
only for that span.

A format (`plain.tex`, `latex.ini` and every file it `\input`s) is one
node, run sequentially, files read in place through the environment
provider (`api.rs` `Graph::provide`); its hint is `Any`, and a group
open at its end is an error. In a document, `\documentclass`,
`\usepackage`, `\RequirePackage`, `\input` and `\include` are one slice
each; package-kind files run inline, a document-kind `\input` file is
sliced into paragraph regions with hints (`Run::doc_file_regions`).
No node leaves an engine scope open.
There are no token-frame children: a macro body, a conditional, `\the`
and `\csname` expand inline in the node that reads the token. See `docs/TARGET_GRAPH.md` for the node and edge
set the tests assert.

## 6. Conditionals

Conditionals are run-local: the condition stack lives on the run and a
conditional is never split across nodes. The syntactic pass keeps a
conditional in one region, and a format is one node. There are no condition-level cells,
no node `ifdepth` argument and no depth checks; `\currentiflevel`,
`\currentiftype`, `\currentifbranch` and `\showifs` read the run's
own stack. The output routine runs inline in its page step with its own
stack. See `docs/TARGET_GRAPH.md`.
