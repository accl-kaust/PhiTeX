# Why a one-word edit re-runs exec nodes (amsldoc census)

Owner: edit-causes agent, for Phi. Branch `edit-causes` (from edit-sweep 6e710e39).

## 1. Census (accl job 11869, release, commit f5ca7e9e, 2026-10-16)

Instrument: `PHITEX_RUN_LOG=1` records every run attempt (`Stats::run_log`:
node, op, resumed, speculative, how it ended, round); a rerun's cause gets
its round and the first place where the value read differs
(`structural_diff`). `PHITEX_SWEEP_CENSUS=1` prints, per sweep edit, every
exec node run with its unit, runs and causes, and one cause label per exec
run. The engine reports one cause per node per evaluation (the last), so a
node run twice in one round has its earlier run labelled "speculative run,
run again" or "earlier run, run again".

Run: `PHITEX_SWEEP_N=5 cargo test --release -p phitex-tex --test latex sweep_amsldoc -- --ignored`.

### Exec runs per edit, by cause

| edit (text, unit, word) | exec runs / nodes | pdf:state | ps | new node | Text | park-resume | other |
|---|---|---|---|---|---|---|---|
| tex u177 `with` (page 4) | 48 / 47 | 29 | 13 | 3 | 1 | 1 | Range lines 1 |
| tex u283 `lines` (page 7) | 40 / 39 | 27 | 7 | 3 | 1 | 1 | Range lines 1 |
| tex u503 `These` (page 18) | 27 / 26 | 20 | 2 | 3 | 0 | 1 | Other 1 |
| tex u721 `structure` (page 30) | 24 / 24 | 14 | 7 | 3 | 0 | 0 | |
| tex u921 `answer` (page 38, last) | 2 / 2 | 0 | 1 | 0 | 1 | 0 | |
| toc u1 `Options` (via .tex) | 90 / 58 | 28 | 18 | 3 | 1 | 1 | spec-again 28, Range lines 2, cs:Hy@tempa 1, cs:@currentlabelname 1, cs:r@options 1, wfile:toc 1, Loop wdef r@options 1, Refuted pdf:state 1, earlier-again 3 |
| toc u66 `References` (page 38) | 30 / 30 | 0 | 29 | 0 | 1 | 0 | |

Every exec run is in round 1 (no exec runs in a second round on these
edits) and runs per exec node are 1.0-1.02 for the main-text edits: the
problem is how many nodes run, not runs per node.

### Grouped by cause field

| cause | 5 main-text edits | 5 .toc edits | what it is |
|---|---|---|---|
| Changed `pdf:state` | 90 (64%) | 116 (+4 refuted) | pieces with a read-time PDF writer command (`\pdfstartlink` etc., `pdfout::command` -> `step`) read the writer state key; the place nodes define it too, and the key is a hash of the whole writer state including the bytes of the pages placed so far: any page whose bytes change gives every later reader a new key, though the object counter and tables they use are equal. Readers from the edited page to the end of the document. |
| Changed `ps` | 30 (21%) | 56 | pieces after the edit on the same page that fold in their own run (`needs_own_fold`, `fin_touch`, or a cut) read `ps`; the page so far carries full line boxes, so every later version differs (diff at `ps/20/...`: the page contents) |
| new node (re-made by its parent) | 12 | 12 | 3 per edit: children of a resume that was itself re-made (the resume's identity carries its continuation and the cutting node's id) |
| speculative run, run again | 0 | 82 | read-back edits only: speculative runs refuted (the round's last cause is again pdf:state or ps) |
| Text | 3 | 5 | the edited node itself (u921/u177/u283; the .toc edits' edited paragraph) |
| park-resume | 3 | 3 | a parked run resumed |
| Range lines, Other | 3 | 9 | |
| regime cells (`cs:Hy@tempa`, `cs:@currentlabelname`, `cs:r@options`), `wfile:`/`Loop` | 0 | 15 | read-back edits: real value changes |

Equal-value reruns: none on exec (the engine cut off on equal values
everywhere the census looked; the one EQUAL seen is a finisher's
`cs:@deferlist`). `aftertoken`, `param:maxdepth`, `cs:if@newlist`,
`count:0` do not appear as exec causes.

Other operations per main-text edit: place 10-81 runs (`pdf:state`
Changed/Refuted: the same chain), resume 13-29 runs over 7-16 nodes (mostly
re-made, no cause recorded; `pdf:state` 1-2, `Range gdefs`/`streams`),
finish 5-9, fold 6-9 (`ps`).

### Reading

1. The dominant cause (two thirds of exec runs on main-text edits) is the
   writer-state chain: `pdf:state` names the whole writer state, bytes
   included, so a page changed by the edit changes the key read by every
   later piece with a read-time PDF command and by every later place.
   These pieces need only what pdfTeX's read-time commands read (the
   object counter, the object/dest/name tables), which a one-word edit
   does not change. This is in pdfout.rs (the edit-tail agent's file).
2. Second: pieces joining the page chain by folding in their own run
   (`needs_own_fold` from `fin_touch`, and cuts) read `ps` and re-run with
   every change of the page above them.
3. Third: resumes re-made (not re-run) by any change upstream, their
   children new nodes (3 exec runs per edit; resumes 7-16 nodes).

## 2. Fixes and numbers (accl, release, amsldoc, `PHITEX_SWEEP_N=5`)

| step | commit | exec runs, 5 main-text edits | cold rounds / runs | new resumes + new exec (5 edits) |
|---|---|---|---|---|
| baseline (census) | f5ca7e9e | 48/40/27/24/2 | 2 / ~2300 | 43 + 12 |
| locals counted only where they differ at the candidate | 4bb78f85 | 46/40/27/25/2 | - | |
| resume and fold positional identity (cherry-pick of profiles 4d1965e3) | 81df5e96 | 42/36/23/21/2 | 2 / 2306 | 0 + 0 |
| defines-side own fold dropped entirely (rejected) | 5655c2c6 | 32/32/22/17/2 | 3 / 3013 | |
| defined after a candidate counts only if its value differs from the one at the candidate | 8bb7a72a, a7cc35b4 | 39/36/23/21/2 -> 38/35/23/21/2 | 2 / 2348 | 0 + 0 |
| own-fold prediction only on fields more than half of a routine's runs read | 9817cbf4, b0a4491d | 35/34/23/21/2 | 2 / 2463 | 0 + 0 |

amsldoc `--exact-pdf` at j1 and j8 is byte-identical to the epoch oracle
after every step. The sweep's comparisons against cold builds show no
differences. The suites match the baseline: the edits reds
one_word_edit_reruns_little and pdf_edit_reruns_one_page_step are red at
6e710e39 too, and relaxed_page_edit, red at 6e710e39, is green.

What is left on the main-text edits (11884): `pdf:state` 92 of 115 exec
runs (edit-tail's split of the writer-state key) and `ps` 19. The
remaining `ps` readers fold in their own run on real name-and-value
dependencies: they define `if@newlist` or `parshape` after a candidate
with another value, write to a stream a routine writes, or read
`fontcount`/`count:0`, which routines define anew. Removing those reads
needs the scan design, where a piece does not read the page state to
learn whether a page ends inside it.

The read-back edits' "speculative run, run again" (82 runs) are all
`pdf:state` readers (98 Changed, 17 Refuted, nothing else). A .toc edit
changes two pages, so every reader after the second runs once on each
new key.
