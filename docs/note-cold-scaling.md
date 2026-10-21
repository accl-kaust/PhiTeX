# Cold builds with threads on the clean core: what the time per run loses (2026-10-21)

Status: parked (cold scaling is not an alpha bar). A sibling of
[note-scan-blocks.md](note-scan-blocks.md), which covers the scan's chain.

## The question

amsldoc's cold build is slower at `-j 8` than at `-j 1` (dev 8723a884:
round wall 2737 ms at `-j 1`, 3721 ms at `-j 8`; job 15350). The work is
nearly serial (969 engine steps at `-j 8`, about 2 jobs a step, the steps'
longest jobs 2.2 s of 2.96 s op time), and on top of that every run is
slower at `-j 8` than alone.

## The split

Op time by kind, amsldoc, `-j 8` with the default window against
`CLEAN_WINDOW=1` (one job a step, the same runs as `-j 1`):

| commit, job | | exec per run | finish per run | resume per run | op in all |
|---|---|---|---|---|---|
| 5c64b72d, 15373 | default `-j 8` | 0.49 ms | 4.57 ms | 1.94 ms | 3106 ms (2132 runs) |
| | `CLEAN_WINDOW=1` | 0.27 ms | 3.17 ms | 1.56 ms | 2339 ms (1886 runs) |
| 3e0cabca, 15404 | default `-j 8` | 0.460 ms | 4.39 ms | 1.83 ms | 2837 ms |
| | `CLEAN_WINDOW=1` | 0.237 ms | 2.77 ms | 1.32 ms | 2006 ms |

With one job a step, op time per run is `-j 1`'s exactly; with
concurrent runs it is 25-94% higher by kind, and the default window also
makes 246 more runs (speculations refuted). para4000: +25-26% op at `-j 8`.

What it is made of:

- **Clock, about a third.** `CLEAN_WINDOW=1` at `-j 8` with 7 busy-loop
  processes on the job's other cores: op 2230 ms against 2009 ms on idle
  cores, +11% (job 15406; the default window on that node +35%).
- **Making new values, part of exec's share.** amsldoc built, emptied by
  an edit, and built again in one process (the values already in the
  table: no growth, no new values for the same content): exec's
  inflation +71% cold, +26% on the warm table; the total +47% and +43%;
  resume +31% and +63% (job 15428, test harness, latexmk rounds).
  defuse 29c319d (per-thread interning counts) made every run about 9%
  cheaper in both modes but left the ratio as it was.
- **The rest, unexplained:** resume +31-63%, finish +21-25%.

Ruled out:

| suspect | test | result |
|---|---|---|
| PhiTeX's locks | every `Mutex` of the tex crate timed where a run finds it held (`LockWait::locked`, the `acct: lock waits` line, profile builds) | amsldoc `-j 8` 72-77 ms in all, 69 of them 7 finishers waiting once for the first glyph-to-unicode writer state (which they all need); para `-j 8` 8-15 ms (jobs 15379, 15404) |
| a step's job on a cold core | the batch's first job on the calling thread (`r5-exec-hot`) | op by kind unchanged, no wall gain, and it breaks the `-j N` limit (job 15355) |
| the allocator, page faults | jemalloc runtime config (`_RJEM_MALLOC_CONF`: `retain:true` and no decay, `percpu_arena:percpu`, both) | no config helps; 8-12k minor faults a build (job 15428) |
| PhiTeX's provider | callgrind, cold amsldoc `-j 1` | 0.26% of Ir in all (jobs 15323, 15324) |

## The next test

One resume run timed alone in a quiet process, and the same run while 7
other resume runs go on at once (a harness that runs chosen node texts
concurrently): if it is slower with company, the sharing is in what a
run reads or writes of shared structures (the engine's tables, PhiTeX's
per-run caches); the clock part is the busy-loop control above.
