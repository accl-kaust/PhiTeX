# amsldoc's critical path, link by link (2026-10-21)

Status: a measure on b7ab9721 (layers-base: r8's page chain, dev, r6's
counters). It names each link of amsldoc's realised critical path at
`-j 8` by kind and field, and says which are TeX's own def→use and which
are ours. It answers [spec-layers.md](spec-layers.md) §7 step 2 ("amsldoc's
data chain named node by node").

## The runs

All on acclnode01 (16 CPUs of an AMD EPYC 7763; governor `ondemand` on
all 128 cores, boost on), jobs 15794 and 15799, three reps each. Every
PDF is the oracle's (`98c96e00`).

| | job 15794 | job 15799 |
|---|---|---|
| release `-j 8` wall (s) | 4.62, 4.71, 4.66 | 5.12, 5.11, 4.76 |
| release `-j 1` wall (s) | 3.94 | 4.37 |
| profile+observe `-j 8` wall (s) | 4.99, 4.85, 4.87 | 5.43, 5.15, 5.03 |
| realised critical path | 499 runs; 2.63, 2.55, 2.57 s | 499 runs; 2.85, 2.68, 2.67 s |

- `-j 8` runs 2301 runs for 1415 nodes; `-j 1` runs 1897. `-j 8` is
  slower than `-j 1`.
- The chain has the same 499 heights and the same kinds in all six reps.
- The engine's chain (`critical_chain_heights`) is a height chain: which
  run's result rests on which. It is not a wall-time path. A reader may
  run on a guess before its writer and be confirmed later. Its link times
  jump back and forth (readers at 4.1 s, their writers at 1.9 s), so a
  link's wall is meaningless. This note gives each link's **op**: the
  reader's run on the chain (end − start). Summed, that is 1.73–1.86 s
  over 529 links.
- The engine's realised time (2.55–2.85 s) is the longest chain by run
  time. It is a different walk, and the client cannot read it link by
  link.
- **Recording is broken on this base.** With `DEFUSE_RECORD`, the
  evaluation stops at once: "cycle among 484 nodes", 66 runs recorded,
  0 pages. That happens in an observe build and in a profile+observe
  build. The player then reports "cycle among 502 nodes". So this base
  has no play-model numbers (PLAY_DAG, DEFUSE_RCHAIN); r6's figure
  (199 nodes ≈ 627 ms) is from an earlier base.

## The chain by kind

Mean over job 15799's three reps (range in brackets). "Relay": the
writer defined the value already reaching it, so the value came from
further up.

| Kind | Links | ms on the path | Fields (links) | Real or ours | Layer / §3 case |
|---|---|---|---|---|---|
| a preamble load reading the one before | 5 | 856 (844–863) | `cs:reserved@c` 3, `cattab` 1, `cs:g_file_curr_name_str` 1 | real | §4.1: the preamble's loads run in sequence; §7 step 4 (sequential region, per-command speed) |
| a page node: scan/finish | 78 | 167 | `Created` (finisher placed by scan) 44, `rtn:j` 20, `scan:from:j` 12, `pages:end` 1, the index file 1 | ours | layers 4–5, §7 step 1 (pages as one node) |
| a page node: the cut's continuation | 29 | 109 | `pages:resume` 26, `scan:state` 3 | ours | layers 4–5, §7 step 1 (spec §8: the continuation goes) |
| a page node: scan reads the profiles | 27 | 4 | `vprof` 27 | ours (the range's last contributor) | layer 4 reads one range at the end |
| the pending `\afterassignment` token, empty, defined again unchanged | 103 piece→piece + 20 finisher→piece | 267 (253–291) | `aftertoken` 123 (122 relays of `()`, up to 149 definers back) | ours | none (a new case, see below) |
| a macro or register defined by one piece and used by a later one | 150 | 124 (120–131) | `cs:par` 34, `cs:reserved@a` 33, `cs:if@endpe` 20, `toks:12` 20, `cs:if@inlabel` 13, `cs:reserved@c` 12, `cs:if@nobreak` 5, `cs:if@nmbrlist` 5, … | real | layer 2: TeX's state chain |
| the same, a same-value redefinition | 20 | 32 (30–34) | `cs:if@minipage` 11, `cs:reserved@c` 4, `cs:if@newlist` 2, … | real in form; no new value | layer 2 (a value guess would cut it) |
| the input line count | 27 | 82 (75–87) | `lines` 26 (range), 1 (woken) | ours | layer 1 (prefix sums of lines) |
| the paragraph-shape record | 41 | 61 (57–67) | `parpar` 41 | real reads of one bundled record | layers 2–3 |
| the main list's tail | 18 | 63 (61–64) | `vlist:last` 15, `vlist:since` 3 | real (`\lastskip`-type reads) | layer 2 |
| a finisher's definitions read by text (real) | 1 | 0.2 | `pagepar` 1 | real | §3: `\hsize`, `\vsize` and kin changed by an output routine, predicted per page |
| catcode changes | 5 | 1 | `cattab` 5 | real | §3: catcode changes 2 → 1, the line's regime key |
| the first run, a run unfolding the next | 3 | 2 | creation | — | — |
| a counter | 0 | 0 | — | — | the fold holds (r6, `acc_range_after_def`) |
| `\prevgraf` / `\lastbox` after line breaking | 0 | 0 | — | — | §3 |
| the .aux read-back (rounds) | 0 | 0 | the 3 loop rounds add no link | — | layer 7 |
| an engine wait with no data dependency | 0 | 0 | — | — | — |

In total, about 1.08 s of op is TeX's own chain: the preamble (856 ms),
macros and registers (156 ms), the list tail, the page parameters and the
catcodes. About 0.63 s is ours: the page node (280 ms), the empty
`aftertoken` (267 ms) and the line count (82 ms). The paragraph record's
61 ms is in neither sum.

### Notes on the kinds

- **Counters: 0 links.** No `count:` field is on the chain. r6's affine
  fold holds on amsldoc.
- **`aftertoken` is ours.** `\afterassignment` sets the cell, and the
  next assignment in the same run reads it and sets it back to empty
  (control.rs `after_assignment`). The run then publishes the cell as a
  definition, even though its value is the empty one that reached it.
  - Every later run's first assignment reads that definition, so each run
    with an assignment links to the last run that used
    `\afterassignment`.
  - The value is always empty: 122 of 123 links relay `()` from up to
    149 definers back.
  - TeX needs the token only from one assignment to the next. On this
    chain the token is empty at every piece boundary.
  - The cut is a layer-2 rule. A run whose `aftertoken` ends at the value
    it started with defines nothing, and a piece starts with no pending
    token (a piece cannot end with one pending). None of the six layers
    lists this case; it belongs in §3's table as a guess validated at the
    run's end.
- **The page node** (134 links, ~280 ms): scan → finisher → scan per
  page, 3 links a page. The cut's continuation reads `pages:resume` and
  `scan:state`. All of it goes with §7 step 1.
- **The line count** (`lines`): every environment's opening reads the
  input line number, and the run reads the `lines` range for it
  (control.rs `input_line_no`).
  - The contributions are static (`Exec::static_contribs`). The reader
    does not wait for them; the contributor's run only confirms them.
  - The height chain still counts the range's last contributor
    (`Cause::Range`), so this is a link of validation, not of data.
  - Layer 1 gives line numbers by prefix sums over the lines, with no
    run in the way.
- **The paragraph record** (`parpar`) bundles `\parshape`, `\hangindent`,
  `\looseness` and the penalty arrays in one field. Each link is a
  paragraph whose `\par` reads the record the paragraph before set: list
  items set `\parshape`, and `\par` resets it (§1070).
  - Line breaking's read moves to layer 3.
  - `normal_paragraph`'s read stays in layer 2. It tests `\looseness`,
    `\hangindent`, `\hangafter` and `\parshape` only to skip a definition
    that changes nothing. A reset written without the read cuts it.
- **Same-value redefinitions** (20 links): `cs:if@minipage` and kin are
  set to the value they already had (`\iffalse` again).
  - In TeX's terms these are real def-use links; cold, the reader cannot
    know the value without the writer.
  - The previous build's value is a guess that is right every time here.
- **The main list's tail** (18 links): `vlist:last` read after vertical
  space commands and labels. `vlist:pd` (`\prevdepth` for the interline
  glue) is not on the chain.
- **Preamble**: one load is 774 ms of the path. That one run outweighs
  everything layers 2–3 can cut.

## The 10 longest links

From job 15799, rep 1. Each link: the reader's op, the kind and field,
then the reader's text and the writer's text (first line).

| # | ms | Kind | Field | Reader | Writer |
|---|---|---|---|---|---|
| 1 | 774.1 | preamble load | `cs:reserved@c` | `\usepackage{hyperref}` | `\DeclareMathOperator{\xxx}{xxx}` |
| 2 | 59.5 | page node (continuation) | `scan:state` | resume of a cut (page 48's scan) | scan `-2 \| 48` |
| 3 | 37.8 | preamble load | `cattab` | `\usepackage{amsmath}` | `\makeatother` |
| 4 | 30.9 | preamble load | `cs:reserved@c` | `\usepackage{imakeidx}` | `\usepackage{amscd}` |
| 5 | 30.6 | paragraph record | `parpar` | `To have several equation columns side-by-side, use extra ampersands` | `The \env{align} environment is used for two or more equations when` |
| 6 | 30.6 | macro def-use | `cs:@pdfborder` | `\begin{document}` | `\hypersetup{pdfborder=0 0 0}` |
| 7 | 20.7 | line count | `lines` | `\begin{table}[p]` | `In \emph{all} math environments (inline or display), blank lines` |
| 8 | 15.6 | preamble load | `cs:g_file_curr_name_str` | `\documentclass[leqno,titlepage,openany]{amsldoc}[1999/12/13]` | the format's every-job text (`\the\LaTeXReleaseInfo …`) |
| 9 | 13.4 | page node (continuation) | `pages:resume` | resume of the first cut | resume before it |
| 10 | 11.6 | page node (scan/finish) | `Created` | finisher of page 5 | scan `-3 \| 5` |

## What layers 2–3 cut

As spec-layers §7 step 2 defines them (line breaking out of expansion),
layers 2–3 cut:

- the **paragraph record's** 41 links (~61 ms), once `normal_paragraph`
  writes without reading;
- the line-breaking share of each expansion link's op (the spec's model:
  about 40% of each link). The chain's expansion links (pieces and
  resumes, the cut's continuations left out) are 1.47–1.55 s of op. Of
  that, the preamble's loads are 0.86 s and the text about 0.65 s. The
  share was not measured here.

Steps other than 2–3 cut the rest:

- step 1 (pages as one node): the page node's 134 links, ~280 ms;
- a layer-2 rule that is not in the spec yet: no `aftertoken` published
  or read across a piece boundary, 123 links, ~267 ms;
- layer 1's prefix sums: the line count, 27 links, ~82 ms.

TeX's own chain stays:

- the preamble, 856 ms (§7 step 4);
- macros and registers, 156 ms;
- the list tail and the catcodes, ~64 ms.

So amsldoc's chain floor is the preamble plus about 0.2 s of document
text.

## Reproduce

- `scripts/accl/accl submit cmd bash bench/r15/chain.sh 3`: builds
  release, observe and profile+observe; three `-j 8` reps of each kind
  and one `-j 1`; the governor; a recording and the player.
- The profile+observe runs write `PHITEX_CHAIN_TSV` (api.rs `dump_chain`:
  each link's run, cause, field, times, relays and text).
- `python3 -I bench/r15/chain.py chain-N.tsv --top 10`: the table, the
  causes by operation pair, the longest links, and the chain in order.
