# The corpus manuals that finish but differ from pdfLaTeX (note, 2026-10-28)

Status: what the differs agent left open. The set is the 233 pdfLaTeX
manuals that finished but differed at -j1 on 75bdefadd (the docs
agent's sweep 911115707 had 256, of which 23 were already identical).
Every number below is from accl.

## Where it stands

| commit | what it fixed | identical of the 233 |
|---|---|---|
| 75bdefadd | (start) | 23 |
| 2eabbe30f / c322bfde | own-spec glue is no `zero_glue` for protrusion (`\hskip\z@` before a heading number) | 44 (3f92a51b) |
| 760be421 | an `\immediate\pdfxform` counts the pending forms it writes (pgf `/pgfsmask<n>`) | |
| 3f92a51b | a file name in braces (`\openin\x{name}`, TeX Live's `scan_file_name_braced`) | |
| 15ffe22b, 882857e2 | a run that closes its scope inside its paragraph ends there, the rest a resume; defuse `levels()` keeps a closing node's local definitions inside the scope it closes | 73 (882857e2) |
| b73794fb | a `\glueexpr` glue is a spec of its own (KOMA-Script heading numbers protruded) | 110 (b73794fb) |
| c7cd0129 | pdfTeX's full "Font ... not loadable: ..." error text | (no bytes change) |

Sweeps:
- sweep-882857e24b8f-20261028-164158: all 233.
- sweep-b73794fb4c3f-20261028-173014: the 164 not identical on 882857e2.

The 123 left are grouped below by their first difference: the first
token of the first page's content stream that differs, and the first
word of the page text that differs once page numbers are taken out.
Those two scripts (`pages.py`, `ptext.py`) and the group lists live in
`~/code/tmp/phitex-differs/target/ana/`, outside the repository.
`bench/fixture-tmp.sh NAME` (or `DIR/NAME.tex`) blesses a fixture with
pdflatex in the CI image and builds it with a gate store's phitex at -j1
and -j8. That is how every fixture here was made.

## The 123, by first cause

### A. Fonts that only METAFONT makes: 6

crbox, ghab, chemformula-ru, disser, russ, serbian-def-cyr.

- pdflatex's kpathsea ran `mktextfm` for these: `ghab.tfm`, and the T2A
  LH fonts (`larm1000` …) of the four Cyrillic manuals.
- crbox and ghab also need `mktexpk`: `ghab.600pk` (ljfour), embedded as
  a Type 3 font. That font is the oracle's extra `/F41`, which shifts
  every font number after it by one.
- The four Cyrillic manuals need TFMs only: their glyphs are cm-super
  Type 1 fonts, reached through the map.
- PhiTeX has no mktex and no PK/Type 3 writer.
- Decided: METAFONT, gftopk and mktextfm/mktexpk in Rust, in process.
  Another agent is building them. pdfTeX's error text for the missing
  TFM is in c7cd0129.

### B. Timeout in the last sweep: 1

moderncv (moderncv_userguide). It ran over 120 s on b73794fb. On
882857e2 it finished, and its first difference was a page number in the
table of contents.

### C. Only the font resource numbers differ (same text, same layout): 4, and 4 more in G and H

| document | expansion | difference |
|---|---|---|
| acmart sample-acmlarge | microtype | LinBiolinumTB is `/F220` in pdfLaTeX, `/F99` in ours: 121 off, and the order of all later fonts differs; ours uses a LinLibertineT `/F43` that pdfLaTeX never names |
| anonymous-acm | microtype | `/F236` against `/F86` |
| edmaths | microtype | `/F117` against `/F53` |
| moreenum | none | Pagella (TS1) is `/F77` in pdfLaTeX, `/F76` in ours; every other number is equal |

Also numbering, with other differences too:
- fancyhdr-it: CMTI7 `/F128` against `/F127`.
- sidenotesplus: +2 from page 1.
- perfectcut: SourceSansPro-Bold +1, then equal again.
- biblatex-cheatsheet: the order differs (`/F100` against `/F83`), then
  converges (`/F191` and after are equal).

Ruled out:
- Expanded fonts made while paragraphs are broken. PhiTeX makes them
  in pdfTeX's order (`View::exp_log`, `get_expand_font`). Six fixtures
  with microtype were identical at -j1 and -j8 (accl job 21895): a
  paragraph, a `\parbox`, a minipage, a `\vbox`, an abstract and a
  footnote, each followed by fonts loaded later.
- A paragraph that straddles a page break while the output routine
  loads a font: the routine's font after the paragraph's, as in TeX
  (job 21931, identical).
- pdfTeX's choice of a same-TFM font's PDF resource is the first one
  *used*. `PdfState::init_font` walks the used fonts as `pdf_init_font`
  does, so the numbers come from the load order (`FONT_LOADS`, the
  link's `FontNums`).

Next step: get pdfTeX's order of loads. NFSS loads can be logged by
wrapping `\define@newfont` in a copy of the manual, run by both
engines. Compare that with the order PhiTeX's link numbers its loads
in. The acmart/libertine fonts are virtual fonts with expansion
(`auto_expand_vf`, the `Fresh` locals in pdfout.rs), the likeliest place.

### D. Same text, positions differ: 8

argumentation, diagmac2, guitartabs, heria, pygmentex (`Td`); iacrj
(`Tm`); harnon-cv, snaptodo (`cm`). Not looked at.

### E. Same text, some other content differs: 24

- 10 where the page content streams are equal and another object differs
  (annotations, forms, resources): hologo-example, cmpj, longdivision,
  metastr, nddiss, prtec, rubik, spotcolor, syllogism, tablefootnote.
- 4 with different `TJ` spacing on equal words: easyformat, gitstatus,
  huaz, transparent.
- 5 pgf transparency and graphics states (`gs`):
  - beamertheme-csh, beamertheme-cuerna, thuaslogos, thubeamer: the
    /pgfsmask numbering is right since 760be421.
  - cuerna's `\immediate\pdfxform` of the fading mask was empty in ours,
    with no `/XObject` resource: a second fault, not looked at.
  - svg-inkscape.
- One each: milog (gs/q), rvwrite (k/g), proofread (m/m), typogrid
  (re/re), els-cas-templates (rg/g).

### G. A line broken elsewhere: 14

adjmulticol, amsldoc-it, continue, ddphonism, easy-todo, figchild,
jourcl, karnaugh, metre (greek1), pagecolor, perfectcut, readablecv,
ticollege, yet-another-guide-latex2e. Not looked at. Some may be the
same font size or font numbering faults that show first in a break.

### H. Page, float, footnote or other layout: 66

Recurring kinds among them:
- Page numbers in a table of contents, from a page break further on;
  the first real difference is often a footnote placed on another page:
  droit-fr, teubner, texpower, dtxtut, metacapture, minitoc mtc-2c,
  tableof, unswcover, ijmart, polski, udes-genie-these. In droit-fr,
  metacapture and teubner, our page takes a footnote that pdfLaTeX
  moves on.
- The first heading of the front matter ("Abstract", "Overview") a few
  points higher or lower: fixjfm, ifxptex, fancyqr, listparskip,
  math-operator (mathfont and pinlabel similar, `Tm`).
- One author's manuals that end a page at the repository URL line:
  eoldef, visualtoks, freemath.
- The rest, one or two each: achemso, addfont, amsldoc-vi, asmeconf,
  biblatex-cheatsheet, bytefield, cascadilla, changes, chet,
  correctmathalign, couleurs-fr, coursepaper, dvdcoll, elsarticle,
  euromoney, europecv, examplep, fancyhdr-it, feynmf, figput, fragoli,
  gerdoc, ibycus-babel, limecv, mecaso, minutes, nrc, numnameru,
  przechlewski-book, repeatindex, sageep, scanpages, semioneside,
  shapepar, sidenotesplus, songs, soulpos, tamefloats, tram,
  translation-enumitem-de, tugboat ltubguid, typstfun, ucalgmthesis,
  webquiz, zed-csp.

## Notes for whoever goes on

- `pages::an_edit_that_moves_a_break_out_of_a_claimed_span` failed once
  in a gate here (21709). It is on the intermittents list as a real bug,
  not noise.
- The sweeps' `SWEEP_IDS` is the id list; the 233 ids are the keys of
  `target/ana/cat.json`, and the 123 left are in `target/ana/remaining.txt`.
- mc runs sweeps faster: `CLUSTER=mc SWEEP_IDS=... scripts/accl/corpus-sweep COMMIT`
  (after that commit's mc gate).
