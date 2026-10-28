# Layers: paragraphs, page breaking, finishing (feasibility, 2026-10-21)

Status: a feasibility note, nothing built. The question (user): can the
def-use of paragraphs be a layer of its own, apart from page breaking,
and expansion apart from line breaking? Numbers are from fire-chain
100ab925 and its jobs (15699 walls, 15745 paragraph phases, 15648 merge
times) and from r6-scale's P0 recordings (`note-page-chain-p0.md`).

## 1. The proposed layers

1. **Expand** (the state chain): tokens to definitions and hlists;
   `\hbox` packing stays here (`\wd`, `\settowidth` read it inline).
2. **Break paragraphs** (pure): line breaking, hyphenation, shaping,
   math, vertical packing. Gives each paragraph's profile and items.
3. **Break pages**: TeX's page builder as one plain loop over the
   profiles, not one engine node per page.
4. **Finish pages**: one finisher per page, in parallel, on page-to-page
   state guessed from the previous build and validated.
5. **Write and link**, then rounds for read-back files.

Today, layers 1 and 2 are one piece node, and layer 3 is a chain of
nodes: one scan and one continuation per page.

## 2. Page breaking as a loop (layers 3-4)

- **para4000 at j8, lane on:** the critical chain is 1474 links (1378
  scan/continuation, 93 finish). Each link costs ≈0.85 ms of engine
  time: dispatch 0.80 ms, op 0.17 ms, wait_merge 0.29 ms (15648). The
  scan op itself is 0.09 ms.
- **As a loop:** 692 pages × 0.09 ms ≈ 62 ms, run serially inside one
  node. para4000's pieces read nothing a finisher defines (P0 §1: 0
  refuted), so the path would be about 4 links: piece → loop →
  finisher → link. The chain cost drops from ~1.2 s to ~0.1 s.
- **amsldoc:** the path is 498 links today, and the pages are not its
  bound: exec 342, resume 75, scan 39, finish 29. A page loop removes
  about 70 links. The rest is the piece→piece chain of §4.
- **What the finisher chain truly carries** (pdfTeX/LaTeX, P0 §1):
  - the page counter `\count0`;
  - the marks (bot to top);
  - held-over insertions and the insertion boxes (`box:253`,
    `box:56`);
  - `\deadcycles`;
  - the float lists (three macros);
  - the column state (column height and room, nine column-mark macros);
  - the page parameter record (`\vsize` etc.; the builder takes a
    routine's from its give-back in fold order, never by a finisher's
    place, spec-settle.md §2.1);
  - fonts the routine loads first (`fontinfo:64`);
  - the PDF writer's object count (prefix sums, not a value chain).
  Every item except the float lists is a counter, a short list or a
  constant per page. A guess from the previous build or from the page
  before is right on almost every page (para4000: 692 finishers created
  with a prediction, 0 mispredicted at j8).

## 3. Page → paragraph feedback layer 1 must predict

Counts are from amsldoc (P0 §1, §4 and this branch's traces) and from
the 73 `.tex` files of the latex fixtures (static, files using it).

| feedback | amsldoc | latex fixtures | how layer 1 copes |
|---|---|---|---|
| `\thepage` / `\count0` read in text | 13 readers, 5 with a finisher-only value | 1 (`\thepage`), 2 (`\pageref`, read back) | guess, validated: rerun the reader |
| `\pagetotal`, `\pagegoal` (`page_dimen`) | 3 pieces | 1 each | true page-state readers: they read the loop's state at their candidate and wait for it (rare) |
| `\deadcycles` at `\end` | 3 | n/a | the loop reads it at the end piece |
| object numbers after a page fired inside the paragraph (`\pdflastximage`, `\pdflastlink`, font names) | the run-through fires below; linkloop and imgloop in pdftex | 3 images, 10 links | the piece ends before the read and a second pass cuts at the candidate (`Halt::Recut`, this branch) |
| marks read in text | 9 column-mark macros, 1 each | 1 | guess, validated |
| insertions and floats (lists the text reads when placing) | 3 float-list macros, 2 each; `box:56`, `box:253` 2 each | 5 footnote, 6 figure, 11 table | guess, validated; floats are the likeliest misses |
| `\hsize` set by an output routine | 0 | 0 (4 two-column or multicol files set column widths in text) | a page-parameter field read by its paragraphs |
| `\vsize`, page parameter record | 11 pieces refuted | 0 | the loop reads it; text reads are rare |
| a paragraph a page fires in (run-through candidate) | 101 | not counted | the finisher's check (`check_cut`): definitions are restored; numbers are recut (row 4) |

All of these are read through fields. None needs the page as a node
between paragraphs, except the true page-state readers (row 2).

## 4. Splitting expansion from line breaking (layers 1-2)

**Where a paragraph run goes** (µs per paragraph, `PHITEX_PAR_PROF`,
15745):

| | para4000 | amsldoc |
|---|---|---|
| exec run total | 225 | 648 |
| char_run (characters to the list) | 39 | 26 |
| other expansion and main loop | 26 | 365 |
| line_break, of which hlist conversion / params / break / encode | 88 (6 / 12 / 40 / 29) | 22 (2 / 6 / 7 / 6) |
| page_fold (units, check data, markers) | 68 | 228 |
| finish | 4 | 7 |

(amsldoc's split is from a run of this branch with a defect since
fixed. The run sizes match the exact build within a few percent.)

- **para4000:** expansion is ≈29% of a paragraph, line breaking 39% and
  page_fold 30%. No chain runs through paragraphs (P0 §2: longest 4), so
  splitting buys nothing there. The whole document is parallel already.
- **amsldoc:** expansion is ≈60% and line breaking 3%.
  - page_fold is 35%: per-piece check data and contribution units, not
    typesetting. It is the cheapest target.
  - Of the 498 path links, about 417 are pieces (exec 342, resume 75).
    Each carries its whole run, where its successor needed only state
    outputs: the heading and anchor macros, counters and scratch macros
    of P0 §2.
  - With line breaking and page_fold off the chain, a link costs about
    the expansion share, ≈390 µs instead of 648. So ~40% of the pieces'
    chain time could go.
- **Feedback from line breaking to expansion:**

  | feedback | amsldoc / latex fixtures |
  |---|---|
  | `\prevgraf` after a paragraph | 1 fixture. In para4000, `\prevgraf` is written twice a paragraph and read 4000 times as 0 |
  | `\prevdepth`, `\lastskip`/`\lastpenalty`/`\lastkern` after a paragraph | the main-list tail fields: 244-246 readers of `vlist:pd`/`vlist:since`, 85 linked `vlist:last` links in amsldoc. The interline glue can be emitted symbolically (spec-page-chain §5.2); only readers of the value wait |
  | `\lastbox` after a paragraph | 0 fixtures |
  | `\vsplit` | 0 fixtures |
  | the paragraph shape reset after a paragraph (`\parshape`, `\hangindent`, `\looseness`) | static: TeX resets them at every paragraph end, so no wait |
  | footnote insertions and `\vadjust` from lines | the page loop's, not expansion's |

  Rare and field-shaped: expansion of paragraph n+1 waits for line
  breaking of n only where it reads one of these as a value.

## 5. Verdict

- The page layer as a loop is feasible and is the bigger win for
  para4000-like documents: ~1474 links become ~4, about 1 s at j8.
- Splitting expansion from line breaking is feasible with few true
  feedbacks. It pays on chained documents (amsldoc), where it cuts the
  pieces' chain time by ~40%. Taking page_fold off the chain is the
  first, cheapest part of that.
- Neither needs the scan, continuation or finisher interleaved between
  paragraphs. Page-state readers (3 in amsldoc) and numbers after a run-
  through fire (recut) are the exceptions, and both are handled per read.
