# Scaling, November 2026: measured

Agent "scaling", branch `scaling`. Every number here comes from an accl job named next to it. The results are under `~/code/flinner/phitex-runs/cmd-<job>/`, and the bench scripts are untracked `bench/*-tmp.*` files in the agent's worktree.

The instruments added for this report are all observe-only. The release build prints none of them.

- `PHITEX_RUN_LOG`: each run's start and end on one process clock, its worker, the field that woke it, each commit's end, and each quiet point.
- `PHITEX_REFUTE_LOG`: each refuted read, with the value the reader saw, the value it would see now, the writer, and the position of the read in the reader's run. Each run made again is also marked with whether its products equal its last run's.
- `PHITEX_HINT_LOG`: each run's hint (bounded or not, and why not).

## 0. Where we stand (accl 22448: release build, cold with a warm format, median of 5, one node)

| document | -j1 | -j8 | oracle, one pass |
|---|---|---|---|
| para4000 (plain) | 2.24 s | 1.24 s | pdftex 0.70 s |
| amsldoc | 2.08 s | 3.27 s | pdflatex 0.69 s |
| course cover (one TikZ picture, the course preamble) | 5.96 s | 6.24 s | pdflatex 2.59 s |
| Grail (XeLaTeX) | 35.9 s | 42.5 s | xelatex 9.27 s |

The PDFs are identical at -j1 and -j8 for every document.

## 1. Why -j8 is not faster than -j1 on LaTeX

The critical chain dumped by `PHITEX_CHAIN_TSV` cannot answer this on LaTeX. It is the height chain of the last loop-back pass: 2-7 links for amsldoc and Grail. So the timeline is read from `PHITEX_RUN_LOG` instead (`bench/runlog-tmp.py`, accl 22500 and 22515). A run is *kept* when it is the last committed run of its node in its pass. A pass is the stretch between two quiet points.

**(a) The text is one serial chain of state.**

| doc, first pass | kept text runs that began only after the one before by place had committed | their run time |
|---|---|---|
| Grail -j1 | 815 of 843 | 9.2 s |
| Grail -j8 | 1498 of 2775 | 15.6 s, longer than all of -j1's text |
| amsldoc -j1 | 1038 of 1039 | |
| amsldoc -j8 | 932 of 1536 | 1.75 s |

**(b) The kept runs are 1.5-1.8x slower at -j8.** Grail's first pass has 22.7 s of kept runs at -j8 against 13.5 s at -j1. amsldoc has 2.37 s against 1.30 s. The engine's measured lock wait accounts for 1-1.8 s of it (`values.rs:494/502` interning, `read.rs:1366/282` segment reads under a commit). The rest is the cores beside the chain doing other work; the stress test in accl 22636 measures this.

**(c) Most of the work at -j8 is thrown away.** amsldoc is busy 11.9 s, of which 9.5 s is not kept. Grail is busy 95.9 s, of which 66.5 s is not kept. Workers are idle 56-75% of the time. One Grail node ran 74 times: once for each commit before it.

What refutes them (`PHITEX_REFUTE_LOG`, accl 22611, -j8): the values really differ. 0 of 44 178 refuted reads (amsldoc) and 0 of 100 417 (Grail) saw an equal value under another identity.

| key | refuted reads | what the reader saw → what came | why |
|---|---|---|---|
| `toks:12` (`\everypar`) Grail | 7079 | `{\@nodocument}` → `{}` 2589; `@afterheading`'s list → `[]` 1954; `[]` → `@afterheading`'s 1212 | the body ran before `\begin{document}`'s node committed; heading/paragraph order |
| `skind` Grail / amsldoc | 6186 / 2511 | undefined → a chain growing paragraph by paragraph | real per-paragraph state |
| `count:0` Grail | 2574 | 1 → 4, 9 → 1, … | text reading the page number before the page's finisher |
| `cs:if@newlist` Grail | 2360 | true → false | list state |
| `cs:__hook_preamble_hook:n`, `cs:hook_use:n` Grail | 1939 + 1797 | the preamble's definition → the document's | `\begin{document}` not yet committed |
| `vlist:last` amsldoc | 5630 | the main list's tail, read at 1% into a run | the previous paragraph's last item |
| `cs:par` amsldoc | 1740 | LaTeX3's `\par` → `\@restorepar`, `\@par` | lists and headings |
| `cs:if@nobreak` amsldoc | 1293 | false ↔ true | headings |

**(d) -j1 also throws away about 40%: the passes.** Grail -j1 is three full passes of 14.2, 10.3 and 12.8 s, then a 0.25 s tail. The finishers run again every pass at about 4 s a pass. In pass 3 the page-1 finisher's `\MT@font@list` (microtype's global font list) re-ran a serial chain of 18 text nodes, 4 s. Even our first pass alone is 1.5x xelatex's whole pass (14.2 s against 9.3 s).

**Scaling curve** (accl 22593, a shared node, so rough): amsldoc 2.32 / 2.99 / 3.29 / 3.54 s at -j1 / -j2 / -j4 / -j8. Grail 37.3 / 51.3 / 47.2 / 44.0 s. Each added worker adds waste and slows the chain.

## 2. The user's three questions

**Why does speculation guess wrong at paragraph starts?** There is no prediction for these keys on a cold build. `Exec::predicted_defs` gives a document node only its constant base-level assignments (`hints::const_defs`). A recorded package load gets its record, and only on a warm build. A node that has not run defines nothing, so a reader past it reads the last *committed* definition. When `\begin{document}` or the paragraph before has not committed, that is the preamble's or the heading's value. The comparison is by interned value, and no refutation was an equal value under another identity (table above). The values really differ because LaTeX's state really changes at each paragraph.

**Why so many Any (hint-unbounded) nodes in Grail?** 314 of 752 document nodes (`PHITEX_HINT_LOG`, -j1):

| cause | nodes |
|---|---|
| `\bibitem`: body not read | 65 |
| claimed `\newblock`: defined elsewhere | 62 |
| `\label`: `\if_cs_exist:w` defined elsewhere | 54 |
| demoted, expansion tokens | 13 |
| `\DeclareAcronym` | 13 |
| `\definecolor` | 12 |
| `\begin`: environment defined elsewhere / `\csname` with no literal prefix | 18 |
| `\section`: kernel body not read | 9 |
| `\input`: computed | 8 |

amsldoc has 301 of 676: file not analysed yet 38, demoted 28, `\item` 24, `\begin` 26, `\bibitem` 13.

**But this costs no parallelism today.** The engine never consults `Operation::may_define`: `Ctx::narrow` is a no-op, and no caller in defuse reads a may-define set. A hint feeds only claims, warm-build recording and counters. A better analysis (`\begin{name}` resolved from its literal argument, the format's kernel macros pre-read) buys parallelism only together with a read rule that uses it: "a read of f past an unrun node whose bound excludes f is final". That is WP-P2 below.

**Is a refuted run resumed from a checkpoint?** No. A refuted run starts again from its start. The checkpoints (`crate::resume`, one every 600 commands) survived the 10-25 rewrite, but `resume_edit` uses them only for a run woken by `Cause::Edited`. Measured on the refuted reads (accl 22611, -j8): the refuting field was first read at a median of 0.52 (amsldoc) and 0.59 (Grail) of the reader's reads. So resuming from the checkpoint before the first changed read would keep about half of every refuted run. Of the runs made again, those whose products equal their last run's (where a cutoff would have spared their re-asks) are exec 1217/2293 and resume 402/841 in amsldoc, and exec 1114/2730 and pages 883/1023 in Grail.

## 3. TikZ: how a TikZ document would get faster, and the granularity question

### What we measured (accl 22448 and 22500, the course cover: one `tikzpicture` of 880 arrows over the course preamble)

- **The picture is one node.** In our engine a picture is not opaque in any special way: the hint pass names no pgf or TikZ macro. The picture is part of the paragraph-like text node that holds `\begin{tikzpicture}…\end{tikzpicture}`. That node's run takes 2.27 s at -j1 (node 308, accl 22500). The whole pdflatex pass over the same document takes 2.59 s.
- **The picture runs twice.** `remember picture` reads its own page position (`\pgf@sys@pdf@mark@pos@pgfid1`) from the aux file. The aux file is read back at `\begin{document}`, so the picture's second run reads the position written by the first. That is exactly latexmk's second pass, but here it means 2.09 s more of the same expansion. The first run's work before that read is thrown away.
- **-j8 buys nothing.** The cover has 305 live nodes, and the preamble is loaded serially, package after package (`\usepackage{pgfplots}` alone is a 254 ms node). The picture is a single 2.2 s run on the critical path. At -j8 the preamble nodes are each run 3 times on a guessed state (`@filelist` 107 refutations, `__hook para/before`, `documentclass`), which spends 10 s of CPU to make the wall 0.3 s slower.
- **Where the instructions go (callgrind, 52.4 G instructions for the whole build at -j1, against 22.5 G for pdflatex's one pass):** 93% is `Exec::run`, almost all in `Run::command`'s expansion. That is macro calls, `pass_text` (skipping false conditional branches, 6.3 G summed over its inlined pieces), `next_or_end`/`get_token_tok` (about 7 G) and `State::set`. The engine itself (commit, reads, interning) is under 5%. On TikZ the cost is the interpreter's own token loop, not the engine.

### Granularity options

TikZ's real work is pgfmath (`\pgfmathparse`, `\pgf@x`/`\pgf@y` dimension arithmetic, scratch macros) and pgf's soft path. Each path is a near-pure function of:

1. the picture's options and the styles in force, which are read-only inside the picture except for `\tikzset` inside it;
2. the current transformation and the scratch registers. The scratch registers are written before they are read within the path, so a path does not read another path's leftovers except through `\pgf@picmaxx` and friends (the bounding box, a fold) and named coordinates/nodes (`\pgf@sh@ns@<name>` and similar, defined globally by `\node (name)` and read by later paths);
3. its own text.

**(a) A finer node per path (per `\draw`/`\path`/`\node`/`\fill` statement) or per `\foreach` iteration.**
- What it buys: the 880 arrows of the cover are 880 independent paths. Their only shared state is the bounding-box fold (max/min, a monoid the engine already folds over contributions) and the PDF stream they append to (a sequence contribution). At -j8 that is up to 8x on the picture. For edits, changing one arrow re-runs one path, not 2.2 s. For remember picture, the page-position read happens once at the picture's start, and the paths in picture coordinates do not read it, so the second pass runs only the opening and closing.
- What it costs: (i) each path must be a cut. Today the cut rule is "every paragraph end at text depth 0". A path sits inside the picture's group (depth 1 or more) and inside `\pgfpicture`'s `\hbox`/`\setbox`, so the engine must carry the open groups as scopes. It already does this for regions ending with open local levels (`leave_open`, `Locality::Scope`), but a picture is an open box, not only a group. (ii) The scratch registers (`\pgf@x`, `\pgf@y`, `\pgf@xa`…, `\pgfutil@tempdima`, the `\pgfmath@` macros) are defined by every path and then read by the next path's first statement, which sets them first, so the reads after a def inside the same run never leave the run. That works only if a path's run reads each scratch register after defining it. Most do, but `\pgf@x` is also an output channel (`\pgfpointanchor` returns in `\pgf@x`/`\pgf@y`), and the engine's read set must not record such a read as a read of the path before. Concretely the hint pass must classify the pgf scratch names as "defined before read within a statement" (the scratch-prefix set of the 10-16 design). (iii) Engine cost: a node costs about 10-60 us of engine time (commit, reads filed, intern) against about 2.5 ms of TikZ expansion per arrow on the cover (2.2 s / 880). That overhead is about 2%.
- `\foreach` iterations are the same thing one level down. `\foreach` expands its body once per item with `\pgffor@...` state. An iteration that draws is a path. An iteration that accumulates (`\xdef\list{\list,…}`) is a state chain and gains nothing.

**(b) A coarser node for the kernel and package loads** is already the case: a `\usepackage` is one recorded node (`pkgrec`), predicted from its record on a warm build. Cold, the preamble is serial by nature (each package reads the macros the one before defined). Making it coarser saves engine overhead, about 5% of the cover's preamble, but not wall time. The win for preambles is the format cache: a recorded preamble replayed (`pkgrec`'s predictions).

**Recommendation (a design change, raised here, not built):** cut pictures per path when the hint pass can prove the statement is a path command whose reads of the scratch prefix set are all defined before read within the statement. Keep the picture as one node otherwise (`\pgfextra`, `\pgfmathsetmacro\global…` in a path, `\tikzset` inside the picture). The first step is the measurement: the share of the cover's 2.2 s spent in statements so classified (expected above 90% for an arrow-heavy picture). Work package WP-T below.

## 4. The interpreter against pdfTeX (codehigh, pegmatch)

- The manuals themselves are slow in pdfTeX. One pdflatex pass takes 207 s for codehigh and 234 s for pegmatch (accl 22521, 22522). Our codehigh at -j1 is 298.6 s, which is 1.44x pdflatex's pass. A 300 s limit is about 1.45x one pdflatex pass, so even latexmk's 2-3 pdflatex passes would not fit it.
- A reduced document: codehigh's preamble and two `codehigh` environments (`bench/hi-tmp/chmini.tex`). pdflatex takes 10.7 s; we take 13.4 s at -j1 (USER 13.3 s, SYS 0.09 s, so CPU-bound with no waiting). By callgrind we execute 54 G instructions against pdflatex's 108 G (accl 22539): half the instructions, at about 40% of pdflatex's instructions per second.
- The cache and branch simulation (accl 22576) stopped early: our build's no-progress watchdog fired after 300 s of wall time under valgrind. Its rates per 1000 instructions over the 18.6 G it ran:

| per 1000 instructions | phitex | pdflatex |
|---|---|---|
| I1 misses | 9.5 | 0.14 |
| D1 misses | 4.0 | 3.4 |
| branch mispredicts | 9.6 (indirect 21%) | 12.4 |

  The 68x I1-miss rate is the one large difference: our hot path does not fit the instruction cache. Its cost alone (about 10-15 cycles per miss) explains about 0.15 of the cycles per instruction, not the whole gap. The rest needs hardware counters, which we do not use on accl. Callgrind with the watchdog disabled is the next measurement (WP-I1).

## 5. Edit latency today (accl 22611, shared node: rough; clean rerun 22633/22687)

The median of 3 runs, from the save to the build event, end to end:

| doc | edit | -j1 | -j8 | bar |
|---|---|---|---|---|
| amsldoc | letter | 14 ms (11 runs) | 24 ms (14 runs) | 5 ms |
| amsldoc | word | 15 ms | 24 ms | 5 ms |
| amsldoc | sentence (line change) | 101 ms (84 runs) | 283 ms (298 runs) | 10 ms |
| amsldoc | heading | 50 ms (15 runs) | 60 ms | 20 ms |
| amsldoc | paragraph deleted | 288 ms (182 runs) | 326 ms | 50 ms |
| Grail | letter | 102 ms (5 runs) | 125 ms (7 runs) | 5 ms |
| Grail | word | 100 ms | 122 ms | 5 ms |
| Grail | sentence | 14.5 s (329 runs) | 7.6 s (318 runs) | 10 ms |
| Grail | heading | 702 ms (96 runs) | 807 ms | 20 ms |

### The run list of one heading edit at -j1 (accl 22662)

**amsldoc: 73 ms, 15 runs.**
- Needed: the paragraph (0.15 ms), its continuation (2.0), the finisher (4.2), the toc reader (13.1), the toc and link writers.
- Coarse: `\begin{document}`, which re-runs its whole run (23.7 ms) for one changed aux unit, and the .aux text writer (10.1 ms).

**Grail: 728 ms, 95 runs.**
- The xdv/PDF writer (`dvi`): 92.7 + 121.3 ms. The whole output is written again, twice.
- `\begin{document}` and its continuation: 37.9 + 90.3 ms, the whole aux re-read.
- The `\ref`/nameref reader, a 103 ms resume piece: run twice, the first run before the aux re-read.
- The finishers whose running-head marks moved: about 57 ms.
- The .aux writer: 22.2 + 11.5 ms.
- A serial walk of 66 page-loop rests: 8 ms.

### A `\cite` removed (accl 22666, -j1)

- amsldoc: 408 ms, 143 runs.
- Grail: **53.9 s**, 606 runs, of which 44.7 s is one finisher (the last page's routine, a 4.5 s run) run 10 times. Each bibliography piece before it committed in turn and moved `\@pdfpublisher` or `count:186`/`count:345`. Another 1.6 s is text re-run on `\@currlist`.
- WP0's second step (`Operation::reruns_after_text` for finishers, b15f3a27a) makes that routine wait for the text before its page.

## 6. What happened to the design (as of e35b8080a)

| mechanism | documented | built | used on text runs | evidence | lost |
|---|---|---|---|---|---|
| node ≈ paragraph | cuts-rule, TARGET_GRAPH | yes | yes | `hints::slices_of`; a heading paragraph is its own 0.15 ms node | – |
| run-local save stack | paragraph-node-design | yes | yes | `State`'s group saves | – |
| continuation rule (claims) | spec-continuations | yes | yes | `Ctx::claim`, `r.pulled` | – |
| opaque environments (TikZ) | paragraph-node-design §4 | no | – | no pgf or TikZ name in the TeX engine | never built |
| hint-bounded reads | spec-redesign §4, TARGET_GRAPH | computed | **no** | `Ctx::narrow` is a no-op; no caller of `may_define` in defuse | cf16b3851 / e1cdee844, 10-25 (the old core's `may_cover`/`likely_defines` pending sets) |
| page breaks from vertical profiles | spec-predicted-breaks | partly | yes | para4000 -j8 2.3x; finisher predictions on | – |
| warm predictions | `Speculation::Predicted`, pkgrec | yes | **CLI: no** | `pkg_predict: false` (api.rs:888), true in the live editor | – |
| resume from the first changed read, cutoff | resume.rs | checkpoints | **edits only** | `resume_edit` only for `Cause::Edited` | never wired for `Changed` |
| exact read paths/views | api `PathId`, `View` | yes | yes | re-asks go `through` paths | – |
| edit = 4 runs a letter | goals-and-bars | – | not met | amsldoc letter 11-14 runs; Grail letter 5 runs, 100+ ms | – |

## 7. Restore plan: independent work packages, by payoff

The bars are release bars. Edits on amsldoc and Grail at -j8: a letter or word ≤ 5 ms, a sentence ≤ 10 ms, a heading or label ≤ 20 ms, a page move or a deleted paragraph ≤ 50 ms. Cold: -j8 at least 2x -j1, and -j8 below the oracle's one pass. Each package reports edit ms per kind and cold -j1/-j8 before and after. Every package is mergeless: a reader that waits waits for one named node's commit, through the existing `park_waiting`/`release_waiting`. There is no global lock and no serial commit.

**WP0 (done, being measured): refuted text runs wait for the open text node before them** (`sched::work`, `refuted_wake`). A refuted run reran on a state its predecessors would move again, up to 74 times. Test `refuted_waits`: 300-node chain at 8 threads, at most 2 runs a node; base 1204-1755 (accl 22663). Cold and edit numbers: 22686, 22687. Size: done.

**P1: hint-bounded pending sets in the new core.**
- Evidence: §1-2. The body reads the preamble's `\everypar`, hooks and fonts because `\begin{document}` has not committed. In 31% (amsldoc) and 33% (Grail) of refuted reads the writer was on its first run, so the reader had read past a node that had not run yet (accl 22611).
- Change: an operation's `may_define` (Any, Names or Prefixes) is filed with the node while it is unrun or open. `read::reaching_at` and `read_visible` look for an open node between the reaching definition and the reader whose set covers the field (a per-segment count of open nodes keeps this cheap). If one exists, the read takes that node's prediction (`predicted_defs`) when it has one; otherwise the run parks on that node (`Ctx::wait_on`, `park_waiting`). An Any node covers everything, so a body node waits for `\begin{document}` until it commits. Nothing else waits.
- What it costs: a hint that says Any where the text is bounded serialises its readers. That cost is what makes P1b worth it: `\begin{name}` resolved from its literal argument, the kernel's macro bodies read from the format, so `\bibitem`, `\label`, `\section` and `\item` are bounded.
- Bar: cold Grail -j8 runs ≤ 1.3x -j1's (today 4.4x); wasted CPU ≤ 20% of busy (today 70%). Edits: no change at -j1. At -j8 the letter or word drops from 24 to about 14 ms on amsldoc (no wasted runs).
- Expected cold: -j8 at -j1 or better (Grail 44 → ≤ 35 s, amsldoc 3.0 → ≤ 2.1 s). This alone does not make -j8 faster; it removes the waste that slows the chain, and P2 and P7 give the speed.
- Identity risk: none. The reads are the same; only the timing changes.
- Size: 2 agent-days for P1, 2 more for P1b.

**P2: resume from the first changed read, with a cutoff.**
- Evidence: §2. A refuted run starts over; its first changed read sits at a median of 0.52-0.59 of its reads. Exec reruns with identical products: 1217/2293 (amsldoc), 1114/2730 (Grail).
- Change: `Run`'s checkpoints (one every 600 commands, `crate::resume`) are kept for every text run, not only edited ones. A run woken by `Changed`/`Rebind` goes on from the last checkpoint before the first read of the changed field (`Ctx::resumed` with the read's index recorded with the checkpoint). Its products are diffed against the last run's at each later checkpoint, and it stops there when the state and output digests match again: the cutoff (`Run::product_signature`). The tail is spliced.
- Bar: refuted CPU halves at -j8. Edits: the sentence edit's paragraph-and-after reruns become resumptions (amsldoc sentence 101 ms at -j1 → ≤ 40 ms; Grail sentence 14.5 s → about 1 s with P3).
- Identity risk: medium. `PHITEX_RESUME_CHECK=1` runs the full run beside the resumed one and compares; it goes in the gate's edit sweep.
- Size: 3 agent-days.

**P3: read-back readers resume per unit.** It falls out of P2 if checkpoints are taken at aux-unit boundaries. `\begin{document}`'s aux `\input` then resumes from the first changed unit (37.9 + 90.3 ms on the Grail heading edit → under 5 ms), and so does `\tableofcontents` (13.1 ms on amsldoc). Bar: Grail heading edit ≤ 100 ms with P4, then ≤ 20 ms with P6. Size: 1 agent-day after P2.

**P4: incremental output writers.**
- Evidence: the Grail heading edit spends 214 ms in the xdv/PDF writer (`dvi`, twice) and 34 ms in the .aux writer.
- Change: the writer emits only the pages whose items changed, against the last file (the link already relocates chunks for pdfTeX; XeTeX's direct writer does not). The .aux and .toc writers keep per-unit text and splice.
- Bar: writer cost per edit ≤ 2 ms + 1 ms per changed page. Grail letter 102 ms → about 20 ms.
- Identity risk: medium. The incremental file must be byte-equal to a cold write; the gate's edit sweep compares the history's PDF with the cold one.
- Size: 3 agent-days.

**P5: predictions on by default.** `pkg_predict` on in the CLI as in the live editor, warm builds only. Cold predictions for `\begin{document}`'s outputs come from the format's record of the class's begin-document hook. Bar: warm cold-start (the format and records cached) -j8 runs ≤ 1.1x nodes. Identity risk: low; predictions are confirmed by value. Size: 1 agent-day.

**P6: granularity of resume pieces.** A `\ref`/nameref reader is a 103 ms resume piece in Grail, and every heading edit re-runs it in full. P2's resumption cuts that to the part after the read. Otherwise cut resume pieces at paragraph ends as exec pieces are. Bar: no single text run over 20 ms in Grail's edits. Size: 1-2 agent-days.

**P7: kept-run slowdown at -j8 (measured, accl 22684).** A job given 10 CPUs on accl gets 5 physical cores and both of their SMT threads (affinity 2-6,66-70; EPYC 7763, 2 threads a core). amsldoc -j1 takes 2.44 s alone, 3.63 s beside 7 spinning `yes` processes (1.49x) and 3.74 s beside 7 memory walkers (1.53x). That is the 1.5-1.8x of the kept runs at -j8. So: (i) every -j8 measurement above ran 8 workers on 5-6 cores. The A/B jobs from 22715 on take 16 CPUs (8 cores). (ii) A worker on a core's second thread halves the chain's speed. The default worker count should be the physical cores, and a speculative run should never share a core with the chain's node.

**P7 (old text): kept-run slowdown at -j8.**
- Evidence: kept runs are 1.5-1.8x slower at -j8. amsldoc -j1 slows from 2.2 to 2.7-3.5 s beside 7 spinning processes and to 3.6 s beside 7 memory walkers (accl 22636; a clean rerun with the CPU layout is 22684).
- Likely cause: SMT siblings and the all-core clock.
- Change: after P1 removes most of the waste, measure again. If SMT siblings are the cause, size the default worker count to physical cores, and keep speculative runs off the chain's core.
- Size: 1 agent-day of measurement.

**P8: TikZ per-path nodes** (§3). Only if the cover's measurement says the per-path statements carry over 90% of the picture's time. Size: 4 agent-days.

**P9: interpreter CPI** (§4, codehigh/pegmatch). Callgrind with the watchdog off, then the hot path's instruction-cache footprint. Size: 2 agent-days to measure, more to fix.

Order: WP0 → P1 (with P1b) → P2 → P3 → P4 → P5, P6 → P7. P2, P3 and P4 carry the edit bars; P1 and P7 carry cold -j8.

## 8. The deciding experiments (2026-11-02)

### WP0 and P1, measured and backed out

A/B in one job, base and fix runs interleaved, 16 CPUs (8 cores), median of 5, md5s identical (accl 22719):

| | base | WP0 |
|---|---|---|
| amsldoc cold -j1 / -j8 | 2.36 / 3.95 s | 2.74 / 3.99 s |
| Grail cold -j1 / -j8 | 41.1 / 45.9 s | 41.5 / 46.8 s |
| amsldoc -j8 edits: letter / sentence / cite | 21 / 283 / 326 ms | 35 / 586 / 538 ms |

P1, the second version (accl 22752): Grail -j8 makes 9571 runs instead of 15220 and wastes 36.7 s of CPU instead of 63 s, but its wall is unchanged (48 s), and amsldoc -j1 got slower. Both are backed out (ca3e4d01d) and kept on branch `scaling-p1`.

Cutting the waste does not move the wall. Waiting serialises by construction, so this does not by itself prove the chain is inherent; the predictor test below asks whether guessing could keep the parallel runs.

### Would a guess of a paragraph start's state have held? (accl 22793, `bench/predict-tmp.py`)

Each committed text run's reads were logged (`PHITEX_KREAD_KEYS=*`). Every refuted read is then compared with the value the reader's kept run read. The logging inside the commit slowed this run's -j8 schedule a lot (amsldoc 50 s), so the sample holds more speculative reads than a normal build, but the values compared are the final ones.

| | amsldoc -j8 | Grail -j8 |
|---|---|---|
| refuted reads | 30 098 | 132 722 |
| hit: the value the text node before it read (prev) | 42% | 48% |
| hit: the commonest kept value of the key (mode) | 47% | 47% |
| hit: the node's own kept value (warm, a previous build of the same text) | 84% | 79% (the rest: readers outside the text, or fields their kept run did not read) |
| wasted CPU of the nodes all of whose refuted reads prev gets right | 0% | 4% |
| the same for mode | 4% | 1% |
| the same for warm | 49% | 54% |

Per key, prev holds best on `skind` (931/1262 amsldoc, 9571/12674 Grail), `parshape` and the paragraph parameters. It fails on `vlist:last` (228/2341), the page layer's `pages:start:*`, `count:0`, `\@currentHref` and `\@currentlabel`: the per-paragraph state that really changes.

**Verdict:** a cold value predictor recovers 0-4% of the waste, far below the 90% that would make value prediction the pivot. Even a perfect warm prediction from a previous build recovers about half, because the other half of the refuted runs also read the page layer's state (`pages:start:*`, `count:0`, `deadcycles`) or reads made by non-text runs.

### Where a text run's instructions go (accl 22772, callgrind, amsldoc -j1, 13.8 G)

- `Run::command` in text runs (expansion): 57%. Exec 31%, resume pieces 22%.
- Line breaking (`line_break_`): 1.9%. `char_run`: about 2%.
- Finishers: 11% (`ship_out` 3.4%, `box_out` 5.5%). Deflate 1.8%. The link 3.9%.
- The hint pass: about 10%, at startup outside the evaluation. Inside `Closure::call`, the source pass's closures over the format's macros: 1.36 G.

**Typesetting off the chain buys at most about 5% on amsldoc.** The chain is macro expansion. Finishers, shipping and the link are already nodes of their own.

### Useful CPU at -j8 against -j1 (accl 22448, 5 cores)

Busy minus wasted CPU:
- amsldoc: 1.20 s at -j1 against 1.88 s at -j8 (1.57x).
- Grail: 23.1 s against 27.2 s (1.18x).

The kept runs cost more at -j8 (SMT siblings, §7 P7) even when nothing is wasted.

### What this leaves for cold -j8 on LaTeX

The text's state chain is real at paragraph granularity: its start state cannot be guessed cold. What can be parallel:
1. The passes. Grail's three passes of 10-14 s each re-run finishers and text whose aux values moved; a pass that resumes only the readers of changed keys (P2/P3) shortens the second and third.
2. Expansion inside one long run: a TikZ picture or `\dochighinput`, as finer nodes (P8).
3. The hint pass (10% of amsldoc, startup, serial).
4. The interpreter's cycles per instruction (P9: 2.5x pdfTeX's on codehigh).

Of these, only (1) and (4) move both Grail and amsldoc. The ranked order of §7 stands with WP0 and P1 dropped: **P2 (resume and cutoff, also the edit lever) first**, then P3, P4 and P9.

### P2 tried: resuming a refuted run from its last checkpoint (backed out; branch `scaling-p2`)

What was built (166d28314):
- Every exec run takes checkpoints. Their cost (accl 22812, observe build, -j1, base against checkpoints, 3 runs each): amsldoc 2.11 → 2.23 s (+6%), Grail 24.8-25.8 → 25.2-25.7 s (+2%), RSS +2%.
- Each checkpoint records how many reads the run had made.
- A run woken by anything but an edit goes on from the latest checkpoint whose earlier reads all still hold (`Ctx::parked_state`, `Ctx::last_reads_hold`, `Ctx::goes_on_after_reads`). The kept reads are filed first, so later checkpoints' indices hold.

The resume check (`PHITEX_RESUME_CHECK=1`, accl 22814) runs each resumed run again whole and compares the products:

| build | resumed runs | differ from the whole run |
|---|---|---|
| amsldoc -j1 | 20 | 0 |
| amsldoc -j8 | 150 | 4 |
| Grail -j1 | 35 | 12 |
| Grail -j8 | 136 | 38 |

Grail's PDF changed (3a2866fc against 004bb415). The differences take two forms:
- **Log contributions:** "Overfull \hbox … in paragraph at lines N--N" with another N.
- **Publications:** the same fields published at another level, for example `cs:if@nobreak` global against local, and microtype/acro definitions.

So a checkpoint's state is not a function of the reads recorded before it. Some inputs a run uses before a checkpoint are not recorded reads: the family snapshots a table is loaded from are unrecorded by design (`Ctx::snapshot`), the paragraph's start line is carried outside the checkpoint, and the group level of a binding is not in `base_of`. The same restore code serves the existing edit resumption (`resume_edit` for `Cause::Edited`), so edit resumption may carry the same differences. That is worth a check of its own: the edit sweep with `PHITEX_RESUME_CHECK`.

**P2's real first step** is to make a checkpoint's state a pure function of the reads before it:
1. record the snapshot-loaded items a checkpoint carries as reads;
2. put the paragraph's start line and the binding levels into the checkpoint;
3. gate on `PHITEX_RESUME_CHECK` reporting 0 differences over the corpus's edit sweep and cold amsldoc/Grail at -j1 and -j8.

Only then is resuming worth measuring. Size: 3-5 agent-days.

### P2, the blocker located (accl 22826, 22834; branch `scaling-p2` cd6f840b1)

- **The edit resumption is sound.** Edit resumptions (`Cause::Edited`) checked against whole runs: letter, word and sentence edits on amsldoc and Grail at -j1, every one "same", 3 of 3 for each kind (accl 22826).
- **The new path fails only past a page candidate.** Change resumptions refused from any checkpoint that has a page candidate marked before it: 0 resumptions are left in amsldoc and Grail cold at -j1 and -j8, and the PDFs are identical again (98c96e00, 004bb415; accl 22834).
- **Every checkpoint in practice is past a candidate.** The state there holds what the page's routine left at that candidate. It is loaded (`Held::Loaded`, "a finisher's state at its candidate … no read recorded"), not read. So a moved page state passes `last_reads_hold`, and the resumed run goes on from a stale state. That is the cause of all the differences in 22814.
- **What P2 needs first:** record a loaded candidate state as a read of the fields it was loaded from (or file the candidate's state digest with the checkpoint, and refuse a resumption when it moved). Only then is a resumption past a candidate valid. Edits never meet this, because an edit's resumption keeps every read of the last run, and a moved page state wakes the node for that reason.

## 10. Cold -j8 on LaTeX: where the wall goes (agent "coldscale", branch `coldscale`)

Every number below comes from accl. The jobs ran on 16 CPUs (8 cores; -j16 runs on 32 CPUs) with release builds, unless the row says "po" (a profile+observe build with `PHITEX_RUN_LOG`). The scripts are in the agent's worktree: `bench/{dist,gaps,gapwhy,held,big,storm}-tmp.py` read the run logs, and `bench/{ab,envab}-tmp.sh` run the A/B jobs.

**One caveat for every Grail number.** Grail's -j8 wall is bimodal on the same node and build. It comes out 27-31 s in some jobs (accl 23045, 23073) and 38-48 s in others (23010, 23059). The difference is whether pass 2 cascades: in 23010, pass 2 re-ran 1310 text nodes against 501 at -j1. Two Grail numbers are comparable only when they come from interleaved runs in one job.

### 10.1 Where -j8's wall goes (accl 23010, po, dev b951e87e1)

Passes, in ms. Each cell is the span (the busy CPU in brackets):

| | pass 1 | pass 2 | pass 3 | wall (release, median of 3) |
|---|---|---|---|---|
| Grail -j1 | 16 264 (15 407) | 10 999 (10 528) | 10 568 (10 285) | 36.2 s |
| Grail -j8 | 19 381 (47 430) | 24 918 (33 467) | 11 584 (12 006) | 39.9 s |
| Grail -j16 | 20 222 (64 380) | 16 943 (35 580) | 12 360 (13 465) | 41.5 s |
| amsldoc -j1 | 1 489 | 579 | – | 2.27 s |
| amsldoc -j8 | 3 049 | 998 | – | 3.43 s |

The text chain is the kept text runs in place order (`gaps-tmp.py`). Its runs and the gaps between them:

| | text chain runs | gaps in the chain | what runs in the gaps |
|---|---|---|---|
| Grail -j1, each pass | 10.7 / 5.8 / 5.6 s | 4.95 / 4.89 / 4.84 s | finishers: 4.6 s each pass |
| Grail -j8 | 17.2 / 11.8 / 6.2 s | 8.1 / 19.1 / 5.3 s | wasted exec/resume runs, and the finishers |
| amsldoc -j8 | 1.9 / 0.4 s | 1.5 / 0.6 s | wasted exec runs |

Three causes, measured:

1. **A 4.4 s finisher on the chain in every pass, at every -j.** Grail's page-1 finisher runs hyperxmp's XMP packet: 41.9 M commands (docs/note-page-layer-2026-11.md §3). The text after page 1 cannot start until it commits: it reads `deadcycles` and the routine's globals, and `Ctx::wait_for_settling` waits for the commit. The finisher is a new node in every pass (`Created`: nodes 537, 1952, 2395), so it runs 3 times. That is 13 s of Grail's 36 s at -j1, and a 4.5 s gap in every -j8 pass.
2. **Runs released at a quiet point were serialised on one worker.** Fixed (f192245ad, §10.2).
3. **The page frontier's holds serialise the text behind the page loop.** At -j8, 841 runs are held at the frontier in one Grail build; 330 of them wait 1 s or more, and the longest waits 16 s (`held-tmp.py`). In pass 2, Grail's TikZ figures (0.7-1.3 s each) read `\@freelist` at `\begin{figure}`, are held, and run one after another as the loop passes them (accl 23065, `PHITEX_RUN_LOG` names each held field). The fields held, counted over one build: `marks:splitfirst:1` 458, `__hook …` 354, `MT@font@list` 260, `if@newlist` 157, `pages:start:anchor` 151, `count:31` 136, `if@ignore` 93, `@freelist` 47.

**Waste by distance past the text frontier** (the coordinator's question; `dist-tmp.py`, accl 23010, pass 1). d is the number of kept text nodes between a run and the frontier when the run started. "Kept" is the share of the CPU at that distance that was not thrown away.

| d | Grail -j8 CPU / kept | Grail -j16 | amsldoc -j8 | amsldoc -j16 |
|---|---|---|---|---|
| 0 | 9.9 s / 1.00 | 9.3 s / 1.00 | 1.38 s / 0.83 | 1.20 s / 1.00 |
| 2-7 | 0.48 s / 0.55 | 0.44 s / 0.40 | 0.16 s / 0.46 | 0.18 s / 0.42 |
| 8-31 | 2.3 s / 0.17 | 2.6 s / 0.13 | 0.61 s / 0.19 | 0.62 s / 0.17 |
| 32-255 | 15.3 s / 0.25 | 16.6 s / 0.19 | 3.5 s / 0.11 | 6.0 s / 0.07 |
| ≥ 256 | 14.4 s / 0.21 | 30.2 s / 0.14 | 4.4 s / 0.04 | 14.1 s / 0.06 |

The frontier's own runs (d = 0) take as long as -j1's whole text: 9.9 s against 9.9 s. The far runs are mostly waste, but they do not lengthen the d = 0 runs. The wall is lost in the gaps (causes 1 and 3), not in the far speculation.

### 10.2 Fixed: runs released at a quiet point go on the ready heap (f192245ad, in dev-next)

At a quiet point inside a pass, the runs held at the frontier or waiting for a commit were seeded into the quiet worker's block. A block is stolen from only past `BLOCK_MIN` (96), so that one worker ran them one after another while the others idled. Grail -j8 (accl 23051, frontier trace): worker 0 was busy 7.5 s of 10.3 s, the other seven 0.9-2 s each. Node 1550 was released at 10.5 s and ran at 20.9 s.

The fix puts them on the ready heap, by place, where every idle worker takes the earliest (`Worker::ready_all`). A/B in one job, median of 5 (accl 23059):

| | base | fix |
|---|---|---|
| Grail -j1 / -j8 | 32.27 / 41.84 s | 32.39 / 38.34 s |
| amsldoc -j1 / -j8 | 2.14 / 3.62 s | 2.13 / 3.64 s |

md5s identical. Scoped tests are green (accl 23058). defuse `review::next_readers_follow_their_stops_across_splits` fails 1 in 30 on the base and 2 in 30 on the fix (accl 23071): it was red before this change (gate's claim).

### 10.3 Measured and not built

| experiment | result | verdict |
|---|---|---|
| **Window past the frontier**: deal the first nodes round-robin, so every worker starts near the front (`PHITEX_DEAL=1`: a window of n nodes; `=8`: 8n) | Grail -j8 28.99 / 28.54 / 28.93 s, -j16 30.20 / 29.69 / 31.29 s (blocks / n / 8n); amsldoc -j8 3.32 / 3.16 / 3.37 s, -j16 3.62 / 3.40 / 3.81 s; busy within ±10% (accl 23045). ltnews -j4: 330 s with the window, where blocks timed out past 700 s in the same job (23169) | **Same result as P1.** It cuts or moves the waste but not the wall. The wall is the d = 0 chain plus its gaps; neither the window nor P1 touches the gaps. |
| **Never hold at the frontier** (`PHITEX_HOLD=0`); hold in pass 1 only (`=1`) | amsldoc -j8 3.36 → 2.74 s (=1: 3.42 s); Grail -j8 29.7 → 25.7 s (=1: 28.3 s), PDFs identical (23073). Repeated in 23092: amsldoc 3.42 → 3.07 s, Grail 29.4 → 26.6 s, Grail busy 57 → 116 s. beautybook-en -j4 18.5 → 16.5 s, lineno -j4 49.7 → 38.8 s, but lineno -j1 64 → 75 s and numerica -j4 39.3 → 40.8 s with busy doubled (23091); md5s identical on all five | The holds cost 10-18% of -j8's wall on amsldoc and Grail. Removing them doubles the wasted CPU, and they were added for convergence (3dd09934a, c24aff9c3: beautybook-en never settled). Not committed. The page-layer-sensitive sample (classicthesis, ArsClassica, elsarticle, kotex-utf, tex-vpat, spath3 at -j4/-j8 ×5) has not been run, so nothing here shows that a hold is unnecessary. |
| Text runs on a finisher's prediction instead of waiting for its commit (`PHITEX_WAITSETTLE=0`, together with HOLD=0) | amsldoc -j8 2.74 s, Grail -j8 26.4 s against 26.6 s with HOLD=0 alone (23092) | Grail's 4.4 s finisher stays on the chain: the text after it is refuted when it commits. |
| A re-asked text rerun waits for the first never-committed text node before it (`read::hole_before`; a98670c25) | ltnews -j4 318 → 509 s; amsldoc and Grail within noise (23119, 23120) | Reverted (4ef68fa02). The readers re-asked in ltnews meet committed definers running again, not holes. |
| WP0 again: a re-asked text rerun waits for the first open node before it (`read::open_before`) | ltnews -j4 601 s, past the limit, with a stopped evaluation's PDF; numerica -j4 past 700 s; amsldoc -j8 past 1500 s (23142, 23143) | It stalls on today's dev. Dropped. |

### 10.4 The hint pass: computed, and not on -j8's critical path

At -j8 the analyses already run on the pool. amsldoc's walk takes 141 ms at -j1 but 25.7 ms of wall at -j8 (183 ms busy). What stays serial is the chain of definitions (13.5 ms) and the engine push of 988 nodes (32 ms). Grail's is 18 ms + 38 ms (accl 23010).

So at -j8 the hint pass costs amsldoc about 80 ms of its 3.4 s, and Grail about 80 ms of its 30-40 s.

Making the bounds drive the schedule (P1's pending sets, or a "final read" rule) cuts wasted runs. But the wall is lost in the frontier holds and the finisher chain (§10.1), which no bound touches, and P1 measured exactly that: waste down, wall unchanged. Nothing in this branch trusts `may_define`.

So the answer is option (2): the hint pass does not pay for the schedule today. Its serial remainder is 45-55 ms (the definitions' chain and the engine push). That remainder is worth cutting only once the gaps are gone.

### 10.5 The -j4 storms (ltnews, numerica, atableau, …)

ltnews: -j1 19.8 s; -j4 166, 183, 208, 318 s, past 700 s and past 900 s in six builds of the same commit (accl 23116, 23142, 23120, 23169, 23192). The md5 is the same whenever a build finishes.

Pass 1 at -j4 (po, 23116) makes 157 237 text runs for 10 852 kept nodes:
- Kept runs at the frontier (d = 0) take 29.6 s of CPU. -j1's whole text chain takes 5.3 s.
- Small resume pieces take 180-260 ms per kept run, against under 1 ms at -j1.
- The Changed wakes: 90 018 come from a definer's first commit (a new piece) and 39 552 from a definer's rerun.
- By field: `count:301` 28 126 runs (70 s of CPU), `toks:12` 16 491, `skind` 73 253, `if@nobreak` 3 103.

The storm is therefore two things:
- re-asks on every new or re-run definer before a reader;
- the kept runs themselves running far slower than at -j1.

The second needs measuring first. A probe is on the branch (80b0d564f): `PHITEX_RUN_LOG` now prints, for each run, the reads computed again on a moved layout (`read::on_one_layout`) and the reads it filed.

What the probe ruled out (accl 23210, ltnews -j4, 505 s; 267 252 runs): a moved layout is not the cause. There were 270 retries in the whole build. The runs over 100 ms (610 of them) file the same median 1147 reads as the runs of 1-10 ms (96 844 of them). The same work takes 100x longer in some runs, so the time goes to waiting inside a run (locks, values interning), not to work. The lock accounting is accl 23222.

### 10.6 WP-T, the first measurement (accl 23146)

| | phitex -j1 | pdflatex |
|---|---|---|
| course cover, all 880 art statements | 4.37 s | 2.80 s |
| the first 441 statements | 3.89 s | 2.45 s |
| no art | 2.17 s | 1.24 s |

Every one of the 880 statements is a literal `\draw` (505) or `\fill` (375) of `figs/cover_art.tex`. So 100% of the art qualifies by syntax, and the art is 2.2 s, 50% of the build.

But the statements are not independent:
- each `opacity=` registers an ExtGState in pgf's global resource list;
- the first `Stealth` tip fills the arrow cache.

So per-statement nodes need those two as folds (a set union, a cache filled once), or they chain the way `count:301` chains ltnews.

Ceiling with a perfect WP-T: cover -j8 ≥ 2.17 s + 2.2/8 s ≈ 2.5 s, 1.75x. The preamble stays serial.

TikZ manuals, -j1 / -j8, median of 3, against one oracle pass:

| manual | -j1 | -j8 | oracle |
|---|---|---|---|
| tikz-cd-doc | 7.2 s | 10.0 s | pdflatex 2.9 s |
| circuitikzmanual | 307 s | 468 s | xelatex 123 s |
| pgfplotsexample | 1.13 s | 1.12 s | pdflatex 0.67 s |

On all three, -j8 is at best equal to -j1, for the reasons of §10.1, before granularity matters. WP-T comes after the gaps.

### 10.7 Ranked levers (measured ceiling first)

1. **Grail's hyperxmp finisher: 4.4 s × 3 passes, on the chain at every -j.**
   - The finisher should keep its identity across passes, so that a pass whose page-1 inputs held does not run it. That is page keys and loop rests: the pages area.
   - The text after it should not wait for it when what it reads is the routine's prediction from the last pass.
   - Ceiling: Grail -j1 −8.8 s, -j8 −9 to −13 s.
2. **Frontier holds that serialise the text** (−10-18% measured on -j8 with no holds; the wasted CPU doubles).
   - The rule has to keep the convergence the holds give (beautybook-en, lineno) and stop the serialisation.
   - With pages3 (adf163503416eac99), who changes the cut policy next to it.
3. **-j4 storms:** first find why kept runs are 200x slower at -j4 (23210), then the rule for re-asks on new definers.
4. **WP-T** (cover ≤ 1.75x), after 1-3.
