# The speculation window (`PHITEX_SPEC_WINDOW`, off by default)

## The problem

At more than one thread the text runs ahead of the page layer: a piece
of text runs as soon as a worker is free, on the definitions that reach
it then, and the page fold places each page's finisher later. Where the
page routines and the text exchange state on most pages (a two-column
document full of floats: the routine hands back `\@freelist`,
`\@currlist`, `\@colroom`; the text's float macros read and extend them,
and set counters and boxes the next routine reads), a run ahead reads a
state the routine before it has not made yet. Every refutation then
travels down the chains such state threads through the text (each
float's `\@currlist`, a `\toks` register a continuation passes on, the
page parameters a float environment resets) and back into the routines,
whose reruns move the pages the fold cut. floats2col at -j8: 194 fires,
776 finishers made, 1 400 to 3 300 finisher runs, run to run; at -j1
194 and 194.

Holding single reads by position (the frontier's holds of the fields
the page layer defines, `Ctx::hold_at_frontier`) does not reach it: most
refutations are of fields only the text defines, along chains that start
at a routine. Four policies that held reads were measured and dropped
(the fold waiting for missed give-back guesses; the text waiting on
definers to run again; the frontier moved back to a re-asked finisher;
finishers waiting on text to run again): none moved the count outside
its run-to-run spread.

## The window

The text may run at most **W** places past the page layer's frontier
(`Engine::frontier`, the point past which the layer may yet place a
node). A place is a slice of the text as pushed (`order::GAP` labels
apart); a node's distance is the labels between it and the frontier's
node, over `GAP`. Continuations of a slice stand in its gap: distance 0.

- **What holds a node back.** A run of a node outside the settling layer
  (`Operation::settles_at_quiet` false), in the frontier's unit, more
  than W places past the frontier stops at its first read
  (`Ctx::hold_at_frontier`, the hold the layer's own fields already
  use) and runs again once W reaches it or the frontier passes it
  (`release_frontier`), each by the one test `sched::frontier_holds`.
  At one thread nothing is held: one worker takes the runs in order.
  Within W it runs as now. Nothing else waits: no commit is held, no
  lock is taken, a node commits where it stands (positions partition
  the commits as before). The frontier's own node and its continuations
  (distance 0) always run, so the fold is never held by the window.
- **How W is measured.** No fixed number: W adapts to how often running
  ahead was wasted, as a congestion window does.
  - W starts whole (`WINDOW_START`, the whole text): it narrows only
    where the page layer refutes a run ahead of it, so a text that
    exchanges nothing with the layer is never held (para4000, plain
    text whose routine only ships its pages: no run is held, nothing is
    waited for). The first such refutation brings W to half the distance
    it was made at; the runs past that from then on are held.
  - A text node's run records its distance past the frontier when it
    read (`Node::ahead`). When that run is refuted (a reader re-asked by
    a changed definition, `commit::reask_reader`) and it had run ahead
    (distance > 0) by a change of the page layer's own (a routine's
    give-back, a node the fold placed: the definer is a settling node),
    W halves, to no less than 1 place and no more than half the distance
    it was refuted at. A refutation by the text alone says nothing of the
    window: a run behind the frontier meets it as well.
  - Each run ahead that commits widens W by one place, and the runs the
    window held are let go where within it once W has grown by half
    since they were last looked at. Once the layer has refuted a run
    ahead in the evaluation (`sched::exchanged`), a window's worth of
    runs in widens it by one place (congestion avoidance after a loss;
    `PHITEX_WINDOW_AIMD=0` keeps the place a run).
  - When the layer moves the frontier forward (a fold step's commit)
    and no run ahead was refuted since the last move, W grows by twice
    the places the frontier moved, at least doubling (to at most 2^20
    places, the whole text); once exchanged, by one place.
  So a document whose text and routines exchange nothing (para4000)
  refutes nothing ahead and W stays whole: nothing is held; one whose
  routines hand back state every page (floats2col) refutes on most
  pages and W stays near one page of text. (Opening W whole once the
  routines' give-backs matched their guesses, as a first version did,
  opened it on floats2col too: its routines give back what they gave
  last while the text's float macros read and extend what they handed
  back. The refutations are the measure of the exchange, not the
  give-backs.)
- **The fold's own window.** The other half of the chain: since 10-29
  ("no speculation depth") a clean build's fold never goes past a page
  whose routine's give-back no finisher has made yet, so each page waits
  for the routine before it (para400pdf at -j16: a critical path of 128
  pages; DVI the same). With the window on, the fold goes on the guess
  (`Run::guess_give_back`, what the routine gave back last) where the
  guesses have held: each give-back found against the guess made for its
  page is counted held or missed (`Ops::give_back_guesses`), and the fold
  trusts the guess once at least 4 held and no more than one in eight
  missed, and only while W has never narrowed (`Ctx::spec_window_whole`:
  the layer has refuted no run ahead of it in this evaluation; W growing
  back whole does not bring the trust back, the exchange is the
  document's). Not a
  reversal of 10-29: a document whose routines and text exchange state
  (floats2col) narrows W and never earns the trust, and folds as it does
  now; one whose routines give back the same each page
  (plain text, a running head) folds on, its pages beside their
  routines. A miss refolds from that page, as a guess always did.
- **Reruns on a clean build wait for their moving sources.** With the
  window on, `sched::hold_for`'s chain rule, an edit's until now, holds
  a clean build's runs too: a node running again waits for a source of
  its last run that is to run again and moved a reader
  (`read::open_definer_before`); not the movers an edit made (a clean
  build makes none). Without it floats2col's text ran again in waves
  along the chains its float lists thread through (`PHITEX_WINDOW_CHAINS=0`,
  mc 83862: 1 524 to 4 816 finisher runs at -j8/-j16, CLI builds of 3 to
  43 s against 1.2 s); with it 204 to 1 447.
- **Edits.** The window applies to a build's first evaluation (the
  clean build, `Build::settles == 1`) only. An edit's evaluation runs
  with it off: an edit runs few nodes, most of them before the
  frontier, and edit speed wins ties; the window costs an edit nothing.
- **Off.** With the flag off (the default) nothing above runs: no
  distance is read, W is never touched, the holds are the layer-field
  holds alone.

## Progress: no livelock

The window decides when a run begins, never what it makes: a held run
is a node left dirty and queued nowhere (`park_frontier`), and the run
it makes once let go is the run it would have made. So the window can
break the evaluation only by keeping it from ending. It cannot:

1. **One test.** Whether a run waits is one function,
   `sched::frontier_holds`: a run the window held (`Node::held_window`)
   waits while it stands more than W places past the frontier and has no
   pass; any other held run waits while it stands past the frontier.
   `work` asks it before a run begins, `park_frontier` as the run parks
   and again under the list's lock, `release_frontier` as it lets runs
   go. (The livelock seen at -j16, 2 in ~20 runs, was two tests that
   disagreed: let go within W, a run was tested before it began by
   "past the frontier", parked, found not held by "past W", queued, and
   taken again, for ever: every worker busy, runs ending, nothing
   moving, so the stall check never fired.)
2. **A release is a run.** A run let go by the window gets a pass
   (`Node::window_pass`): its next run is not held by the window,
   whatever W has become meanwhile (the pass is taken at its first
   read). So every hold is of a run queued by its node's creation or by
   a wake, and a wake comes only from a commit: the window makes no
   more holds than there are nodes and wakes, and a run is let go at
   most once a hold. No
   cycle of holds and releases goes on without a commit. A run let go
   while W shrank runs ahead once; that is the one waste the pass
   costs, at most one run a hold.
3. **The window never keeps the only work from running.** When every
   worker is idle and none is at a task, and runs are held by the
   window, W widens to the nearest of them and they are let go
   (`Pool::release_window_only`). Only then: while a run goes on, what
   it commits widens W or wakes what it held.
4. **So the schedule is fair**: every dirty node runs, at the latest
   when nothing else can, and each run is one the engine without the
   window could have made in some order. The evaluation reaches the same
   fixpoint it reaches at any order of runs; the window only chooses one.

## First reads: a read before its definer

The same race as the window's at the start of a text: on a clean build
every piece is made at once, and with more threads than ready work they
all start together. Under load at -j16, 121 of para400dvi's 626 nodes
ran again because they read `\hsize` before the first piece committed
its `\hsize=6in` (docs/note-read-ahead-race.md). The value read is
defined (the format's), so the wait for a definer of an undefined
control sequence did not catch it.

The rule, the window's own (on where the window is: a clean build's
evaluation at more than one thread): a text node's read of a field
before its run's first command is done (`Run::load_slow`, `cmds <= 1`:
nothing to throw away) waits for the nearest node before it in its unit
that has not committed and whose operation names that field among what
it may define (`MayDefine::Names`, or a prefix of it in
`MayDefine::Prefixes`); it runs when that node commits
(`Ctx::wait_for_first_definer`, `Ctx::wait_on`, a wait and not a run).
The nodes are found once a run, at its first such read
(`read::holes_naming_definitions`), and each read after is checked
against them. A later read, the run's work begun, goes on as before: a
definition that moves it re-asks it.

- **Not `Any`.** A node that may define anything (a package load, a
  LaTeX piece whose hint is unbounded) is not waited on by this rule:
  every read after it would wait for it, the text after it in order.
  Its new names are the undefined-control-sequence wait's
  (`Ctx::wait_for_definer`).
- **Order.** A wait is always for a node before the reader, so waits
  form no cycle; the node waited for is let go by the window's rules
  like any other, and a held one at the latest when nothing else runs.
- **Edits.** Off with the window: an edit leaves almost every node
  committed, and edit speed wins ties.

## What it does not do: floats2col

floats2col (194 fires) is not brought to its bar (at most 1.1 finisher
runs a fire at -j8 and -j16) by the window. Measured on mc (specwin2):

- the window as above: 196 to 1 292 finisher runs a build (20 builds,
  mc 83876); with congestion avoidance 197 to 887 (mc 83892);
- W held at 0 once exchanged (`PHITEX_WINDOW_FLOOR=0 PHITEX_WINDOW_PIN=1`,
  only the frontier's own slice runs): 194 to 527 (mc 83901);
- W 0 from the start as well (`PHITEX_WINDOW_START=0`): 211 to 1 330
  (mc 83913).

So text running ahead of the fold is not what runs the finishers again:
with no run past the frontier at all they still run 2 to 7 times a fire
in some builds. The reruns are of finishers re-asked by state the text
before their page defines (`cs:@freelist`, `count:242`, `box:247`), and
of that text re-asked by the scope structure (`skind`, 12 757 of the
reruns over 20 builds) and the float lists. The 194/194 a first version
of the window measured came from a fault, not a policy: a run let go
within W was parked and queued again for ever until the frontier passed
it (the livelock above), each such run a worker spinning, so few runs
ran at once. What remains is the exchange between the routines' group
and the text's scope structure behind the frontier, not speculation
ahead of it.

## Measures

`PHITEX_TEX_TIME` prints the window's last W, its halvings and
doublings, the runs it held and the first reads that waited for a
definer (`acct: window`; `PHITEX_WINDOW_LOG` prints the same after the
clean build's evaluation).
