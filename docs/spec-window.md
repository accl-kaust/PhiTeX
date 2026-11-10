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
  - Each run ahead that commits widens W by one place (so a window's
    worth of runs in doubles it: as fast as the text runs, not only as
    the fold moves, which may be in bursts of many pages), and the runs
    the window held are let go where within it once W has grown by half
    since they were last looked at.
  - When the layer moves the frontier forward (a fold step's commit)
    and no run ahead was refuted since the last move, W grows by twice
    the places the frontier moved, at least doubling (to at most 2^20
    places, the whole text).
  - The page layer measures the exchange itself: each routine's
    give-back is compared with the guess made for its page
    (`Ops::give_back_guesses`). Once at least 4 have held and no more
    than one in eight missed, the routines hand back nothing the text
    could not have run ahead on, and W opens whole (`Ctx::open_spec_window`),
    the held runs let go at the layer's commit.
  So a document whose text and routines exchange nothing (para4000,
  amsldoc) refutes nothing ahead, W doubles every page and the text
  runs as far ahead as before after a few pages; one whose routines
  hand back state every page (floats2col) refutes on most pages and W
  stays near one page of text.
- **The fold's own window.** The other half of the chain: since 10-29
  ("no speculation depth") a clean build's fold never goes past a page
  whose routine's give-back no finisher has made yet, so each page waits
  for the routine before it (para400pdf at -j16: a critical path of 128
  pages; DVI the same). With the window on, the fold goes on the guess
  (`Run::guess_give_back`, what the routine gave back last) where the
  guesses have held: each give-back found against the guess made for its
  page is counted held or missed (`Ops::give_back_guesses`), and the fold
  trusts the guess once at least 4 held and no more than one in eight
  missed. Not a reversal of 10-29: a document whose routines hand back
  something new each page (floats2col) never earns the trust, and folds
  as it does now; one whose routines give back the same each page
  (plain text, a running head) folds on, its pages beside their
  routines. A miss refolds from that page, as a guess always did.
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

## Measures

`PHITEX_TEX_TIME` prints the window's last W, its halvings and
doublings, the runs it held and the first reads that waited for a
definer (`acct: window`; `PHITEX_WINDOW_LOG` prints the same after the
clean build's evaluation).
