# Pieces that read ahead of their definer (2026-11-10)

For cut1's speculation window (`PHITEX_SPEC_WINDOW`). This is why
`chain::chains` stays known-red (`scripts/accl/gate-known-reds.txt`,
issue #10): its `pages` fixture runs 57 to 65 times for 46 nodes, 11 or
12 of them resumptions, by run (1 red in 60 at 32 threads, accl 27895).
`chain::census_plain`'s last red (issue #11) was its page chain counting
the text node that woke the first page (reuse1, 2026-11-12).

## The race

On a clean build every text piece is created at once, and with more
threads than ready work they all start together. A piece that reads a
field the first piece assigns (`\hsize=6in`, `\def\para`) reads it
before that piece has committed. It gets the format's value, or
nothing. When the first piece commits, the change wakes every such
reader, and each one runs again (cause `Changed param:hsize`,
`ChangedStart cs:para`). The result is exact either way, but the
first runs are thrown away.

Numbers, all on mc. "Runs a node" counts reruns only; resumptions are
counted apart since runs1.

| document | machine | threads | reruns | runs a node |
|---|---|---|---|---|
| para400dvi (626 nodes) | 8 CPUs, busy | 16 | 129 (121 `param:hsize`, 8 at start) | 1.21 |
| para400dvi | 8 CPUs | 64 | 0 | 1.00 |
| para400dvi | 8 CPUs | 152 | 0 | 1.00 |
| para400dvi | gate tests shard (shared node) | default | | 1.29 (mc 83782) |
| plain `pages` fixture (46 nodes) | 16 CPUs | 16 | 15 (`\para` 6, cut structure 8) | 1.15 (mc 83747) |

Whether a reader starts before its definer commits is a matter of
timing, so the counts move with load. A busy node at 16 threads is the
worst case seen.

## What runs3 already does (191f85159)

`Ctx::wait_for_definer(fields, nothing_done)`: a run that meets a
control sequence undefined, where a node before it has not committed
and its operation's `MayDefine` names that sequence, stops. It files
nothing and runs when that node commits (`park_waiting`). A stop at the
run's first command counts as a wait (`Report::waits`,
`Stats::waits`), not as a run. That removed the `\para` reruns: the
pages fixture went from 15 reruns to 0–5 over 8 builds (mc 83781). It
does not cover `\hsize`, which is read with a value (the format's), not
undefined.

## The general design (not built: speculation policy, for the window)

The rule: every first read of a field in a run waits for the nearest
node before it that

- has not committed, and
- whose `Operation::may_define` names the field (`Names`) or a prefix of
  it (`Prefixes`).

The wait uses `Ctx::wait_for_definer`'s mechanism, so the reader files
nothing and its next run is not a rerun. If no such node exists, the
read goes on as now.

- **Cost.** `read::hole_before_defining` scans the segments before the
  reader for holes, which is O(pieces) per call. Compute the set once
  per run instead: on the run's first load, collect the uncommitted text
  nodes before it with their `MayDefine` values. Each later first-load
  then checks the field against that set, which is O(holes before).
  Record a hole read as `hole_before_defining` does, so a hole that
  commits, moves or goes reruns the reader and the set is rebuilt.
- **`Any`.** A node whose may-define set is `Any` (a package load, a
  `\csname` definer, most of a LaTeX preamble) makes every later reader
  wait. For a preamble that is TeX's own order anyway: the body cannot
  run correctly before the preamble has run. It does end the
  preamble-time speculation the engine does now, so it should be
  measured on the corpus (Grail, amsldoc, the TikZ manual) for
  clean-build wall time at -j8 and -j16 before it is kept.
- **Inside the window.** It belongs in the window's policy as a narrower
  "don't run past a definer" rule. A reader inside the window may still
  run on a guess where the window allows it. A reader past the window
  waits only on a definer whose `MayDefine` names what it read, not on
  every hole.
- **Edits.** An edit leaves almost every node committed, so the set
  before a reader is empty or small, and edit speed should not move.
  Check with the gate's amsedit shards.
