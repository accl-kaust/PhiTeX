# Relative counters and derived names (spec, 2026-11-06)

Status: the design note of 2026-11-03 (this file, then docs/note-relative-counters.md, proposed
by xref, asked for by the manager), made a spec by rel1 on 2026-11-06
after measuring where today's edits wake readers of shifted counters
(§1.2). It extends the deferred reads of docs/spec-deferred-reads.md
(`defer.rs`) beyond the page-layer registers. Bars: every structural edit
(a section, a figure, a citation added or deleted) under 100 ms at one
thread, no slower at eight; exactness unchanged: every edited PDF is
byte-identical to a full rebuild and to the oracle.

The idea in one line: a counter a piece only steps is not a value in the
piece but **the value reaching the piece plus the piece's steps** (`old +
delta`, a *symbol*). An edit before the piece moves `old`; every use of
the symbol that gives the same answer for any `old` (a step, a restore, a
name whose new field holds what the old one held, a comparison whose
outcome stays) keeps the piece standing; every other use is an ordinary
read (*forcing*), exactly what TeX does, and the piece runs again.

## 1. The defects

### 1.1 The two defects as first seen (2026-11-03)

**Counters that only count.** biblatex steps `instcount` at every
citation and every bibliography entry (`\addtocounter{instcount}\@ne`,
biblatex.sty 4913, 4961, 10085, 10397, 11747). It names things after it:

- the page tracker reads `blx@page@\number\c@instcount` (`\ifcsundef`,
  `\csuse`, 3124-3139);
- it writes `\abx@aux@page{\the\c@instcount}{\the\c@page}` to the
  `.aux` (10203).

A `\cite` added before position P shifts every later citation's number
by one. On the biblatex example 50-style-authoryear, every later exec
node and the whole `\printbibliography` run (1.1 s) ran again, twice:
once on `count:302` and once on the `.aux` round's `blx@page@<n>`. That
cost 1.6-1.9 s on mc (job 78926). The output does not change: citation
n+1 now reads `blx@page@<n+1>`, which holds what `blx@page@<n>` held
before. Today every later node's definitions change all the same:
`count:302` = old + 1, and the `.aux` lines carry the new numbers. So no
checkpoint cutoff can stop the wave.

Other counters with the same shape:

- hyperref's anchor counters (`Hy@linkcounter`, `\theHsection` names,
  `Item.<n>`, `Hfootnote.<n>`);
- `\c@page` used inside names (`\label`'s `page@<n>`, zref's
  `abspage`);
- tcolorbox and tikz `remember picture` ids (`pgfid<n>`);
- `\c@footnote` in `\footnote` anchors.

**File sizes read for presence.** LaTeX's `\IfFileExists` is expl3's
`\file_if_exist:nTF`. It expands `\tex_filesize:D` (`\pdffilesize`) and
then tests the digits only for blankness:
`\__file_full_name_auxi:nn`'s `\if:w \scan_stop: #1 \scan_stop:`, then
`\tl_if_blank` (expl3-code.tex 12570-12660). A heading edit changes the
size of the `.toc`/`.aux`, so `\tableofcontents`' and
`\begin{document}`'s readers run again (amsldoc: TOC resume 19 ms,
`wsize:amsldoc.toc`). The digits do differ, so no checkpoint can cut
this off either.

### 1.2 Measured (2026-11-06, dev 345254467, one thread, profile build)

Who reads a shifted counter, how (`PHITEX_READS`, `PHITEX_DEF_TRACE`,
`PHITEX_SYM_TRACE` in the edit's round; accl 25710, 25733, 25745, 25753):

| edit | counter | runs, ms | read where | shift-invariant? |
|---|---|---|---|---|
| amsldoc add section | `count:187` = `\g__mark_int` | 53 resume, 114-142 | `\int_compare:nNnTF \g__mark_int < {99999}` in `\@kernel@before@insertmark` (all 58 forced reads) | yes: the outcome stays (§2.6) |
| beamer delete section | `count:339` (a counter of `\beamer@overlaycounterresets`) | 14 exec, 625-744, a chain frame after frame | `\global\c@x=\temp` in `\beamer@restorecounter`, `\temp` this frame's symbol plus 2 steps (59 forces) | yes: a step (§2.5) |
| beamer delete section | `count:285` = `\beamer@totalheads` | 30 exec, 1190-1430 | `\ifnum\beamer@headcounter<\beamer@totalheads`: `\dohead`'s loop over every `\@@head<n>` of the `.nav`, in every frame | no: the loop runs over a shorter list. Not a relative counter (§7) |
| beamer delete section | `count:332`, `365`, `280`, `31`, `297` | forced, no reruns of their own | `\csname` names with a symbol inside | yes where the new name holds the old one's meaning (§2.2) |
| biblatex add citation | `count:332` = `blx@sectionciteorder@0` | 21 exec, 140-157 | `\immediate\write` of `<bcf:citekey order="N">` to the `.bcf` (26 forces) | no: the `.bcf` text changes (§4, refusals) |
| biblatex add citation | `count:302` = `\c@instcount` | 5 exec, 39 | `\ifnum` (7) and `\csname` (4) | yes (§2.2, §2.6) |
| Grail delete figure | `count:303`, `count:292` | 18 resume, 52 | the caption's number, typeset | no: the digits are typeset |
| amsldoc delete figure | `count:189` = `\g_shipout_readonly_int` | finishers | a page-layer register | already deferred (docs/spec-deferred-reads.md) |

Two shapes make a **chain** (frame after frame, each woken by the one
before): a piece that forces a counter and then *sets* it by value
(beamer's restore) publishes a definition the next piece reads. A piece
that only steps publishes a relation (`+k`, `crate::regfold`) and no chain
forms. So the first rule is that no use of a symbol turns the piece's own
steps into a value unless TeX's value could differ from `old + delta`.

## 2. Mechanism: relative symbols

### 2.1 Relative registers

A register is *relative* in a piece when every write the piece makes to
it is an increment by a constant, with `\advance` or `\addtocounter`
(through the run's register map, `Run::reg_*`, as for the page-layer
registers):

- The piece does not define the register's value. It contributes its
  map, `+k`: the regfold composition already used for `\c@page`.
- A read of the register (`\the`, `\number`, `\arabic` reaching
  `\number`) yields a symbol `R(loc, P, j)`, where j is the piece's own
  steps before the read. It stands for "the register reaching piece P,
  plus j". It flows as the page-layer symbols flow (§2 of the spec):
  through `\edef`, `\xdef`, `\toks`, deferred `\write`s, marks.
- The register is relative only while the piece's writes are all
  increments. The first absolute write (`\setcounter`, a `\global` of
  another value, `\c@x=\c@y`) makes it concrete for the rest of the
  piece: that read forces, and the piece defines the value from then
  on, as today.

Which registers are candidates: those the hints show only `\advance`d,
plus a per-document list kept from the last build (registers that never
took an absolute write). A wrong guess costs a rerun, never a wrong
output: a forced read is an ordinary read.

### 2.2 What forces a relative symbol

As in the spec's §2, forcing is any place TeX needs the digits now, plus
two new rules:

- **Numeric tests:** `\ifnum`, `\ifodd`, `\numexpr`, an assignment of
  another register, `\romannumeral`, `\int_compare`. These force (an
  ordinary read of the reaching value, plus j).
- **Typesetting, `\immediate\write`, `\message`, `\special`:** these
  force.
- **`\csname ... \endcsname` with symbols inside: a *relative name*, not
  a force (new).** The name is `prefix <R> suffix`. Its meaning is a
  *derived read*: field `cs:prefix<v+j>suffix`, where v is the
  register's value reaching P. The engine records the read as "the
  value of the field named by f(v)", with f fixed by the prefix, j and
  the suffix (an engine read path, `Step::Derived`; §3). The piece
  depends on the *answer*: when v moves, a re-ask computes the new name,
  reads that field, and wakes the piece only if the answer differs.
  biblatex's `blx@page@<n+1>` after the shift holds what `blx@page@<n>`
  held, so the citation is not woken.
- **`\ifx` / `\if:w` / `\if_meaning:w` against a token that cannot be a
  digit** (`\scan_stop:`, `\q_nil`, a non-digit character): answered
  without forcing. A non-empty digit string never begins with such a
  token. Against a digit, these force.
- **`\string`, `\detokenize`, `\meaning`:** these force (they make
  characters).

### 2.3 Definitions and writes made with relative symbols

- `\gdef`/`\xdef` of a *relative name* (`\csgdef{blx@page@<R>}`) defines
  a field whose name depends on v: a derived definition. The piece's
  output is "the definition of f(v)". The engine resolves it when it
  commits the piece's outputs (it reads v there, as an ordinary read
  the commit makes). A shift of v moves the definition to the next
  name. Readers of the old name are woken as for any moved definition.
  Readers that read through the same f (§2.2) see the same value and
  stand.
- A deferred `\write` holding symbols (`\abx@aux@page{<R>}{<page>}`) is
  resolved by the finisher at shipout, as page-layer symbols are. The
  `.aux` text changes (shifted numbers), but its per-key definitions
  (`wdef:`) come out as `blx@page@<n>` values shifted by one name. Their
  readers read through the derived path and stand.
- An `\immediate\write` forces (the text is printed now).

### 2.4 Symbolic file sizes

`\pdffilesize{<written file>}` in a piece yields a size symbol
`S(file, P)`, with one fixed property: it stands for a non-empty digit
string exactly when the file is there.

- Its *presence* is read through `Step::Present` on `wsize:<file>`. Only
  forcing reads the value.
- Forcing follows the §2.2 rules: numeric tests force, and
  `\int_compare_p:nNn {#4} = {#2}` (expl3's no-extension branch)
  forces.
- `\if:w \scan_stop: S \scan_stop:` and `\use_none:n S ?` / `\tl_if_blank`
  answer from presence alone. `\use_none:n` takes the symbol as one
  token. That is exact, because every first-token comparison of a
  non-empty digit string with a non-digit gives the same answer whatever
  the digits are.
- A symbol reaching typesetting, a write, or a name forces. None of
  these happens in `\file_if_exist` for a name with an extension
  (`.aux`, `.toc`, `.out`, `.bbl`).

### 2.5 Assignments of a symbol

`\count<n>=<symbol>` (or a `\countdef` register holding one) where the
number ends at the symbol (`defer::try_symbolic`, `control::assign_int`):

- this piece's own symbol of the same register with `j = 0`: a restore,
  the run's map the identity (docs/spec-deferred-reads.md §2);
- this piece's own symbol of the same register with `j != 0`: the value
  reaching the piece plus `j`, a **step**: the run's map becomes `+j`
  whatever it held (`defer::restore_plus`, `State::restore_plus`), neither
  a read nor a definition. beamer saves `\the\c@x` after the frame stepped
  it and sets it back at every overlay; forced, that set was a definition
  by value that the next frame read (the chain of §1.2). Only where a step
  may be a relation (`Run::map_ok`) and the run made no definition of the
  register; elsewhere the digits are forced;
- another register's or another piece's symbol with `j = 0`: an alias
  (`defer::alias`); with `j != 0`: forced.

### 2.6 Comparisons and expressions

- `\ifnum <symbol or register unread> <rel> <constant>`: the run reads
  only the comparison's outcome, `Step::Cmp(k - j)` on the register's
  field (`Run::reg_order`, `scan::Num::Reg`). It stands while the outcome
  stays: `\g__mark_int < 99999` after any shift that keeps it below.
  Two relative operands, an operand that does not end at the symbol (a
  digit follows), a negated one: forced.
- `\numexpr <symbol> + <constant>` (or `-`) as a deferring scan's one
  quantity: a symbol with `j` plus the constant (`etex::scan_expr`'s
  `sym`, `Run::defer_sym`), so `\the\numexpr\c@x-1` and
  `\ifnum\numexpr\c@x-1>100` stay relative. Any multiplication,
  division, a sum of two symbols, an offset of 2^16 or more: forced.

## 3. Engine support needed

`Step::Derived(f, j)` is a read path whose answer is the value of the
field named `prefix <v+j> suffix`, where v is the value of a base field
(the register's reaching value, `At::Before(P)`):

- **Recording:** the piece records it as one read, with the base field
  and the derived field both noted.
- **Re-asking:** when either the base or the derived field moves, the
  engine computes the name anew, reads it, and compares the answer.
  Equal: the read stands (no wake). This is the comparison `range_stands`
  makes for folds, applied to one composed lookup.
- **Derived definitions:** the definition's field is computed at commit.
  The index files it under the computed name, so a later reader of that
  name sees it.

This is generic: any `\csname` built from a counter goes through it. It
covers hyperref's `Hy@...`, `Item.<n>`, `pgfid<n>` and `page@<n>`
names. The page-layer `\c@page` symbols of defer.rs become one case of
relative registers (their maps are the finishers' steps).

## 4. Exactness

- A symbol is resolved to the digits `\number` would have printed at
  that point in TeX's order (the spec's byte-identity rule). Forcing is
  an ordinary read. A derived read answers exactly what TeX's
  `\csname` lookup of the computed name answers at that position.
- `PHITEX_RESUME_CHECK` and `PHITEX_HINTS=adversarial` must stay green.
  A register wrongly guessed relative falls back to forcing at its first
  absolute write. The run is then the one it would have been, its reads
  concrete.
- `defer::leaked` covers the new symbols: none may reach the link, a
  files' node or a tool. A `\write` is resolved at shipout, or forced
  for `\immediate`.

## 5. Plan and tests

1. **Size symbols (§2.4).** Test: a heading edit in a document with
   `\tableofcontents` wakes no `\IfFileExists` reader (amsldoc's TOC
   resume, `wsize:` gone from its reads). A `.toc` going absent → present
   forces, and the PDF equals a cold build.
2. **Relative registers in pieces (§2.1, §2.2 forcing only, no derived
   reads).** Test: a counter only stepped and typeset at the end gives
   the same PDF. Run counts unchanged.
3. **`Step::Derived` in the engine (§3) and relative names (§2.2).**
   Tests in `crates/core/defuse` (re-ask compares the composed answer).
   A plain test: `\csname x@\the\c@n\endcsname` after a step added
   before it does not wake the reader whose answer is the same.
4. **Derived definitions and resolved writes (§2.3), then biblatex.**
   Test: the biblatex example, `\cite` of a cited key, under 100 ms
   local -j1 and -j8. No `\printbibliography` rerun. PDF equals cold
   and the oracle.
5. **Sweep:** biblatex manuals, hyperref documents, tikz `remember
   picture` documents, at -j1/-j4/-j8.

## 6. Refusals (forcing is the answer, by design)

Each is an ordinary read of the value reaching the piece plus `j`, the read
TeX makes, so the piece runs again when it moves:

- the digits typeset (a caption's or a heading's number), `\message`,
  `\special`, `\string`, `\meaning`, `\detokenize`;
- `\immediate\write` to a file: its text is in the file now, and a tool
  reads it (biblatex's `.bcf` citation order). Deferring it would put a
  symbol in a files node and a tool's input; the line's digits do change
  under a shift, so the tool runs again anyway;
- a name the run binds itself among those of the same shape
  (`defer::binds_like`), digits next to the symbol in a name or a number,
  a minus before it;
- a symbol met in another piece's run than its own (resolved there as the
  register reaching its piece, `defer::resolve`, which is a read);
- a register the run set by value (`\setcounter`) or loaded and wrote:
  concrete for the rest of the run.

## 7. What a shift does that this does not cover

- **A loop over a list whose length moved** (beamer's `\dohead` over the
  `.nav`'s heads in every frame, 1.2-1.4 s of the beamer delete-section
  edit): each frame reads every `\@@head<n>` and the count; after a
  deleted section the list is shorter and its later entries carry other
  page numbers, so the frame's reads do change. 27 of its 30 reruns
  published nothing a reader saw. Standing there needs the frame's
  dependence narrowed to what the loop's outcome used, a different
  mechanism (a read of the loop's outcome, not of each entry), not a
  relative value.
- Fonts first loaded by a later figure (Grail delete figure, 4 s) and
  `pdf:rtdests`, `HyPL@LastType` finishers: other lanes.

## 8. Where it stands (2026-11-06)

Built and on dev before this spec: size symbols (§2.4); `\the`/`\number`
of any integer register unread in a piece is a symbol with the piece's
steps (§2.1); steps after a candidate as a relation (`Run::step_ok`);
`Step::Derived` in the engine (`Ctx::read_derived`, `crates/core/defuse/
tests/derived.rs`).

Built by rel1: derived names in `\csname`, `\ifcsname`, `\ifx` and a
character run's lookahead, `\ifnum` and `\numexpr` of a relative
register (from xref11/xref12's WIP, merged); assignments of the piece's
own symbol plus steps (§2.5). Tests: `tests/defer.rs`
`a_name_built_from_a_stepped_counter_reads_through_the_name`,
`a_counter_set_back_to_its_value_after_steps_is_a_step`.
