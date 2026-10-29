# XeTeX corpus sweep and where XeLaTeX stands (2026-10-28)

Agent "xetex"'s last report on XeTeX (docs/spec-xetex.md is the
design). It covers the corpus sweep (milestone 3), the user's paper at
its fixpoint, the one deliberate difference from TeX Live, and every
xelatex difference still open, each grouped by its first cause with a
reproduction.

## 1. The sweep

`scripts/xetex/sweep.sh` (accl job 21761, dev 34a96c04 + the origins
commit 8976c9cd). The documents are those of
`crates/output/xdvipdfmx/tools/corpus.sh`: 211 in all.

- 192 l3build tests: fontspec, unicode-math and polyglossia `.lvt`.
- 7 probe documents.
- 12 xelatex documents from `tests/e2e`.

Each runs once under TeX Live 2026's `xelatex -no-pdf
-interaction=nonstopmode`, which is the oracle (SOURCE_DATE_EPOCH=1700000000,
FORCE_SOURCE_DATE=1, fontconfig on TeX Live's fonts only). Each also
runs under `phitex --compat=xelatex -no-pdf` at `-threads=1` and
`-threads=8`. The XDVs are compared byte for byte.

| outcome | documents |
|---|---|
| XDV byte-identical, -j1 and -j8 | 83 |
| no XDV from either (log-only tests) | 80 |
| XDV differs (same verdict at -j1 and -j8) | 47 |
| our XDV missing where xelatex wrote one | 1 |
| timeouts (120 s xelatex, 300 s ours) | 0 |

-j1 and -j8 give the same verdict on all 211 documents. Error counts
(log lines beginning `!`) agree on 169 of 211.

## 2. The differences, by first cause (largest first)

### 2.1 Page-one specials of l3build's `regression-test.tex` lost (46 XDVs)

`regression-test.tex` issues `\special{dvipdfmx:config z 0}`,
`\special{dvipdfmx:config C 0x40}` and `\special{pdf: docinfo ...}` in
vertical mode, before `\documentclass`. In xelatex's XDV they stand at
the top of page one's body. Ours has none of the three, and every later
position on the page moves with them. This one cause accounts for 46 of
the 47 differing XDVs (42 at byte 156, 3 at byte 209, 1 at byte 594):
each oracle XDV has `dvipdfmx:config` and ours does not.

Reproduction (accl jobs 21911 and 21951; files in the job's `scratch-x`
form):

- Same: `\input regression-test.tex` + `\documentclass{article}` +
  `\begin{document}` + a word (with or without `\START`).
- Same: a `\special` before `\documentclass`, in the document itself.
- Differs, `fs5`: the same document plus `\usepackage{geometry}
  \geometry{papersize={9cm,6cm},margin=0.5cm}`.
- Differs, `fs6`: the same document with `\START\OMIT` before the word.
  `\OMIT` writes to the log only.

**Cause (found 2026-10-29, fixed in 6de5ca3b1).** The page builder
was not the cause. `regression-test.tex` is a distribution file, so the
run reads it inline, as one load. It does
`\input{regression-test.cfg}`, and the `.cfg` in the job's directory
is a document file cut into regions. A document run stops at a
document file and leaves the rest of its input as child nodes
(`Run::leftover`). The rest of the file it was reading inline became a
node of kind 0, a format's node. That node ran lines 215-302, and
whatever it put on the main list never reached the pages. The three
specials were dropped, and a letter in that rest ended the build with
no DVI at all. The same happens to any file read inline (one with
`\endinput`, `\inputlineno` or `\everyeof`) that inputs a document
file and then contributes to the main list. The fix gives the rest, and
a written file's units read the same way, the run's own kind, as its
line frames already had (`crates/engines/tex/src/run.rs`, `file_left`).

How it was cut down (mc 76025-76195, accl 22174-22178):

- The trigger is `\input{regression-test.cfg}` inside an `\input`
  file that is read inline, followed by the specials. It does not need
  open conditionals, `\documentclass` or `\OMIT`.
- In plain TeX: `q.tex` is `\input in1` / `\special{AAA}` /
  `\endinput`, where `in1.tex` is `\relax`, and the document is
  `\input q` / `hello\bye`. pdftex keeps the special and ours lost it.
  8 of 9 variants differed. The one that matched was
  `\csname endinput\endcsname`, which is not seen in the text, so that
  file is cut into regions as usual.

Test: `inputs::a_file_read_inline_goes_on_after_the_document_file_it_inputs`.
It fails on dev before the fix (accl 22182) and passes with it (accl
22181).

Against TeX Live, byte for byte (accl 22181, 22183):

- plain v1-v9 (pdftex DVI);
- the latex cut-downs oc1-oc10;
- fs5 and fs6, under both xelatex (XDV) and latex (DVI).

The 46 corpus XDVs themselves need a fresh sweep (§6).

### 2.2 ICU line breaks: a break before `〜` for `ja` (1 XDV, fixed here)

`probe-xelatex-cjk`: xelatex keeps `バ〜！？` in one native word, ours
broke it before `〜` (U+301C WAVE DASH). TeX Live's ICU 78 gives the
locale `ja` (and `zh`) its `line` entry, `line_normal.brk`, which has no
break before `〜` or `゠`. Only `@lb=normal` and `@lb=loose` select the
`_cj` rules that allow one. We passed ICU4X `content_locale` ja for the
bare locale, and in ICU4X that turns on the `_cj` behaviour. The fix:
the content locale only under an `lb` keyword
(`crates/engines/tex/src/xetex.rs` `line_breaks`). Fixture
`oracle/fixtures/xetex/ini/linebreak-cj.tex` (ja, zh, zh_TW, ja_JP and
the three `lb` values, with `〜` and `゠`). Its oracle was made by TeX
Live's xetex (job 21929) and it is part of `ini_linebreak`. It fails
before the fix (job 21969: byte 359, 2148 bytes for 2076) and passes
after (job 21929), at -j1 and -j8.

### 2.3 The job writes no XDV (1 document)

`probe-xelatex-filemoddate` (`tests/e2e/xelatex-filemoddate.tex`):
under `--compat=xelatex` the run stops with `! Missing \endcsname
inserted.` at line 15 (`\shape\logdate`), then 198 errors after it.
xelatex has none. The document takes `\filemoddate{\jobname.log}` apart
character by character. The first thing to check is what our
`\filemoddate` of the job's own log gives in compat mode (§4: it is the
build's clock, `D:20231114221320Z` under this SOURCE_DATE_EPOCH). Not
investigated further.

## 3. Logs: error counts (42 documents differ)

- **`\showbox` and `\showlists` (36 documents; exit status 0 against
  xelatex's 1).** TeX §1293–§1298 shows the box or the lists, then
  `print_err("OK")` and `error`. In nonstop mode that writes `! OK.`
  to the log and sets the history, so the exit status is 1. Our
  `show_whatever` (`crates/engines/tex/src/control.rs`) scans the box
  number and shows nothing. These are mostly polyglossia's and the
  probe tests' `\showbox0`. This is pdfTeX's path as well, and the XDV
  is unaffected.
- **`probe-xelatex-math-body`.** xelatex has `! Misplaced alignment tab
  character &.` 21 times and stops (`! Emergency stop.`). We have
  `! You can't use '&' in ... mode.` 21 times and go on. This is the
  `&` of an alignment preamble when its alignment has ended.
- **`fontspec-fontload-onesize-nosc`.** For a font file that is not
  found, xelatex writes `! Font TU/...=[TimesLTStd-Bold.otf]/OT at 10.0pt
  not loadable: ...` (8 times) and `! Too many }'s.` twice. We write
  `! Font not loadable.` without the font's name and file, and no
  `Too many }'s`.
- **`fontspec-script-lang-chinese`, `-hebr`.** One error more than
  xelatex: one more `Cannot use \XeTeXOTlanguagetag with nullfont` and
  one more `Use of \??? doesn't match`. xelatex has one `Missing number`
  there instead.
- **`probe-xelatex-lang`.** xelatex reports `! Missing number, treated
  as zero.` at `\c@م` (an Arabic counter name); we report nothing.
- `probe-xelatex-filemoddate` (§2.3).

## 4. The user's paper at its fixpoint, and the one deliberate difference

`latexmk -xelatex` (4 xelatex runs, 3 bibtex, 54 s) against `phitex
build -j8 --output-format xdv` (151 s, 2.7 GB resident), accl job 21421.
The XDVs have the same length, 1967692 bytes, and 16 pages each. They
differ in 75 bytes, all in hyperxmp's XMP packet: the four dates and the
InstanceID uuid made from them. The PDFs differ only in those, in
xdvipdfmx's font subset tags and in /ID, which it seeds from the date.
hyperxmp reads `\filemoddate` of the job's own `.log`. TeX Live gives
the file's real mtime even under SOURCE_DATE_EPOCH with
FORCE_SOURCE_DATE=1. We give the build's clock, which is the epoch
under SOURCE_DATE_EPOCH, so builds reproduce. This is the one deliberate
difference (docs/spec-xetex.md §1, decided with the coordinator).

The paper's cost (151 s against 54 s, 2.7 GB) is the page loop's: a
give-back that differs from its guess restarts every later page. The
docs agent owns that. `oracle/fixtures/latex/pdfdocs/floats2col.tex`
and the ignored test `latex_twocol_floats_runs` are its acceptance
test: finisher runs within 2x the fires at -j1 and 3x at -j8. On
8f0fd2a6 they were 15572 runs for 194 fires at -j1.

## 5. What stands (measured)

- All ini fixtures and plain documents: XDV and PDF identical to
  xetex's at -j1 and -j8.
- 15 xelatex documents, including xeCJK and ctexart: XDV identical at
  -j1 and -j8. Their PDFs through our xdvipdfmx are identical except
  gfx's, whose EPS needs Ghostscript.
- Glyph origins for XDV builds (`xelatex_glyph_origins`, all 15
  documents): each page's code count equals xdvipdfmx's, every glyph
  has an origin, and -j8 gives the same as -j1.
- The CLI: `xelatex`, `-no-pdf`, `--output-format xdv`, watch.

## 6. Open, in order of size

1. §2.1 is fixed (6de5ca3b1). The corpus sweep has to be rerun to
   confirm the 46 XDVs.
2. §3, `\showbox`/`\showlists` logging (36 documents' logs and exit
   statuses).
3. §2.3, `\filemoddate` of the job's log in compat mode.
4. §3's single-document log differences.
5. The paper's cost (§4, the docs agent's).
