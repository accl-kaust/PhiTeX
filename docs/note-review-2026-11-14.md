# Reviewing with an agent, 2026-11-14 (branch `review-notes`, `review-ui`)

What was built is specified in docs/spec-editor-protocol.md §9: `phitex
session` (start, exact wait, problems, notes, changes), notes in the
viewer anchored in the source, the agent's edits reviewed as word-level
changes against the git index or `.phitex/baseline/`, comments on
changes, and a review with a verdict. This log is the review passes made
with it in headless Chromium, what each found, and the numbers.

Screenshots and the driving scripts: ~/code/flinner/phitex-runs/review-2026-11-14/
(one folder a pass). The passes drive the real viewer of a real
`phitex session start` (release build, `--shelf`) from a sandboxed
Chromium (scripts/sandbox); the "agent" is the `phitex session` CLI.

## Passes

| pass | what it did | found |
|---|---|---|
| p1 | first note end to end: select, popup, compose, note, resolve, changes, accept | the popup covered the line below the selection; its buttons wrapped; the page was not refit when the side opened; notes in an `\input` file had no mark |
| p2, p2b | empty states; five notes; a spot note (Alt+click); 30 notes; a note over a page break; an edit that moves one note and detaches another; reply by keyboard; dark; 360 px wide; 1600 px wide; Clear resolved | marks drawn where the text was before an edit (refreshed on the build's origins now); long notes not clamped; a spot note's quote showed source markup; pages drawn from the build before (a frame asked as a build came in was labelled with the new build's page and never drawn again: frames now carry the hash of the page they were drawn from; stale pages 0 in 3 runs, were about 1 in 2) |
| p3, p3b | git: 50 changes, a change over two paragraphs, a comment, two draft comments, Request changes with a summary, the agent's revision of a commented change, Accept all, Accept while a build runs, Show typeset, Reject | a change's id moved when another was accepted (ids now count alike changes, not positions; an insertion led by a space is said one way); a comment jumped to an identical change elsewhere (comments follow their change's place, a decided change keeps its comments); "Waiting for your agent" shown on plain notes; the verdict list grew without bound; change marks drawn on Compare's pages; nothing left to see after Accept (a Reviewed section with the verdicts now); the finish dialog had no title |
| p4a, p4b | p2 and p3 again on the fixed build | nothing |
| p5 | real documents: amsldoc, a paper with a figure and BibTeX, a XeLaTeX document; a comment from the browser, the agent's loop timed | the first amsldoc run stopped in the driver: no popup came on the title's words (`amsldoc-failed.png`, the first build still on screen); the rerun passed and the cause was not pinned down. Otherwise nothing |

`stop` said the watch "did not end" when it ended between two checks (now
polls every 10 ms); `session notes` printed the wrong context; `start
--json` said `viewer: null`; pending errors had no file:line: found while
driving, fixed, covered by tests.

## The `\input` gap (temporary fallback)

The engine's glyph origins attribute the text of an `\input` file to the
`\input` command in the including file, so a selection in that text maps
to the wrong place. Until the origins' rework (with the origins agent)
gives each glyph its own file and byte, a note made there is anchored by
searching its file for the words the viewer saw (`seen`), and the viewer
finds the note's page boxes by the page's text. Both are marked
TEMPORARY in the code (review.rs `add_note`, review_text.rs `find_seen`,
cli.ts `boxes`) and go when the origins are right.

## Numbers

`phitex session` (local, 16 threads, release):

| | amsldoc | paper | xelatex |
|---|---|---|---|
| `start` (format cached) | 5.5 s | 1.7 s | 27 s first time (font index) |
| `wait` after a save (build inside) | 68 ms (build 58.7) | 28 ms | 20 ms |
| `wait`, nothing changed | 8 ms | 8 ms | 8 ms |
| `wait --notes` woke after Save/Submit | 11-30 ms | | |

One xelatex `resolve` took 1093 ms once (others 9 ms); not reproduced.

Edit speed, amsldoc at -j1 with `--shelf` (acclnode11, release, base
7cd832c21 = dev before the review, new 8f7d415fc; watch's build event
`wall_ms`, medians of 8 saves and back, rounds alternating; the box ran
other load, 5-12):

| | base | new |
|---|---|---|
| word edit, run 1 (3 rounds) | 8.2 ms | 8.0 ms |
| word edit, run 2 (6 rounds) | 10.0 ms | 10.7 ms |
| paragraph insert, run 1 | 321.4 ms | 334.6 ms |
| paragraph insert, run 2 | 358.9 ms | 321.0 ms |

Per-round medians spread 6.5-12.3 ms (word) and 315-424 ms (paragraph) on
both; the two agree within that noise, the order flipping between runs.
The review adds a file hash per read (`Host::fed`) and the hub's work
after the build event.

## The agent's side (feedback from a real session on Grail)

An agent used `phitex session` on Grail and found: the engine choice
invisible, `.phitex/` appearing in `git status`, no counts of the
problems, the output folder unsaid, and nothing saying whether the verbs
reach a watch the user started. Fixed, then driven again (the CLI from
bubblewrap sandboxes, target/play/agentpass{,2,3}.sh, logs beside them):

| case | says now |
|---|---|
| fontspec paper in git, `session start` | `engine xelatex (picked: the document loads fontspec) · output folder . (the default: beside the main file)`; `phitex: made .phitex/ … and added it to …/.git/info/exclude`; `git status` without it |
| fontspec behind `\ifXeTeX`, `--engine pdflatex` | the fatal error, then `fontspec runs only under XeTeX or LuaTeX, and this build ran pdflatex: build with XeLaTeX (--engine xelatex, or % !TEX program = xelatex in the main file); LuaLaTeX is not supported yet` |
| luacode | `paper.tex needs LuaLaTeX (it loads luacode), which PhiTeX does not support yet: …` |
| `-o out`, a .latexmkrc's `$out_dir` (--exact-latexmk), phitex.toml | `output folder out (-o)`, `… ($out_dir in .latexmkrc:1)`, `… (output-dir in phitex.toml)` |
| overfull lines in two files | `problems --summary`: `overfull 7, of which ledger.tex 5, main.tex 2` / `reference 2, of which main.tex 2`; `--json` the same as objects |
| a watch started in a terminal, verbs from a subfolder | found, `status` and `wait` as for a started one |
| the watch in one sandbox, the agent in another with the network | found (before: "no phitex watch runs here": its pid is another PID namespace's, and a reader with a writable folder removed the watch's file) |
| … the agent without the network | `a phitex watch of this folder runs (pid 3 …) but its socket, 127.0.0.1:37661, does not answer from here: … (bubblewrap: --share-net)` |
| … the agent without `$XDG_RUNTIME_DIR` | found in `/run/user/<uid>/phitex` (before: `the sessions folder: …/sessions: Read-only file system`); with no watch, the folders it looked in |

Discovery files now carry the watch's PID and network namespaces: a
reader removes a file only when it can tell the watch is gone (the same
PID namespace and the process gone, or the same network and the socket
refused).
