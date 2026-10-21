# Cold builds with threads on the clean core: what the time per run loses (2026-10-21)

Status: answered 2026-10-21 (see [The answer](#the-answer-2026-10-21)); parked before
that (cold scaling is not an alpha bar). A sibling of
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

## The answer (2026-10-21)

The test above was run, along with a probe of the clock each run sees.
Answer: **the same work does not cost more with company. It runs at a
lower clock.** Of amsldoc's extra op time at `-j 8` (+745 ms), about
half is clock, about 40% is repeat runs, and about a tenth is page
writers waiting for one shared table. Nothing PhiTeX shares between threads costs a
measurable amount, except one table built under a lock.

Setup: dev e7aa893f, a profile build. harrier (EPYC 7763, 64 cores, SMT2,
an L3 per 8 cores), `-c 32` = 16 cores and their siblings over 2 L3s.
Governor `ondemand` on `acpi-cpufreq`: P-states 2.45/2.0/1.5 GHz, boost
on, sampling every 8 ms, up_threshold 95. `perf_event_paranoid` is 4, so
there are no hardware counters. Jobs 15589 (baseline and company),
15596 (thread caches), 15599 (clock probe, placement), 15627 (spinners)
and 15641 (A/B), all on harrier except 15596. Scripts are in `bench/r11/`.

### The clock each run sees

`PHITEX_CLOCK_PROBE=1` (profile) times a fixed chain of 4096 dependent
multiply-adds at each run's start and end, and gives the core's clock at
that moment by kind:

| amsldoc | exec | resume | finish | scan |
|---|---|---|---|---|
| `-j 1` | 3.07 GHz | 3.06 | 2.97 | 3.08 |
| `-j 8` | 2.31 | 2.32 | 2.22 | 2.19 |
| `-j 16` | 2.48 | 2.39 | 2.52 | 2.43 |
| `-j 8`, one job a step on the pool | 2.49 | 2.68 | 2.64 | 2.67 |

The reason: at `-j 1` one core is always busy, so ondemand runs it at
the top P-state plus boost. At `-j 8` the same work is spread over 8
workers that sleep between a step's few jobs. Each core is busy well
under 95% of a sampling period, so ondemand sets a lower P-state, and a
woken core starts at that P-state. A run then costs the same cycles in
more time. Pinning makes little difference: one L3, two L3s, or SMT
pairs all land within a few %.

### Holding the clock: spinners as a control

`SCHED_IDLE` busy loops (`chrt -i 0`), one per CPU of the build, give
way at once to any build thread but keep each core busy, so ondemand
holds it at the top. Every run then sees 3.07 GHz at `-j 8` too (job
15627). Per node, over the nodes run exactly once at both `-j 1` and
`-j 8` (the same work), the time `-j 8` / `-j 1`:

| | exec | resume | scan |
|---|---|---|---|
| clock free | sum 1.34, p50 1.44 | sum 1.33, p50 1.32 | sum 1.24, p50 1.21 |
| clock held | sum 1.06, p50 1.10 | sum 0.96, p50 1.00 | sum 0.99, p50 1.00 |

Over the nodes run once in both, op time in ms, `-j 8` minus `-j 1`
(`same` = those nodes' first runs, `repeats` = the extra runs at
`-j 8`):

| | total | same work | repeats |
|---|---|---|---|
| clock free (j1 vs j8) | +745 | +428 | +316 |
| clock held (j1-spin vs j8-spin) | +379 | +71 (pdfpage +77, the rest about 0) | +308 |

- **Clock**: about 350 ms of amsldoc's same-work inflation, and all of
  the inflation in resume, scan and finish.
- **pdfpage, +77-94 ms**: 7 page writers wait about 11 ms each for the
  first one to build the glyph-to-unicode writer state
  (`pdfout.rs` `g2u_states`, the 72 ms `lock waits` line). All of them
  need it, so this costs about 11 ms of wall, not 77. Note: `pdfpage`
  (and `tool`, `toolnode`) count as `exec` in `op time by kind`
  (`KINDS` has no such entry), which made exec's per-run inflation look
  larger than it is.
- **Repeat runs, about 310 ms**: speculative runs refuted, and runs
  again (143 scan, 54 resume, 44 exec, 1 load of 38 ms, 3 finish). That
  is extra work, not inflation (the engine's flow and the speculation
  work).

### What else was ruled out

| suspect | test | result |
|---|---|---|
| hardware sharing between processes (L3, memory, power) | amsldoc `-j 1` with 7 other amsldoc `-j 1` processes on the job's cores | op +23% (2060 -> 2531 ms); 7 busy loops alone +10%: package boost, which the clock probe sees as 3.5 -> 3.1 GHz |
| PhiTeX's thread caches (decoded and encoded values, character nodes, control sequence names) | hits, misses and the misses' time per cache (`PHITEX_CACHE_STATS=1`, profile) | misses cost 61 ms at `-j 1`, 190 at `-j 8`, 278 at `-j 16` (decoded: 11.8k -> 46.7k misses); included in "same work" above, which is about 0 with the clock held |
| a run on another thread than the one before (migration, cold caches) | `CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1` (profile): one job a step, always on the pool | op +12%, all of it clock (2.5-2.7 GHz); 0 with spinners |
| CCX and SMT placement | `taskset`: 8 cores of one L3, 4+4 over two, 4 cores with both threads | within a few % of the default |
| hardware counters | `perf_event_open` through `bench/r11/pstat.py` | refused, `perf_event_paranoid` 4 on accl |

### Busy-waiting workers (A/B only, not merged)

`bench/r11/keep-warm.patch` (exec.rs): the pool's workers, and the
caller waiting for its batch, wait busy (pause, then yield every 64
turns) while an evaluation runs, up to 20 ms after their last job, like
OpenMP's blocktime. The evaluation's executor's drop ends the wait. Job
15641, harrier, interleaved:

| | j1 | j8 | j16 |
|---|---|---|---|
| amsldoc release wall, e7aa893f / with the patch | - / 3.19 s | 3.78 / 3.40 | 3.77 / 3.49 |
| para4000 release wall | 3.37 / 3.30 | 2.98 / 2.60 | 2.93 / 2.76 |
| amsldoc op, profile, without / with | 2267 / 2272 ms | 3089 / 2772 | 3157 / 3016 |
| amsldoc commit, profile, without / with | 374 / 378 ms | 1141 / 973 | 1113 / 1030 |

All md5s are 98c96e00 (amsldoc, 4 x j8 and 4 x j16 per binary) and
41467932 (para4000). `-j 1` does not change (no pool). The release
`-j 1` base is missing: its run included a format build. The profile
`-j 1` is the comparison.

Not merged. It tunes the engine to this node's ondemand governor and
costs power on laptops. The cause is the scheduler's idle gaps between
steps, which the engine's flow work addresses. The `-j 1` baseline here
is a single core at full boost (3.1-3.5 GHz). A `-j 8` against `-j 1`
comparison on an ondemand node partly compares clocks: say so with any
bar.

### The governors on accl (2026-10-21)

| node | driver | governor |
|---|---|---|
| acclnode01 | acpi-cpufreq | ondemand |
| acclnode05 | acpi-cpufreq | schedutil |
| harrier | acpi-cpufreq | ondemand |
| acclnode03, acclnode04 | (down) | |

Slurm allows `CpuFreqGovernors = OnDemand,Performance,UserSpace`, and a
job step can ask for one. `srun --cpu-freq=Performance` ran its step
under `performance` on all three nodes, and `--cpu-freq=High` under
`userspace` at 2.45 GHz. The governor files cannot be written from a job.
`job.sbatch` runs its command outside any `srun` step, so a run that
wants a fixed clock has to wrap the command in
`srun --cpu-freq=Performance` (the sbatch option alone sets it only for
steps).
