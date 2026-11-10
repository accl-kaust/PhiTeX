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
  use) and runs again once the frontier moves (`release_frontier`).
  Within W it runs as now. Nothing else waits: no commit is held, no
  lock is taken, a node commits where it stands (positions partition
  the commits as before). The frontier's own node and its continuations
  (distance 0) always run, so the fold is never held by the window.
- **How W is measured.** No fixed number: W adapts to how often running
  ahead was wasted, as a congestion window does.
  - W starts at 8 places.
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

## Measures

`PHITEX_TEX_TIME` prints the window's last W, its halvings and
doublings, and the reads it held (`acct: window`).
