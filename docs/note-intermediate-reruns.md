# Intermediate reruns at -j8 (2026-11-15, agent intermed, branch `intermed`)

The question. The seeded-reads experiment (branch `seeded-r2`,
docs/spec-seeded-reads.md §7) found that work does not fall even when the
guesses are right: amsldoc -j8 makes 9 000-9 700 runs for about 1 930
nodes, against 2 725 at -j1. The hindsight oracle assumed a reader runs
again only when the final value differs; the engine re-asks it at every
commit whose value differs from what it read. How much of the work is
that, where does it come from, would "wait until the definer settles"
remove it, and what would seeding the page layer from the previous build
save?

Scope: measure and design only. No engine change is proposed for merging
here; the probe below is observe-only.

## 0. Answers in short

1. **Classes** (one instrumented build each, Table 1). At -j8 only 12-16 %
   of runs are kept (amsldoc clean: 1 366 of 8 923). Re-asked by an
   intermediate value (a): 2 616 runs, 2.7 s CPU (amsldoc), 1 295 / 1.4 s
   (acm), 4 189 / 6.4 s (Grail). Re-asked by the final value (b): 1 518 /
   1.4 s, 835 / 1.5 s, 1 532 / 8.9 s. Stopped or parked (c): 728 / 0.5 s,
   241 / 0.4 s, 668 / 5.3 s. **A class the question did not name is as big
   as (b): runs of nodes that are later claimed by a sibling or removed**
   (2 247 / 0.9 s, 640 / 0.4 s, 1 628 / 1.1 s). The "1 930 nodes" are
   1 366 live nodes plus 557 nodes that end claimed (Inert) or gone.
2. **On the critical path** (a) and (b) are small: 2-22 % of the run
   time on the observed path (amsldoc 10-13 %, 113 of 1 147 ms clean;
   acm 12-22 %; Grail 2-4 %, 138 of 6 411 ms). The waste does not sit on the chain. What does is the
   kept runs themselves, and those cost **1.35-1.48x more CPU at -j8 than
   the same runs at -j1** (amsldoc, acm), 2.2x with holds on.
3. **Who makes the intermediate values**: the text layer's own commit
   cadence, not the page layer's. Definers of class (a) are pieces and
   continuations (`exec`, `resume`): 2 323 of 2 616 (amsldoc), 1 189 of
   1 295 (acm), 3 258 of 4 189 (Grail; finishers and the page loop 917).
   Each piece commits once per change of its own inputs, and each commit
   re-asks every reader up to the next definer: `count:188` in amsldoc,
   60 definers, 72 changes, 925 re-asks of 63 readers; `skind`, 32
   definers, 51 changes, 3 015 re-asks of 905 readers (1 090 of them by
   relocations, which have no definer). The fields: scope kind (`skind`),
   counters, `\everypar` (`toks:12`), `\if@nobreak`, `\par`, club
   penalty: the paragraph-start state after headings and lists; then
   `vprof` (page-loop reads, cheap) and continuations.
4. **The rule** "re-asked by an unsettled definer: wait, and be re-asked
   once when it settles", replayed on the logs with the oracle's notion of
   settled (the definer commits no more in this pass), removes 1 919 runs
   / 2.3 s CPU (amsldoc), 897 / 1.3 s (acm), 3 382 / 6.0 s (Grail), about
   half of all reruns. Only 53-71 % of class (a) is caught: the rest are
   re-asks by a definer that is settled, whose value is later hidden by
   another definer (relocations, continuations, claims). **The engine has
   no signal for "will not commit again"** except the quiet point and the
   prefix scan (`unsettled_definer_before`); every existing hold that
   approximates the rule was measured: **each cuts runs and slows the
   build** (`PHITEX_CLEAN_HOLDS=1`: runs -51 to -69 %, wall +30 to +48 %;
   Table 5).
5. **Seeding the page layer** (cuts, continuations, finishers, page
   steps) on a second build reaches 20-36 % of the runs not kept (amsldoc
   3 056 runs / 1.9 s CPU; acm 812 / 0.8 s; Grail 2 443 / 3.8 s, Table
   6). The text chain's own intermediate and final reruns (amsldoc 3 234
   runs / 3.0 s, acm 2 060 / 2.3 s, Grail 2 666 / 8.4 s) and the claimed
   siblings stay. The full persisted graph runs no node on an unchanged
   second build.
6. **Recommendation**: build the persisted graph first (all of it, not the
   page layer alone); do not build the wait rule. Details and risks in §6.

## 1. Instrument and method

`PHITEX_INTERMED_LOG=1` (profile builds; prints nothing otherwise),
beside `PHITEX_RUN_LOG=1`, on stderr:

- `imr` every run: node, operation, place, start and end on the run log's
  clock, thread CPU, worker, how it ended (ok, stands, stopped, waits,
  waits-only, held at the frontier), the wake it began on (kind, field,
  definer), the quiet count at its start, whether it settles at quiet
  points, and **every read with the value seen** (kind, field, anchor or
  fold range, value id);
- `imw` every wake delivered (`sched::mark_stale`, `mark_dirty`,
  `hold_wake`): time, reader, its state, the cause, the wake's definer and
  **the node whose commit made the wake** (a thread-local set around the
  commit in `sched::work`; relocations' walks have no definer of their
  own);
- `imv` every re-ask by a changed definition (`commit::reask_reader`): the
  value the reader saw and **the value it is re-asked by**;
- `imc` every commit: its changed definitions with values, its changed
  contributions;
- `img` a node gone or taken by a claim (`Node::set_state`, `cas_state`
  to Gone, Inert, RunningInert).

Value ids are interned in one process, so equal ids are equal values
(docs/scaling-2026-11.md: 0 of 44 178 refuted reads saw an equal value
under another id).

Scripts in `bench/intermed/`: `parse.py` (log to pickle), `classify.py`
(classes, path, replay), `q4.py` (§5 buckets), `tables.py`, `quiet.py`,
`infl.py`; drivers `run.sh`, `run2.sh`, `po.sh`, `knobs.sh`, `po2.sh`
(acclnode11, binary, cache and documents on local `/tmp`). A "second"
build is a new process on a cache that holds the document's records of
two earlier builds (seedrun.sh's snapshot); "clean" has the format and
Shelf packs but no records of the document.

**Classes.** The kept run is a node's last committed run of the
evaluation. Every other counted run (waits-only runs are not runs, as in
the engine's count) is classed by what ended it:

- (a) committed, then re-asked in the same pass by a value of the field
  that differs from what the node's last run of the pass reads at the
  same read (from `imv` where the wake was a changed definition; else
  from the node's next run that read the field);
- (b) the same, by the final value;
- (c) not committed: stopped mid-run, parked (waits for a definer, yields
  to the prefix) or held at the page frontier;
- (d) other: the node ends claimed or gone (`d:gone`), its pass's last
  run re-asked by the next loop-back pass (`d:next pass`, -j1 has these
  too), or a wake with no field (Structure, Created, Output).

A wake's definer is *unsettled* when it commits again later in the same
pass (hindsight; the engine cannot know this at the time).

**Critical path.** Walked back from the build's last commit: each run's
enabler is the latest of its wake's committing run, the node its last
run waited for, its placer (Created), the page frontier's move, its own
last run's commit, or a quiet point (then on from the last run ended
before it). Only the run time on the path, by class, is reported; the
gaps between links (queueing, commits, quiet points) are not split
reliably by this walk.

## 2. Every run that is not kept

Release wall times (acclnode11, local disk, median of 3; after these the
second build of amsldoc and acm saves nothing, Grail's saves its
loop-back pass):

| doc | clean -j1 | clean -j8 | second -j1 | second -j8 |
|---|---|---|---|---|
| amsldoc | 1.74 s | 2.05 s | 1.74 s | 2.01 s |
| acm | 2.49 s | 2.17 s | 2.15 s | 2.15 s |
| Grail | 11.48 s | 9.52 s | 8.11 s | 7.30 s |

**Table 1.** Runs / CPU ms by class (one profile build each; profile
builds are 5-15 % slower than release).

| doc | build | -j | runs | kept | (a) intermediate | (b) final | (c) stopped / parked | (d) claimed or removed | (d) next pass | (d) other |
|---|---|---|---|---|---|---|---|---|---|---|
| amsldoc | clean | 1 | 2725 | 1351 / 771 | 871 / 25 | 250 / 24 | 1 / 1 | 0 / 0 | 252 / 437 | 0 / 0 |
| amsldoc | clean | 8 | 8923 | 1366 / 1169 | 2616 / 2720 | 1518 / 1420 | 728 / 499 | 2247 / 921 | 255 / 731 | 193 / 293 |
| amsldoc | second | 1 | 2725 | 1351 / 778 | 871 / 25 | 250 / 24 | 1 / 1 | 0 / 0 | 252 / 439 | 0 / 0 |
| amsldoc | second | 8 | 9748 | 1355 / 1189 | 2907 / 2930 | 1606 / 1362 | 718 / 429 | 2710 / 1069 | 255 / 640 | 197 / 322 |
| acm | clean | 1 | 1503 | 623 / 1202 | 527 / 22 | 96 / 144 | 4 / 6 | 4 / 23 | 246 / 697 | 3 / 36 |
| acm | clean | 8 | 3949 | 624 / 1619 | 1295 / 1412 | 835 / 1479 | 241 / 433 | 640 / 444 | 246 / 800 | 68 / 42 |
| acm | second | 1 | 1397 | 623 / 1208 | 504 / 19 | 75 / 56 | 3 / 6 | 3 / 23 | 189 / 509 | 0 / 0 |
| acm | second | 8 | 4772 | 628 / 1512 | 1601 / 1524 | 974 / 1294 | 268 / 473 | 914 / 544 | 190 / 519 | 197 / 98 |
| Grail | clean | 1 | 3416 | 1256 / 5098 | 1079 / 68 | 379 / 1797 | 5 / 8 | 7 / 0 | 679 / 3709 | 11 / 68 |
| Grail | clean | 8 | 10163 | 1259 / 5984 | 4189 / 6369 | 1532 / 8947 | 668 / 5303 | 1628 / 1128 | 646 / 4907 | 241 / 508 |
| Grail | second | 1 | 2075 | 1256 / 7379 | 684 / 42 | 123 / 18 | 0 / 0 | 1 / 0 | 3 / 160 | 8 / 53 |
| Grail | second | 8 | 8924 | 1273 / 8885 | 2578 / 4309 | 1668 / 6158 | 653 / 5925 | 2483 / 1414 | 3 / 145 | 266 / 475 |

At -j1 class (a) is the page loop folding each piece's profile as it
comes (871 runs of `pages` reading `vprof`, 25 ms in all): cheap and
inherent. At -j8 class (a) is expensive text runs.

The claimed class: a piece that reads past its slice claims the next
pushed siblings (`Ctx::claim`, `Run::claim_count`). At -j1 the claimer
runs first and its siblings never run; at -j8 they run on speculation,
3-4 times each, before the claim lands: amsldoc 1 244 runs of 341 claimed
nodes, Grail 1 309.

Grail's (c) is pictures. On the second build: 10 picture runs stopped
mid-run (1.7 s CPU), 18 that yielded to the prefix (1.3 s), 2 held at the
page frontier (0.94 s), each begun on its figure's state before that
state settled.

**Table 2.** Run time on the observed critical path, by class (ms; the
rest of the path is queueing, commits and quiet points).

| doc | build | kept | next pass | (a) | (b) | (c) | other | build end |
|---|---|---|---|---|---|---|---|---|
| amsldoc | clean | 554 | 466 | 31 | 82 | 3 | 11 | 2527 |
| amsldoc | second | 742 | 380 | 39 | 131 | 26 | 39 | 2545 |
| acm | clean | 166 | 418 | 28 | 148 | 40 | 15 | 2471 |
| acm | second | 795 | 275 | 14 | 148 | 125 | 18 | 2321 |
| Grail | clean | 2854 | 1502 | 77 | 61 | 1877 | 40 | 9932 |
| Grail | second | 4277 | 75 | 43 | 196 | 1967 | 91 | 7798 |

What the waste costs the path is indirect. **Table 3**, the same kept
runs (matched by operation and place) at -j1 and -j8:

| doc | kept runs matched (-j8 / held) | CPU -j1 | CPU -j8 | x | CPU -j8 with `PHITEX_CLEAN_HOLDS=1` | x |
|---|---|---|---|---|---|---|
| amsldoc | 1162 / 1195 | 675 ms | 1000 ms | 1.48 | 1565 ms | 2.19 |
| acm | 622 / 581 | 1202 ms | 1617 ms | 1.35 | 1616 ms | 1.34 |
| Grail | 396 | 2301 ms | 2242 ms | 0.97 | | |

amsldoc's pass 1 ends at 1.25 s at -j1, 1.93 s at -j8 and 2.69 s at -j8
with holds; its runs' CPU per read is 1.9 µs, 2.2 µs and 4.0 µs; lock
waits are 0, 1.49 s and 0.27 s. The held build makes a third of the runs
yet each kept run costs more: the per-run cost at -j8 is not only
contention from the waste (§6, open question).

## 3. Class (a): the fields and the definers

Top fields of class (a) at -j8, runs (CPU ms); `#` folds numbers:

- amsldoc clean: `skind` 564 (216), `vprof` 391 (28), `count:#` 381
  (824; `count:188` alone 228 runs, 456 ms), `cs:if@nobreak` 185 (211),
  `toks:#` 176 (239; all `toks:12`, `\everypar`), `cs:par` 166 (93),
  `pages:out:#:#:#` 75 (1), `param:clubpenalty` 69 (92), `cont:#:#` 65
  (36), `box:#` 36 (114), `cs:sectionmark` 33, `cs:if@newlist` 31.
- acm clean: `skind` 381 (255), `vprof` 226 (95), `toks:#` 95 (204),
  `cs:addresses` 86, `count:#` 82 (200), `cs:if@nobreak` 70, `cont:#:#`
  64, `pitems` 37, `param:everypar` 19 (63).
- Grail clean: `skind` 1 761 (923), `vprof` 542 (554), `pages:out` 225,
  `pages:rest` 219, `box:#` 202 (557), `count:#` 174 (390), `toks:#` 127
  (342), `cs:if@nobreak` 86, `cont:#:#` 65 (104), `cs:@freelist` 35
  (220).
- Second builds: the same fields in the same order.

**Table 4.** Class (a) and (b) by their definer, runs (CPU ms): the
operation of the node whose commit re-asked (text: `exec`, `resume`;
page layer: `finish`, `pages`, which settle at quiet points), and whether
that node commits again later in the pass.

| doc | build | (a) unsettled: text / page | (a) settled: text / page | (b) unsettled: text / page | (b) settled: text / page |
|---|---|---|---|---|---|
| amsldoc | clean | 1254 (1370) / 134 (438) | 1069 (646) / 158 (267) | 701 (666) / 77 (213) | 555 (479) / 184 (62) |
| amsldoc | second | 1405 (1459) / 167 (544) | 1140 (678) / 146 (234) | 696 (620) / 80 (241) | 648 (444) / 179 (56) |
| acm | clean | 704 (908) / 39 (62) | 485 (396) / 38 (20) | 285 (680) / 24 (57) | 415 (553) / 110 (188) |
| acm | second | 757 (784) / 23 (9) | 793 (723) / 27 (8) | 449 (674) / 6 (91) | 421 (430) / 97 (100) |
| Grail | clean | 2240 (2665) / 718 (1616) | 1018 (1669) / 199 (396) | 442 (2762) / 157 (505) | 600 (5323) / 331 (356) |
| Grail | second | 881 (1611) / 572 (1162) | 842 (1180) / 117 (220) | 557 (3323) / 113 (281) | 876 (2429) / 119 (119) |

So the layer whose commit cadence makes the intermediate values is the
**text layer**: pieces and continuations that commit on unsettled inputs
of their own (a heading's piece commits `\if@nobreak`, `\everypar` and
the club penalty from the state its own predecessor has at that moment,
and commits again when that moves). Two things amplify it:

- **Fan-out.** One intermediate commit re-asks every reader up to the
  next definition. `count:188`: 60 definers (58 continuations), 72
  changed commits, 925 re-asks of 63 readers (9-16 each). `toks:12`: 396
  definers, 638 changes, 1 396 re-asks. `cs:if@nobreak`: 161 definers,
  329 changes, 934 re-asks.
- **Relocations.** `skind` (the kind of the open scope, `run.rs`
  `SCOPE_KIND`): 32 definers and 51 changes, but 3 015 re-asks of 905
  readers, 1 090 of them by walks with no definer: a node lifted or
  moved (a claim, a continuation placed anew; `commit::lifted_walks`,
  `reask_rest`) re-asks the readers of its old place.

The page layer is a minority of class (a) on amsldoc and acm (11 % and
6 % of runs, by definer) and a fifth on Grail (finishers' give-backs,
`@freelist`, boxes). Page-layer fields read by the page loop (`vprof`,
`pages:out`, `pages:rest`) are many runs and little CPU, except Grail's
`vprof` (554 ms).

First-use caches are not a visible source here (fonts and
`fontinfo:#` appear only on acm's second build, 48 + 18 runs).

## 4. The rule: wait for a settled definer

### The replay

For each node and pass, each re-ask by a definer that commits again later
in the pass (hindsight) is dropped; the dropped re-asks from one definer
become one re-ask at its last commit of the pass, made only if the
reader's value of the field then differs from what it read before the
first dropped one. The kept run, if among the dropped, still runs, at
that last commit at the earliest. Variants: the rule for every definer
(oracle), for page-layer definers only (pages), for text definers only
(text).

| doc | build | rule | runs removed | CPU removed | runs added | CPU added | delay to the end on the trace's timeline |
|---|---|---|---|---|---|---|---|
| amsldoc | clean | oracle | 1919 | 2269 ms | 171 | 218 ms | 1321 ms |
| amsldoc | clean | pages | 195 | 598 ms | 20 | 34 ms | 69 ms |
| amsldoc | second | oracle | 2200 | 2649 ms | 184 | 202 ms | 1389 ms |
| amsldoc | second | pages | 237 | 726 ms | 31 | 57 ms | 68 ms |
| acm | clean | oracle | 897 | 1296 ms | 144 | 225 ms | 437 ms |
| acm | clean | pages | 55 | 71 ms | 18 | 96 ms | 53 ms |
| acm | second | oracle | 1215 | 1247 ms | 137 | 196 ms | 570 ms |
| acm | second | pages | 28 | 54 ms | 3 | 0 ms | 17 ms |
| Grail | clean | oracle | 3382 | 6041 ms | 425 | 669 ms | 3669 ms |
| Grail | clean | pages | 866 | 1693 ms | 71 | 115 ms | 2442 ms |
| Grail | second | oracle | 1999 | 4858 ms | 272 | 564 ms | 3716 ms |
| Grail | second | pages | 678 | 1294 ms | 44 | 189 ms | 2530 ms |

The delay column propagates each deferred kept run's wait along the
path's enablers on the trace's own timeline: pessimistic (the definers'
last commits would come earlier with less work, and the freed CPU is not
credited), but it says which way the path goes: the kept runs the rule
defers (166 in amsldoc) are ones that ran early on a value that held,
and under the rule they wait.

### What "settled" the engine can know

| signal | what it says | mergeless? | as "D settled" |
|---|---|---|---|
| quiet point (`publish_finals`, every worker idle) | the line has nothing left to run in this pass | yes: idle detection, no lock, no merge thread | exact but a barrier: text waiting for it re-runs once a pass, one wave a quiet point; the page layer already uses it for its wakes to the text (`settles_at_quiet`, `hold_wake`) |
| page frontier (`Engine::frontier`, `Ctx::hold_at_frontier`) | the page layer places nothing before it | yes: an atomic the page loop moves | page-layer fields only |
| settling waits (`Ctx::wait_for_settling`) | a read reaching a settling node's prediction, or a transient definer, waits for its next commit | yes: per read, per definer | "next commit", not "last" |
| `Node::commit_seq` | odd while the node commits (placer and child seqlock) | yes | "committing now" |
| `read::unsettled_definer_before(D)` | the first node before D in its unit that is dirty or running and may define | yes: segment read locks, no global lock; O(prefix) a call | D clean and none before it: settled for this pass. A prefix condition: re-asks wait in text order |
| `read::open_definer_before`, `mover_before`, `Node::moves` (`sched::hold_for`) | a source of my last run (or a node before me) that moved a reader and is to run again | yes: per node | the existing approximation of the rule, off on clean builds; `PHITEX_CLEAN_HOLDS` turns it on |

None says "D will not commit again" before the pass ends, except the
prefix scan, whose answer is the prefix's: a reader waits for every
unsettled node before its definer, in text order.

### Measured: the engine's own variants (Table 5)

Release, acclnode11 local disk, 6 clean builds each (the knob driver's
"second" builds turned out to be clean: the document's key depends on
its directory), median (min-max):

| doc | -j | holds | wall | runs | user CPU |
|---|---|---|---|---|---|
| amsldoc | 1 | none | 1.74 s | 2725 | 1.72 s |
| amsldoc | 8 | none | 2.05 s (1.88-2.17) | 9300 | 8.10 s |
| amsldoc | 8 | `CLEAN_HOLDS=1` (wait for movers before) | 3.03 s (2.76-3.19) | 2906 | 4.48 s |
| amsldoc | 8 | `CLEAN_HOLDS=2` (reruns wait for an open source that moved) | 2.39 s (2.30-2.73) | 8446 | 7.27 s |
| amsldoc | 8 | `CLEAN_HOLDS=3` (=1, but a node never run is no mover) | 2.61 s (2.28-3.07) | 4740 | 6.08 s |
| amsldoc | 8 | `SPEC_WINDOW=1` | 2.92 s (2.71-3.15) | 5896 | 7.76 s |
| acm | 1 | none | 2.51 s | 1503 | 2.43 s |
| acm | 8 | none | 2.31 s (2.13-2.38) | 3746 | 6.62 s |
| acm | 8 | `CLEAN_HOLDS=1` | 3.01 s (2.78-3.26) | 1835 | 4.19 s |
| acm | 8 | `CLEAN_HOLDS=2` | 2.82 s (2.73-2.92) | 3257 | 5.86 s |
| acm | 8 | `CLEAN_HOLDS=3` | 3.06 s (2.83-3.12) | 2625 | 5.60 s |
| acm | 8 | `SPEC_WINDOW=1` | 2.99 s (2.90-3.13) | 3248 | 6.19 s |
| Grail | 1 | none | 11.55 s | 3416 | 11.44 s |
| Grail | 8 | none | 9.46 s (9.17-9.82) | 8835 | 35.0 s |
| Grail | 8 | `CLEAN_HOLDS=1` | 13.03 s (12.73-13.47) | 3917 | 17.3 s |
| Grail | 8 | `CLEAN_HOLDS=2` | 11.31 s (10.68-13.47) | 8517 | 32.5 s |
| Grail | 8 | `CLEAN_HOLDS=3` | 11.99 s (11.77-13.08) | 6132 | 26.5 s |
| Grail | 8 | `SPEC_WINDOW=1` | 11.95 s (11.32-12.24) | 7130 | 28.6 s |

Every variant that waits makes fewer runs and a slower build; the one
that waits most (`=1`) is at -j1's run count and slower than -j1 on all
three. Instrumented (`=1`, amsldoc): pass 1 ends at 2.69 s (-j8 1.93 s,
-j1 1.25 s), 4 424 runs held by the chain rule, and the kept runs cost
2.2x their -j1 CPU (Table 3). Waiting puts a park, a wake and a requeue
on each link of a chain that is the build (hindsight: 89-98 % of the kept
work is on the -j∞ chain), and it does not bring the per-run cost back to
-j1's.

**Verdict on the rule.** The oracle form would remove half the reruns,
but its signal does not exist; its realizable forms are the prefix scan
(text-order waits) or the per-source holds, and those are measured: fewer
runs, 17-48 % slower. Its upside is bounded by Table 2: (a) and (b) are
2-22 % of the path's run time.

## 5. The persisted page layer and the persisted graph

**Table 6.** Second builds at -j8 (and clean for contrast): every run not
kept, by what made it. "Page-layer value": the definer settles at quiet
points or the field is the page layer's (`vprof`, `pages:`, `pitems`,
`gdefs`, `cont:`, `marks:`, `wstreams:`). Runs (CPU ms).

| doc | build | (a) page-layer value | (b) page-layer value | held at the page frontier | removed (Gone) | claimed (Inert) | (a) text value | (b) text value | stopped / parked | structure, placement | loop-back pass | all not kept |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| amsldoc | clean | 759 (796) | 468 (378) | 336 (213) | 1003 (254) | 1244 (667) | 1857 (1924) | 1050 (1042) | 392 (286) | 193 (293) | 255 (731) | 7557 (6583) |
| amsldoc | second | 815 (881) | 464 (393) | 345 (219) | 1432 (403) | 1278 (666) | 2092 (2049) | 1142 (970) | 373 (211) | 197 (322) | 255 (640) | 8393 (6752) |
| acm | clean | 403 (255) | 251 (418) | 181 (109) | 59 (132) | 581 (313) | 892 (1156) | 584 (1061) | 60 (323) | 68 (42) | 246 (800) | 3325 (4609) |
| acm | second | 309 (122) | 206 (382) | 205 (154) | 92 (142) | 822 (403) | 1292 (1402) | 768 (912) | 63 (319) | 197 (98) | 190 (519) | 4144 (4452) |
| Grail | clean | 1584 (2826) | 790 (1096) | 300 (346) | 319 (578) | 1309 (550) | 2605 (3543) | 742 (7851) | 368 (4957) | 241 (508) | 646 (4907) | 8904 (27161) |
| Grail | second | 1020 (1500) | 560 (599) | 307 (1230) | 556 (499) | 1927 (915) | 1558 (2809) | 1108 (5559) | 346 (4696) | 266 (475) | 3 (145) | 7651 (18426) |

Seeding the page layer from the previous build (its cuts, continuations,
finishers' and page steps' definitions, the pieces' contributions) would
at best remove the first four columns on a second build:

- amsldoc: 3 056 of 8 393 runs not kept, 1.9 s of 6.8 s CPU;
- acm: 812 of 4 144, 0.8 s of 4.5 s;
- Grail: 2 443 of 7 651, 3.8 s of 18.4 s (the frontier column holds
  Grail's held pictures, 1.2 s).

It would not remove the text chain's reruns (the two "text value"
columns: 3 234 / 2 060 / 2 666 runs, 3.0 / 2.3 / 8.4 s), which come from
text definers committing on unsettled inputs (§3), nor the claimed
siblings (a fact about the text, known from the previous build's claims
but not part of the page layer), nor the relocation walks that the
claims cause. On the path, the page-layer columns hold little outside
Grail: amsldoc's (c) on the path is 26 ms; Grail's is 1.97 s of
picture runs stopped, yielded or held.

The full persisted graph (docs/note-persisted-graph.md) removes every
column on an unchanged second build: the loaded graph is quiet, as the
watch's after its first build, and the build runs no node. After an edit
between the processes it runs what the watch runs for that edit.

## 6. Recommendation

**Build first: the persisted graph**, whole (records, order, claims,
continuations, finishers, loop states), per docs/note-persisted-graph.md.

- Expected gain at -j8 on a second build of an unchanged document: every
  run, so the build is the graph's load, the input checks and the PDF
  write. Today: amsldoc 2.01 s, acm 2.15 s, Grail 7.30 s (-j8 release).
  The load is not measured; the note estimates a few MB for a paper.
  With an edit between the processes: the watch's edit time for that
  edit instead of a whole build.
- Why not the page layer alone: it reaches 20-36 % of a second build's
  runs not kept and a few percent of its path (Table 2, 6); the bulk of
  the reruns are the text chain's, whose values only a full graph seeds.
  Seeding the definers' values without their structure was rule 1 of the
  hindsight note, built on `seeded-r2`: right seeds, no fewer runs.
- Mergeless: loading is per node and per segment; no lock, no merge, no
  serial pass beyond reading the file.
- Risks: a stale graph is a wrong PDF, not a slow one, so invalidation
  must be complete (every file the job read, by content key; the engine
  id; options that change output) and the watch-equals-cold check must
  cover loaded graphs over the corpus at -j1 and -j8; the file's size on
  large documents (hundreds of MB for a thesis, by the note's estimate)
  and its write time at the end of each build; it does nothing for clean
  builds.

**Do not build the wait rule.** Its oracle signal does not exist; the
engine's realizable forms already exist as switches and were measured
slower at -j8 on all three documents (Table 5), and the reruns it targets
are 2-22 % of the path's run time.

**For clean builds this data points elsewhere**, to be measured before
anything is built:

1. The per-run cost at -j8. The same kept runs take 1.35-1.48x their -j1
   CPU at -j8 and 2.2x in the held build, which makes a third of the runs
   (CPU per read 1.9 µs at -j1, 2.2 µs at -j8, 4.0 µs held; lock waits
   1.5 s of 8.5 s CPU at -j8). The chain is the build, so this factor is
   the build's. Next: a held -j8 amsldoc build under callgrind or perf on
   acclnode11, kept runs against the same runs at -j1, to find whether
   reads over a line of open nodes, lock spins or the hold checks pay it.
2. Claimed siblings (14-25 % of -j8 runs, 0.4-0.9 s CPU): their runs are
   never kept and none is on the path. A sibling that a hint says may be
   claimed could wait for its predecessor's first run; but that is a
   wait on the chain like the holds of Table 5, so it needs the same
   measurement first.
3. Grail's pictures: picture runs thrown away mid-run (stopped, yielded,
   held: about 3.9 s CPU on the second build; class (c) is 1.9-2.0 s of
   the path). This
   is the hindsight note's rule 2 and the all-at-once note's "picture
   children early on guessed state, checked at commit".

## 7. Caveats

- One instrumented build per configuration; release times are medians of
  3 (Table 2's times) or 6 (Table 5). Run counts vary 5-10 % from build
  to build at -j8.
- Class (a) versus (b) compares the value the reader was re-asked by
  (`imv`) with the value its last run of the pass read at its own place;
  for range and fold wakes, with the next run's read of the field. A
  reader whose next run did not read the field again is counted (b).
- The definer of a relocation's wake is the node whose commit made the
  walk, not a definer of the field.
- The critical-path walk is approximate between links; only the run time
  on it is reported by class.
- The replay's delay does not credit the freed CPU; the measured holds
  are the better guide to what waiting does to the wall time.

## 8. The -j8 critical path, attributed (2026-11-16, agent jcpu-loc)

The walk of §1 followed a re-ask that a commit's check of its own reads
made (no `by`, Structure, Range, Changed with no site) into the node's
own refuted run, and never saw the paths that queue a node without a
walk's wake. The log now carries, all under `profile` and
`PHITEX_INTERMED_LOG` only:

- `imw` lines for every moment a node becomes runnable (`enq`: waiters
  and pullers re-asked, freed siblings, created nodes, releases) and
  every running node told a read moved (`rdirty`, `note`), with the
  definer of a re-ask that has no node of its own (the field's or the
  line's structure's last mover, `obs::im_changed`), the node publishing
  as it goes (`im_committing` over the run) and the place outside a run
  or commit (quiet point, an idle worker's release, an edit taken);
- `imp` lines for parks (frontier, wait, yield, with the node waited for)
  and a `quiet:` line at every quiet point;
- on `imr`, how the node was taken (own heap, block, dealt, keep,
  injector, steal of the first or the second, block steal) and from
  whose queue.

`classify.critical_path` then splits the path into run time (kept and
thrown away), queue, commit (a link queued during its definer's commit
waits for the rest of it), quiet points, and stale (a definer's change
reached a run in flight: the rest of that run and its own commit).
mc (mcnode42, 8 cores of one L3, profile build, logs on), clean builds:

| doc      | end ms | run (kept) | queue | commit | quiet | stale | path links | hop on path |
|----------|-------:|-----------:|------:|-------:|------:|------:|-----------:|------------:|
| amsldoc  | 2944   | 1907 (1059)|  389  |  476   |   94  |   77  |   746      | 19 %        |
| acm      | 3084   | 2155 (1213)|  141  |  291   |  183  |  314  |   341      | 12 %        |
| Grail    | 13505  | 11126 (4971)| 824  | 1033   |  236  |  282  |   702      |  7 %        |
| para4000 | 856    |  537 (537) |   35  |  156   |  128  |    0  |  1399      | 10 %        |

(A second build of each: amsldoc 2976 = 1826 run, 345 queue, 452
commit; Grail 13416 with one picture run in flight 6.8 s after its read
moved, `stale` 6776.)

Hops. Over every kept link 34-46 % run on another worker than the
commit that let them go (para4000 10 %), but on the critical path only
7-20 %; most of the others are the right thing (para4000's finishers
taken off the page chain). On amsldoc's path the hopping links are
re-asks in flight that rerun on their node's own worker (48), frontier
releases by an idle worker (24), steals of the second node (30) and of
the first (2). Their kept runs cost 1.39x their -j1 time (226 ns per
1000 instructions, 2.49 GHz, IPC 1.78) against 1.15x for links that stay
(187 ns, 2.86 GHz, IPC 1.87; -j1 154-163 ns, 3.10 GHz): moving every
hop of amsldoc's path home would save about 35 ms of 2944, acm's about
20 ms, Grail's nothing. The remaining per-instruction excess of links
that stay is the clock (2.5-2.9 GHz on the path against 3.10 at -j1).

Measured: a thief taking the node after the first only once that node
had been queued `offer_ns` (the queue's own `since` counts from when it
last went non-empty, so a node pushed onto a queue that never emptied was
a thief's at once) moved neither the hop rate nor the time (n11, local
disk, 4-5 interleaved: amsldoc -j8 2.07 -> 2.12 s, acm 2.22 -> 2.25,
Grail 9.20 -> 9.31, para4000 0.67 -> 0.65); reverted.

Where the path's time is instead: runs thrown away on it (amsldoc 848
ms, acm 942, Grail 6155), commits (amsldoc 476 ms, 16 %), and queue,
which on amsldoc is mostly links waiting on a worker free of runs while
the others idle (125 ms over 316 links: the wake's latency) and on
Grail links waiting through a busy worker's heap while others idle (380
ms over 31 links). Tools: bench/intermed (parse, classify), and in the
agent's scratchpad cpq.py (path split), hop2.py (hop classes), qd.py
(queue gaps), cpcost.py (path runs against -j1).
