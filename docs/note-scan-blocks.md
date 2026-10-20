# Scan blocks instead of a scan step per cut: parked (2026-10-21)

Status: parked. The code is on branch `r5-blk` (one commit on dev d8ba5b98).
Dev keeps a scan step after every cut.

## The finding

On the clean core the critical path of a cold para4000 build is made of
scan nodes only: 4321 nodes at -j 8 (6093 at -j 1), step to step through
`scan:block` and step to the continuation it places. There is one step per
cut, and every paragraph end is a cut, so the chain is about one link per
paragraph. Each link costs a whole engine step (about 0.8 ms at -j 8) around
an 80 us scan op.

What a step needs from the step before is only the state at the start of the
current page (`scan:block` changes only where a page fires, 692 times in
about 4000 steps). The steps were grouped in blocks of about 64 pieces
(`block_end`) before commit 6858b324; since then every cut also places its
own step.

## Why 6858b324 placed a step after every cut

A page that fires at a cut must be placed by a node whose window does not
include the rest of the cut piece. The rest (the resume) reads what the
output routine leaves (`gdefs`, through its range read). If the placer's
window includes the rest, the placer depends on the rest and the rest on
the finisher the placer places: a cycle. A step right after the cut piece,
before its rest, folds a window that ends at the piece. This reason still
holds.

## The experiment

`r5-blk`: a cut places a step only at a block end or at `\end`; the block's
step places the finisher and continuation of a page firing at a cut with
`follow_after` (right after the cut piece, before its rest). With defuse
0ff16b8 (the anchor's own continuations after other placers'
`follow_after` nodes) the placement cycle converges.

Walls, plain release builds, one node (job 15345):

| para4000 | -j 1 | -j 8 | -j 16 | -j 32 | runs |
|---|---|---|---|---|---|
| dev d8ba5b98 | 3.19 s | 2.93 s | 3.23 s | 3.23 s | 15668 |
| r5-blk | 2.76 s | 2.23 s | 2.31 s | 2.54 s | 11311 |

para4000 is exact. The rest is not (job 15344):

| red | first lead |
|---|---|
| pages: `vsize_document_pages`, 161 runs for 142 nodes | each resume after a page firing at its cut runs again once, when the block's step places the page's finisher before it: the variant's cost, not a cycle |
| pdftex: `vfgap`, the page uses `/F5` where pdfTeX has `/F4` | the PDF fonts are numbered in another order: the order the records reach the line in, with no step after the cut piece |
| latex: 10 `latexmk_*index*` tests, PDF object numbers shifted (object 16 where 19) | the same numbering order, through the index rounds |
| amsldoc: wrong PDF (md5 b6d31a4a), exit 1, `Extra \endgroup` at line 2357 | scan defects on pages 50 and 51: the break found from the profiles (candidate 407) differs from the one found from the items (candidate 350) |

## Decision

Parked: about 25% off para4000 against open wrong-output bugs of unknown
size. Taking it up again starts from the amsldoc page 50 defect.
