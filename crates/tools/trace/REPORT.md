# What pdfTeX's tracing log says about the def-use structure of real documents

Measured with `phitex-trace` (this crate) on the oracle fixtures, from
pdfTeX 1.40.29's e-TeX tracing log, without implementing TeX. The
numbers describe the graph the log lets us see; the crate's
documentation (`src/lib.rs`) lists what the log does not show, and the
`--conservative` rows add those reads by rule. Wall-clock times are not
reported: this machine is loaded, and the tool measures structure.

## Commands

```sh
# every cargo command and every pdftex run through the sandbox
scripts/sandbox crates/tools/trace/corpus.sh target/trace-corpus
#   = for each oracle/fixtures/latex/docs/*.tex:
#       phitex-trace record DOC.tex --engine pdflatex --out X.jsonl --scratch S
#       phitex-trace analyze X.jsonl --json X.json
#       phitex-trace parse S/DOC.log --main ./DOC.tex --engine pdflatex --conservative --out Y.jsonl
#       phitex-trace analyze Y.jsonl --json Y.json
#     for each oracle/fixtures/plain/*.tex: the same with --engine pdftex
#       (pdftex -ini -etex on a wrapper: \pdfoutput=1 \input plain, the
#       tracing settings, \input DOC)
#     phitex-trace table *.json
```

The LaTeX documents run through the installed `pdflatex` format (the
oracles were recorded with `latex` in DVI mode; `href.tex` therefore
stops at hyperref's `dvips` driver check and `lst.tex` hits a listings
error: their logs are parsed up to the error and reported as such).
The pgf manual's sources (`upstream/`) are not checked out here and
were skipped. The plain fixture bodies are run on TeX Live's `plain.tex`
(loaded untraced, as a format would be), not on the fixture's
`plaintext.tex`.

## Columns

- steps: traced actions (primitives, characters, font selections,
  macro calls, conditionals, group characters);
- fields: distinct state slots named by an assignment or a known read;
- critical path: the longest chain of def-use edges, in steps and as a
  percentage of steps;
- edit impact: for up to 200 random lines of the main file that write
  something, the steps transitively dependent on those writes (the
  line's own steps excluded), p50 and p99 as a percentage of steps;
- memo repeat: the fraction of macro calls whose (macro, arguments'
  text, values of the fields read during the call's extent) repeats an
  earlier call's (a call's extent runs to the next macro call);
- preamble summary: fields written before `\begin{document}` and read
  after it (`-` where there is no `\begin{document}`); body writes read
  in the preamble were 0 for every document.

## Results

### Default (reads the log shows, plus the static per-primitive table)
| document | steps | fields | critical path | edit impact p50 / p99 (% of steps) | memo repeat | preamble summary |
|---|---|---|---|---|---|---|
| `ams.tex` | 250076 | 3671 | 272 (0.11%) | 0.008 / 0.236 | 0.709 | 202 |
| `bib.tex` | 105065 | 2363 | 248 (0.24%) | 0.011 / 0.212 | 0.662 | 59 |
| `enlarge.tex` | 136798 | 2594 | 308 (0.23%) | 0.169 / 1.490 | 0.725 | 71 |
| `figures.tex` | 92777 | 2312 | 252 (0.27%) | 0.006 / 0.455 | 0.646 | 58 |
| `floatgoal.tex` | 122094 | 2258 | 320 (0.26%) | 0.003 / 0.674 | 0.723 | 67 |
| `gfx.tex` | 169548 | 3047 | 264 (0.16%) | 0.002 / 0.178 | 0.692 | 155 |
| `hrefcell.tex` | 679583 | 9592 | 442 (0.07%) | 0.002 / 0.026 | 0.710 | 251 |
| `href.tex` | 678568 | 9585 | 442 (0.07%) | 0.002 / 0.038 | 0.708 | 322 |
| `layout.tex` | 383913 | 5041 | 336 (0.09%) | 0.003 / 0.479 | 0.726 | 268 |
| `lst.tex` | 207522 | 4189 | 252 (0.12%) | 0.011 / 0.329 | 0.680 | 422 |
| `ltmc.tex` | 125593 | 2760 | 296 (0.24%) | 0.015 / 0.625 | 0.674 | 124 |
| `math.tex` | 91830 | 2375 | 248 (0.27%) | 0.019 / 0.395 | 0.641 | 44 |
| `micro.tex` | 383784 | 4419 | 262 (0.07%) | 0.001 / 0.362 | 0.796 | 352 |
| `packages.tex` | 139171 | 2776 | 252 (0.18%) | 0.002 / 1.107 | 0.686 | 124 |
| `rules.tex` | 216245 | 3741 | 266 (0.12%) | 0.001 / 0.221 | 0.662 | 300 |
| `sections.tex` | 104387 | 2488 | 248 (0.24%) | 0.014 / 0.308 | 0.673 | 69 |
| `tabcr.tex` | 107714 | 2427 | 248 (0.23%) | 0.005 / 0.474 | 0.674 | 81 |
| `tables.tex` | 92404 | 2390 | 248 (0.27%) | 0.002 / 0.328 | 0.642 | 56 |
| `tikz.tex` | 873582 | 13218 | 312 (0.04%) | 0.003 / 0.621 | 0.788 | 672 |
| `titlepage.tex` | 108158 | 2373 | 298 (0.28%) | 0.043 / 1.021 | 0.687 | 65 |
| `display.tex` | 563 | 117 | 5 (0.89%) | 0.533 / 1.599 | 0.250 | - |
| `hyphens.tex` | 483 | 79 | 4 (0.83%) | 0.207 / 0.207 | 0.571 | - |
| `pages.tex` | 2141 | 85 | 5 (0.23%) | 10.462 / 22.746 | 0.475 | - |
| `story.tex` | 527 | 132 | 3 (0.57%) | 0.569 / 20.114 | 0.171 | - |
| `table.tex` | 909 | 162 | 20 (2.20%) | 0.000 / 15.072 | 0.451 | - |

### --conservative (every step after a catcode write reads `catcode:*`; a step with an unknown read depends on the step before it)
| document | steps | fields | critical path | edit impact p50 / p99 (% of steps) | memo repeat | preamble summary |
|---|---|---|---|---|---|---|
| `ams.tex` | 250076 | 3672 | 273 (0.11%) | 0.012 / 0.338 | 0.693 | 202 |
| `bib.tex` | 105065 | 2364 | 249 (0.24%) | 0.012 / 0.406 | 0.641 | 59 |
| `enlarge.tex` | 136798 | 2595 | 309 (0.23%) | 0.176 / 2.413 | 0.708 | 71 |
| `figures.tex` | 92777 | 2313 | 253 (0.27%) | 0.016 / 0.666 | 0.624 | 58 |
| `floatgoal.tex` | 122094 | 2259 | 321 (0.26%) | 0.004 / 1.496 | 0.705 | 67 |
| `gfx.tex` | 169548 | 3048 | 265 (0.16%) | 0.004 / 0.326 | 0.675 | 155 |
| `hrefcell.tex` | 679583 | 9593 | 443 (0.07%) | 0.001 / 0.040 | 0.702 | 251 |
| `href.tex` | 678568 | 9586 | 443 (0.07%) | 0.003 / 0.073 | 0.700 | 322 |
| `layout.tex` | 383913 | 5042 | 337 (0.09%) | 0.007 / 0.922 | 0.716 | 268 |
| `lst.tex` | 207522 | 4190 | 253 (0.12%) | 0.016 / 0.358 | 0.663 | 422 |
| `ltmc.tex` | 125593 | 2761 | 297 (0.24%) | 0.029 / 0.913 | 0.655 | 124 |
| `math.tex` | 91830 | 2376 | 249 (0.27%) | 0.025 / 0.593 | 0.618 | 44 |
| `micro.tex` | 383784 | 4420 | 263 (0.07%) | 0.001 / 0.676 | 0.780 | 352 |
| `packages.tex` | 139171 | 2777 | 253 (0.18%) | 0.006 / 2.143 | 0.668 | 124 |
| `rules.tex` | 216245 | 3742 | 267 (0.12%) | 0.005 / 0.322 | 0.643 | 300 |
| `sections.tex` | 104387 | 2489 | 249 (0.24%) | 0.025 / 0.667 | 0.650 | 69 |
| `tabcr.tex` | 107714 | 2428 | 249 (0.23%) | 0.008 / 0.662 | 0.653 | 81 |
| `tables.tex` | 92404 | 2391 | 249 (0.27%) | 0.008 / 0.509 | 0.619 | 56 |
| `tikz.tex` | 873582 | 13219 | 313 (0.04%) | 0.007 / 0.957 | 0.773 | 672 |
| `titlepage.tex` | 108158 | 2374 | 299 (0.28%) | 0.048 / 1.055 | 0.667 | 65 |
| `display.tex` | 563 | 117 | 8 (1.42%) | 0.533 / 2.131 | 0.250 | - |
| `hyphens.tex` | 483 | 79 | 8 (1.66%) | 0.828 / 0.828 | 0.571 | - |
| `pages.tex` | 2141 | 85 | 8 (0.37%) | 0.047 / 23.681 | 0.475 | - |
| `story.tex` | 527 | 132 | 12 (2.28%) | 1.708 / 22.391 | 0.171 | - |
| `table.tex` | 909 | 162 | 20 (2.20%) | 0.000 / 16.502 | 0.451 | - |

### Per-document detail

`target/trace-corpus/<name>.md` and `.json` (not committed) hold the
full report per document: chain-length distribution, preamble and body
graphs, region summaries. Three region rows, for scale:

| document | paragraphs: regions / avg steps / avg reads from outside / avg visible writes | environments: the same |
|---|---|---|
| `ams.tex` | 24 / 10418.9 / 188.5 / 14.3 | 11 / 8421.0 / 543.7 / 7.3 |
| `sections.tex` | 41 / 2545.0 / 68.8 / 7.4 | 4 / 17811.8 / 492.8 / 15.0 |
| `tikz.tex` | 3842 / 226.4 / 26.6 / 0.5 | 2 / 253278.0 / 31771.5 / 20.0 |

## Reading the numbers

- The critical path is a few hundred steps in every LaTeX document, at
  0.04-0.3% of the steps, and it hardly moves under `--conservative`
  (+1 step: an unknown read chains a step to its predecessor, whose own
  chain is short). The visible graph is extremely wide and shallow.
- The preamble (50-80% of the steps) ends in a summary of 44-672
  fields that the body reads; nothing flows back. Memoizing the
  preamble as a function of its inputs is a clean cut.
- An edited line reaches a median of 0.01% and a 99th percentile of
  0.2-2.4% of the steps. Most of a body's steps are macro calls whose
  (macro, arguments, read values) repeat earlier calls: 62-80% of all
  calls.
- The caveats are structural, not statistical: the log hides every
  operand read (`\the\count1`, `\ifx\a\b`, `\hskip\parindent`) and
  every catcode read during tokenization, and reports lines only at
  group and conditional boundaries, so line attribution in the plain
  documents (hundreds of steps, plain.tex untraced) is coarse and their
  edit-impact percentiles are not comparable with the LaTeX ones.
  Hyphenation, the page builder's reads of inserts and box-register
  reads are modeled by a static table or not at all.
