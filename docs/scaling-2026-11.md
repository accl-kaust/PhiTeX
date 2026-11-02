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
