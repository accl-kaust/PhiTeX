# Cold scaling: para4000 and amsldoc at -j8 (2026-10-29)

Status: put aside. The user decided on 2026-10-29 that today's speed is
enough for the alpha on plain TeX and LaTeX (checklist §0), so this work
stopped here. The note records what was measured, what was fixed, what was
tried and not merged, and the next idea.

## How it was measured

- **Builds.** `bench/scale-tmp.sh DOC J [ENV…]` on the measure branches
  (`docs-scale`, `docs-base-m`, `docs-others-m`, `docs-lines2-m`). It builds
  the CLI with `observe,profile` (fastdev), warms the format, then runs DOC
  cold at `-j J` with `PHITEX_TEX_TIME=1`.
  - "Wall" is the process's wall time.
  - "acct" is the evaluation's wall time.
- **Node.** One job per mc node (16 CPUs). Several jobs at once on one node
  doubled the times and changed the counters, as `target/dx/mcdocs.sh`
  shows when it runs six builds side by side (para4000 `-j 8` read 3342
  `param:hsize` refutations there, and 2 when it ran alone). Only the runs
  made alone are quoted below.
- **Commit sections.** The `docs-scale` branch has `obs::sect`. It sums the
  time of each section of `commit` by whether the committing operation is
  the page loop's, and prints a `sect:` line after the accounting.

## Fixed: a kept placement's stranded check read the whole text (para4000)

**Branch** `docs-others`.

**Where the time went.** At `-j 8`, para4000's page loop commits took 552 ms
of a 1092 ms evaluation (the loop's whole time, commit and re-ask), and all
of it lay on the page chain. Almost all of it was before the lock:
`match_placements` and the `others` list, 504 ms.

**Why.**
- A loop node stands at the line's end, and its kept rest is next to it.
- Its new finisher goes after the text node where the page fires.
- `others` was one span, from the lowest anchor of all the placements to
  the farthest kept placement. So `foreign_between` listed every node from
  that page's text to the line's end: 4794 nodes for the first page, about
  7 fewer for each page after.
- `stranded` then checks each listed node.

**Fix.** Each kept placement gets its own span: from the lowest floor its
check can have (the placer, or the anchor of a FollowAfter at or before it)
to that placement. The spans are merged before they are walked.

**Result.**

| para4000 | before (dev 382573d41) | after |
|---|---|---|
| `-j 8` process wall | 1.25 s | 0.72 s |
| `-j 8` acct | 1092 ms | 553 ms |
| `-j 8` page-loop commits | 552 ms | 36 ms |
| `-j 1` process wall | 2.53–2.83 s | 2.26 s |
| `-j 1` page-loop commits | 634 ms | 93 ms |

The md5 is 41467932 at both thread counts.

## Measured: why amsldoc does not scale

These are alone-on-node runs of dev 8c32eb85d and 382573d41. Every PDF is
98c96e00.

| | `-j 1` | `-j 8` |
|---|---|---|
| process wall | 2.68 s | 2.50–2.73 s |
| acct | 2.61 s | 2.39–2.94 s |
| op summed | 2.24 s | 6.8–11.0 s |
| of which wasted | 1.03 s | 5.4–9.0 s |
| exec runs / nodes | 891 / 1054 | 4068–6627 / 1054 |
| resume runs / nodes | 221 / 200 | 1304–1607 / 200 |
| finish runs / nodes | 325 / 57 | 148–195 / 57 |

- **At -j8, most runs are speculation on text that is about to change.**
  The refutations, by field: `skind` 386–649, `lines` 365–618, `cs:par`
  362–512, `vprof` 378–469, `cs:if@nobreak` 298–438, `toks:12` 235–366,
  `parshape`, `cs:hook_use:n`, `count:187`, `param:clubpenalty`.
- **Each early commit wakes every reader after it.** In a `PHITEX_WAKE_LOG`
  run (mc 76681), node 867 (one `\begin`) ran 58 times. About 40 of those
  were `Range lines` wakes, one for each earlier piece's first commit of
  its `lines` contribution. The others came from `cs:par`, `cs:hook_use:n`,
  `param:leftskip` and `vlist:last` changes, as earlier pieces ran again.
- **Runs at -j8 are slower than at -j1.** An exec run takes about 1.5 ms at
  `-j 8` against 0.56 ms at `-j 1`. Lock wait was 0.47–1.36 s summed, most
  of it at `values.rs:483/491` (interning).
- **-j1 wastes a second too.** Finishers run 5.7 times each, 801 ms. They
  are re-asked on `count:189` (133) and `cs:@abspage@last` (44). The page
  loop's 951 runs are resumptions on `vprof` (663). These are reruns in
  TeX's order. A `PHITEX_WAKE_LOG` run at `-j 1` is at mc 76701 (dev
  382573d41, `~/code/flinner/phitex-runs/mc/cmd-76701/out.txt`), for
  whoever takes this up.
- **The engine's critical chain is unusable here.** `critical_chain_heights`
  names 2–3 links for amsldoc. It does not count commits or re-asks, so it
  cannot say where the wall time goes.

## Tried, not merged: a piece's lines declared before it runs

**Branch** `docs-lines2`.

- **Engine side.** `Operation::static_contribs` is back in the engine. It
  gives the contributions a node files before it runs: they go into its
  segment's sums and its provisional record. A first run that makes the
  same ones re-asks nobody.
- **TeX side.** Exec declares its `lines` from its frames
  (`run::declared_lines`): the lines its segments begin, by file, as §362
  counts them.
- **Result.** amsldoc `-j 8` `lines` refutations went from 517–618 to
  365–375. Wall did not move beyond the noise: 2.57/2.58 s against
  2.50/2.73 s.
- **What remains.**
  - A cut piece declares all its lines, but contributes only those before
    its cut. Its resume, which has nothing to declare from its arguments,
    contributes the rest. Each cut changes the fold twice.
  - The other fields above cascade in the same way.

## The next idea

The cascade is general. A reader is re-asked each time a definer before it
commits a new answer, whether or not that answer is final. The fix that
removes it for every field:

- **When a run is woken** by a definer that is still open (dirty, running,
  or a hole), or that has an open node before it in the unit, the woken run
  should wait. It runs again once that definer's prefix has run
  (`read::open_before`, `park_waiting`), not at each intermediate commit.
- **What that bounds.** Each text node then runs twice at most: its first
  speculative run, and the one after the clean prefix passes its definer.
  That is the time of the true chain without the contention the wasted runs
  cause.
- **What to measure first.** Count, from a `PHITEX_WAKE_LOG` run, the wakes
  whose site committed again later. That is the waste this would remove.
- **After that.** Value prediction from records, for the fields every
  paragraph sets (`toks:12`, `cs:if@nobreak`, `cs:par`). That is what would
  let the first run be right.
