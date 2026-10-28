# Failures seen only sometimes at -j8 and -j16 (note, 2026-10-28)

Status: the intermit agent's open findings, handed to the reds agent.
Fixed and on dev: the lost wake of `At::End` readers (7ffbf9b1) and the
block-head-first scheduling rule (0ccf5e9b). Everything below is open.
Numbers come from accl jobs unless a job says mc.

## 0. The reds agent's queue (2026-10-28, at the session's halt)

In order. Each item has its state and the next step. Loop scripts are
in `~/code/tmp/phitex-reds/.reds/`, outside the repository:

- `amsloop.sh N J...`: N cold amsldoc builds per -j, with md5 and the
  skip trace.
- `showbox*.sh`, `specials*.sh`, `stack*.sh`, `shelfrepro.sh`: pdftex
  oracles.

Submit them with `CLUSTER=mc scripts/accl/accl submit cmd bash -c '...'`.

### 0.1 The read-on rule (branch reds11, fa388673)

**Cause.** The `\item` pieces of amsldoc skipped to the document's end
at -j16 (21 of 40 builds, mc 75763). A speculative piece read a mix:
imakeidx's two-parameter `\@wrindex` (its `\let` had committed) and
the kernel `\makeindex`'s `\index` (imakeidx's `\makeindex[intoc]` had
not). The mix opened an `\if@endpe` inside protected@write's `\edef`,
and the skip then pulled 735 to 774 siblings.

**Rule (main's decision).** A read-on (a pull that claims the next
sibling) is taken only when every node before the puller has
committed. Otherwise the run halts with `Halt::Hole` and is filed with
that node, which runs it again when it commits, is taken, is removed or
moves.

Test: `fatal::a_read_on_waits_for_the_text_before_it`.

**First version (3a90060b).** A waiting run committed with
`Ctx::unchanged`. Gate 22038-22049 was red:

- amsldoc -j8 and -j16 gave wrong PDFs: 43 pages, and at -j16 "File
  ended while scanning use of \@newl@bel".
- Walls were 5.0 s at -j8 and 5.2 s at -j16, against 2.7-3.1 s and
  2.3-2.7 s.

**Second version (fa388673).** The waiting run ends as a starved run
does (`stall "hole"`, nothing defined). Gate 22056-22067 was red, so
**do not merge reds11**:

- amsldoc was wrong at -j1 and at -j8 (md5 e35ec8b6, 43 pages); -j16
  was right.
- Walls were 32.5 s at -j1, 9.7 s at -j8 and 9.2 s at -j16.
- editkinds -j1 differed.

A wrong output at -j1 is not a race. Next step:

1. Count the waits per build, and the node kinds waited on. The rule
   waits for every node before the puller that has not committed,
   including page-layer nodes, finishers and paragraph children. Some of
   these wait on text after the puller. Each wait that ends in a stall
   also takes back that node's definitions, which re-asks its readers.
2. Restrict "before" to the text nodes (`exec`, `resume`) of the
   puller's unit, and keep a waiting run's last record (`unchanged`)
   only when the run is not a first run.
3. Find the lost output at -j1 with `PHITEX_NODE_DUMP` on -j1 builds. The
   suspect is a waiter whose wake is lost between `file_waits` and
   `wake_waiters`, so its stall stands at the end.

### 0.2 §1069 and `\showbox` / `\showlists` "! OK." (branch reds13, a0a22209, on dev)

**Done.**

- `\showbox` displays the box as `show_box` does (new `showbox.rs`,
  §180-§198), then "! OK.". It counts for the exit status and never
  towards the hundred errors.
- An extra `}` is reported: "Too many }'s", or "Extra }, or forgotten
  \endgroup / $ / \right".
- Tests: `diagnostics::showbox_displays_the_box_and_says_ok` (oracle mc
  75792) and `an_extra_right_brace_is_reported`, 16 of 16 on mc 75793.
- Gate 22080-22091 (the same change on top of reds11 is reds12,
  145eacb9).

**Left, each with its next step.**

- `\showlists` logs the empty line and "! OK." only.
  - Its activities need: each nest level's entry line (a field on
    `nest::Level`), the page's status at the outer level (a read of the
    page loop's state), prevdepth and spacefactor, and the display of
    noads (§690-§698).
  - Next step: the outer vertical level with an empty page. "### vertical
    mode entered at line 0" and "prevdepth ignored" is most of the
    l3build uses.
- `\show`, `\showthe`, `\showtokens`, `\showgroups` and `\showifs` exit
  0. pdftex exits 1, because `error()` sets the history (mc 75789).
  - The CLI's exit status comes from "! " lines (compat.rs, session.rs).
  - Next step: a `Build::history` (`log:errors` plus the show events),
    and compat's exit status read from it.
- Layout differences:
  - The log is laid out in batch mode. pdftex in nonstop mode prints an
    extra blank line before "! OK." when the terminal's offset is not 0
    (mc 75790).
  - Errors have no context lines.
  - Overfull and underfull boxes show no box (§663 calls `show_box`).
    `Run::show_box_lines` now makes that display.

### 0.3 The 46 XeTeX page-one `\special`s (note-xetex-corpus.md §2.1)

**State.**

- Plain probes match pdftex's DVI: a `\special` before the first box,
  with `\write`, `\message`, a kern or a penalty after it
  (`.reds/specials.sh`).
- fs5 and fs6 differ under pdflatex's DVI path too, not only under
  XeTeX. fs5 is `regression-test.tex` plus `\geometry`. fs6 is
  `\START\OMIT` before the first word.
- The lost specials are issued in vertical mode by
  `regression-test.tex` lines 215-300, before `\documentclass` (mc
  75769 printed them).

**Next step.**

1. Cut fs6 down: drop `regression-test.tex` and keep its two `\special`s,
   then `\OMIT`'s `\typeout`, then `\documentclass`.
2. Compare `pdftex -output-format=dvi` with `phitex --compat=pdftex
   -output-format=dvi`.
3. Then follow the whatsits through the page builder's first page
   (§1000: a whatsit on an empty page is kept, not discarded) in
   `pages/`.

### 0.4 The rest, in order

- The intermittents below: §1 (stale reads at `Me`, branch
  intermit-scope b503f662), §2 (the placement orphan), §3 (core.rs
  counters).
- Run-137 cut mode: 858 of 863 uncut, so the indentation is wrong at
  -j16. Main asked for the cause with a test that fails before the fix.
- Closed: Shelf's input-stack overflow. It is no PhiTeX difference:
  pdftex on Shelf's tree overflows at the same place (mc 75779). The
  oracle rule is in note-corpus-differs.md.

## Tools

Loop scripts live outside the repository, in `~/code/tmp/intermit-tools/`:

- `loop` / `loopmc`: N cold builds of one corpus manual (`latex/<dir>`
  under the CI image's doc tree) or of a file in `EXTRA` (docdir `h/x`).
  They use a commit's gate binary (`BINV=` gives `cli/phitex`, with
  observe; the default is `cli/prof/phitex`) and run in the CI image.
  - Each build prints one JSON line: md5, exit, check_reads stale count,
    nodes run, runs.
  - With `WANT_MD5`, only bad builds are kept (wrong md5, exit != 0, or
    a stale read). Without it, every build is kept.
  - Settings: `LOOP_PAR` builds at once, `SL` for sbatch options, extra
    `ENV=VAL` arguments for the builds.
- `sum` / `summc`: tabulate a loop's builds.
- `tloop COMMIT SUITE TEST N PAR`: one phitex-tex test N times on accl,
  from `gate-bin/COMMIT/tests`.

## 1. Stale reads at -j16: readers at their own position never re-asked after a scope change

amsldoc `--exact-pdf -j16` with `PHITEX_CHECK_READS=1` (observe CLI).
On dev ae946367 + 0ccf5e9b (5163f523), 4 of 60 builds ended with stale
reads; md5 and exit were right every time (accl 21908, kept runs in
`im-ams16st-174226/docs/bad*` on acclhead1). Before 7ffbf9b1 the count
was 25 of 100 (a5cdfa90, accl 21848), one of them exit 1 with wrong md5
ba0244c0. After it, 15 of 100 (11d80b11, accl 21849). The -j8 loops
showed none.

Every remaining stale read has the same shape: an exec or resume node's
read at its own position (`Anchor::Me`) saw a value and now finds none:

    node 858 (exec) read Field { f: Field(ValueId(9223373136366428494)), at: Me } path 0 saw [14, 1, 0, 1, 1, ()] but now none
    node 1424 (exec) read Field { f: ...733254, at: Me } saw ["macro", 0, [], ["\if@newlist", ...]] but now ["ma...
    node 3769 (resume) reads ...428494, ...403689, ...403736 saw [0, 0, 0, 0, 0] but now none

"Now none" at the node's own position means the definition it read is
no longer visible there. Either it was removed, or a scope close was
filed between the definer and the reader. Bracket changes re-ask in
`commit::file_effect` -> `reask_after_effect_at`, which walks the
segments after the change and collects their readers after the lock.
A run that began before the change is covered by `scope_gen` in its
commit's check. Which path lost the wake is not found yet.

This is the likely cause of the stray diagnostic ("\begin{error} on
input line 2365 ended by \end{document}", exit 1 with right bytes, the
reds agent's amsldoc -j16 run 194, accl 21198). A text piece that keeps
a stale `\@currenvir` reports exactly that.

Next step: b503f662 (branch intermit-scope, on 77d3ec47) makes
`check_invariants` name the field and, for a read that now finds
nothing, give the definition the close hides (node, label, depth) and
the reader's depth. Its mc build was submitted (mc job 75723). Then:

1. Run `loopmc ams16 b503f662... latex/amsmath amsldoc.tex 16 100
   PHITEX_CHECK_READS=1` with `WANT_MD5=861b74bd6b22fb11405533e21c65fe02`
   and `BINV=` (the observe CLI). Read the field and the hidden definer.
2. Trace that field with a `PHITEX_TRACE_FIELDS` probe: the tr- lines of
   9dbab443 and 490dfe1f on branch intermit-probe (on a5cdfa90), extended to
   `reask_after_effect_at`.

## 2. A placement orphan at -j16

Seen once in the same 60 builds (bad22): `check_reads: node 6535 is on
the line, its placer 5977 holding it no more`. A node is still placed
after its placer's record dropped it. This is claim or placement code
(`commit::retract_products`, `relocate`, `claim_walk`), the reds
agent's area. Reproduce with the same loop and grep for "holding it no
more".

## 3. defuse tests/core.rs bounds that depend on machine load

Measured:

- In the default parallel run of the file, 10 of 10 runs had a red
  (accl 21636 on bbbfbbc3):
  - `commits_do_not_wait_on_each_other`: commit_max_ns 2-12 ms.
  - `a_learned_shape_runs_the_chain_side_by_side`: 156-163 ms against
    a 150 ms bound.
- With those tests run one at a time, 10 of 10 passed (accl 21691).
- `commits_do_not_wait_on_each_other` alone still fails, and the failure
  rate follows workers per CPU (accl 21763, 21856, 21858):

  | workers | CPUs | red |
  |---|---|---|
  | 32 | 32 | 30 of 30 |
  | 16 | 16 | 12 of 30 |
  | 15 | 16 | 8 of 30 |
  | 16 | 32 | 1 of 30 |

  - The commit-laps of the long commits (profile, accl 21847) put 1-5 ms
    in phases that do no work ("checks" over 0 reads, "contribs" over
    0 contributions, "index"). Several threads show the same ~2.5 ms at
    once, with no lock wait. That is preemption, not engine work.
- On a loaded node (acclnode01), with half the threads and the timing
  tests alone (af7a6387, branch intermit-core, NOT taken), 25 of 50 runs
  still had a red (accl 21910), against 1 of 50 on acclnode03
  (accl 21909):
  - `scales_with_threads`: 8 threads 164-185 ms against 1 thread
    809 ms, under its 5x bound.
  - `a_node_that_never_contributes_is_no_stop`: a count assertion
    ("the reader ran once, past the quiet node still running").

Plan (coordinator's): assert each property with the engine's own
counters, which preemption cannot move, and keep wall times as a report
only.

- `commits_do_not_wait_on_each_other`: no commit waited on another's
  lock (`lock_wait_ns`, or per-commit overlap evidence), not
  commit_max_ns.
- `scales_with_threads`: the work's shape (critical path against total
  runs, busy time per worker), not 5x wall.
- `a_node_that_never_contributes_is_no_stop`: a count should not
  depend on load. Find why it does; it may be a real ordering bug (the
  `next_after` reader re-run when the worker holding contribution 2 is
  delayed).

## 4. Status of the other items

- **marginpars28** (`latex_marginpars_settle`): fixed by 7ffbf9b1.
  - The CLI at -j8 gave a wrong PDF in 10 of 120 builds, and 23 of 120
    had stale finisher reads (b070fd91, accl 21414). After the fix,
    0 of 120 (accl 21607).
  - The test itself was red in 1 of 30 runs before (accl 21402) and 0 of
    60 after (accl 21608).
  - The run counts still vary at -j8: 1623-2681, median 1966, on
    dev+fix (accl 21754). With 0ccf5e9b they are 1987-2967, median 2320
    (accl im-mp4-1d15b6). The block-head rule costs marginpars about 18%
    more runs; amsldoc saves about 25%. The churn is in finishers
    (`finishers: 4000-8000 created` for 31 fires), page layer, docs
    agent.
- **amsldoc high mode** (13-15k runs in a quarter of the -j8 builds):
  fixed by 0ccf5e9b.
  - 32 builds one at a time went from 3923-13912 runs (median 6726,
    9 of 32 above 11k) to 3613-6869 (median 5291), same md5 (accl 21759
    and im-ams4-1d15b6).
  - At -j16 the spread is still 6.2k-17.6k on builds without 0ccf5e9b
    (accl 21848, 21849); with it, the lowest of 60 builds was 4816 (accl 21908, im-ams16st); its
    full spread was not tabulated.
- **tqft** at -j8 was exact in 6 of 6 and 12 of 12 on b070fd91 (accl
  21385, 21407) and 12 of 12 with 7ffbf9b1 (accl 21611). The md5
  23588526 was never reproduced.
- **editkinds -j8** gave "same" in 12 of 12 sessions on a5cdfa90 and
  12 of 12 with the fix (accl 21917, 21920). Not reproduced.
- **pages::an_edit_that_moves_a_break_out_of_a_claimed_span** passed 60
  of 60 on b070fd91 and 60 of 60 with the fix (accl 21400, 21610). Not
  reproduced.
