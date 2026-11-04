# Relative counters and symbolic file sizes (design note, 2026-11-03)

Status: proposed by xref and asked for by the manager (2026-11-03). It
extends the deferred reads of docs/spec-deferred-reads.md (`defer.rs`)
beyond the page-layer registers. Bars: a biblatex `\cite` edit under
100 ms, and a heading edit that changes a file's size without waking
`\IfFileExists` readers. Exactness is unchanged: every edited PDF is
byte-identical to a full rebuild and to the oracle.

## 1. The two defects

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

## 6. Where it stands (2026-11-03)

Built: step 1 (size symbols); step 2 (`\\the`/`\\number` of any integer register
unread in a piece is a symbol with the piece's steps j, named by the register's
number; a symbol among a macro's delimiters is its digits). Steps after a
candidate are a relation too (`Run::step_ok`), which removes a strict chain: the
walk of a change stopped at each run that had published a value, so citations
ran one after another at any thread count.

Not built: steps 3-5. Measured on 50-style-authoryear after step 2 (mc 79454,
profile+observe, -j1 / -j8): citeadd 1591 / 1532 ms, of which `\\printbibliography`
and its neighbours through `count:302` 615 / 650 ms (6 runs) and `cs:blx@page@63`
527 / 564 ms (1 run), citations through `count:332` 103 / 170 ms (18 runs), the
rest of `\\begin{document}` on the `.aux` as text 113 / 143 ms. The `.aux` stays
text there because of real reads: `\\blx@pagesum@old` (`\\blx@checksum`, an
`\\xdef` per line), `blx@segm@...`, `blx@prelim@lastcites@...`. Needed beyond §2.2:
`\\numexpr` of a symbol plus a constant and `\\ifnum` of a symbol against a constant
(`Step::Cmp` with j), as `\\blx@iffirstonpage` and `\\blx@initsep` do.

`Step::Derived` can be filed thus: the read on the base field with the path
(`read::step` has `eng`; the anchor goes in the step), and a witness reader on
the name's field whose answer is the composed lookup, so an old name's move
compares equal; a re-ask that stands with a new name files a witness there,
after the walk's lock (`commit::reask_reader` runs with none held).

A relative `\\csname` result must itself be a symbol (`cmd::SYM`, its default the
forced read), not the concrete token: the token can reach a macro body, and the
run would then depend on the name with no read of it.
