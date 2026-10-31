# Page streaming: measured, designed, not built (2026-10-29)

Status: put aside. On 2026-10-29 the user ended the editor-protocol work
before page streaming was built. This note is where the next agent starts:
the goal and what exists, what was measured, why the trigger first planned
(a finisher's commit with nothing open before it) is not enough, and the
design that follows from the numbers.

## The goal (option B, provisional)

Show page k as soon as its content can no longer change in the ordinary
case, before the whole evaluation settles. The event is
`{event: "page", k, hash, version, provisional: true}`, sent when page k
is first known, and its pixels are drawn from that page's own shipment.

- Provisional pages are display only. They are never written to disk,
  never served by `phitex.savePdf`, and never part of the exact file.
- The viewer drops any page whose version is older than the one it has.
  When the settled build's page has the same hash, it does not draw the
  page again.
- (C) folds in: when the evaluation ends, the settled pages' events go
  out first, and drawing starts from the layout before the files are
  written and before the diagnostics.

## What exists

- **Viewer.** `viewer/src/session.ts` `onWatch` already handles
  `{event: "page", k, hash, pages?}`. It records the hash, grows the
  page list and calls `sink.layout`. The browser then asks for that
  page's `frame`.
- **Native frames.** `Viewer::frame(k, scale)` (`crates/editor/live/src/view.rs`)
  draws strips from `Viewer::pdf()`, the last build's whole PDF. A
  provisional page therefore needs a PDF (or bytes the strips can draw)
  of its own, and `frame` must use it while it is newer than the settled
  page.
- **Engine, where a page is made.** `pages/finisher.rs`
  `Run::finish_page` assembles the page, replays it, runs the output
  routine and calls `publish_page`. The progress hook (`note_page`, near
  line 234) sits at its start.
  - `\shipout` is `pages/routine.rs` `ship_out` (line ~402). It decodes
    the box and calls `pdfout.rs` `ship_apart`.
  - `ship_apart` keeps the page as a `PagePiece` under its key and puts
    the key in the line (`XPAGES`). Its PDF side is the `PdfPage` child.
  - `PdfPage` is `final_only`: it runs once the loop-backs settle, never
    during the evaluation.
  - The link (`finish_exact`) numbers every page's objects across the
    line.
- **Engine, writing one page.** `write_page_with(v, pc, bx)` writes one
  page on a writer of its own, folded from the base (`base_fold`). The
  page's sealed lines are filled from their pieces' units (`sealed_units`
  reads `PITEMS` through the node's `Ctx`). That read can only happen
  inside a node, so a provisional page's writer is a child operation:
  like `PdfPage`, but not `final_only`.
- **A one-page file.** No function makes one yet. Linking a line that
  holds only that page's record, the way `finish_exact` links, would
  give a page whose object numbers differ from the exact file's. For
  display that does not matter.

## What `open_before` covers

`defuse::read::open_before(eng, me)` scans every node from the start of
`me`'s unit up to `me`, and returns the first one that is open
(`Node::is_open`: to run again, or for the first time). It costs O(n)
per call. Its only user is the stuck check in `sched.rs`, and `Ctx` has
no accessor for it.

The measurement patch adds `Ctx::open_counts`, which returns the open
nodes before me in my unit and in the whole line. It lives on branch
`stream-measure` (pushed to origin), with the job's script, and is not
for merging.
In every finisher run of Grail and amsldoc the two counts agreed: a TeX
document is one unit.

## Measured (mc 76736 and 76751, `bench/stream-measure-tmp.sh` on `stream-measure`)

How the runs were made:

- `phitex watch -j 16`, fastdev with `phitex-tex/profile`, and
  `PHITEX_FIN_TIMES=1`.
- Every finisher run prints its end time, its page (the finisher's
  `normal`), and the open nodes before it.
- Times are counted from the build's start (cold) or from the file's
  write (edit), and run to the watch's `ok:` line.
- "Clean" means a finisher run that ended with nothing open before it.

| build | end (ok line) | first page clean | pages clean at a run of theirs | the rest |
|---|---|---|---|---|
| Grail cold (xelatex, 18 pages = 36 column fires, 9.2 s of format) | 57.5 s | 21.0 s | all 36: 0–29 by 27.6 s, 30–35 at 39.6–40.2 s | every finisher runs again at 44.5–53.0 s (the second pass: references) |
| amsldoc cold (pdflatex, 44 pages) | 10.2 s | 7.2 s | pages 1–3 (7.2, 7.9, 9.5 s) | pages 4–44: their last runs end while nodes before them are still open |
| amsldoc edit, line 305 | 146 ms | 56 ms (the edited page) | that page only | the pages after it run last at 74–90 ms, never clean |
| amsldoc edit, line 2427 | 356 ms | 65 ms (the edited page) | that page only | the pages after it run last at 72–177 ms, never clean; the evaluation ends at 345 ms on resume nodes |
| Grail edit, line 260 (a word typed on page 1) | 125 ms | 64 ms (column 2 of page 1), 87 ms (column 1 of page 2) | both columns the edit ran again | no other finisher ran |

Grail's edit timeline (build 2) is 112 ms from the change: 102.5 ms of
evaluation, then about 7 ms (the session's diagnostics 1.0 ms, the build's
thread joined 0.8 ms, the viewer's PDF and its diagnostics 2.9 ms, the
problems placed 0.9 ms). There, (C) would gain about 5 ms.

After the evaluation, amsldoc's edit timelines show:

- the link: 1.2–1.4 ms;
- the watch's files: about 1 ms;
- the viewer's PDF and diagnostics: 0.6 ms.

The evaluation itself is 96–98 % of an edit. On its own, (C) gains next
to nothing; the time is inside the evaluation.

## What the numbers say

1. **The trigger.** "A finisher commits with nothing open before it" is
   not the right trigger.
   - On amsldoc, a finisher's last run almost always ends while some
     node before it is still open: speculative text runs in parallel.
   - That node then commits without changing the page, and the page is
     final with no commit of its own.
   - So the trigger must be "the settled prefix passed this finisher":
     when the lowest open position moves past a finisher whose last run
     stands, that page is ready.
2. **What defuse needs.** defuse has no settled prefix today (the
   design memory says so). The smallest addition is:
   - the lowest open position, kept as each node opens and closes (a
     count of open nodes per segment, with the first non-zero segment
     found on a close);
   - an event when it advances, listing the marked nodes now behind it.
   This is `Engine::mark` and `note_settled`, which already exist for the
   quiet points, made to fire mid-evaluation. The finishers would be the
   marked nodes. The O(n) `open_before` scan cannot be called at every
   commit.
3. **The gain.**
   - Cold: Grail's first page at 21 s instead of 57.5 s; amsldoc's at
     7.2 s instead of 10.2 s.
   - Edits on amsldoc: the edited page at 56–65 ms instead of 146–356 ms.
   - Grail's edit: the edited page at 64–87 ms instead of 125 ms.
   - The pages after an edit are not shown sooner by the commit trigger.
     The prefix trigger would show each one as its last run is passed.

## Where the hooks go

- **defuse.** The settled-prefix event described above: an
  `EditSource::settled`-like callback, or `Engine::take_settled`
  answered mid-evaluation.
- **pages.** Mark each finisher node as it is placed.
  - In the callback, the page's key comes from the `PagePiece` its
    `ship_out` kept (`ship_apart`'s `key`).
  - Its writer runs as a non-final child (`write_page_with` on its
    `base_fold`), linked alone into a one-page PDF.
- **live.** Add `Viewer::page_provisional(k, hash, version, pdf)`.
  `frame(k)` draws from it while its version is newer than the settled
  build's. The event is `{event: "page", k, hash, version,
  provisional: true}`. `Session` takes the callback through `Status`'s
  meter, as the progress does.
- **The spec and the frontends.** Add the event's fields to
  `docs/spec-editor-protocol.md` §2. The viewer already draws a `page`
  event; agree `version`/`provisional` with Phi Frontends.
- **Test.** A document with slow later pages, where page 1's event
  arrives before the build's `build` event.
