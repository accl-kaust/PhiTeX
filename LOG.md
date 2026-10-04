# partex log

A dated record of what was changed, discovered, measured and decided,
kept as the project goes. DESIGN.md holds the current design; this file
holds how it got there. The two together are the raw material for a book
about the design.

Conventions:
- Entries are in time order, newest at the bottom.
- Each entry names its commits, and its branch if it is not on master
  yet. Every number comes with its conditions: the document, the binary,
  and a warm or cold cache.
- An entry says why as well as what: the measurement that prompted a
  change, the alternatives rejected.
- On a merge conflict in this file, keep both sides, in time order.

## Before this log (from the git history)

- **2026-09-24.** Scaffolding: plan, sandbox, upstream pins, corpus index
  (`0131e81`). IR design for tracked state, region memoization and
  compiled expansion (`b54910e`); every IR must have a canonical,
  round-tripping text form (`9425baa`).
- **2026-09-25.** Exact PDF backend: ship-out, Type 1 subsetting, virtual
  fonts, ToUnicode, JPEG, outlines, object streams. The PGF manual's 1135
  pages match pdfTeX byte for byte.
- **2026-09-26.**
  - The modern command line: `partex build/watch/check/why/trace/clean`
    (`527de71`).
  - BibTeX 0.99e and MakeIndex 2.18 run in process and between
    `-converge` passes, with identical outputs.
  - **partex-incr**, the language-independent incremental runtime, with
    a stub language and an oracle interpreter (`8275203`). On the stub,
    10k statements with 24 threads:
    - a one-line edit takes 0.44 ms (document-shaped) or 0.28 ms (cheap
      statements)
    - a full recorded build takes 58 ms, against 60 ms for a plain run
    - parallel rounds: 4.8× with speculation, 3.5× with suspension.
  - Then: tracking coverage and the sanitizer, warm rounds, flat
    recording (`479199a`); source lines as token values, with a fast path
    for invisible edits (`e6b2b20`); Send state, effects as values, the
    link step, and the engine as a partex-incr `Machine` (`9ee50b0`);
    watch soundness (`cbb6d84`); fast machine mode with a Merkle state
    hash and line cells (`60b6eec`); watch latency (`70fdb1a`).
- **2026-09-27, early.**
  - Watch latency round 2 (`1486fdb`).
  - Machine convergence (`988db7d`), machine-mode memory (`de7db3a`),
    session path round 3 (`aaf8546`).
  - `partex watch` defaults to machine mode (`c50dec2`).
  - Faster expansion: −22% instructions on the course (`6350740`).
  - Strings by content and PDF object table cells: adding a paragraph
    went from 29 s to 2.7 s (`14f0abd`).
  - The persistent content-addressed store (`d84f11d`).
  - Virtual PDF object ids and soft save-stack reads: adding a `\Cref`
    went from 27 s to 2.0 s, adding a `\label` from 25 s to 3.3 s
    (`fd18f82`).

## 2026-09-27

### Fast persistence merged (`3d8def3`, `37932d2`)

**What changed.** The store loads lazily and in parallel, and saves
incrementally in parts. The clock became a tracked read instead of part
of the job's identity. Blobs are compressed in-tree.

**Measured** on the course (295 pages), with every PDF byte-identical to
a cold build:
- rebuild with nothing changed: 4.7 → 0.94–1.0 s
- one-word edit after a restart: 5.1 → 1.7–1.9 s
- save after an edit: 5.2 → 1.05 s
- another day, for a document that prints `\today`: 54 s cold → 3.6 s
- store per job: ~578 → 248 MB.

**Decisions.**
- **The clock is a read, not identity.** The host serves the date like a
  file, and regions guard on it. `SOURCE_DATE_EPOCH` and
  `FORCE_SOURCE_DATE` leave the identity. The directory, command line,
  parameters and TeX environment stay in it, because they change name
  resolution everywhere.
- **Compression is written in safe Rust (`lz.rs`).** zstd was rejected:
  the crate links C through unsafe FFI, against the no-`unsafe` rule.
- **Persistence was timeboxed.** Restarts are rare compared with edits.
- **Process.** A second merge of the branch failed to compile against
  new machine cells; it was caught locally and never pushed. New rule:
  merge master and run the gate immediately before reporting ready.

### Why a restart with nothing changed takes 1 s (coordinator)

A copy of the course, master's binary, warm page cache: restarts took
1.02–1.08 s.

**Phases.**
- open 25 ms, host 60 ms, root 18 ms
- **the final state's decode, 662–694 ms, plus a 54 ms integrity check,
  on one thread**, in parallel with the regions (~200 ms) and the index
  (~320 ms)
- the change check, 18 ms
- linking and writing the outputs, only ~50 ms.

The first guess, that re-linking and re-compressing the PDF was the
cost, was wrong.

**Not I/O.** The thread was on the CPU for 764 ms of about 750 ms of wall
time, with 6 major page faults.
- perf: kernel syscalls 146 ms; malloc/free/memcpy 145 ms; decompression
  84 ms; decoder primitives 75 ms; TFM font data 69 ms; page faults
  54 ms; the font map 46 ms; the shared-value table 36 ms; hashing 32 ms.
- strace: 57,647 `pread`s for 83.6 MB. 44,106 were under 256 B and
  13,074 between 256 B and 4 KB; 64 reads of 64 KB or more carried
  64.8 MB. There were also 22,949 `mprotect` calls, from glibc arenas
  growing a page at a time.
- `pdftex.map` has 42,270 entries; the course uses 29 fonts. Each map
  entry is loaded as its own value.

**Conclusion.** The cost comes from the representation: many tiny
values, rebuilt one allocation at a time. The parser is not the problem.
A no-op restart should answer from input stamps and stored output hashes
without loading the final state (DESIGN §7.9, §7.15).

### Converge passes cost more than a cold pass (coordinator)

book.tex (392 pages, the course's sibling), a cold watch start without
aux files:
- pass 1: 54.7 s
- pass 2, after BibTeX: re-ran 57.8M of 58.4M commands in 93.8 s
- pass 3: re-ran 57.8M of 58.5M commands in 110.7 s.

Re-running nearly everything cost 1.7–2× a first pass. The explanation
came in the investigation below.

### Directions from the user

- **"partex-incr is plenty fast. The representation, dep tracking,
  regions and dynamic choices are lacking."** The slowness is the TeX
  adapter's.
- **No "if most is dirty, run cold" fallbacks.** A rebuild runs what is
  needed, at the cost of running it.
- **"A simple `\label` change should take milliseconds, just like
  partex-incr predicts."** The target is not matching cold.
- **"Why are you unable to partially execute a region? Is hierarchy too
  much?"** "Catcodes don't change when I edit a single word. Why do we
  have to rerun?"
- **"Your deps should be tracked dynamically. This is beyond well known:
  LISP, Julia, Python."**
- **"Why do we still use aux? Can't it be in memory, in our own IR?"**
- **Process.** At most 2 agents at a time; bite-sized tasks, each ending
  in a merge; regular progress emails; DESIGN.md kept current, and this
  log.

### Investigation: why TeX on partex-incr is slow (coordinator)

The course (66.06M commands; cold build 53–55 s in 282 regions at grain
131,072), master's binary, in-process rebuilds with per-span statistics,
the cells that made each region dirty, and perf over the cold and
rebuild windows of one process. Full tables and the design that follows
are in DESIGN §7.15.

**A one-word edit takes 0.37 s.** One span re-ran 149,020 commands in
297 ms, 2.0 µs per command against 0.83 µs cold. With
`PARTEX_MACHINE_FINE_GRAIN=131072` the same span took 159 ms (1 cut
instead of 29), so **one cut costs about 5 ms**.

**A full `\linespread` rebuild takes 79.7 s**, against 55.5 s cold. At
the coarse re-recording grain it takes 56.1 s, running at cold speed. The
extra 24 s in the profile is all per cut, 5.9 ms each:
- state hashing +9.7 s
- B-tree indexes keyed by cell +4.4 s
- allocation +2.8 s
- freezing the token store +2.5 s
- page faults +1.8 s
- cut bookkeeping +1.3 s
- clones +0.9 s.
TeX's own execution took 0.9 s more.

**A `\Cref` takes 0.89 s.** 360k of its 524k commands re-run in a region
50 regions later. It reads `Numbering`, the hash of the whole PDF
numbering log, only to write `N 0 R` into PDF bytes.

**A `\label` takes 1.3–1.5 s.** 697k commands re-run over six coarse
regions:
- the edited line;
- `\@savsf`, a scratch register, twice;
- `Rest`, twice;
- the `.aux` read back as one cell.
Fixed costs on top: prefix replay 98–144 ms, splice 140 ms, link
47–64 ms.

**Holes are ruled out.** Rebuilds are sequential and plant none.

**Four causes:** atomic regions; cuts that cost O(state); lump cells;
identities by creation order.

**Target:** nested memoized units with dynamic read sets over fine
cells, red-green revalidation with early cutoff at every level,
O(changes) checkpoints, and resuming and stopping inside regions.
`\write` streams become entry logs, and pdfTeX's numbers become holes
resolved at the link. Stages separate (paragraph, page builder, shipout),
with token-consumer tracking, so that an edit confined to one paragraph
re-expands nothing.

### Status of work on branches (14:30; their own entries follow on merge)

- **`persist` (Agent B).** Merged; its entries follow below.
- **`machine-converge` (Agent C).** Merged; its entries follow below.

### Persistence, watch cancellation and no-op restarts (Agent B, branch `persist`)

Measured on a copy of the user's course (299 pages, pdflatex, `-o _out`,
`SOURCE_DATE_EPOCH` fixed so that PDFs compare byte for byte), on the
branch's release binary, warm page cache; "cold" is a partex build of the
same text with the store off.

**The final state loads while the rebuild runs (e31aeae).** A restart
plus an edit spent most of its time decoding the saved build's final
state (0.66-0.73 s on the 295-page course), although a rebuild needs it
only at its end, for the read-set cutoff. The final state now loads on
its own thread and the rebuild takes it when it gets there; the store
also keeps its reference lists in memory between a session's saves
instead of reading them back. Restart plus a one-word edit 1.70 s ->
1.52-1.60 s; a save after an edit 1.05 s -> 0.64-0.70 s.

**Accumulators compared against stale versions (caa0b50).** New property
tests found that a rebuild compared an accumulating cell (glyphs used,
`\write` files, PDF numbering) with the version an old region recorded,
which is the whole value of *its* run, not of the run it is now part of.
Such a comparison now counts as unknown. The stub machine gained an
accumulator so the tests exercise this.

**Watch cancellation (e66c743, a6295c7).** Before this, a watch rebuild ran
to its end before it looked at newer edits, so a user typing into a
document whose rebuild takes 30 s waited for every stale rebuild.
`Build::rebuild_or_stop` now stops at the next region boundary when a
newer edit arrives, once at least one span has re-run (so a stream of
edits still makes progress). What re-ran is kept, and the build records a
*frontier*: the first region not reached, plus the cells that may differ
there between the new prefix and the regions recorded after the old run.
The next rebuild goes on from the frontier with the newer edit. The subtle
part is that the regions before the frontier no longer hold the old run's
values, so a frontier cell is compared with the old run only once a
region after the frontier has written it. Outputs and saves wait for a
rebuild that ran to its end. Measured on the course, from the last
keystroke to a PDF byte-identical to a cold build: 8 keystrokes 150-300 ms
apart cost the same either way (0.19 s, and 33 s for an edit that moves
every later page), because most edits rebuild faster than one types and a
converging watch already folds the edits made meanwhile into its next
pass. It pays when typing lasts longer than the rebuilds: 36 keystrokes
into a ch12 sentence, 2.7 s with cancellation against 60.6 s without.

Two lessons came out of measuring rather than assuming. The first version
statted all 880 files the job read, the TeX tree's included, every 20 ms,
and that made an isolated 31 s rebuild 2-4 s slower; now it looks at the
job's own files every 50 ms. The second: coarsening (merging fine regions
back to the grain while the watch waits) counted how recent a region was
in rebuilds, stopped ones included. A 30-stop burst therefore aged the
regions it had just re-run, coarsening merged them, and the next rebuild
of the course produced a wrong PDF and then panicked on an empty token
list. Recency now counts in rebuilds that ran to their end. The merge
itself is not faithful either (bug B below), but it predates cancellation.

**A file read back came back from disk (2c92cf7).** Testing stops on real
LaTeX documents (`PARTEX_MACHINE_EDIT=a;;b;;c`, `PARTEX_MACHINE_STOP`,
`PARTEX_MACHINE_SANITIZE`) turned up a bug that is on master too. LaTeX
reads its `.aux` from disk at `\begin{document}` and reads it back at
`\end{document}`, writing the `.toc` from it. When a rebuild replayed a
region in the middle of that read-back, the restore swapped the file
being read for the file of the same name as served from disk, so the
`.toc` got old page numbers and the "Label(s) may have changed" warning
disappeared. The restore now keeps a read-back file as the snapshot had
it, as the machine already counts its lines. A new e2e case,
`machine_edits`, makes four in-memory edits to a 40-section document
(with and without stops) and compares every file with pdfTeX; it fails
without the fix.

**No-op restarts (fc3dd78, 4acbb90).** The user found a 1 s restart with
nothing changed unacceptable. Nearly all of it was decoding the saved
final state, just to find out that nothing needed rebuilding. Now each
settled build leaves a small record beside the store, holding every file
it read (by stamp and content hash), the files it looked for and did not
find, the clock if it read it, each output as written, and the result as
reported. A restart checks the files first. Stamps are git-style: a file
whose times are within 2 s of the record could still change within the
same clock tick, so it is hashed instead. If nothing changed, the recorded
result is the result: no load, link or write. A watch loads the build in
the background for its first edit. On the course, `partex build` with
nothing changed went from 0.92-0.96 s to 0.07-0.08 s, and `partex watch`
from 0.91-0.97 s to 0.06-0.08 s to the result. The check looks at 883 files
in 8-12 ms. A restart after an edit is unchanged (1.53 s).
`PARTEX_STORE_QUICK=0` turns it off.

**Open, found along the way.**
- *Bug B:* a region merged by coarsening does not always replay to its
  constituents' last snapshot. A box register's content hash at the cut
  (1327…) differs from the hash of the same value in the region's own
  snapshot (2773…), already right after it is recorded. So a value is
  hashed differently in a snapshot than in the running engine: either the
  snapshot does not capture the box as the engine has it, or the hash
  depends on more than the box. A sequential replay tolerates this; a
  merged region replayed alone does not. This is next.
- *Bug C (DVI mode only):* an edit in section 5 of a 47-page document,
  then one in section 2, ships section 2's page with the old line.
- A paragraph in ch12 whose any edit adds a line moves every later page:
  27.7 of 66 million commands re-run (31 s), on master too. That is the
  document's own dependency structure, not a bug, but it is where
  cancellation matters.

### A footnote added: what a first use moves, and making it converge (Agent C, `machine-converge`)

All times on the course (the copy of 27 September, 295 pages, 66.06M
commands, 282 cold regions at grain 131,072), release binary, in-process
rebuilds (`PARTEX_MACHINE_EDIT`), warm from the cold build of the same
process; every output compared with a plain run of the edited files
(PDF, aux, out, toc, log with its memory statistics masked) and
identical unless said otherwise.

- **Destination names as one accumulating cell** (`4608066`). They were a
  cell per place, and `Rest` kept their count, so a destination made
  earlier in one run left every later `Rest` different. Now
  `MCell::Dests`: a region's value is the names it added, read at the
  end. Alone this changed nothing measurable for the footnote (27.2 s →
  29.8 s, noise): the probe showed the hash table, strings and fonts still
  differing.
- **A bug in names placed by name** (`25b4f2f`). `c15a9fc` placed
  `hash_extra` names where their characters pick. A format made in
  machine mode then had names spread over `hash_extra`, and a run loading
  it that places names as §260 does put the next one at `hash_high + 1`,
  over an existing name (`\sys_if_shell_restricted_p: already defined` in
  the converge e2e cases with `PARTEX_MACHINE=1`; the gate does not run
  e2e in machine mode, which is how it got in). INITEX now places names
  as §260 does.
- **Names are cells, and the string pool leaves `Rest`** (`601f194`).
  pdfcol makes `pdfcolfoot@current` at the first footnote; the old run
  never does, so the hash table and the strings differ for good. A
  control sequence's name is now part of its eqtb cell's value, a hash
  slot's link is a cell, and a lookup that misses a name (or enters it)
  reads where the name is. Why the pool can go: every reference to a
  string the run made is already hashed by its characters, so which
  strings exist, and their order, is observable only through those
  references. Side effect: hashing every string of the run was a fixed
  cost of every cut; a rebuild that re-runs everything went 80.8 s →
  75.7 s.
- **Fonts: numbered by load order, placed by identity** (`daea98e`,
  `c12bdac`). The footnote's paragraph needs two auto-expanded fonts
  (microtype's font expansion makes an instance per ratio when a
  paragraph first needs it) that the old run never made. tex.web numbers
  fonts `font_ptr + 1`, so ~190 later fonts were numbered +2, and with
  them the font identifiers, every character node, the PDF writer's
  per-font state. Two notions of a font's number were one: where it sits
  (a slot) and what pdfTeX calls it (`/F12`, DVI's number, the load
  order). They are now apart; a machine's host gives slots from a
  registry by identity (the TFM file's contents with name, area and
  size; an expanded font's base and ratio), shared by every run of a
  build and saved with it, so a font keeps its slot however many fonts
  came before it; in a cold build every slot is tex.web's number.
  pdfTeX's elink chain of expanded fonts became a map by base and ratio,
  which is what the chain is searched for.
- **Fonts are cells; `/F` names and stream compression go to the link**
  (`036d7af`). Each slot is a cell (identity, parameters, codes, what
  the PDF writer keeps of it); the fonts by name, the expanded fonts and
  the load order are cells, the last accumulating. A font's metrics
  follow from its identity, so reads of them need no cell (that is why the
  TFM file's contents are part of the identity). The catch: pdfTeX prints
  `/F` names inside compressed content streams, so every page after the
  footnote observed the load order. So `/F` names became relocations and
  the link compresses every stream `pdf_begin_stream` begins (memoized by
  content in the host), writing the numbers in first. A slot not loaded
  must compare equal however much of the arrays reach it: before that
  fix the probe showed 200 "differing" empty slots and the footnote still
  took 20 s. Result: **footnote 27.2 s → 3.2 s** (11 of 282 regions:
  the paragraph, readers of a footnote counter above 255 and of
  `\@gtempa`, both different for good, and the end; 1.84M commands), its
  removal 0.88 s, `\Cref` 0.81 s, `\label` 1.34 s, `\linespread` 77.7 s.
  Affine holes were not needed: the counter's readers are few.
- **A soundness bug: words compared by bits** (`7a9f120`). With region
  boundaries moved to cheap points (below), a `\linespread` rebuild gave
  a different PDF. The probe (`PARTEX_PROBE_CS=baselinestretch`, added in
  `e322703`) showed both runs' `\baselinestretch` naming token list 21663,
  holding `1.04` in one store and `1.05` in the other.
  `Tex::eqtb_differences`, which setting `Rest` uses to keep this state's
  eqtb words, took equal words for equal cells, so the restored state kept
  the old run's list. Not a missing dependency: the tracker read it, and
  the rebuild patched it into the wrong store. Words naming objects are
  now compared by content where the two states' chunks are not shared.
  This was latent on master too, waiting for two runs to give one list
  number to different contents at a boundary.
- **Grain growth, and cheap boundaries** (`e322703`; the cheap boundaries
  parked on the local branch `cheap-cuts-wip`, `13e4a16`). A cut costs
  about 5 ms of O(state) work (hashing, `MCell`-keyed indexes, the token
  store's freeze), and a rebuild re-recorded every re-run at 1,024
  commands: 4,401 regions for a `\linespread` edit. `Config::grow` (off by
  default) doubles the grain after each cut along a re-execution:
  `\linespread` 84.1 → 67.5 s, a one-word edit 0.51 → 0.37 s, the next
  edit there 74 → 78 ms. Boundaries only where the state is small (outside
  every group, between paragraphs) remove the `\label`'s `\@savsf` chain
  (0.99 → 0.46 s at grow 2: no group is open at a cut, so the saved
  scratch register is no read across it) but lengthen a `\Cref`'s re-run
  (0.82 → 1.23 s), and did not make cuts cheaper. The lesson for nested
  units (DESIGN §7.15): checkpoints must be both cheap and fine, and
  placed where no group is open.
- **Store layout**: the persisted machine build is `partex machine
  build/6` (`MCell`, `V`, `CellValue`, `FontArrays`, `PdfOut`, effects
  all changed); `partex-store/5` unchanged.

### Latency targets set (coordinator, prompted by the user)

The user asked for the target compile time for a single space, a
`\label`, a `\footnote` on page 1 of a 100,000-page book, and a deleted
paragraph.

**The rule.** Latency follows the size of the change in the output (the
pages whose bytes change), never the size of the document; bookkeeping
is O(change).

**The targets** (DESIGN §7.15, "Latency targets"):
- an invisible edit: < 1 ms
- a word or space inside a paragraph: < 5 ms
- a `\label`: < 2 ms
- a footnote added or a paragraph deleted: about 1 ms per changed page,
  typically one chapter, 20–50 ms
- a renumbering of 100k later pages: 2–4 s on 24 cores, with the page in
  view in < 20 ms
- a restart with nothing changed: < 50 ms.

**Where today stands.** Measured costs extrapolate to about 70 s for a
one-word edit near the end of a 100k-page book: prefix replay is about
0.5 ms per page, and the link about 0.2 ms per page.

### Exactness without compression; output modes (coordinator, prompted by the user)

**The user's decision.** "We don't have to be byte-exact unless the user
wants. So long as the visible content is 1:1, who cares about
compression? We can have both modes."

**How to measure it.** The user's suggestion: turn compression off in
pdfTeX itself, with `\pdfcompresslevel=0` and `\pdfobjcompresslevel=0`,
and compare the bytes. `scripts/pdfcheck uncompressed DOC.tex` does this
for both engines, with the same output path and a fixed
`SOURCE_DATE_EPOCH`; the `/ID` depends on the output path, so the paths
must match. With master's binary:
- the e2e LaTeX document: byte-identical, 171,207 bytes
- the 299-page course: byte-identical, 10,079,539 bytes.

**Checking fast mode.** For a mode that compresses, packs or numbers
objects freely, `scripts/pdfcheck same A B` compares content. It uses
qpdf's QDF form: streams decompressed, object streams unpacked, keys
sorted, objects renumbered in traversal order, `/ID` ignored.
- Verified: the course re-encoded at level 1 without object streams
  (4.17 MB) and at level 9 with them (3.16 MB) compares equal to the
  original.
- pdfTeX's compressed and uncompressed outputs compare equal.
- A one-byte change in a content stream is reported.

**The design** (DESIGN §7.15, "Output modes").
- **exact**: byte-identical, the oracle test, and the default of the
  `pdflatex`-compatible command line.
- **fast**: the default of `partex watch` and `partex build`.
- What fast mode buys:
  - deflate off the critical path;
  - page streams spliced from blocks at deflate full-flush boundaries;
  - object ids stable across edits;
  - incremental PDF updates.
- AGENTS.md records the contract. Exact stays byte-for-byte; fast is
  checked by `same` against the same oracle.

**Regions** (the user: "regions should be hierarchical and dynamically
chosen, not every chapter or every page; we have our own page IR").
§7.15 now says:
- the tree of candidate extents is observed during execution;
- a policy picks units and checkpoints from measured cost and observed
  volatility, splitting and merging them;
- the page IR is built from functional blocks with stable identities,
  so a renumbered folio is one footer block.

### Bug B: a merged region replayed a box register as it was (Agent B)

The symptom: after coarsening, replaying a rebuild's regions one by one no
longer gave the final state (the sanitizer, after three edits of
`edits.tex`). Bursts of edits made it worse: stopped rebuilds aged fresh
regions and coarsening merged them (see above); a later rebuild of the
course then gave a wrong PDF and a panic. The difference was one box
register, `\box53`, by content hash.

Following one register through the regions found the cause. A region
that read box 53 left it with different content, the same eqtb word (box
id 3) and no write reported. The operation was `\wd`/`\ht`/`\dp`
(§1247): tex.web writes `mem[box(b)+c].sc` directly, and partex changes
the box in its store in place under the same id. A sequential replay
hides this, because each region's `Rest` snapshot holds the box as it
changed. A merged region, though, carries the write of the earlier
region that set the box and applies it after `Rest`, putting the old
dimensions back. Setting a box dimension now reports a write of the
register. A unit test fails without the fix. After it, three and four
edits with the sanitizer on pass on the branch as it stood (329b7cc
contents), and 30 stopped rebuilds plus a take-back give a PDF identical
to cold.

A first reading went wrong in an instructive way: a trace records
`version_of(Some(value))`, a hash of the value's own content hash, so the
recorded 1327… and the snapshot's 2773… looked like a mismatch but were
the same value. The real clue was a register whose content changed
without any write.

Why the sanitizer missed it: its shadow diff compares eqtb words, not
the boxes, glue and shapes they name. Comparing those by content is
still to do.

Found while checking the fix on master a6df975, and left open: with
fonts as cells, replaying the first rebuild's regions from the start no
longer reproduces the final state's `Glyphs` (the glyphs-used
accumulator), with or without this fix; the outputs are still identical.

### Dense integer ids for the rebuild's indexes (Agent B)

Agent C's profile of a `\linespread` rebuild (every region re-run, 4,399
cuts) put 4.4 s into B-tree indexes keyed by cell. We measured it on
fb8abf5, on the course (one process: a cold build, a one-word edit in
ch15, a second edit nearby, then `\linespread` in the preamble). The
splice, the step that puts a rebuild's new regions into the build and
its indexes, cost 1.94 ms per cut. That is 8.5 s of the 85.8 s rebuild.

There were two causes. First, each of a region's ~860 guards was an entry
in a `BTreeMap<MCell, …>`: a dozen cell comparisons, and a `Line` cell
compares its file's path bytes. Second, each cell's keys are a sorted
vector, and a new region lands in the middle of it. A cell that every
region reads (`Rest`, the font parameters, common registers) therefore
shifted thousands of keys once per cut.

The indexes now hold each cell's keys by a small id (`CellIndex`). The
machine already numbers most cells densely for flat recording
(`Machine::index`), but up to about 6.5 million (registers above 255, hash
links, font slots). So the numbers go through pages of 1,024 allocated on
first use, not one flat vector. The cells without a number (lines, files,
PDF objects) are interned in an open-addressing table by hash; partex-incr
is `no_std`, without a hash map. That alone gave 0.81 ms per cut.

A splice now also stages its changes as (id, key, added) and applies them
at the end, grouped by id with a counting sort: one merge per cell
touched. Each cell's cost is then its keys plus the changes, once per
rebuild. The splice fell to 0.21 ms per cut, and the rebuild to 71.6 s.
A one-word edit's splice went from 52.8 to 19.2 ms, now mostly the
removal of the one coarse region it replaces (tens of thousands of
cells). The loaded build's indexes take 0.35 s instead of 0.61 s: the
keys come in order and are appended, so no sort is needed.

All outputs are byte-identical to a classic cold build of the same text.
Checks: the property tests with the sanitizer; a unit test of the index
against a B-tree reference, with random adds, removals, clears and staged
batches, over cells both numbered and not; and the gate. Cells stay the
keys of traces and of the store; ids are private to a build. Machine.rs
is untouched.

One measurement lesson: pdfTeX's trailer `/ID` hashes the output file's
path, so comparing runs byte for byte needs the same output path, not
only the same `SOURCE_DATE_EPOCH`.

### What a region cut costs now, and two pieces made O(change) (Agent C, `f1fd7f5`, `34346a3`)

The coordinator's profile on master before names left `Rest` put state
hashing first among a cut's costs (9.7 s of the 24 s a `\linespread`
rebuild spends on 4,393 cuts). Measured again on the merged binary
(a6df975; the course, `\linespread` changed so every region re-runs,
4,399 cuts, perf with frame pointers over the rebuild window): the
`Rest` hash is 0.36 ms of a cut. Hashing every string the run made was
most of it, and that went with names as cells. What a cut costs now,
about 7 ms besides TeX's own work: the snapshot 2.9 ms (the freeze 1.3,
the engine's clone 1.2, the hash 0.36), the guards' versions 0.7 ms, the
splice's index 2.1 ms (B's dense ids have since taken it to 0.21 ms),
and dropping replaced boxes and sorting the trace's cells the rest.

Two O(state) pieces of the snapshot became O(change) (`f1fd7f5`): the
virtual object table (about 5,000 entries, cloned whole at every cut)
now sits in 64 shared shards, and a `Flat` (string pool, input buffer,
input and parameter stacks) is no longer compared with its shadow a
second time when cloned right after its commit. Counted in instructions
(perf stat, user; the wall clock was too noisy on a shared machine for a
1% effect): the `\linespread` rebuild 364.9 G → 360.4 G, a one-word
edit's rebuild 1.77 G → 1.71 G, the cold build 302.7 G → 303.0 G, outputs
identical. Dropped after measuring: marking the token store's changed
chunks on the token path so the freeze visits only those. The test runs
at every reference-count change, that is at every macro call, and cost
the cold build 3.7 G (1.2%), more than the scans it saved. The lesson,
recorded in DESIGN §7.15: a checkpoint costs O(changes) only if changes
are recorded where they are cheap to record, which for token lists means
keeping reference counts apart from contents.

Also: the gate now runs the e2e cases in machine mode too (`34346a3`),
after names placed by name in a format got through it. The sanitizer
fails on the course's first rebuild ("region 1's guard fails on replay:
Rest") on the baseline binary as on the new one: that is bug D, next.

### Prefix replay is not O(position) (Agent B)

The premise to test was that a rebuild replays every clean region before
the first dirty one, one at a time, so that its cost grows with the
edit's position: about 0.5 ms per page, or 48 s on a 100,000-page book.
The measurements say otherwise. A run of clean regions is already
replayed as one restore of the last region's exit snapshot, patched with
the cells that differ (`replay_span`, `Machine::replay_exit`).

On the course, one-word edits in ch01, ch15 and ch28, in one process
after a cold build:
- `replay_ns` was 58, 59 and 49 ms;
- each rebuild made 2–3 separate replays;
- a temporary timer put 12–23 ms in each restore, whether it covered 3
  or 349 regions.

So the cost tracks the number of restores, not the number of regions. A
perf profile of the rebuilds puts the restore in O(state) work: thawing
the token store from the snapshot, hashing the entry state, cloning the
font tables.

To see position on its own, a generated 1,287-page article (1,000
sections) took one-word edits at sections 10, 500 and 990. `replay_ns`
was 392 ms (12 separate replays where dirty and clean regions
alternated), 38 ms and 17 ms. The last replayed 617 regions in one span,
so the edit nearest the end was the cheapest to replay.

What does scale with a span is re-adding each region's share of the
accumulating cells (glyphs, numbering, `\write` files, font order,
destinations). We built a variant that took them from the snapshot when
none had changed since it was recorded. It gave 16.8 against 17.3 ms on
the 617-region span, and identical outputs. It was dropped rather than
add a trait parameter and a change to machine.rs for nothing measurable.

Kept (build.rs only): the exit patch now finds a changed cell's last
write in the span through the writers index. Before, it searched the
span's traces once per changed cell, O(changed cells × span), up to
0.9 ms per span on the course. Cells that differed accumulate over a
session, so that search would grow with both the session and the
document.

The next lever for rebuild latency is therefore the restore itself: a
snapshot restore that costs O(changes), which is the checkpoint work in
DESIGN §7.15. Outputs, on the course and on the article: byte-identical
to a classic pass of the final text.

### The link, measured; the last link taken again (Agent B)

After every rebuild a watch relinks the whole document. On the course,
over a one-word edit, a `\label` and a `\Cref` (each then taken back),
the link took 25–43 ms, or 58 ms for the `\Cref`, plus 1.5–3 ms to write
the files. This was on the watch's path, which already memoizes deflate
by content. Timers inside `effects::link` split it:
- Resolving virtual object numbers, 20–27 ms: a pass over every effect
  that clones each one and assembles 10.1 MB of stream data across 407
  streams, with 3 ms of that in hashing the streams for the memo.
- Object streams and the xref, 2–8 ms (16.5 ms when the `\Cref`'s new
  objects missed the memo).
- Copying into the files, about 2 ms.

The first step asked for was to skip the link when no region's effects
changed. A splice now compares the effects it takes out with those it
puts in, in program order, and `Build::take_effects_changed` tells the
watch. When they are equal, the watch writes its last link's output
again, and only the `\write` files are compared with the disk. This is
exact by construction, since `effects::link` reads nothing but the
effects. It costs what was replaced, and coarsening never trips it:
merged regions carry the concatenation of their effects. A property
test checks that "unchanged" always means the same effect sequence,
through stops and coarsening, and it saw more than 100 unchanged links.
The switch is `PARTEX_MACHINE_LINK_REUSE=0`.

It hit once in the four links a `\label` makes (26 ms saved). Printing
the first differing effect showed why the other three missed:
- The log changes while the PDF does not: the first pass adds "Label(s)
  may have changed", and the second drops it.
- Taking the edit back re-runs regions whose objects get other virtual
  ids (`Num(Create(750611959))` against `Num(Create(63039149))`), which
  resolve to the same pdfTeX numbers.

Neither changes a byte of the PDF, but element-by-element comparison
cannot see that. The next step is a TeX-specific equivalence: per file,
and up to a consistent renaming of virtual ids. The step after that
keeps the previous link's resolved per-region bytes, so resolving costs
only the changed regions. All outputs, on 12 PDFs with reuse on and off,
were byte-identical to classic cold builds of each state.

### Bug D: three ways a replay missed the final state (Agent C, `84b0bf3`)

The machine sanitizer replays a build's regions in order from its
starting state and compares the digest with the final state's. It
failed on the course ("region 1's guard fails on replay: Rest") and on
B's stoptest sequences, with outputs still byte-identical: a rebuild
runs from recorded snapshots, and only the replay composes the regions'
writes, so a write the tracker lost, or a version that depends on
something that is not state, shows only there. To find them, the digest
is now kept in named parts, and `PARTEX_MACHINE_REPLAYCHECK=1` prints the
first region whose guards fail on replay with what in `Rest`, the input
stack and eqtb differs from the state the region really began in; `=2`
prints the first region after which the replayed eqtb differs from the
region's own exit.

- **A location taken for a string.** On stoptest, the replay's `Rest`
  differed in the input stack. A token-list record's `name` is the control
  sequence the macro was called by (§390), and `Rest` hashed it as a string
  number: by its characters if it fell among the run's strings, else as a
  number. Setting `Rest` from cells imports control-sequence names by their
  characters as new strings (names as cells), so the replayed state had
  tens of thousands of strings more, and location 48961 fell among them
  there and past them in the real state. Token lists' names are hashed as
  locations now, in `Rest` and in a boundary's position.
- **Sizes that are not state.** The glyphs used were hashed with the
  length of the PDF writer's per-font table, which a state set from font
  cells extends to every font it is given. `font_ptr` after setting
  `FontOrder` counted the null font (not in the digest, but it would place
  a font wrongly with the registry off). And loading a format did not
  report the fonts by name as written, so a region that begins with it,
  or a merged region containing it, guarded `FontName(cmex10)` at its
  empty value.
- **A restore that is a change.** On the course, box 73 (LaTeX's
  `\@outputbox`) went void across region 35, a ship region, with no write
  recorded. Soft reads (`69b536b`) take a group's end that restores the
  value saved at the region's entry as no change. But after a global
  assignment in the same group, a later local assignment saves the
  location again (§279), and the group's end restores that global value
  first; the entry's value never comes back. A second save at the same
  level now makes the location written. This one predates fonts as cells
  and names as cells; the replay failed earlier before, which hid it.

Checks: a unit test for each (each fails without its fix); B's stoptest
sequences E1 E2 E3, E1 E4 E5 E2, E5 E4 and E2, and the course (cold, a
one-word edit, then a `\label`), all sanitizer-clean with outputs
identical to plain runs. Still open: under the sanitizer the
`modern_watch` e2e case fails "the rebuild's output differs from a fresh
build's", also with fonts as cells off; its outputs are right without the
sanitizer. The persisted machine build's layout is `/7` (the digest
changed).

### Why a `\label` takes 5 s in watch mode; virtual ids at the job's end (Agent B)

A `\label` added in ch15 of the course takes 4.4–6.9 s end to end in
`partex watch`, over two passes. We broke it down with a watch-side dump
of each pass's dirty regions and the cells that made them dirty (on
88b1952).

Pass 1 re-runs the edited line's region and a chain of regions dirty
only through `Rest`: 2.4 s.

Pass 2 runs because the `.aux` changed. It takes 4.3 s, for two reasons.

First, the region that reads the `.aux` at `\begin{document}` is dirty
through 852 `Line` cells. The new `\newlabel` line is inserted in the
middle, and line cells are keyed by line number, so every later line
"changed": 132k commands, 1.07 s.

Second, three distant regions are dirty only through one hash link,
`Link(433598)`. That is the slot the new name `\r@partex:test` was
chained into, and lookups of other names probe through it: 1.8 s, plus
one more region through `Rest` after them.

The step of reading and executing the new entry itself is tens of
commands. The target (< 2 ms) needs both causes gone, and DESIGN §7.15
now gives the two changes:
- name lookups guard on the name's own cell, not on the probe path (C's
  area);
- lines of job-written files are keyed by entry content and occurrence,
  with the line counter kept out of `Rest` and a region boundary per
  entry during the read-back.

The unstable virtual ids came from the same `.aux`. We printed every
allocation (seed, count, id) and every position that made objects, for a
cold run, the `\label` added, and the `\label` removed. Only one step
differed: the job's end. It starts while LaTeX reads the `.aux` back at
`\end{document}`, so its seed hashed that file's line (one more after a
`\label`) and the string number of its name. All 407 objects the end
makes (fonts, the page tree, the catalog, destinations) got new ids,
the numbering log changed, and so did every effect naming them. In
machine mode, `finish_pdf_file` now seeds them with a constant, so they
are named by their order. With the `\label`, 0 of 543 ids now differ
from the cold run's, against 407 before. Taking the edit back also
gives the same ids.

### The build sanitizer compared statistics partex does not reproduce (Agent B)

e2e `modern_watch` failed under `PARTEX_MACHINE=1
PARTEX_MACHINE_SANITIZE=1` with "the rebuild's output differs from a
fresh build's", though its files were byte-identical to pdfTeX's without
the sanitizer. We dumped both outputs and diffed them. They differed in
one line of TeX's memory statistics at the end of the log: 29604
strings in the rebuild, 453 in the fresh build. Masked as the oracle
comparisons mask them (`xtask/src/mask.rs`), they were equal.

So this was not a watch bug. A rebuild's string pool counts differ from
a fresh run's; partex deliberately does not reproduce them (AGENTS.md),
and the digest leaves them out. The sanitizer, though, compared the raw
output.

The runtime now asks the machine what to compare
(`Machine::comparable_output`, by default all of it). TeX masks the
digits of exactly the statistics lines, matched whole against mask.rs's
templates with digit runs as wildcards
(`partex_core::statistics_line`), so that a sentence with "3 out of 5"
in a PDF, or pdfTeX's real object counts, are still compared. The new
e2e case `modern_watch_sanitized` runs the watch session with the
sanitizer on, and it fails without the mask. Files touched:
partex-incr (the hook, the sanitizer), partex-core sanitize.rs and
lib.rs (the matcher), machine.rs (the three-line hook), xtask e2e.rs.

### Lines keyed by entry: a closer look before building it (Agent B)

Change 2 for a `\label`'s second pass, the lines of a job-written file
keyed by entry (DESIGN §7.15), was looked at before building it. As
first stated it is not sound. A region that read the `.aux`'s entries
depends on their order, so guards would have to be "the entry after E
is F", with the reader's position as its last entry. That position is
in `Rest`, in the boundary identity and in how a restore reopens a file
at its line.

The line counter would then become an additive cell: a reused region
would otherwise leave the old run's line number, one off. Every read of
it would need tracking: `\inputlineno`, the `l.N` of error contexts, box
warnings and more, 39 places in 20 engine files.

And the first second pass after a cold build would gain nothing. The
`.aux` read at `\begin{document}` lies inside one coarse region of 131k
commands, with no boundary to resynchronise at. The gain needs a
refined region, or a boundary per entry in the cold build (1,735 more
cuts on the course).

It touches machine.rs throughout, where the name-lookup work is going
on, plus the state hash, the store layout and the engine's line counter.
That is days of work, not an hour.

Measured meanwhile: growing the re-recording grain
(`PARTEX_MACHINE_GROW=2`) takes the `\label` from 4.37 to 3.97 s and its
removal from 3.93 to 2.57 s, with PDFs byte-identical to cold.

### Name lookups read where the name is, not its chain (Agent C, `86deeeb`)

B's breakdown of a `\label` under `partex watch` put 2.3 s of its second
pass on `Link(433598)`: the last slot of the hash chain that the new
`\r@partex:test` had been appended to. tex.web finds a name by walking
the chain of its hash code (§259) and enters a new one at the chain's
end (§260), so every name entered later in that chain read the changed
link. With names as cells and names placed by their characters, the
chain itself is the only thing that still depended on the order names
were made in.

Machine mode now places the names a run makes by probing, and finds
them the same way, without chains (`Tex::probe_names`,
`PARTEX_MACHINE_PROBENAMES=0` off). A name goes to its hash code's slot
if that is free, else to the first free place of its probes in
`hash_extra`. Since names are never removed, a name is found there or
before the first free place, and a missing name is missing there. So a
lookup reads the names at its start slot and its probes, up to the name
or a free slot: a name made elsewhere, even with the same hash code,
changes no other lookup, and a lookup that found a name missing has read
the very slot the name is made in later. The format's names stay in
their chains, which the run no longer changes. The location of a name is
not something TeX can observe, so this is invisible to documents; where
names would overflow differs, which is a capacity limit.

One slip on the way: the first version collected the probe sequence
(hash_extra + 64 places, 600,064) into a vector at every lookup that
missed the name cache, which made a cold build of the course take
minutes. The e2e documents are too small to show it, and so is B's
stoptest; the course in machine mode did.

Measured on the course under `partex watch` (a fresh store, the `\label`
added in ch15 and taken back): the label 4.47 s → 3.17 s, its second pass
2.04 s (133 regions) → 0.85 s (51 regions); taken back 3.98 s → 3.42 s.
The PDFs are the same with and without, and the label's is
byte-identical to a plain two-pass run of the edited text. Sanitizer
clean on the course and on B's stoptest sequences; the unit test
`a_lookup_reads_where_its_name_would_be_not_the_chain` fails with
chained lookups. Measured before switching, and parked: a snapshot
restore's share of a one-word rebuild on the course is 28–34 ms of about
100 ms (replay_ns over 12 toggling edits), the next thing to make
O(changes).

### Bug C: a DVI page shipped from a buffer the rebuild did not compare (Agent B)

In DVI mode, an edit in section 5 of a 47-page document followed by one
in section 2 shipped section 2's page with the old line. PDF mode, and
each edit alone, were right. The machine sanitizer caught it too ("the
rebuild's output differs").

The DVI writer keeps bytes in a buffer and writes them out half a buffer
at a time (§597–§598). After each page, partex hands the file only what
has already been written out; the rest of the page stays in the buffer,
part of the state. The writer's state hash left that buffer out, and the
file's position with it, on the premise that "a splice relocates that".

For DVI that premise is false. A page's `bop` carries the absolute
offset of the previous page (§640). A movement command is reused only
while the earlier one is still in the buffer (§611, `dvi_gone`). So a
DVI page's bytes depend on everything before it.

What happened in the second rebuild: the re-run of section 2 shipped
the new page into the buffer, met the old run at the next boundary
because its state looked equal, and stopped. The old region that later
flushed the buffer then replayed the old run's bytes. The file came out
the old run's, consistent throughout, which is why only the page's text
showed the error.

In machine mode, where outputs are effects reused as they are, the state
hash now takes the writer's unwritten bytes (from `gone` on, in buffer
order) and its position (offset, pointer, limit, `gone`, last `bop`):
`DviWriter::hash_placement`. The first version put them in the writer's
hash for every mode, and e2e `cutoff` and `resident` lost their early
cutoffs. Those cases use the checkpoint session, which relocates DVI
output itself (at a cutoff it takes the new run's writer by page), so it
keeps the position-free hash. After a DVI page changes length, the re-run now goes on to the
job's end, because every later position differs. That is correct and
costs what it must: making DVI output position-independent would need
the `bop` pointers as link-time relocations, and §611's buffer-dependent
movement reuse would remain. On this document the second rebuild takes
0.17 s. The e2e `machine_edits` harness now takes a format, and
`machine_edits_dvi` runs the two edits in DVI mode against pdfTeX; it
fails without the fix. Files touched: partex-engine `dviout.rs`,
partex-core `dvi.rs` and `statehash.rs` (one line: the writer's
placement is hashed when effects are on), `xtask/src/e2e.rs`,
`tests/e2e/edits-dvi.tex`.

### Snapshot restores: what is O(state), and three pieces made O(change) (Agent C, `120de28`)

B measured a restore at 12–33 ms in proportion to the state. Profiled on
the course (frame pointers, 12 one-word edits made and taken back in
ch15, the cold build excluded): a replay of clean regions was 21% of a
rebuild. Thawing the token store took 10%, hashing the restored state's
`Rest` at the seek 6%, and cloning and freeing snapshots most of the
rest.

- *Thaw by content.* A restore reused the running store's lists only
  where a chunk was the snapshot's by address. After a rebuild's
  re-execution the running store and the old run's snapshot share few
  chunks, though most lists are the same. Lists are now taken as they are
  when their hash and fields are the snapshot's: 143k taken and 25k
  copied over the 12 rebuilds.
- *Rest's version from the trace.* After a restore, a replay patches
  only cells, none of which `Rest` hashes when names, fonts and object
  numbers are cells, so the version the trace kept with the snapshot is
  the state's. A font whose tags changed keeps its metrics in `Rest`, and
  an input file rebased to edited contents changes what is left of it:
  either one makes the replay hash. Verified by hashing anyway on the
  course's edits (`PARTEX_MACHINE_VERIFY_REST=1`, and always under the
  sanitizer).
- *Shared trees and destination names.* The object table's lookup trees
  and destination names were cloned into every snapshot and freed with
  it. They are shared now and copied when they change.

Result: the replay went 26.3 → 23.4 ms per rebuild, a rebuild 96 → 90 ms
(wall time on a shared machine, noisy), 5.79 → 5.71 G instructions over
the 12 rebuilds. Outputs identical to cold. A smaller win than the
profile promised: what is left is O(state) in a way these did not touch.
The replaced engine is dropped, and each snapshot a replay seeks to has
unique parts (the font tables, the PDF writer's per-font table, a
thousand entries each). Those are cloned when the snapshot is taken and
freed when the next one replaces it, since no trace holds a seek's
snapshot.

### The tracker sanitizer compares what eqtb words name (Agent B)

The tracker sanitizer's shadow diff checks that every cell whose value
changed across a region was reported written. It compared eqtb words.
Bug B, a `\wd` that changed a box register's box in place under the same
id, left the word the same and slipped through.

The shadow diff now also looks at every word that is equal in both
states but names something: a token list, glue with its lineage, a
shape, or a box. It compares the named values, taking a shared list or
`Arc` as equal without looking. It scans only the eqtb locations in use
(up to `eqtb_top`, not the 600k-slot `hash_extra` capacity), a chunk's
words at a time. The first version walked every slot, and the course
ran at 0.66 s a region. On a 47-page LaTeX document at 60-command
regions (1,312 of them), a sweep takes 96.8 s with the check against
76.5 s without, about a quarter more. `PARTEX_SANITIZE_NAMED=0` turns it
off.

With bug B's fix reverted locally, it names `box 0` for `\wd0=5pt`
(plain format, 2-command regions). With the fix, it is clean. A unit
test changes a box in place, for registers 7 and 300.

The first run also flagged `\endtemplate`, which names the permanent
empty list, in every region. `tok_valid` looks only at a token store's
thawed lists, so on a frozen store (a sanitizer's entry clone) it calls
every list invalid. The check now tests existence by the store's length.
`tok_valid`'s behaviour on frozen stores is worth a look in tok.rs.

The sweep (`PARTEX_SANITIZE_LOG=file` now collects each run's findings,
for runs whose standard error e2e does not keep) turned up one kind of
finding, which predates this change: it shows with the new check turned
off. `\dump` copies eqtb, the hash and the fonts into the format past
the tracker, so poisoning finds the dump region's output changing with
cells it said it did not read. This hits all 17 e2e cases that dump
plain or e-TeX formats. Reporting reads of everything dumped removes
the first cells named, but `\lineskip` and `\baselineskip` still show,
so the fix is not small and is left open. Not finished within the
hour: the e2e cases that build LaTeX formats (each over 12 minutes
under the sanitizer at 60-command regions) and the full course. The
course's first 300 regions were clean.

Also seen: a run in INITEX with no format showed no changed or written
cells at all, not even a `\setbox`. The sanitizer may not track such
runs, which is worth a look.

### Job-written files, step 1: a region per input file read (Agent C, `c543b12`)

The series in DESIGN 7.15 (committed first, `d6b666f`) makes a
`\label` cost what it changes in the `.aux` read-back. Its first step
is the one that needs nothing else: cut where an input file begins or
ends. Before it, the `.aux` read at `\begin{document}` sat inside a
131k-command region with the preamble's end and the first pages, so a
pass 2 that saw one new `\newlabel` re-ran all of that and found no
boundary to stop at before the grain's next cut.

What: `TexMachine` remembers the file being read at the last boundary
(`in_open` and the name). A boundary where that changed is a candidate
of level 2, which the chooser cuts once the region has cost
`Config::file_cut` commands, whatever the grain. The .aux read is then
its own region, and its end is a point where the new run's state is the
old run's but for the entries' definitions. Chapters and `\input`s get
the same cuts, which is where most one-word edits sit.

Choosing the threshold, on the course (in one process, all outputs
identical to plain runs; word = a one-word edit, then the `\label`):

| `file_cut` | cold instructions | regions | word | `\label` |
|---|---|---|---|---|
| off | 305.1 G | 282 | 0.381 s | 0.857 s |
| 2,048 | 318.2 G | 709 | 0.262 s | 0.405 s |
| 16,384 | 309.3 G | 464 | 0.254 s | 0.324 s |

16,384 keeps nearly all of the gain for a third of the cold cost.

Under `partex watch` with a fresh store (B's scripts, a copy of the
course): the `\label` 3.17 -> 1.96 s (pass 2 0.85 s over 51 regions ->
0.50 s over 14), its removal 3.42 -> 1.66 s; the label's PDF is
byte-identical to a plain two-pass run. In one process: a footnote
1.9 s, a `\Cref` 0.68 s, `\linespread` 70.6 s, all identical to cold
(3.2, 0.81 and 77.7 s when last measured, several commits back, so not
all of the difference is this change). The sanitizer is clean on the course (a word, then
the `\label`) and on the stoptest sequences E1 E2 E3, E1 E4 E5 E2,
E5 E4, E2. The persisted layout is `/9`.

What is left: pass 2 still re-runs every region from the new entry to
the `.aux`'s end, since each later line moved (step 2 keys positions by
entry, step 3 the reads).

`tok_valid` (B's question): its callers outside the sanitizer are
`show_token_list` (printing, on a running engine) and `import_cell`,
which thaws the store first. Neither sees a frozen store, so the
length-based test B's check uses is the only one that needed it.

### The link's number resolution, per region and cached (Agent B)

Resolving virtual object numbers was 20–27 ms of each 25–43 ms link: one
pass that cloned every effect and assembled 10.1 MB of stream data in
407 streams. That pass is now split. The numbering (the `Num` events'
replay) stays whole, since it is cheap. The resolution is done per
region, and the watch keeps each region's resolved effects by its key
(`LinkCache`).

A region is taken again only if all of these hold:
- the build did not put it in since the last link (`Build::take_touched`;
  a renumbering of keys counts as putting in every region);
- its object-stream counters and open file at entry are the same;
- the numbers it writes are the same. This check walks its effects and
  relocations, not their bytes.

Nothing else goes into a region's resolution, so the output is exact by
construction. A unit test adds an object in the first region, so the
numbers of untouched later regions move and they must be resolved again;
at every step it compares against `link` of all the effects. On the
course in watch mode, on master 53b54ef with this change, and the same
binary with `PARTEX_MACHINE_LINK_CACHE=0` as before:
- one-word edit: link 35.7 → 16.0 ms;
- taking it back: 39.4 → 13.5 ms;
- `\label`: 29.2 + 27.2 → 23.2 + 13.0 ms;
- taking the `\label` back: 26.4 → 13.2 ms;
- `\Cref`: 41.7 → 35.4 ms.

The `\Cref` gains least because it moves every later object's number:
467 of 682 regions are resolved again, and their bytes really do change.
All twelve PDFs were byte-identical to cold builds, and `scripts/pdfcheck
same` agrees. What remains, about 13 ms, is the layout over every effect,
the copy and the numbering replay. Files: partex-core `effects.rs`,
partex-incr `build.rs` (touched keys, keyed traces), partex-cli
`machinehost.rs` (the watch).

### Job-written files, step 2 tried and dropped: no line count is stable on both sides of an insertion (Agent C)

Step 2 of the plan keyed a position in a file by the lines left instead
of the lines read (`Machine::at` only: `Rest` and the line cells went on
counting from the start, so it was meant to change no reuse). It was
built, with a per-level cache of the files' line counts so that `at`
stays O(1).

It is wrong. After an edit that inserts lines, the keys of the
boundaries *before* the insertion go stale, since the file's total
moved. A clean region there is replayed, and its recorded entry no
longer matches the state it is replayed onto. The sanitizer's chain
check caught it on the stoptest document: a word edit near the top,
then a new paragraph three sections later. The run stopped with
"regions do not chain (region 2)": a region at line 5 was recorded
with 560 lines left and replayed with 562. The same sequence is clean
without the change. None of the sanitizer runs so far had inserted a
line: every edit in B's stoptest sequences and in e2e stays within its
line.

Doing the same in `Rest` is no better. It would dirty every region
before the insertion instead of every region after it, and
`restore_rest` from the end would put those regions' files back one
line off. Keys by content are not unique (blank lines, `\]`), so a sync
by content can replay a region at another place whose lines its cells
never checked. The replacement (DESIGN 7.15, step 3) renames the old
build's positions through the diff of the file, which the host has.
Regions that read only unchanged lines then mean the same in the new
file. Line numbers read as values stay a cell of their own, which is not
renamed.

Measured on the course, why it matters beyond the `.aux`: a comma added
in ch15 rebuilds in 0.181 s (1 dirty region, 76k commands). Splitting
the same paragraph in two, which inserts two lines, takes 1.796 s (13
dirty regions, 1.73M commands), since the rest of the chapter re-runs.
Pressing Enter costs ten times typing a letter.

Kept: `machine_edits` (e2e) now also inserts a paragraph and a blank
line, and has a third run with `PARTEX_MACHINE_SANITIZE=1`. With the
lines-left keys it fails ("4 rebuilds ... for 6 edits": the sanitized
run panics); without them it passes. The attempt is not committed.

### Renaming lines, part 1: the input's line numbers leave `Rest` (Agent C)

The first part of the rename through the diff (DESIGN 7.15, step 3),
behind `PARTEX_MACHINE_RENAME=1`, off by default. With the switch on,
the machine's `Rest` hash leaves out `line`, the line stack and the
lines read of every served file. They become one cell,
`MCell::Positions`. Its value keeps the numbers together with the names
of the served files by slot (an input level, or 256 plus a `\read`
stream), because a rename needs to know which file a number counts in.
Every region reads it at its start and writes it at its end, as it
does `Rest`. Setting it puts the counters back and reads each served
file on from its line; `replay_exit` sets it after taking the
snapshot. The cell holds exactly what `Rest` left out, so on its own
the change reuses nothing new. The switch is hashed into `Rest`'s
version, and the persisted layout is `/10`.

Checked with the switch on:
- All 34 e2e cases are identical in machine mode, including
  `machine_edits`'s sanitized run with its two line insertions and
  `modern_watch_sanitized`.
- The stoptest sequence with a paragraph inserted is sanitizer-clean,
  with the same regions and PDF as with the switch off.
- On the course, a paragraph split and then a word edit is
  sanitizer-clean.
- A unit test covers the partition and setting the cell:
  `positions_leave_rest_as_a_cell_of_their_own`.

| course, ch15, same process | off | on |
|---|---|---|
| a comma | 0.189 s | 0.181 s |
| a paragraph split (two lines inserted) | 1.800 s | 1.767 s |

Both edits have the same dirty regions and commands either way, and
all four outputs (PDF, `.aux`, `.out`, `.toc`, log) are identical to
plain runs of the edited text.

Found on the way: `Rest` holds more line numbers than the input's. The
nest records where each mode began (`mode_line`, §213), the condition
stack where each `\if` began (§489, and `skip_line`, §493), and the
page builder keeps `pack_begin_line` (§661). A boundary inside a
paragraph after an insertion still differs through them, and most cuts
fall inside paragraphs. Part 2 now moves those copies into `Positions`
as well, each tagged with its file, alongside the value cell.


## 2026-09-28

### The second design: why the rebuilds were slow, located in the code, and the unit model (coordinator)

At 23:00 on the 27th the user paused everything and asked for a new
design rather than more work under the old one: "a good multi pass
incremental build with dynamic regional hierarchical dependencies
should be VERY fast." Two static readings of master `41bc504` (no
builds, no edits) supplied the facts. What they found, as file
references so the next reader can check them:

*Where cuts fall.* The only test for a boundary candidate is that the
top input level is a file (`run.rs:68`, checked at `big_switch`,
`maincontrol.rs:111`): nothing about the mode, the group level, the
list or the page. Letters never return to `big_switch` (§1034–1040,
`maincontrol.rs:307`), so inside a paragraph candidates are the spaces
and the commands. Exhausted token lists are popped lazily (§357,
`scanner.rs:231`), so the first file command after a macro is not a
candidate. Levels: 2 at a file edge, 1 everywhere else; nothing for a
paragraph start. The output routine runs from a token list
(`page.rs:439`), so it has no boundary; only the stop before
`\shipout` is special-cased. The parked branch `cheap-cuts-wip`
(13e4a16) had the right predicate (`cur_level == LEVEL_ONE && save_ptr
== 0 && mode == VMODE && list.is_empty() && input_ptr <= in_open`) and
was dropped because `\Cref` got slower when cuts stopped falling right
after ships.

*The page builder.* `Builder` (`partex-engine/src/builder.rs:112`)
holds the page list, `so_far`, the insertion list, `last`, `contents`,
`best_break`; it is hashed whole into `Rest` ("page",
`statehash.rs:1695`). `build_page` has eleven call sites (§1076, §1091,
§1094, §1096, §1100, §1103, §1026, §1054, §812, §1145, §1200). The
reads of `\pagegoal`…`\pagedepth` (§421, `scan.rs:268`),
`\insertpenalties`/`\deadcycles` (§419, `scan.rs:259`),
`\lastskip`…`\lastnodetype` at the list's head (§424, `scan.rs:410`),
`\prevdepth`/`\spacefactor`/`\prevgraf` (`scan.rs:225`) and the marks
(§386, `expand.rs:61`) are untracked. Tracked: `\box255`, the
insertion boxes, `\outputpenalty`, and the builder's parameters
(`page.rs:233`). `fire_up` resets `last` (§991, `builder.rs:330`).
The contribution list is `nest_at(0).list`, a flat `Vec<Node>`; the
64-node chunks exist only in snapshots (`machine.rs:1147`).

*What a cut costs.* `Tex` is `#[derive(Clone)]` (`tex.rs:54`);
`Snapshot::of` (`machine.rs:1215`) is commit, clone, then
`Lists::share`, a compare of every node on the page. The token store's
commit scans every list's flag twice (`tok.rs:187`, `:228`) and
memcmps every list whose flag is set; the flag is set by `get_mut`
(`tok.rs:515`), which `macro_call` reaches through `add_token_ref`
(`expand.rs:571` → `input.rs:579` → `tok.rs:628`) only to bump the
reference count that lives inside the record (`tok.rs:29`). So every
macro expanded since the last cut is compared. Parameter lists are
allocated and released per call through the same path
(`input.rs:622`). Each `Flat` commit memcmps its whole allocation, not
its used prefix (`flat.rs:72`), and `Flat::segments` memcmps the pool
again for the `Rest` hash (`statehash.rs:1539`). The clone copies
whole: the object stores, `xregs.cells`, `xeq_level`, `save_eqtb`,
twelve per-font vectors, `pdf.fontw`'s trees, `pdf.ship`, `pdf.out`'s
buffers, `cur_mark`, the exceptions, the memo tables, the log and
`\write` buffers, the host's maps. The `Rest` hash computes from
scratch the save stack, the primitive tables, all `hyph_word` entries,
the exceptions, the node lists and the PDF writer
(`statehash.rs:1520–1824`).

*What a restore costs.* The snapshot's `Tex` is cloned again
(`machine.rs:1233`); `thaw` copies all of eqtb, hash and the save
stack (`journal.rs:127`) and the five `Flat`s; `thaw_from` is at least
O(#lists) (`tok.rs:416`); the tracker is reset to default after every
`set` (`machine.rs:2667`, `:2822`) and its slot arrays, 18 bytes per
slot for eqtb + 196,608 + hash + fonts, are reallocated and zeroed at
the next step (`machine.rs:244`); `replay_exit` does one `set` per
patched cell. `Build::rebuild` never restores a dirty region's own
entry snapshot: re-execution starts from the state after
`replay_span`. Guards are `(cell, version)` with no record of where in
the region the first read fell (`trace.rs:22`), so nothing could resume
inside a region.

*Lines.* `MCell::Line` is versioned by the line's raw bytes
(`machine.rs:1687`); `changed_cells` compares by index
(`machinehost.rs:720`); machine mode has no tokens-under-catcodes
comparison (that exists only in the session's `tokendeps.rs`).

The diagnosis and the design are DESIGN.md §7.16: units cut at clean
points; the page builder out of `Rest`, replayed as a fold over each
unit's contributed items with its observables as cells; a checkpoint
as shared roots instead of a clone; reference counts off the token
path; restore by rebase; regions at clean points with the existing
chooser and coarsening as the policy; then positions, entry logs,
the link per file, fast mode, the loop. Rejected: keeping the
region-and-`Rest` model and peeling more cells out (each peel removed
one cascade and the next appeared; the page builder is the lump that
cascades on every layout edit); a lazy page builder inside regions
(§7.5), which changes when the output routine runs relative to the
paragraph's text and so what `\c@page` reads see, unlike the fold,
which only changes the replay; in-region checkpoints as a separate
mechanism (unnecessary once a cut is cheap, since a region can then
be a clean-point stretch).

Working mode from here (the user's decision, 23:50): one Opus agent in
the main checkout, tasks of about 30 minutes, the agent never commits,
the coordinator gates, commits and pushes; milestone emails say whether
the schedule holds. The schedule, 33 tasks, about 16.5 agent hours,
about 25 hours wall with gates, from 2026-09-28 00:10:

| phase | tasks | agent time | milestone |
|---|---|---|---|
| 0 baseline harness (five edits: space, word, `\label`, footnote, Enter) | 1 | 0.5 h | |
| 1 quick wins: tracker across `set`; clean-point candidates; the chooser rule | 3 | 1.5 h | M1 |
| 2 cheap cuts: refcounts off the token path, changed ids, `Flat` marks, `Checkpoint` of roots, chunked and `Arc` containers, scratch flushed, memoized digest, restore by rebase, regions at clean points cold | 10 | 5 h | M2 |
| 3 the fold: `Page` value, observable cells, the delta, deferred fire, the fold in `rebuild_or_stop`, measurement | 6 | 3 h | M3 |
| 4 positions and streams: rename parts 2–6, entry logs, measurement | 7 | 3.5 h | M4 |
| 5 output and the loop: link per file, the watch loop, lines by tokens, fast mode | 6 | 3 h | M5 |

The two paused WIP branches are inputs, not lines of work: `persist`
(`ee91af0`, B's freeze at O(changes), with a 5–8% cold regression from
a flag test on every stored token) is consulted for phase 2 and
superseded by the refcount change; `machine-converge` (`3ae550d`,
rename part 2) is cherry-picked in phase 4.

### Task 0: the five-edit harness and its baseline (Agent)

Every later task reports against the same five edits, so they are now
one command: `bench/edits.sh DOCDIR EDITSFILE [BIN]`, with the course's
edits in `bench/edits/course.txt`. For each edit the script runs one
process in DOCDIR, as the probe's `edit.sh` does: a cold machine build
of the unedited job, then one rebuild after the edit, applied in memory
(`PARTEX_MACHINE_EDIT`). Every edit therefore starts from the same
build, not from the previous edit. It prints one Markdown table from the
rebuild's statistics line and the link line. It first checks that each
`from` occurs exactly once in the file, because the engine replaces the
first occurrence and only reports a missing one after the 55 s build.
The spec is passed inside the sandbox's `env`. `scripts/sandbox` copies
`PARTEX_*` variables line by line, which would cut a spec that contains
a newline.

One number was missing: how many restores a rebuild makes. Every other
column already existed in `Stats` (`partex-incr/src/build.rs`). The new
field `Stats::restores` counts the `replay_span` calls that restored
the old run's state through `Machine::replay_exit`. That is the restore
DESIGN §7.16 prices: clone, thaw, and tracker reset. `replay_span` now
returns whether it did that. The time of a restore stays in
`replay_ns`, which also covers single-region applies.

What each column is:
- **rebuild ms**: the rebuild's wall time;
- **commands re-run**: `executed_cost`;
- **dirty regions / total**: the old regions re-executed, over the
  regions before the edit;
- **cuts**: `recorded_regions`, the new regions recorded (each is a cut
  with its snapshot);
- **restores**: `restores`, and `replay_ns` in ms;
- **link ms**: the link, which is not part of the rebuild time.

Baseline, measured 2026-09-28 00:10–00:15 on the course probe copy
(`~/code/tmp/course-ap`). Conditions: release binary built from master
`b822276` plus this counter; in process; one pass; a cold build of
54–55 s before each rebuild, not counted; 464 regions after the cold
build.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 194 | 76119 | 1 / 464 | 15 | 1, 25.6 | 67.5 |
| word | 1478 | 1457463 | 8 / 464 | 44 | 3, 44.3 | 63.3 |
| label | 444 | 237937 | 4 / 464 | 31 | 3, 44.0 | 65.4 |
| footnote | 2643 | 2191391 | 19 / 464 | 155 | 8, 99.8 | 70.0 |
| enter | 1834 | 1727637 | 13 / 464 | 84 | 3, 45.8 | 64.7 |
| word-local | 198 | 76119 | 1 / 464 | 15 | 1, 25.9 | 66.9 |

The specs, all in `ch15.tex` (`<NL>` is a newline):
- `space`: `penalty methods raise the weight` → `penalty methods raise  the weight`
  (line 69; the second space makes no token);
- `word`: `That limit is expensive.` → `That limit is dear.` (line 32);
- `label`: `zero where the constraint holds.` →
  `zero where the constraint holds.\label{partex:bench}` (line 67, the
  end of a paragraph);
- `footnote`: `Placement is a constrained problem.` →
  `Placement\footnote{Bench.} is a constrained problem.` (line 18, the
  first sentence of the first section);
- `enter`: `the solution first. \doc{ePlace} adjusts` →
  `the solution first.<NL><NL>\doc{ePlace} adjusts` (line 69 split in
  two by a blank line: two inserted lines).
- `word-local` (added at 00:20, at the coordinator's request, after the
  other five): `starting small so` → `starting tiny so` (line 69). It is a
  word edit that does not reflow past its paragraph, set beside `word`.
  It re-runs only the edited line's region, 298 (checked with
  `PARTEX_MACHINE_PARTS=7`), with the same commands as `space`. This row
  ran from its own one-line file while `cargo xtask check` ran in the same
  checkout (load 9.5), so its time may be high.

What the numbers say, and what was odd:
- The `label` row is one pass. A watch makes a second pass because the
  `.aux` changed, and the harness does not measure that pass.
- `space` and `enter` repeat the 7.15 measurements: 0.181 s with 76k
  commands, and 1.796 s with 13 dirty regions and 1.73M commands. So the
  harness measures what the earlier probes measured.
- The `word` edit costs eight times the one-word edit of §7.15 (0.37 s,
  149k commands). A second run with `PARTEX_MACHINE_PARTS=7` showed
  why. It gave the same 1,457,463 commands in 1588 ms (log in
  `runs/bench-word-why`). Region 294 reads the edited line. Regions
  295–299 are dirty only through `Rest`, and 298 also through its
  sealed lines. Region 393 reads `Numbering` (the `\Cref` region of
  §7.15), and 463, the end, reads `Glyphs` and `Numbering`. Shortening
  a sentence just before a figure reflows its paragraph, and the page
  state in `Rest` differs until the chapter's end. This is cause 2 of
  §7.16. The old measurement's word happened to leave the layout alone.
  Both kinds of word edit matter: this one for the fold (phase 3), a
  layout-neutral one for the cheap cuts (phase 2).
- Region 393 (360k commands, about 330 ms) re-runs in `word`,
  `footnote` and `enter`. In `word`, the one checked, the only reason
  is that it reads `Numbering`.
- Rebuild times vary by about 7% between two identical runs
  (1478/1588 ms). Counts do not vary.
- The in-process path (`machinehost::run`) never touches the store.
  "Warm" here means a build made in the same process, not one loaded
  from `_store`. A whole run of the harness takes about 5 minutes,
  almost all of it in the five cold builds.

Time: about 15 minutes of work, from 00:03 to 00:18, including three runs
of the harness (one test edit, the five, and the `word` rerun).

### Task 1: the tracker keeps its tables across `set` and restores (Agent)

The static read behind §7.16 said a restore pays for new tracker tables,
several times per replay. `set` and `replay_exit` replaced the tracker
with `CellTracker::default()`. A restore's new engine, a clone, came with
a default tracker too, since a clone of the tracker is empty. The next
step's `reserve` then allocates and zeroes the tables. On the course that
is 1,465,481 slots at 18 bytes each (eqtb with `hash_extra` 600000, the
196,608 registers above 255, the hash's links, 9001 fonts), about 26 MB.

What changed (`crates/partex-core/src/machine.rs`):
- `CellTracker::reset` puts the tables back to a fresh tracker's state.
  It clears the slots its logs name, which the region-end `clear` already
  does. Then it resets the layout, the overflow flag and the logs'
  lengths, and keeps the allocations.
- `CellTracker::adopt` gives a restore's new engine the running engine's
  tables, reset (`restore_rest`, just before `self.tex = new`). The object
  log's room moves the same way (`ObjTab::adopt_log`).
- The first `reserve` after a reset sizes the tables exactly as a fresh
  tracker does (`Log::resize`, and a `sized` flag in both logs). An
  append then fails at the same length as it would in a fresh one, and
  the overflow paths stay the same.
- `PARTEX_MACHINE_KEEP_TRACKER=0` restores the old path.

Why clearing by the logs is exact. A slot's state is set only together
with an append to a log. The one exception is `group_end`, which pops a
soft read, and it clears that slot's `SOFT` and `SAVED` bits when it pops
the slot's last entry. The bits left, `READ` and `WRITTEN`, are logged.
`soft_level` is left stale, and is read only for a slot in state `SOFT`
or `SAVED`, which `soft_read` sets together with it.

After an overflow a log may miss a slot, so the tables are made again.
Between a restore and the next `reserve`, a fresh tracker counted any
tracked access as an overflow. A kept one records it instead. Both are
then reset by a `set` or stopped by the overflow assertion at the cut, so
no run that finishes differs.

The unit test `a_reset_tracker_is_a_fresh_one` checks this. It runs 48
rounds of random reads, writes, soft reads, saves, restores and group
ends, over two layouts, through `reset` and `adopt`. The reset tracker
must report the same reads, writes, pending soft reads, logs, slot states
and room as a fresh one. Leaving out the `clear` in `reset` makes it
fail.

Measured. Release binary from `ac9c7fc` plus this change, course probe
copy, `bench/edits.sh`, in process, one pass. Measured 00:32–00:40 on 24
cores, load average 4.2–4.8 (a browser and the user's watch running).
Before is the same binary with `PARTEX_MACHINE_KEEP_TRACKER=0`.

| edit | rebuild ms before → after | restores (n, ms) before → after |
|---|---:|---:|
| space | 176 → 190 | 1, 24.3 → 1, 25.3 |
| word-local | 193 → 194 | 1, 25.6 → 1, 23.3 |
| label | 455 → 499 | 3, 43.6 → 3, 47.1 |
| footnote | 2783 → 2686 | 8, 90.1 → 8, 87.7 |

(Task 0's baseline for the same four rows was 194, 198, 444 and 2643 ms.)
All of these differences are noise, about ±7% between identical runs. I
checked with a second measurement: one process, 20 rebuilds alternating
the `space` edit and its undo. The mean was 33.7 ms (switch off) against
34.5 ms (switch on), excluding the first.

A temporary print, since removed, showed why:
- A fresh `reserve` costs 1.7–2.9 ms in the process once the allocator
  is warm, and 7–9 ms for the first ones. A release micro-test gave
  5.3 ms fresh against 9 µs reset.
- The old path paid this once per restore, not once per `set`: the
  resets of one replay come before a single step.
- The footnote rebuild made 8 fresh reserves with the switch off and 1
  with it on, so about 13 ms of 2.7 s is saved.
- The one left comes from `Build::rebuild` starting from
  `new_initial.clone()`: a clone's tracker is empty, so the first
  restore has nothing to adopt.

The restores column did not move because a restore's 25 ms is not the
tracker's: the tables are built at the next step, inside `run_ns`. That
column is the clone of the snapshot's engine and the thaw (cause 3 of
§7.16), which phase 2's rebase targets.

Output: the PDFs of the two modes differ only in `/ID`, which depends on
the output path. That holds for the footnote rebuild and the 20-rebuild
sequence.

Rejected:
- A process-wide pool of tracker tables, to cover the rebuild's clone as
  well: `partex-core` is `no_std`, so it has no locks or thread locals.
- A hook in the runtime to hand the previous final state's tracker to
  the rebuild's working state: it belongs with the checkpoint-of-roots
  work, for about 2 ms per rebuild.

Time: about 32 minutes (00:23–00:55), a little over the 30-minute budget. The
extra went on finding out why nothing moved.

### Task 4: reference counts off the token path (Agent, branch cuts)

A macro call marked its body changed. `begin_token_list` (§323) bumped the
body's reference count through `get_mut`, and `end_token_list` (§324)
dropped it again. Both wrote the list's record, so every macro expanded
since the last cut was compared with its shadow at the next commit
(DESIGN §7.16.3). The input stack's references are no longer counted
(`input.rs`, `tok.rs`, and `Flat::flat` in `flat.rs`).
- `begin_token_list` adds no reference for types `macro` and above.
  `end_token_list` calls `input_level_ended` instead of
  `delete_token_ref`.
- `refs` now counts only the tables' references (eqtb, the save stack,
  marks, whatsits, the memo's pins, the intern table).
- `delete_token_ref` drops a table's last reference. It then walks the
  input stack (`cur_input` and `input_stack[..input_ptr]`, levels with
  `state = token_list`, `index >= macro`, `start = id`). If a level
  still reads the list, the list is kept with `refs = -1` (a zombie) and
  not freed.
- `input_level_ended` frees a list with `refs < 0` when no other level
  reads it. A table that takes the list again (`\let`, `\toks`, a
  `\def` of the same body through the intern table, a memo pin) brings
  `refs` back to 0, one reference, as the count would have.

Why nothing observable changes (the argument is in the comment on
`delete_token_ref`):
- A zombie keeps its tokens, so the text read and `show_context` are
  tex.web's.
- The count reached zero exactly when neither a table nor a level held
  the list, and that is where the list is freed now. Ids are therefore
  reused in the same order, and the memo's generations and the format
  are as before. The format has no zombies: `final_cleanup` ends every
  level before `\dump`.
- `refs = -1` is in the frozen record, so a checkpoint holds it with the
  input stack it belongs to. The state hash never read `refs`
  (`list_hash` covers the flag and the tokens), and `same_as` and
  `refreeze` compare it only between states of the same run.
- `flush_list` never looked at the count.

Measured on 2026-09-28, 02:20–02:50. All runs on a private copy of the
course probe (`~/code/tmp/course-ap-t4`: the same sources, `_out` and
format as `course-ap`, copied so as not to share `runs/` with the other
agent). Release binaries, in the sandbox, on 24 cores at a load average
of about 4. "Before" is master `23ed8ca` plus a temporary counter, which
has since been removed.

A plain (not machine) pdflatex pass, `perf stat -e instructions:u,cycles:u`:

| binary | instructions | cycles |
|---|---:|---:|
| before (two runs) | 261.501 G, 261.501 G | 129.0 G, 131.6 G |
| walk tests `state`, `index`, `start` per level | 264.237 G (+1.05%) | 131.9 G |
| walk tests `start` first | 261.665 G (+0.063%) | 128.8 G |
| walk: branchless scan of `start`, full test on a match | 261.532 G (+0.012%) | 128.5 G |
| the same, reading a checkpoint's stack too (kept, below) | 261.773 G (+0.104%) | (gate running) |

The expected decrease did not come. A probe build counted the walks in
one plain pass: 21,190,344 walks over 489,586,332 levels, 23.1 on
average, and 1,285,421 of them (6.1%) found a level. A table drops a
list's last reference in about one macro call in four: `\@ifnextchar`
redefines `\reserved@a` and `\reserved@b` every time, and every group
end destroys its local definitions. So the walk is not rare, and it costs
about what the count saved: two read-modify-writes of the record per
call, 75M calls. `perf annotate` showed the first form's loop at 9
instructions per level, most of the +1.05%. The kept form tests `start`
alone, without a branch per level, and makes the full test only on a
match. The first full gate then found a panic in the e2e case
`readback` (watch mode, both modes). `overlay.rs` patches eqtb cells
into a checkpoint before that checkpoint resumes. Its `eq_destroy`
reaches `delete_token_ref` while the checkpoint's input stack is still
frozen, and a frozen `Flat` has an empty live vector. The walk now reads
a flat stack directly, through the new `Flat::flat`, and a frozen one
through `Flat::prefix`. That costs 0.09% more, which puts the kept form
at +0.10%, inside the 0.5% gate. The PDF is identical to before's
apart from `/ID`, which depends on the output path.

The commit, on the course's cold machine build (464 cuts): 1,050,363
lists compared before, 423,751 after, so 2,264 per cut becomes 913. What
is left are lists written between two cuts; parameter lists are among
them (the stack-arena item of §7.16.3), but I did not break the number
down.

`bench/edits.sh`, rows `space`, `label`, `footnote` and `word-local`
(one in-process rebuild after a cold build, one pass):

| edit | rebuild ms before → after | cuts | restores (n, ms) before → after |
|---|---:|---:|---:|
| space | 183 → 198 | 15 | 1, 24.0 → 1, 25.5 |
| label | 485 → 492 | 31 | 3, 47.2 → 3, 44.4 |
| footnote | 2706 → 2790 | 155 | 8, 87.1 → 8, 88.3 |
| word-local | 224 → 196 | 15 | 1, 24.8 → 1, 25.3 |

Commands re-run and dirty regions are unchanged: 76119, 237937, 2191391
and 76119. All the differences are inside the ±7% noise of Task 0. The
1,350 fewer list compares per cut are small memcmps, a small part of the
cut's cost. The four rebuilt PDFs are byte-identical (`cmp`) to
before's.

Rejected:
- A count of reading levels kept per list outside the frozen record.
  The test would be O(1), at about three instructions per call and per
  end. But the machine replaces an input stack without its store
  (`Machine::adopt_position`, `machine.rs:1596`), and thaws and loads
  stores under stacks. Each of those would have to recount, and a stale
  count frees a list that is still being read. The walk takes
  everything from the stack at the moment of the test, so it cannot go
  stale.
- A "begun" flag per list, to skip the walk for lists that were never
  read. It goes stale after a thaw or `adopt_position` in the direction
  that frees wrongly, unless an epoch bumped at each of them guards it.
  That is the next step if the walk shows up again. A probe would first
  have to count how many of the 21M lists were ever begun.
- The persist branch's list of changed ids is not part of this task.
  It is the next row of §7.16.3's table, and now has 913 ids per cut
  to hold instead of 2,264.

The harness rows and the four PDFs come from the form before the
checkpoint fix, which touches only the walk. The full gate
(`cargo xtask check`) passes with the kept form: e2e 34/34 in both
modes, and trip and etrip in both modes.

Time: about 48 minutes (02:12–03:00), over the 30-minute budget. The
extra went on the walk: finding its cost and measuring three forms (one
release build and one perf run each), then the checkpoint case the
first gate found.

### The order after the paper draft: optimization before features; clean points by group context (coordinator, 02:40–03:00)

The user asked for a review of the plan against their paper draft
(`paperv2.tex`, a reading draft with placeholder numbers). Outcomes:

- *Clean points need a fixed group context, not level one.* TeX Live
  2026's `latex.ltx` reads the `.aux` inside `\begingroup … \endgroup`
  both in `\document` (line 9486) and in `\enddocument` (line 15264),
  and every environment is a group, so a level-one rule excluded list,
  theorem and proof paragraphs, beamer frames and every `.aux` entry.
  The save stack is residual state (it is hashed in `Rest`). A
  paragraph start is the first candidate after `new_graf` (§1091) and
  its `\everypar`: an indented paragraph's list holds the indent box,
  so "list empty" held only after `\noindent` (T2 found this too).
  DESIGN 7.16.1 (by T2's agent) and 7.16.5.
- *T2's first measurement* (branch `units`, not merged): with level-3
  candidates cutting at `file_cut`, the space edit re-ran 32,870
  commands in 116 ms instead of 76,119 in 194 ms (9 cuts instead of
  15; cold regions 562 instead of 464). 43,945 level-3 candidates on
  the course, about twice the estimate: every command between
  paragraphs in outer vertical mode qualifies. T2 and T3 are merged:
  level 3 fires the chooser's cut, with `PARTEX_MACHINE_CLEAN_CUTS=0`
  giving the old boundaries.
- *Entry logs are served files renamed through the diff*, one mechanism
  with line renaming, not two (DESIGN 7.16.5).
- *Convergence*: passes as rebuilds exist only in `partex watch`;
  `partex build` moves onto machine mode (phase 7). BibTeX and
  makeindex stay whole stages memoized by content: a `.bst` has state
  across entries, so an incremental BibTeX is not exact. Pass 2 after a
  cold build reflows every paragraph whose `\ref` or `\cite` resolves;
  warm parallel units are what make it cheap.
- *Parallelism, by value* (DESIGN 7.16.9): warm parallel units first
  (they also make global edits such as `\linespread`, 79 s today, a
  few seconds); per-page emission from a page IR (PDF ship-out writes
  bytes today); cold speculation last and timeboxed (a guessed page
  fires in the wrong places, counters chain, floats wait for output
  routines); the stage pipeline not pursued (every `build_page` can
  fire before the next command; about 1.5× at best cold).
- *Order* (the user: "getting end-to-end optimizations fully before
  adding more features"): phases 1–6 as planned, then warm parallel
  units and convergence, PDF ship-out onto page IR, cold speculation;
  PNG, PDF inclusion and biber last. Their interfaces are written down
  now (DESIGN 7.16.10) so the optimizations leave room for them.
  Checked in the tree: `pdf/image.rs` reads JPEG only; none of the
  user's course documents use biblatex, biber or makeindex.

### Task 2–3: clean points as cuts (Agent, branch units)

A candidate boundary was any command about to be read from a file, of
level 2 at a file edge and 1 elsewhere, whatever the mode, the list or
the page (cause 1 of §7.16). Now a candidate that is a **clean point**
(§7.16.1) is of level 3, and the chooser's `file_cut` rule, which was
already `level >= 2`, cuts at it once the region has run 16,384
commands. That merged the planned Task 3 (the chooser rule) into this
one: the coordinator chose it after I reported that level 3 could not
be added without changing the boundaries.

The predicate, `Tex::clean_point` (`run.rs`), at a candidate: a file on
top of the input stack (token lists may wait below), no output routine
active (§1025), and one of
- *outer*: `nest_ptr = 0`, vertical mode (with its sign, §211), the
  contribution list empty (§215);
- *paragraph start*: `new_graf` (§1091) set `Tex::par_start` when it
  began a paragraph at `nest_ptr = 1`, and this is the first candidate
  since, with the nest still at `nest_ptr = 1` in horizontal mode.

Any group level qualifies (**fixed group context**, decided by the user
and the coordinator at 02:53). LaTeX reads the `.aux` file inside
`\begingroup … \endgroup` and every environment is a group, so
`cur_level = level_one` would leave out most of a document. It is safe
because the save stack is in `Rest`: `statehash.rs`, section "tables"
(about lines 1520–1532), hashes `save_ptr`, every entry below it (a
saved eqtb word by its location and content, anything else by its
bits), then `cur_level`, `cur_group`, `cur_boundary`. `TexMachine::step`
reads `Rest` at every region's entry, so a region that begins inside a
group is replayed only in the same group context.

The paragraph start was first "the paragraph's list empty", which fires
only after `\noindent`: `\indent` and a letter put the indentation box
there (§1091). It is now a flag. `new_graf` sets it in machine mode.
The next candidate outside an output routine takes it (clears it), and
it is a start only if the nest is still the paragraph's. So a paragraph
that `\par` ends before any candidate (an `\everypar{\par}`) has no
start. An output routine that `new_graf`'s own `build_page` fires runs
before the paragraph's first command; its stops before `\shipout` have
a token list on top and leave the flag alone. The flag is hashed in
`Rest` and kept by the machine's store (`save_machine_extras`), since
it decides a later candidate's level. What the list holds at the start,
the indentation box and whatever `\everypar` appended, is in `Rest` as
before. A paragraph that begins with a letter also has its first word
there: §1090 backs the letter up, and the main loop (§1034–1040) reads
the rest of the word from the file without returning to `big_switch`.
So its start is the candidate after the first word.

Exhausted token lists are now popped at `big_switch`, in machine mode
only (`Tex::pop_exhausted_lists`, `maincontrol.rs`). Without the pop, a
paragraph start after a non-empty `\everypar`, and the first command
after any macro, were not candidates at all. The pop does what §357's
`get_next` would do next, with no token read in between, so TeX cannot
observe it:
- `end_token_list` (§324) changes the same reference counts, the
  parameter stack, and `align_state` for a `u_template`;
- no `show_context` can run in between;
- an exhausted `v_template` whose `end_template` was backed up (§1131
  looks for it) is never on top, because the backed-up list above it
  still holds that token (§325);
- `memo.list_ended` does not read `memo.top`.

`PARTEX_MACHINE_CLEAN_CUTS=0` (`machine::set_clean_cuts`) turns off the
pop, the flag and level 3 together, and gives back the old boundaries.

Diagnostics: after a cold build (`machinehost::run` and `Watch::new`)
the host prints the candidates by level 0–3, the clean ones split into
outer and paragraph starts, and the cuts by level (`TexMachine::census`,
not state: not saved, not hashed). `PARTEX_MACHINE_PARTS=5` prints each
region's cut level, from the last rebuild or else the cold build.

Unit test `clean_points_are_level_3` (`machine_store.rs`, beside the
other machine tests) runs a one-line INITEX job with `\nullfont`. A
letter then returns to `big_switch`, so every character is a candidate.
The test checks:
- between two paragraphs, and inside a group, the candidate is outer;
- an indented paragraph that begins with a letter, and one begun by
  `\noindent`, has its first candidate as the start;
- later candidates in a paragraph are level 1, even with the list
  empty;
- a paragraph that `\everypar{\par}` ends at once has no start.

Measured 02:59–03:06 with `bench/edits.sh` on the course probe copy.
Conditions: release binary built from master `23ed8ca` plus this change;
in process; one pass per edit; 24 cores; load average 7.6 at the start
and 4.1 at the end, because the other agent was benchmarking in its own
copy.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 127 | 32870 | 1 / 568 | 9 | 1, 24.0 | 68.0 |
| word | 1521 | 1454185 | 9 / 568 | 44 | 3, 45.8 | 68.8 |
| label | 260 | 73989 | 3 / 568 | 19 | 2, 37.3 | 66.5 |
| footnote | 2324 | 1847899 | 19 / 568 | 136 | 9, 98.3 | 68.8 |
| enter | 2030 | 1722276 | 17 / 568 | 87 | 3, 49.9 | 67.5 |
| word-local | 135 | 32870 | 1 / 568 | 9 | 1, 24.1 | 66.6 |

The same binary with `PARTEX_MACHINE_CLEAN_CUTS=0`, `space` only, gave
186 ms, 76,119 commands, 1 / 464, 15 cuts, 1 restore in 24.1 ms. Those
are Task 0's counts exactly, so the switch gives back the old boundaries.

Against Task 0's baseline:
- `space` and `word-local` re-run 32,870 commands instead of 76,119, in
  127–135 ms instead of 194–198.
- `label` re-runs 73,989 commands instead of 237,937, in 260 ms instead
  of 444.
- `footnote` re-runs 1.85M instead of 2.19M, in 2.3 s instead of 2.6 s.
- `word` (1.45M, 1.5 s) and `enter` (1.72M, 2.0 s, 17 dirty regions
  instead of 13) barely move. They are dirty through `Rest`, the page
  builder (cause 2), which the fold (phase 3) addresses. Finer cuts do
  not help them.

Cold build, switch on: 562 → 568 regions against 464, and 57.4–60.8 s
over the six runs. Switch off: 56.9 s. The difference is within the
noise of a loaded machine.

Candidates in the cold build, levels 0–3, and the regions cut at them.
The one region more in each total is the job's end.

| predicate | seen | clean: outer, paragraph starts | cut at |
|---|---|---|---|
| switch off (old levels, no eager pop) | 0, 165,700, 957, 0 | — | 0, 211, 252, 0 |
| first try: `level_one`, "list empty" start | 0, 151,253, 604, 43,945 | not counted | 0, 197, 109, 255 |
| as built: fixed group context, flag start | 0, 142,383, 479, 52,940 | 51,728, 1,212 | 0, 197, 101, 269 |

The eager pop adds about 29,000 candidates, the first file command
after a macro: 166,657 without it against 195,802 with it. Most clean
points are outer, and the coordinator expected 15–25k. As built there
are 52,940. The likely reason is that between two paragraphs every
command read from the file is a clean point: a `\section`, a
`\begin{…}`, each preamble line. This was not measured. There are 1,212
paragraph starts. For scale, the chapters have about 425
blank-line-separated paragraphs that begin with a letter; items,
captions and frames make up the rest.

Found:
- The first version, before the coordinator's answer at 02:53, kept the
  chooser as it was. It still changed the boundaries, because
  `file_cut`'s `level >= 2` sends level 3 to the file rule. The eager
  pop alone added only 2 cold regions (466), and the `space` row was
  unchanged.
- A paragraph continued after a display (§1200, `resume_after_display`)
  is at `nest_ptr = 1` in horizontal mode with an empty list, but no
  `new_graf` ran, so it is not a start. Adding it is one line if it is
  wanted.
- `cur_level = level_one` would have kept out everything inside
  `\begin{document}`'s `.aux` read and inside any environment. It went
  with the coordinator's "fixed group context" decision.

Rejected:
- A per-trace level field in `Trace`, for the diagnostics: `Trace` is
  saved in the store, so its format would change for a diagnostic. The
  levels live in the machine's `Census`, by exit boundary, instead.
- Emitting the candidates that only the eager pop exposes as level 0,
  to keep the boundaries in the first version: that was moot after
  option (a).

Gate: `scripts/sandbox cargo xtask check` passed (03:07–03:12, 5 min
13 s). It ran fmt, clippy with and without `trace`, the wasm32 check and
the workspace tests, and e2e 34/34 identical in both modes, including
`machine_edits`, `machine_edits_dvi` and `modern_watch_sanitized`.

Time: about 28 minutes to the question (02:13–02:41), then about 21
minutes after the answer (02:53–03:14), including the gate.

### Merge of Task 2–3 and the course sanitizer (coordinator, 03:15–03:24)

`78a3ad5` merged into master as `0039ae7`, on top of Task 4 (`4714e1a`):
the pop at `big_switch` now calls Task 4's `end_token_list`, which ends
an input level's hold on its list. The LOG conflict was two appended
entries, kept both in time order. The gate passed on the merge
(`scripts/sandbox cargo xtask check`, e2e 34/34 identical in both
modes). The course sanitizer, `PARTEX_MACHINE_SANITIZE=1` with
`bench/edits.sh` on a separate probe copy (`~/code/tmp/course-master`),
one process per edit in parallel, passed on `space`, `word`, `footnote`
and `label`. The counts are the agent's to the command (32,870,
1,454,185, 1,847,899, 73,989 re-run; 1, 9, 19, 3 dirty regions of 568),
so Task 4 underneath changed no boundary.

A binary copied for such runs must keep the name `partex`. Under another
name it runs as a compat program of that name (`compat::select` takes
argv[0]'s file stem, as for a `pdftex` link), so `--compat=pdftex` is
not an option there: the usage message, exit 2.

Phase 1 is done. Planned 150 agent minutes for T0–T4, spent 144.

### Task 5a: parameters in a stack arena, built as a pool of argument lists (Agent, branch cuts)

Macro arguments were ordinary token lists (§388–§392). `macro_call`
allocated one per argument (`tok_new`, then `alloc`: a free id, a
generation bump, `live`) and stored each token through `get_mut`.
`end_token_list` freed them (`flush_list`, then `release`: the intern
table, the fields, the free ids). On the course that is 59M lists per
pass. The goal was to take arguments out of the store's `alloc`,
`release` and `get_mut`, so that a later changed-id list in `get_mut`
does not cost one test per argument token.

Built: a pool in the store, not an arena beside it (`tok.rs`,
`expand.rs`, `input.rs`, `memo.rs`, `main.rs`).
- `Tex::arg_new` takes an empty list from `TokStore::arg_free`, marking
  it changed once, or allocates one if the pool is empty.
- `Tex::arg_flush` clears the list and gives it back, still allocated.
  It is called where arguments were flushed: `end_token_list` (§324),
  the scratch list after scanning, `runaway_argument` (§396), and a
  replayed memo entry.
- `Tex::arg_push` stores a token without marking the list. That is safe
  because the list was marked when it was taken, and no snapshot is
  taken while `macro_call` scans: snapshots are taken at boundaries in
  `main_control`. A `debug_assert` checks the mark.
- `arg_free` is part of the store: cloned, persisted and compared
  (`PartialEq`) like `free`, so a restored run takes the same ids as a
  cold one.
- `PARTEX_PARAM_ARENA=0` (`set_arg_pool`) allocates and frees as before.

Why a pool and not an arena. A parameter level is read by list id,
like any token list: the token path (`tok_and_next`), `bulk.rs`'s runs,
`show_context` (§314), the memo's `memo_peek` and `memo_finish` (through
`param_stack`), and the state hash's "input" section. An arena outside
`lists` would need either a branch per token on the level's type or a
second reader everywhere. A pool keeps every reader and the checkpoint
form as they were: a live argument at a checkpoint is a list frozen by
content, as before, and the `Rest` hash still hashes `param_stack`'s
lists by content.

Measured 2026-09-28, 03:05–03:35, on `~/code/tmp/course-ap-t4`.
Release binaries, sandbox, 24 cores, load average about 4. "Before" is
the Task 4 binary (master `4714e1a`'s code).

| plain pdflatex pass, `perf stat` | instructions | cycles |
|---|---:|---:|
| before | 261.773 G | 128.6 G |
| pool, `arg_new` and `arg_flush` inlined | 262.101 G (+0.13%) | 128.6 G |
| pool, both out of line (kept) | 260.544 G (-0.47%) | 129.6 G |
| kept, `PARTEX_PARAM_ARENA=0` | 262.334 G (+0.21%) | 150.0 G (load) |

- Inlined, the pool cost more than it saved. An instruction profile
  (`perf record -e instructions:u`) showed `alloc` and `release` down
  1,750 samples of 2M instructions, and `end_token_list` up 1,764: with
  the flush inlined, every call of `end_token_list` saved more
  registers. Out of line, as `release` was, the pool saves 1.23 G
  instructions.
- The PDF is identical to before's apart from `/ID`, which depends on
  the output path. That holds with the switch on and off.

A snapshot's commit, on the cold machine build (464 cuts), counted with
a temporary counter since removed:

| | lists compared | lists frozen anew |
|---|---:|---:|
| pool off | 423,751 (913 per cut) | 220,540 |
| pool on | 438,048 (944 per cut) | 213,590 |

The count did not fall. Taking and freeing an argument's list mark it
changed, so a commit still compares the lists used since the last one.
It finds most of them empty, as frozen, so only 3% fewer are frozen
anew. An earlier probe classified the 913 lists of Task 4 by their last
use: 417 per cut had been arguments, 392 `back_input` lists (§325, one
single-token list per call), and 103 others. Arguments and backed-up
lists share the store's free ids, which are reused last in, first out,
so the ids that change between two cuts are mostly recycled short-lived
lists of either kind. What would make the count fall is a commit that
skips unchanged pool lists: it would refreeze only the lists that are
live at the commit (`param_stack[..param_ptr]` and `arg_list`) or were
live at the previous one. A clone taken without a commit, and the state
hash's `hash_of`, would then have to know those lists too, which does
not fit in the time left for this task. `back_input`'s lists are the
other half of the count.

`bench/edits.sh`, rows `space`, `label`, `footnote` and `word-local`,
run with the kept form plus the commit counter:

| edit | rebuild ms (Task 4 → 5a) | commands re-run | cuts | restores (n, ms) |
|---|---:|---:|---:|---:|
| space | 198 → 218 | 76119 → 76119 | 15 → 15 | 1, 25.5 → 1, 28.6 |
| label | 492 → 433 | 237937 → 237937 | 31 → 31 | 3, 44.4 → 3, 41.4 |
| footnote | 2790 → 2708 | 2191391 → 2184433 | 155 → 151 | 8, 88.3 → 8, 103.6 |
| word-local | 196 → 197 | 76119 → 76119 | 15 → 15 | 1, 25.3 → 1, 24.5 |

The four PDFs are byte-identical (`cmp`) to Task 4's. The times are
noise (±7%). The footnote rebuild re-ran 6,958 fewer commands and made
4 fewer cuts: a cutoff came earlier. I have a guess at the cause but did
not check it. Eqtb words hold list ids, and a macro's cell compares its
id. Arguments no longer draw on the free ids that definitions use, so an
edit that changes the number of macro calls moves fewer definitions to
other ids.

Rejected:
- An arena outside `lists`, read through an offset carried by the input
  level. That needs a branch per token on the level's type, or a second
  reader in the token path, `bulk.rs`, `show_context` and the memo.
- Parameter slots fixed by `param_stack` position. `param_ptr` can fall
  while arguments are being scanned (levels end inside `get_token`),
  so a slot's list could be the one being filled.
- A per-token write without the mark for every list. It is safe only
  for lists that no snapshot can see mid-write.

Time: about 35 minutes (03:01–03:36), including the gate (passed: e2e
34/34 in both modes, trip and etrip in both modes). Most of it went on
the measurement: why inlining made it slower, and the commit counts.

### Merge of Task 5a and the course sanitizer (coordinator, 03:38–03:48)

`2eafb7e` merged into master as `adc56ab`, on top of Task 2–3's clean
cuts; the LOG conflict was two appended entries, kept both in time
order. Reviewed: a parameter's list is read only by copying (the memo,
`memo.rs:934`, `:1170`) or through a PARAMETER level (§359,
`scanner.rs:223`), which sits above its macro's level and so ends
before it, so the list is never read when §324 returns it to the pool.
The gate passed on the merge (e2e 34/34 identical in both modes). The
course sanitizer on `space`, `word`, `footnote` and `label` passed
(03:47). Its counts, against Task 2–3's alone:

| edit | commands re-run | dirty regions / 568 | cuts |
|---|---:|---:|---:|
| space | 32,870 (same) | 1 (same) | 9 |
| word | 1,454,185 (same) | 9 (same) | 44 |
| footnote | 1,813,966 (1,847,899) | 18 (19) | 133 (136) |
| label | 40,056 (73,989) | 2 (3) | 16 (19) |

So the pool changes what is dirty, not only the cost: `label` loses a
dirty region. This supports the agent's guess from its own harness
(`footnote`, 155 → 151 cuts). Arguments no longer take the free ids
that definitions draw from, so after an edit more definitions get the
same list id as in the old run, and fewer eqtb words differ. A
definition's word holds its list's id. A word whose list is equal by
content but has another id reads as a change, unless the body was
interned to the same id. That is a dependency on allocation order, not
on content. Not fixed here; noted for the checkpoint work (7.16.3),
where ids should be stable or compared by content. Not yet measured how
many dirty regions it accounts for (`PARTEX_MACHINE_PARTS=7` would
show).

Why Task 5b goes after the scans and not after the per-cut count:
`TokStore::frozen_now` (`tok.rs:202`) and `commit` (`tok.rs:229`) each
walk every list, and `TokStore::clone` copies `free`, `interned` and
`arg_free` whole. Those are O(all lists) per cut, while the 944 lists
compared per cut are O(1) each, most of them empty.

### Task 14: the page builder as a cell (Agent, branch units)

Started from `0039ae7`, which merged Task 2–3 (`78a3ad5`) on top of
B's Task 4.

**Follow-up to Task 2–3 first.** `resume_after_display` (§1200) now
sets the paragraph-start flag where it calls `build_page` at
`nest_ptr = 1`, as `new_graf` does. A paragraph that goes on after a
display has a start again. The coordinator decided this at 03:16: the
predicate only chooses where cuts may fall, and `Rest` gives the
correctness. `clean_points_are_level_3` gained a case. `$$ $$` with
`\nullfont` gives "Insufficient symbol fonts" and an empty display, and
the paragraph after it still has its start. On the course, paragraph
starts go from 1,212 to 1,589. All levels 0–3 are now 0, 142,035, 450
and 53,317, with 51,728 outer clean points; regions are cut at 0, 197,
101 and 270 of them; the cold build has 569 regions.

**The cell.** `MCell::Page` is the page builder's state (`Tex::page`,
the `Builder` of `partex-engine`, §980–§982). It sorts right after
`Positions`, so it comes after `Rest`: setting `Rest` keeps the current
page (`restore_rest` with `keep_eqtb`), and setting `Page` after it
replaces it.
- Its value, `V::Page(PageValue)`, is the builder without its list plus
  the list in 64-node chunks. When a snapshot of the state exists, which
  is at every cut, the chunks are that snapshot's, which `Lists::share`
  already shares with the previous snapshot. So a page that only grew
  shares all its full chunks.
- Its version, `page_version`, hashes every field: `contents`, the list
  (the boxes by content, through the nodes' `Hash`, as `Rest`'s "page"
  section did), `so_far`, `max_depth`, `least_cost`, `best_break`,
  `best_size`, `ins`, `insert_penalties`, `last` and `discards`.
- Each snapshot computes the version once (`SnapBody::new`, also on a
  load from the store). That replaces the page hash `Rest` computed at
  every snapshot, so a cut costs the same.
- With the switch on (the default), `Rest` in machine mode leaves the
  builder out (`statehash.rs`, section "page": `PAGE_CELLS`, served
  scope only). `leaves_rest(Page)` is true, the digest adds the page's
  version after `Rest`'s, and the store saves `V::Page` (tag 17). No
  other format changed.
- `PARTEX_MACHINE_PAGE_CELL=0` gives the old `Rest` hash exactly: the
  old tuple is hashed as before.

**Reads and writes.** Every engine access to the builder now goes
through `Tex::page()` (a read) or `Tex::page_mut()` (a write, which
also reads). Each calls a new `Tracker::page_access` hook, which is
empty for every tracker but `CellTracker`. `CellTracker` keeps one byte
(`READ` if read first, `WRITTEN`), cleared at the region's end.
`prepare_cut` turns it into a guard on the entry's version and a write
of the exit's value. The access points, by section:
- §994 `build_page`: the page is taken out and put back, once per call,
  after the early return for an empty contribution list or an active
  output routine. So a call that has nothing to contribute does not
  touch the page.
- §1012–§1023 `fire_up`: the insertions to empty (§1018); the builder's
  own `fire_up` (the break, the marks, the insertion boxes, `last`
  reset); `release_held` and `page_disc` cleared in the default output
  (§1023).
- §1026, the end of an output routine: `insert_penalties := 0`, the
  held-over list and `page_disc`.
- §1105–§1106 `delete_last` in vertical mode with an empty list:
  `last_glue`.
- §1110, e-TeX `\pagediscards`: the list taken.
- §1054 `its_all_over`: whether the page is empty.
- §419 `\insertpenalties`, §421 `\pagegoal` … `\pagedepth`, §424
  `\lastpenalty`, `\lastkern`, `\lastskip` and `\lastnodetype` at the
  outer level with an empty list: reads.
- §1245 `alter_page_so_far` and §1246 `alter_integer`'s
  `\insertpenalties`: writes.
- §986 `show_page_status`, from `show_activities` (§218): reads. It now
  shows a copy of the list instead of taking it out and putting it back.

Not tracked, because it is the machinery and not the engine: the state
hash (`statehash.rs`, and the debugging `page_skeleton_hash`), the
format (`save_state.rs`), the snapshots' `Lists::take` and `put`, and a
unit test. `\vsplit` (§977) never touches the builder; its discards are
`split_discards`, still in `Rest`. `\deadcycles`, the marks
(`cur_mark`), `\outputpenalty` and the contribution list are not in
`Builder`: they stay in `Rest` or eqtb for Task 15. In this task a
region that touches any field reads and writes the whole cell.

**Restore and replay.** `set(Page)` assigns the builder. `set(Rest)`,
through `restore_rest` with `keep_eqtb`, keeps the running engine's
page, since the cell is set after it if the region wrote it.
`replay_exit` restores the old run's exit snapshot, page included. If
the page differs in D, it is set from D after that, like every other
changed cell. `known` stays true because `Page` leaves `Rest`.

**Tests.** Two new unit tests:
- `the_page_leaves_rest_as_a_cell_of_its_own`: a change to the page
  leaves `Rest`'s hash alone and changes `Page`'s version; setting the
  cell back restores it; a snapshot's value equals the running engine's.
- `the_tracker_sees_the_page_read_and_written`.

The store's round-trip samples gained a `Page` cell and a `V::Page`
value. The first gate run found a race between two tests: the
positions test switches the process-wide `POSITION_CELLS` while the new
test hashes `Rest`, and `line_stack`'s hashed length depends on it. The
two tests now take a spin lock (`Switches`) held by any test that
switches or depends on those globals.

**Numbers.** Measured 03:39–03:46 on the course probe copy with
`bench/edits.sh`. Conditions: release binary from `0039ae7` plus this
change; in process; one pass per edit; 24 cores; load average 5–9 (the
other agent running).

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 151 | 32870 | 1 / 569 | 9 | 1, 27.4 | 76.6 |
| word | 1631 | 1454185 | 9 / 569 | 44 | 3, 44.9 | 76.0 |
| label | 252 | 73989 | 3 / 569 | 19 | 2, 36.4 | 68.1 |
| footnote | 2701 | 2106940 | 20 / 569 | 140 | 8, 96.1 | 69.5 |
| enter | 1994 | 1722276 | 17 / 569 | 87 | 3, 47.8 | 70.2 |
| word-local | 140 | 32870 | 1 / 569 | 9 | 1, 24.5 | 68.9 |

With `PARTEX_MACHINE_PAGE_CELL=0`, the same binary gave these (for
`enter`, `PARTEX_MACHINE_PARTS=7` lists the same dirty regions by
index):
- `space`: 131 ms, 32,870 commands, 1 / 569;
- `footnote`: 2,934 ms, 2,106,940 commands, 20 / 569;
- `enter`: 1,967 ms, 1,722,276 commands, 17 / 569.

So the cell changes no count. `footnote`'s rise from Task 2–3's
1,847,899 commands (19 / 568) comes from §1200's new starts, which
moved the cuts, not from the cell. Cold build: 61.3–66.8 s over the six
runs, and 65.2 s with the switch off, under a load of 5–9. That is not
distinguishable from Task 2–3's 57–61 s at a lower load.

**Why nothing moved: `word`'s dirty regions.** With
`PARTEX_MACHINE_PARTS=7`, region 353 reads the edited line, and regions
354–359 are dirty through `Rest` only, not `Page`. Region 357 also
reads sealed lines. A second run with `PARTEX_MACHINE_PARTS=4` and
`PARTEX_MACHINE_PROBE_ALL=1` compares the edited run's state with the
old one at each old region entry. It shows which part of `Rest`
differs:
- Regions 354–357: `pdf`, part `last_link`. The page is equal. Line 32
  has a `\Cref{fig:con-penalty}` after the edited word, so it makes a
  `\pdfstartlink`. With virtual objects, its object number is seeded
  by the step's position (`vseed = at()`), which hashes `cur_input.loc`.
  The edit ("expensive" → "dear") moved the link five columns left, so
  `\pdflastlink`'s value (`pdf.last_link`) differs. It stays different
  until the next link sets it again. This is cause 4 of §7.16
  (positions): the fix is to name a virtual object by a position that
  an edit to the left of it on the line does not move. That belongs to
  phase 4, not to the page.
- Regions 358–359: `pdf`, part `ship`, and `Glyphs`. A page with the
  changed word was shipped, so the writer's state differs, as it must.
- Then 485 (`Numbering`) and 568, the end (`Glyphs`, `Numbering`), as
  in Task 0.

`enter` shows the same thing: 364–371 are dirty through `Rest` (365
also reads an object), none through `Page`. `footnote` has regions dirty
through `Rest` too (354–358, 429, 455–456, 486). The rest are through
`\@gtempa` and a register above 255 (the footnote counter). So after a
reflow, no mid-paragraph region was dirty through the page, both
before this change and after it. `Rest` differs in the PDF writer's
state, and there the page cell cannot help. This answers the
coordinator's question for `word` and `enter`: not yet. The page is
out of `Rest`, but the next thing a reflow changes is the writer's
object naming and ship state.

**Sanitizer.** `PARTEX_MACHINE_SANITIZE=1` on the course passed for
`space`, `word` and `footnote` (03:34–03:37, three runs in parallel, so
their times mean nothing). The counts were the same as in the timed runs.

**Gate.** `scripts/sandbox cargo xtask check` passed at 03:54–03:59 in
5 min 13 s. It ran fmt, clippy with and without `trace`, the wasm32
check, the workspace tests, and e2e 34/34 identical in both modes,
including `machine_edits`, `machine_edits_dvi` and
`modern_watch_sanitized`. The first run (03:48) failed only on the test
race described above.

Time: 03:16–04:00, about 44 minutes, including the §1200 follow-up.
The two sanitized-and-diagnostic rounds and the two gate runs took
most of it.

Rejected:
- A separate flag in `Tex`, as `fonts.order_read` is: AGENTS.md wants
  state read through accessors that call the tracker, so the hook is on
  the `Tracker` trait, a no-op for every other tracker.
- `Cell::Page` in `track.rs`'s `Cell` enum: every tracker would have
  received it, including the session's dependency recorder, which would
  have changed watch mode outside the machine.
- Treating a write that does not read (`\pagegoal=…`) as a write only:
  kept coarse on purpose. Task 15 splits the observables.


### Merge of Task 14, and the order after it (coordinator, 04:02–04:11)

`fba2b86` merged into master as `02664e7`, on top of Task 5a's pool. The
LOG conflict was two appended entries, and both were kept in time order.
The gate passed (e2e 34/34 identical in both modes). The course
sanitizer on `space`, `word`, `footnote` and `label` passed (04:10).
Counts on the merge, 569 cold regions:

| edit | commands re-run | dirty regions | cuts |
|---|---:|---:|---:|
| space | 32,870 | 1 | 9 |
| word | 1,454,185 | 9 | 44 |
| footnote | 2,070,065 | 19 | 133 |
| label | 40,056 | 2 | 16 |

`footnote` is 2.07M on the merge, against 1.81M after Task 5a and 2.11M
on Task 14's branch alone. The §1200 starts move its cuts. That is
being measured (Task 14b, item 6).

*The order changes.* Task 14 showed that the fold (7.16.2) is not what
keeps `word` dirty. Its regions 354–357 differ only in `Rest`'s
`pdf.last_link`: a link's virtual object number, seeded by an input
position that the edit shifted by five columns. So before Task 15 comes
Task 14b: pdfTeX §447's `\pdflast*` values (`last_obj`, `last_xform`,
`last_ximage`, `last_ximage_pages`, `last_ximage_colordepth`,
`last_annot`, `last_link`, `last_x_pos`, `last_y_pos`, `retval`) leave
`Rest` as cells. Their one reader is `last_item` (`expr.rs:410–421`),
and each has a setter. It is the observables pattern of 7.16.2 applied
to the PDF writer.

Rejected: seeding virtual numbers by a position the edit does not move.
It would fix `last_link` and leave the next value derived from a
position, while a cell read where it is read covers all of them. The
same task sorts `footnote`'s dirty regions by cause, and that decides
whether the fold or positions come next for agent A.

*Task 5b's first attempt* (agent B, 03:40–04:05, not in the tree): a
changed-id list on every write to a token list cost +2.70% on the plain
pass. Its entry, with the first measurement of what a cut costs piece by
piece, lands with Task 5b. The decision, made 04:06: short-lived lists
(`back_input`'s, arguments) are pooled and handled by liveness rather
than by change tracking, and the changed-id list goes on long-lived
lists only.

### Task 14b: the `\pdflast*` observables as cells (Agent, branch units)

Started from `02664e7`, which merged Task 14 (`fba2b86`) on top of B's
Task 5a.

**Why.** Task 14's diagnosis of `word` found regions 354–357 dirty
through `Rest`'s `pdf.last_link`, and the page was not involved. A
`\Cref` after the edited word makes a `\pdfstartlink`, whose virtual
object number is seeded by the step's position, and the edit moved the
link five columns. Nothing in those regions read `\pdflastlink`. The
coordinator's decision (04:02): make the value a dependency where it is
read, not a new seeding. Re-seeding would only move the problem to the
next position-derived value; cells read where they are read cover all
of them.

**The cells.** `MCell::PdfLast(k)` is one cell per value of pdfTeX
§447's `last_item` codes (`pdf::PdfLast`, 0–9):
- `last_obj`, `last_xform`, `last_ximage`, `last_ximage_pages`,
  `last_ximage_colordepth`;
- `last_annot`, `last_link`, `last_x_pos`, `last_y_pos`;
- `retval`.

A region that reads `\pdflastlink` is guarded on that value only. The
version is the value's hash and the value is `V::Int`. The cell sorts
after `Page`. The store adds `PdfLast(a0)` at the end of the cell enum;
no value tag is new. Every access goes through `Tex::pdf_last` (a read)
or `Tex::set_pdf_last` (a write, which does not read), which call a new
`Tracker::pdf_last_access(k, write)`. `CellTracker` keeps a read-first
bit and a written bit per value.

Readers, checked with grep for every field:
- `last_item` (`expr.rs`, pdfTeX §447): `\pdflastobj`, `\pdflastxform`,
  `\pdflastximage`, `\pdflastannot` and `\pdflastlink` read the value
  and then `final_num`. `final_num` goes through `ObjTab::numbering()`,
  which sets `log.forced`, so the read of `Numbering` is recorded as for
  every other virtual-number reader. `\pdflastximagepages`,
  `\pdflastximagecolordepth`, `\pdflastxpos`, `\pdflastypos` and
  `\pdfretval` read the value only.
- `\immediate\pdfobj` and `\immediate\pdfximage` (`maincontrol.rs`, pdfTeX
  §1623), and `\immediate\pdfxform` (`pdf_immediate_xform`,
  `ship.rs`): each reads the value its own command just wrote, so it is
  a read after a write and gives no guard.
- `\pdfrefobj`, `\pdfrefxform`, `\pdfrefximage` and the annotation and
  link commands scan a number and read no `last_*` field. The outline
  code's `last_outline` and `\pdfmatch`'s `last_match` are other fields
  and stay in `Rest`.

Writers:
- `\pdfobj`, with and without `reserveobjnum`, and `useobjnum`'s error,
  which sets `retval` to −1 (`pdf/ext.rs`);
- `\pdfxform`, `\pdfannot` (with and without `reserveobjnum`) and
  `\pdfstartlink` (`pdf/ext.rs`);
- `\pdfximage` (`pdf/image.rs`), which sets the image and its pages and
  colour depth;
- `\pdfsavepos` during shipout (`pdf/ship.rs`), which sets `last_x_pos`
  and `last_y_pos`. Nothing made a plain cell awkward there: the ship
  and the `\write`s that read the position run in one region, as a read
  after a write.

**Out of `Rest`, and the switch.** With the switch on, the machine's
`Rest` hashes the PDF state through `pdf::WithoutLast`. It destructures
every field, so a new field fails to compile until it is placed, and
skips the ten values. The digest adds their versions. `set(Rest)` keeps
the running engine's values, and `set(PdfLast(k))` assigns one.
`PARTEX_MACHINE_PDF_LAST=0` hashes `pdf` whole, as before.

**Tests.** `the_pdf_last_values_are_cells_of_their_own` checks two
things. The tracker: a write is not a read, and a read after a write
is not a read. The cell: a changed `last_link` leaves `Rest`'s hash
alone and changes the cell, and setting the cell back restores it. The
store's samples gained a `PdfLast` cell.

**Numbers.** Measured 04:13–04:20 with `bench/edits.sh` and
`PARTEX_MACHINE_PARTS=7` on the course probe copy. Conditions: release
binary from `02664e7` plus this change; in process; one pass; load
average 4.5–6.6. The cold build took 58.3–62.7 s over the six runs.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 149 | 32870 | 1 / 569 | 9 | 1, 27.1 | 70.0 |
| word | 897 | 718176 | 6 / 569 | 33 | 4, 56.8 | 66.1 |
| label | 216 | 40056 | 2 / 569 | 16 | 2, 36.9 | 67.2 |
| footnote | 2603 | 2070065 | 19 / 569 | 133 | 8, 116.6 | 68.1 |
| enter | 1903 | 1722276 | 17 / 569 | 87 | 3, 47.5 | 65.9 |
| word-local | 140 | 32870 | 1 / 569 | 9 | 1, 25.1 | 67.9 |

With `PARTEX_MACHINE_PDF_LAST=0`, the same binary gave:
- `space`: 131 ms, 32,870 commands, 1 / 569, 9 cuts;
- `word`: 1,590 ms, 1,454,185 commands, 9 / 569, 44 cuts, which is
  Task 14's `word` exactly.

So the switch alone makes the difference.

**`word` now:** 718,176 commands in 897 ms, from 1,454,185 in 1,631 ms.
Regions 354–356 are clean. The six still dirty, with their commands
(the re-executed spans of `Stats::spans`):

| region | commands | dirty through |
|---:|---:|---|
| 353 | 259,196 | the edited line (31) |
| 357 | 32,870 | 9 sealed lines: the page that ships the reflowed paragraph |
| 358 | 33,933 | `Rest` (the PDF writer's ship state after that page) |
| 359 | 25,318 | `Rest` (the same) |
| 485 | 360,212 | `Numbering` (the `\Cref` region of §7.15) |
| 568 | 6,647 | `Glyphs`, `Numbering` (the end) |

The coordinator expected 354–357 to go clean. 357 did not: it is the
ship of the reflowed page, which reads the paragraph's changed sealed
lines, so it is a true dependency. What `word` costs next is two
regions: 353 (259k, the edited line's region, whose size is cut
placement) and 485 (360k, `Numbering`).

`label` dropped to 40,056 commands and 2 dirty regions. They are 357,
the edited line, and 568, the end, which reads `Written(2)` (the
`.aux`). Before, it was 73,989 commands and 3 regions.

**`footnote`, diagnosed (no fix).** It has 19 dirty regions and
2,070,065 commands. With `PARTEX_MACHINE_PARTS=7`, and a second run
with `PARTEX_MACHINE_PARTS=4` and `PARTEX_MACHINE_PROBE_ALL=1` for the
parts of `Rest`:

| through | regions | commands |
|---|---|---:|
| the edited line | 353 | 262,293 |
| `Rest` only | 354, 355, 356, 357, 486 | 861,441 |
| `Rest` and eqtb | 457, 458 | 58,975 |
| eqtb only (`\@gtempa`, a register above 255, the footnote counter's neighbours) | 407, 411, 412, 423, 428, 429, 454, 456, 567 | 520,497 |
| `Numbering` (485), and `Glyphs` and `Numbering` (568, the end) | 485, 568 | 366,859 |
| `Page` | none | 0 |

The parts of `Rest` that differ at 354–357 are:
- "tables", the save stack: the footnote's local assignments are saved
  inside an open group;
- `pdf`, in `objs`, `out` and `ship`: the footnote's hyperref objects
  and the shipped page;
- at 356–357, also "scalars: cur_val".

At 486, and at many regions that are dirty through eqtb anyway (403,
410, 425, 428, 453, 455), only `cur_val` differs. `cur_val` is the
last value scanned, a scratch register, and here it is the footnote
counter shifted by one. Whether it is dead at every clean point is a
question for a later task. If it is, leaving it out of `Rest` makes 486
clean (92,562 commands).

So for `footnote`, the fold (Tasks 15–18) would not help yet: no region
is dirty through `Page`. What blocks it is the save stack and the PDF
writer's objects and ship state in `Rest`, eqtb values the footnote
really changes (the counter and `\@gtempa`, true dependencies), and
`Numbering`.

**The footnote regression of Task 14 (1.85M → 2.11M commands).** I
compared the spans of Task 2–3's run (19 regions, 1,847,899 commands)
with Task 14's (20 regions, 2,106,940). The difference, 259,041
commands, is exactly two changes after region 453:
- +233,891: one dirty region became larger. Task 2–3's 453 had 17,993
  commands; Task 14's corresponding region, 454, had 251,884.
- +25,150: one extra dirty region, Task 14's 455.

Everything else is identical region for region. So it is mostly cut
placement. §1200's extra starts moved a cut, and the next region ran
251,884 commands before it met a candidate the chooser takes. Past 16k
commands the chooser takes a level-2 or level-3 candidate, and past
131k any candidate. So it met no level-2 or level-3 candidate after
16,384 commands and, apparently, no file command at all between about
131k and 252k (a long stretch run from token lists). I did not check
that directly. This is Task 13's policy, not a real dependency.

**Sanitizer.** `PARTEX_MACHINE_SANITIZE=1` on the course passed for
`space`, `word` and `footnote` (04:10–04:13, three runs in parallel),
with the same counts as the timed runs.

**`enter`** (no fix): 1,722,276 commands, unchanged. Its regions
364–371 are dirty through `Rest`; the probe shows `pdf` differing in
`objs` and `ship` after the split paragraph's page is shipped (and in
`last_link` at 364, now a cell). The writer's ship and object state is
the next piece of `Rest` that keeps a reflow dirty, for `word`'s
358–359 as well.

**Gate.** `scripts/sandbox cargo xtask check` passed at 04:20–04:26 in
5 min 25 s. It ran fmt, clippy with and without `trace`, the wasm32
check, the workspace tests, and e2e 34/34 identical in both modes,
including `machine_edits`, `machine_edits_dvi` and
`modern_watch_sanitized`.

Rejected:
- Seeding virtual object numbers by a position an edit does not move:
  the value's readers are what should depend on it (above).
- One cell for all ten values: a `\pdflastxpos` reader would then be
  guarded on the last link.
- Keeping `last_x_pos` and `last_y_pos` in `Rest`: their writer runs in
  the ship-out, but the ship and the readers of the position are in one
  region, so a plain cell is exact.

Time: 04:02–04:27, about 25 minutes, most of it the measurements and
diagnoses running.

### Merge of Task 14b, and a race between the store tests (coordinator, 04:28–04:44)

`e4604d1` merged into master as `1efe489`. The first gate on the merge
failed in `values_known_from_the_last_save_are_not_written_again`: two
saves of one build gave Merkle roots that differ in one child hash. The
test passed six reruns, alone and with its module, so it is a race and
not a defect of Task 14b. Task 14's positions test flips the
process-wide `POSITION_CELLS` under the `Switches` lock. A test that
hashes `Rest` twice without holding that lock can see the switch change
between its two hashes. `9c81056` makes every store test that compares
two hashes hold the lock. The gate then passed (e2e 34/34 identical in
both modes). The course sanitizer passed on `1efe489`'s binary (the
code is the same) for `space`, `word`, `footnote` and `label`, at
32,870, 718,176, 2,070,065 and 40,056 commands re-run, the agent's
counts.

Rejected: an integration-test binary for the switch-flipping tests.
The separate process would remove the race, but those tests need crate
internals. A lock around each test that compares two hashes is local
and says why.

Against the Task 0 baseline, after Task 14b, course, one pass:

| edit | commands re-run, Task 0 → now | rebuild ms (loaded machine) |
|---|---:|---:|
| space | 76,119 → 32,870 | 194 → 149 |
| word (reflows) | 1,457,463 → 718,176 | 1,478 → 897 |
| label | 237,937 → 40,056 | 444 → 216 |
| footnote | 2,191,391 → 2,070,065 | 2,643 → 2,603 |
| enter | 1,727,637 → 1,722,276 | 1,834 → 1,903 |

### Task 14c: dead state leaves `Rest`, diagnosis only (Agent, branch units)

Started from `1efe489`, which merged Task 14b (`e4604d1`). The task was
to canonicalize the fields of `Rest` that are dead at a clean point.
The coordinator's instruction was to stop after the diagnosis and
classification if most of the difference turned out to be live state or
`pdf.out`. Most of it is live, so this entry has the table and no
canonicalization.

**Method.** I added diagnostics, no behaviour change, to what
`PARTEX_MACHINE_PARTS=4` prints for each region whose `Rest` differs
(`machinehost::parts_detail`):
- the scanner's scalars that differ, with both values
  (`Tex::scalars_difference`);
- where the save stack differs: pointers, level, group, boundary and
  the first entries, with the location an eqtb entry saves
  (`Tex::save_stack_difference`);
- the PDF writer: which fields of `ship` differ, which object-list
  heads (`head_tab`, by type), and in `out` also the relocations of
  virtual numbers in the pending bytes.

I ran the probe (with `PARTEX_MACHINE_PROBE_ALL=1`) on `word`, `enter`
and `footnote` on the course, release binary from `1efe489` plus these
diagnostics, 04:32–04:44. The regions are those still dirty through
`Rest` after Task 14b.

| field | regions | class | why |
|---|---|---|---|
| `pdf.ship.link_list` | `word` 358–359; `footnote` 354–357 | (a) dead | the page's link annotations: filled during a page ship, read only at that page's end (`/Annots`, `write_page_marks`), and emptied as the next page ship begins (pdfTeX §752, `pdf_link_list:=null`; `pdf_ship_box_out`) |
| `pdf.ship.dest_list` (and `annot_list`, same shape) | `footnote` 354–357 | (a) dead | the same, §752 |
| `pdf.ship.pdf_v` | `footnote` 354–357 | (a) dead, to confirm | content-stream position, set by `pdf_set_origin` when text begins; not yet shown to be set before every read in a ship |
| `pdf.out.scaled_out` (by elimination: every other hashed field of `PdfOut` is equal) | `footnote` 354–357 | (a) dead | the last number `pdf_print_bp` printed, read by its caller right after |
| `cur_val` (and `glue_origin`) | `footnote` 356–357, 486 (only there); `enter` 375, 392 (probe only, not dirty) | (a) dead, to confirm | the scanner's output register; to be confirmed by the poison run |
| `pdf.ship.last_pages` | `enter` 364–371 | (c) live | the Pages node being filled: read at every page ship (`/Parent`) and at the end |
| `pdf.objs.head[PAGES]` | `enter` 364–371 | (c) live | the head of the Pages objects' list |
| `pdf.objs.head[DEST]` | `footnote` 354–357 | (c) live | the footnote's hyperref anchor is a new destination: a real object |
| the save stack: 65 entries (354–355), 15 (356–357) | `footnote` 354–357 | (c) live | saved values, restored at the groups' ends (below) |

Why `last_pages` and the Pages head differ, from their values: a Pages
node is created every sixth page (`PAGES_TREE_KIDS_MAX`). With virtual
numbers its name is seeded by the position of the step that creates it,
and after the edit that step is at a different place. The value is a
name. What TeX could observe is its final number, and nothing reads
that between pages. So it is class (c) as the code stands, and a naming
problem (positions, phase 4) underneath.

Nothing in the table is class (b). The ten `\pdflast*` values of
Task 14b were that class; nothing else read by few readers differs
here.

**What canonicalizing class (a) would buy.** A region becomes clean
only if everything that differs in it is class (a):
- `word` 358–359 differ only in `link_list`: 59,251 commands (718k →
  659k);
- `footnote` 486 differs only in `cur_val`: 92,562 commands.

That is 151,813 of the 2,054,493 commands these regions re-run through
`Rest`, about 7%:
- `enter` 364–371 (1,133,781 commands) are dirty through
  `last_pages` and the Pages head;
- `footnote` 354–357 (768,879 commands) through the save stack and the
  dest head.

Both would stay dirty. So I stopped here, as instructed.

**`pdf.out`.** Its pending bytes, buffers and the relocations in them
are equal at every one of these regions. Only `scaled_out` differs, a
scratch number, so flushing the writer at candidates (Task 10) would
not change these counts.

**The save stack in `footnote` (diagnosis, no fix).** At 354–355 both
runs have `save_ptr` 939 at group level 21 (group 14, semi-simple,
boundary 355). At 356–357 they have `save_ptr` 342 at level 19. The
pointers, levels, groups and boundaries are the same in both runs, so
it is not the order of first local assignments. The entries that differ
differ in value: saved meanings of `\reserved@a`, `\reserved@b`,
`\reserved@c`, `\@currenvline`, `\pgfutil@reserved@b` and others, 65
entries at 354–355 and 15 at 356–357.

These four regions are 2–16 steps apart at steps 128,582–128,612. They
are the stops before each `\shipout` inside one output routine, where
LaTeX, hyperref and pgf hold about twenty groups open. They are not
clean points. The saved values are the scratch macros as the footnote's
page left them. They are restored when those groups close at the end
of the output routine, so they are live until then. The cuts here are
the ship stops of §7.0, which DESIGN §7.16.1 keeps ("before
`\shipout`"). A unit that is the whole output routine, as the deferred
fire of §7.16.1 gives, would have no boundary inside these groups.

**Re-plan.** The coordinator decides; these are options:
1. A small task: canonicalize the class-(a) fields listed above
   (`link_list`, `annot_list`, `dest_list`, `scaled_out`, `pdf_v` and
   the ship's other per-ship scratch, `cur_val` and `glue_origin`),
   with the poison test. About 150k commands on `word` and `footnote`.
2. The Pages node's name. Seed it by the page number (pdfTeX's own
   `total_pages`, which `Numbering` already orders) rather than by
   the step's position. That makes `enter` 364–371 converge unless the
   page count changes.
3. The ship stops inside the output routine: the deferred fire (a unit
   per output routine) removes these boundaries, and with them the
   save-stack difference.

No gate numbers change: this task added diagnostics only. The gate,
`scripts/sandbox cargo xtask check`, ran at 04:46–04:51. fmt, clippy,
wasm32 and e2e 34/34 in both modes (`machine_edits`, `machine_edits_dvi`,
`modern_watch_sanitized`) passed. One unit test failed:
`every_cell_and_value_round_trips` ("the starting state differs from
its digest"). That is the switches race the coordinator fixed on master
at 04:40, and three reruns of the suite gave 3 failed, then 0, then 0.
This branch does not have that fix yet, and I did not touch those
tests, to avoid a conflict.

Time: 04:29–04:47, about 18 minutes.

### Task 5b: the token store's cut in O(changes) (Agent, branch cuts)

**Before the change: what a cut costs, piece by piece.** The pieces
were timed with a switch, first called `PARTEX_TOK_TIMING` and kept as
`PARTEX_CUT_TIMING=1` (`machine.rs`, `CutTiming`; printed after the
cold build and after each rebuild; the clock is read only when it is
on). It used a host clock (`CellHost::nanos`) and timed each piece of
`take_snapshot_with` separately. It first ran the token store's
`commit`, then an extra `TokStore::clone` that was dropped, then the
`JVec` commits (eqtb, hash, save stack) and the five `Flat` commits.
Then it timed the normal path: the `Rest` hash (`rest_hash_memo`, whose
commits then find nothing left) and `Snapshot::of` (`Lists::take`,
`share`, and the engine's clone).

Conditions: master `adc56ab` (568 cold regions since Task 2–3), release
binary, the course copy `~/code/tmp/course-ap-t4`. One run of
`bench/edits.sh` with the `footnote` row, measured 03:45–03:52 at a load
average of about 4 (the other agent's runs).

| per cut, µs | cold build (569 cuts) | footnote rebuild (142 cuts) |
|---|---:|---:|
| token store `commit` (`frozen_now` and the flag reset) | 1011 | 585 |
| token store `clone`, after the commit | 152 | 133 |
| `JVec` commits (eqtb, hash, save stack) | 1126 | 239 |
| `Flat` commits (pool, starts, buffer, input and parameter stacks) | 596 | 378 |
| `Rest` hash, the commits done | 766 | 629 |
| `Snapshot::of` (lists, clone, the token store's clone within it) | 768 | 637 |
| the cut, all of it | about 4270 | about 2460 |

The store at the last cut holds:
- 58,460 lists;
- `free` 885 ids and `arg_free` 123;
- an empty `interned` table (no slots: nothing on the course is
  interned).

So the token store's commit and clone are the largest single piece of a
cut, about 27% cold and 29% in the rebuild. The `JVec` commits come
next cold, and the `Rest` hash and `Snapshot::of` in the rebuild. The
commit's millisecond is mostly the two scans. Each reads the flag of
all 58,460 records of 64 bytes, 3.7 MB, twice per cut. On top of that
it refreezes the roughly 940 lists that changed. `free`, `interned` and
`arg_free` cost about a microsecond to clone at these sizes. Sharing
them would save nothing measurable, so step 3 of the task was not done.

**The changed-id list as specified costs 2.7% cold, so it is not in the
tree.** `get_mut` kept its flag and pushed the id when the flag went
from clear to set. `copy_into` did the same. `loaded` listed every id,
and thaws, clones and loads started with an empty list. `commit` and
`frozen_now` iterated the ids, with a debug check that the number of
set flags equals the list's length, and `PARTEX_TOK_CHANGED=0` gave
back the scans. The plain pdflatex pass of the course (`perf stat`,
same conditions as above):

| binary | instructions |
|---|---:|
| master `adc56ab` | 260.545 G |
| changed ids, the test and push inline in `get_mut` | 267.577 G (+2.70%), the same with the switch off |
| changed ids, the push out of line (`#[cold]` `mark`) | 272.934 G (+4.75%) |

The token path did change. The old `get_mut` was one store. The new one
loads the flag, tests it and branches. A plain pass calls it on the
order of a billion times, and a plain pass never takes a snapshot, so
it never clears a flag and gains nothing from the test. An instruction
profile (`perf record -e instructions:u`, first form against master)
puts the growth mostly in `TokStore::release` (+1,143 samples of 2M
instructions), `alloc` (+631), `arg_new` (+237) and `arg_flush` (+120).
Those are the calls that make and free the short-lived lists
(`back_input`'s, the arguments'), each running `get_mut` once or twice.
The per-token pushes (`tok_push`, `copy_into`) grew little. I stopped
there to ask which way to go (03:40–04:05).

**Second round (04:06, the coordinator's decision): short-lived lists
by liveness, the rest by changed ids.**

1. *`back_input`'s lists join the arguments' pool* (§325). A backed-up
   token takes a pooled list (`Tex::pooled_new`, `pooled_push`), and
   `end_token_list` frees it with `flush_tokens`, as before.
   `flush_tokens` now gives any pooled list back to the pool
   (`TokStore::flush`, out of line for the reason Task 5a found). The
   T5a names changed: `arg_new` became `pooled_new`, `arg_flush`
   became `flush_tokens`, and `arg_free` became `pool_free`.
2. *Pooled lists are out of the change tracking.* A list is pooled
   when the pool is empty and a new one is allocated for it
   (`TokList::pooled`, kept in `Frozen` too, so a thaw restores it).
   Taking one, storing into it and freeing it set no flag and record no
   id. `TokStore::pooled` lists every pooled id, and `frozen_now` visits
   them all at each snapshot. That costs O(1) for one free both now and
   at the last snapshot, and freezes one in use by content.
   - Pooled lists can be in use at a cut. A token backed up by
     `\input`'s file-name scan waits under the file, and so do the
     arguments of a macro below it. Visiting every pooled id covers
     both.
   - Two readers of the flag learned about the pool. `hash_of` never
     takes a pooled list's hash from the shadow. `thaw_from` always
     takes a pooled list from the checkpoint, since the running
     store's copy may have changed without its flag.
   - `release` asserts that it never frees a pooled list.
3. *Changed ids for the other writers.* `get_mut` tests the flag and
   pushes the id on the clear-to-set transition. `copy_into` does the
   same for a list that is not pooled. `commit` and `frozen_now`
   iterate the ids.
   - `loaded` lists every id; thaws, clones and loads start with none.
   - `PARTEX_TOK_CHANGED=0` gives back the two scans. The ids are kept
     either way.
4. *Safety net (debug builds).* At each commit, `check_changed` asserts
   two things: the ids are exactly the lists whose flags are set, and
   every list agrees with the shadow just made, as a scan of every list
   would find it.

The plain pdflatex pass of the course (`perf stat`, the conditions of
the first table):

| binary | instructions |
|---|---:|
| master `adc56ab` | 260.545 G |
| `back_input` pooled, pooled lists untracked (1–2) | 260.352 G (−0.07%) |
| and changed ids for the other writers (1–3, kept) | 263.122 G (+0.99%) |
| the same, `get_mut` `#[inline(always)]` | 263.124 G |
| 1–2 and chunk-level dirty bits instead of changed ids, measured only | 262.760 G (+0.85%) |

- The pooling alone is free, and a little cheaper.
- The change tracking for the remaining writers costs 1% either way.
  The ids cost 2.77 G instructions (the flag test on each `get_mut`,
  on the order of a billion calls). The chunk bits cost 2.41 G (an
  unconditional store to a byte per 64 lists, and its bounds check),
  and they still leave a scan of the marked chunks.
- An instruction profile of the ids against 1–2 shows the growth spread
  over `alloc`, `release`, `copy_into`, `pooled_new` and code-layout
  effects in `get_token` and `bulk_group_run`. There is no single site
  to fix.
- Both are above the 0.5% gate. The ids stay in the tree, because
  they make the commit O(changes). The coordinator decides whether 1%
  in a plain pass is the price.

`PARTEX_CUT_TIMING=1`, the `footnote` row, the kept binary (1–3), at
04:26–04:28. Per cut, µs, before → after:

| piece | cold (569 cuts) | footnote rebuild (142 cuts) |
|---|---:|---:|
| token store `commit` | 1011 → 477 | 585 → 259 |
| token store `clone` | 152 → 25 | 133 → 23 |
| `JVec` commits | 1126 → 1084 | 239 → 245 |
| `Flat` commits | 596 → 540 | 378 → 425 |
| `Rest` hash | 766 → 361 | 629 → 357 |
| `Snapshot::of` | 768 → 588 | 637 → 575 |

- At the last cut the store held 58,454 lists, 878 free and 124
  pooled.
- The token store's commit and clone fell from 1.16 ms to 0.50 ms cold,
  and from 0.72 to 0.28 ms in the rebuild. What is left of the commit
  is the refreezing of the lists that changed: a new `Arc` of tokens,
  `list_hash`, and a copy of the chunk they are in, for each of about
  470 lists a cut. It is no longer the scan.
- The `Rest` hash halved too. Its `hash_of` finds more lists unmarked,
  so it takes their hash from the shadow.
- The `JVec` commits (eqtb, hash, save stack) are now the largest piece
  of a cold cut, 1.08 ms of about 3.1 ms. That is T6.

Gates for the kept form, run 04:28–04:41 on `~/code/tmp/course-ap-t4`:
- *Sanitizer.* `PARTEX_MACHINE_SANITIZE=1` on the course passed for
  `space`, `word` and `footnote`. The three ran in parallel, one-line
  edit files through `bench/edits.sh`, with no panic.
- *The switch.* The `space` row with `PARTEX_TOK_CHANGED=0` (the scans)
  gives the same rebuild, 32,870 commands and 9 cuts. Its PDF is the
  same as with the ids apart from `/ID`, which depends on the output
  path.
- *The six rows*, all rebuilt PDFs identical:
  - `space`, `label`, `footnote` and `word-local` are byte-identical
    (`cmp`) to Task 4's;
  - `word` and `enter` are identical, apart from `/ID`, to the same
    rows run with master `adc56ab` (the timing binary).
- *The full gate* (`scripts/sandbox cargo xtask check`) passed: e2e
  34/34 in both modes, and trip and etrip in both modes. The first run
  failed only on two lints, a missing `#[must_use]` on
  `cut_timing_take` and `machinehost::run` over 100 lines. The rebuild
  report moved into `report_rebuild`, and the second run passed.

| edit | rebuild ms | commands re-run | dirty / total | cuts | restores (n, ms) | adc56ab: ms, cuts |
|---|---:|---:|---:|---:|---:|---:|
| space | 127 | 32870 | 1 / 568 | 9 | 1, 25.0 | |
| word | 1773 | 1454185 | 9 / 568 | 44 | 3, 48.5 | 1589, 44 |
| label | 250 | 73989 | 3 / 568 | 19 | 2, 36.0 | |
| footnote | 2229 | 1813966 | 18 / 568 | 133 | 9, 92.9 | 2604 and 2441, 133 |
| enter | 1962 | 1722276 | 17 / 568 | 87 | 3, 45.0 | 2179, 87 |
| word-local | 200 | 32870 | 1 / 568 | 9 | 1, 31.3 | |

The harness ran while the gate did, at a load average of 6–9, so the
times are noisy. The footnote rebuild's cuts cost about 0.6 ms less
each by the timing table, about 80 ms over 133 cuts. That is inside the
noise of a single run.

Time: the second round took about 35 minutes (04:06–04:41) plus the
docs. With the first round (03:40–04:05), the task took about an hour.

**Round 3: the changed ids as tracking (T5c, 04:44).** The
coordinator's decision was to keep neither the 1% nor a store without
ids. Recording which lists changed is tracking, so it belongs to the
`Tracker` (AGENTS.md: "state only through accessors that call the
Tracker"). A plain pass should compile to the old code, and machine
mode should pay the test where it buys cheap cuts.
- `Tracker::CHANGED_IDS`, an associated const, is false by default and
  true in `CellTracker`.
- The writers that mark take the tracker as a type parameter:
  `TokStore::get_mut`, `alloc`, `release`, `intern`, `copy_into`,
  `flush`, `commit`, `thaw` and `thaw_from`, all `::<M: Tracker>`.
  - `get_mut` is `if M::CHANGED_IDS { test, and push on the transition }
    else { store the flag }`. After monomorphization the branch is a
    constant, and `Untracked`'s `get_mut` is the old single store. No
    unstable feature is needed: the const is read in ordinary code, not
    in a const generic argument.
  - Every caller is a `Tex<H, T>` method and passes `T`: 9 sites outside
    `tok.rs` (`bulk.rs`, `prefixed.rs`, `memo.rs`, `overlay.rs`,
    `machine.rs`, `run.rs`) and the ones inside it. The store's own
    `Default`, which has no engine, uses `Untracked`.
- `TokStore::ids_valid` says whether `changed` names every marked list.
  - A commit or a thaw for a tracker with the ids sets it; a new store,
    a format load, a clone and a persisted load leave it clear.
  - A snapshot scans unless it is set, so the first commit of a machine
    run scans once.
  - Non-machine checkpoints (`-watch -checkpoint-every`, whose trackers
    have no ids) always scan and stay exact.
  - `PARTEX_TOK_CHANGED=0` forces the scan in machine mode.
  - The debug check compares the ids with the flags only when the ids
    are valid.

Measured 04:47–04:51, the same conditions as round 2, with seven runs
at once (instruction counts do not depend on load, times do):

| run | instructions | cycles |
|---|---:|---:|
| plain pass, master `adc56ab` | 260.545 G | |
| plain pass, round 3 | 260.347 G (−0.08%) | 141.5 G (load) |
| machine cold build, `adc56ab` (the timing binary, timing off) | 300.707 G | 163.6 G |
| machine cold build, round 3 | 302.042 G (+0.44%) | 160.2 G (−2.1%) |
| machine cold build, round 3, `PARTEX_TOK_CHANGED=0` | 302.466 G (+0.58%) | 163.3 G |

- The plain pass is the pooling's −0.08%. The flag test is gone from
  it.
- In machine mode the ids cost instructions: the test on every
  `get_mut`, minus the two scans they save. But they save cycles. The
  two scans read 3.7 MB each per cut, and the build takes 2% fewer
  cycles with the ids than with the scans or at master.
- The cold builds took 60.7 s (ids), 61.8 s (scans) and 62.1 s
  (master), in parallel with each other.

`PARTEX_CUT_TIMING=1`, `footnote` (named `footnote-t` so as to run
beside the sanitizer), µs per cut:

| piece | cold: round 2 → round 3 | rebuild: round 2 → round 3 |
|---|---:|---:|
| token store `commit` | 477 → 503 | 259 → 326 |
| token store `clone` | 25 → 26 | 23 → 24 |
| `JVec` commits | 1084 → 1092 | 245 → 303 |
| `Flat` commits | 540 → 531 | 425 → 545 |
| `Rest` hash | 361 → 363 | 357 → 414 |
| `Snapshot::of` | 588 → 594 | 575 → 692 |

This matches round 2, within the load of seven parallel runs; every
rebuild piece is up by a similar share. The rows:
- `space` (run as `space-r`) 135 ms, 32,870 commands, 9 cuts;
- `footnote-t` 2600 ms, 1,813,966 commands, 133 cuts.

Both PDFs are identical to Task 4's apart from `/ID`.
`PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word` and
`footnote`:
- `space` and `footnote` are identical to Task 4's PDFs apart from
  `/ID`;
- `word` is identical to master's `word` row apart from `/ID`.
- The full gate (`scripts/sandbox cargo xtask check`) passed: fmt,
  both clippy runs, the tests, e2e 34/34 in both modes, trip and etrip
  in both modes.

A note for T6, from reading `journal.rs`. `JVec`'s write path already
has no test: `index_mut` ORs its chunk's bit into a bitset. So T6's cost
is on the commit side. The commit visits every chunk's bit (512 words a
chunk), and for each dirty chunk it compares 4 KB with the base and
copies 4 KB. The persist branch's 64-word dirty blocks were aimed at the
compare.

Time for round 3: about 11 minutes of change (04:44–04:55) and 4 of
measurement, with the gate running at the same time.

### Merge of Task 5b–5c; list ids decide `label` again (coordinator, 04:55–05:05)

`7d05d43` merged into master as `d7a9ae3`, on top of Task 14 and Task
14b. `track.rs` merged cleanly: `CHANGED_IDS` sits next to the
`page_access` and `pdf_last_access` hooks. The gate passed, e2e 34/34
identical in both modes. The course sanitizer passed on `space`,
`word`, `footnote` and `label` (05:04), at 32,870, 718,176, 2,070,065
and **73,989** commands re-run.

`label` is back at Task 2–3's count, 3 dirty regions instead of 2 after
Task 5a. The only code between is the pooling of `back_input`'s lists.
It changes which free ids the definitions draw, exactly as Task 5a's
argument pool did in the other direction. So a `\label` edit's cost
depends on list allocation order: an eqtb word holds a definition's
list id, and a body equal by content but with another id reads as a
change. Two pool changes moved it by a region each way. That rules out
tuning the allocator as the fix.

*Candidate, not scheduled yet:* definitions named by content. On the
course `TokStore::interned` is empty, so nothing is interned. If every
`\def` body were interned by its content hash (`list_hash`, already
computed), its id would be a function of its content. The same
definition would then get the same id in both runs, whatever was
allocated before it. The questions: the cost at `\def`, which hashes
the body once; ids that are stable across a restore (interned ids live
in the checkpoint); and what `\edef` and `\toks` do.

### Task 14d: virtual names an edit doesn't move (Agent, branch units)

Started from `7579f4c`: my Task 14c diagnostics (`8c1fdd8`), with master
merged in for the store tests' lock.

**Why.** Task 14c found `enter`'s regions 364–371 (1.13M commands) dirty
through `Rest`'s `last_pages` and the Pages list head. With virtual
numbers every object is named by `hash(vseed, vcount)`, and
`vseed = at()`, the step's position. The position includes the line
number and `loc`, so an edit moves it: a word moves the columns after it
on its line, and a new line moves every later line of the file. A name
made from a position then differs in every later region that holds it.
Nothing observes the name itself: TeX sees only the final number,
through `Numbering`.

**Census.** Every object is named at `ObjTab::create`, and before this
change all of them by position. By kind:

| kind (creator) | identity TeX defines | held across regions | now named by |
|---|---|---|---|
| page, `OBJ_TYPE_PAGE` (`get_obj` at a page ship, or earlier from a link or destination to the page) | its number | `head_tab`, `ship.last_page`, the page tree (a `Tree` cell), other objects' entries, `Numbering` | (PAGE, number) |
| leaf of the page tree, `OBJ_TYPE_PAGES` (`write_page_object` when `total_pages % 6 == 1`) | its place: level 0, index `total_pages / 6` | `ship.last_pages`, `head_tab`, the page entries' parent | (pages, 0, k) |
| upper levels of the page tree (`pdf_finish_file`) | its place | none: made at the job's end, whose seed is fixed (`END_OF_JOB_SEED`) | unchanged, already stable |
| font, `OBJ_TYPE_FONT` (first use in a ship) | its number | `head_tab`, the font's PDF slot, `Numbering` | (FONT, number) |
| destination, `OBJ_TYPE_DEST`, `STRUCT_DEST` (`\pdfdest`, links to names) | its name or number | `head_tab`, the destination trees (cells), `Dests`, `ship.dest_list` | (DEST, name) |
| raw object, form, image (`\pdfobj`, `\pdfxform`, `\pdfximage`) | `pdf_obj_count`, `pdf_xform_count`, `pdf_ximage_count` | `\pdflast…` cells, `head_tab`, trees, the ship's resource lists | (type, count) |
| thread, `OBJ_TYPE_THREAD` | its identifier | `head_tab`, trees | (THREAD, identifier) |
| outline (`\pdfoutline`) | none (`Id::Num(0)`) | `first/last/parent_outline`, neighbours' entries | position (unchanged) |
| link and annotation made at the command (`\pdfstartlink`, `\pdfannot`) | none | their `PdfLast` cells, the whatsit in the list | position (unchanged) |
| links, annotations, resources and stream made during a ship | none | only until the ship ends (the lists of Task 14c's class (a)) | position (unchanged) |
| object streams (`pdf_os`) | none | `pdf.out`'s open stream, until its hundredth object | position (unchanged) |
| font descriptors, widths, font files, ToUnicode (writefont) | n/a | made at the job's end, fixed seed | unchanged |

**The rule**, written into DESIGN §7.16.5: a name held in state that
crosses a region boundary must come from an identity TeX defines (a page
number, a count, a name, a place in the page tree), never from the
position of the step that made it.

**Built.**
- `objtab::tex_identity(t, i)` names every kind whose `(type,
  identifier)` is unique in a run by `hash("tex", t, i)`.
- `write_page_object` names a page-tree leaf by `("pages", 0, k)`
  through `pdf_create_obj_named`.
- `new_vid` tries `hash(name, 1)`, `hash(name, 2)` and so on until it
  finds a free id. It reads each id tried, as before, so a clash is
  resolved the same way in every run.
- The step's own count (`vcount`) is not advanced for a named object.

Final numbers still come from `Numbering`'s order, so the output does
not change: e2e is identical in both modes. `PARTEX_MACHINE_TREE_NAMES=0`
names by position as before (`objtab::TREE_NAMES`,
`machine::set_tree_names`).

**Numbers.** Measured 05:03–05:11 with `bench/edits.sh` and
`PARTEX_MACHINE_PARTS=7` on the course probe copy. Conditions: release
binary from `7579f4c` plus this change; in process; one pass; load
average 6–12 (the other agent's runs and my sanitized runs). The cold
build took 57–62 s.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 148 | 32870 | 1 / 569 | 9 | 1, 27.5 | 67.0 |
| word | 1557 | 718176 | 6 / 569 | 33 | 4, 61.5 | 66.5 |
| label | 348 | 40056 | 2 / 569 | 16 | 2, 47.4 | 87.1 |
| footnote | 2811 | 2070065 | 19 / 569 | 133 | 8, 118.1 | 67.5 |
| enter | 860 | 588495 | 9 / 569 | 52 | 3, 45.1 | 64.6 |
| word-local | 130 | 32870 | 1 / 569 | 9 | 1, 24.4 | 65.0 |
| delete-par | 2003 | 1840582 | 15 / 569 | 80 | 3, 46.2 | 66.5 |

`word`'s time is load: its counts are Task 14b's (718,176 commands, 897
ms then). The other rows' counts are unchanged but for `enter`.

**`enter`: 1,722,276 → 588,495 commands** (1,903 → 860 ms). Regions
364–371 are clean. The probe (`PARTEX_MACHINE_PARTS=4`,
`PARTEX_MACHINE_PROBE_ALL=1`) shows `Rest` equal at every region from
364 on. What is left:

| span from region | commands | dirty through |
|---:|---:|---|
| 357 | 221,636 | the edited lines (26 `Line` cells, 68 onward) |
| 485 | 360,212 | `Numbering` |
| 568 | 6,647 | `Glyphs`, `Numbering` (the end) |

The coordinator expected line positions to be what is left. Only in
part:
- 357 is the region that holds the edit, at line 69 of the file's
  203. After the two new lines every later position in the file is two
  lines later, so the rebuild can sync only at a boundary whose position
  is unchanged (below). The span of 221,636 commands ends at the first
  one. I did not locate it.
- 485 is `Numbering`, whose version is a hash of its events, names
  included. Every object still named by position, here the links made
  in `ch15` after the edit, changes it.

So positions are what is left, through the sync, which needs an old
boundary's position again, and through `Numbering`. No region is dirty
through a moved line by itself.

**New harness row, `delete-par`.** It deletes the second paragraph of
`ch15.tex`'s first section with its blank line: line 20, "The example:
minimise \(f(x,y)=x^2+y^2\) subject to \(x+y=2\) …", unique in the file,
two lines of text on the chapter's first page. So the page breaks after
it move. It is in `bench/edits/course.txt` with a comment. Its baseline
(this binary): 1,840,582 commands, 15 dirty regions of 569, 80 cuts,
2,003 ms. With `PARTEX_MACHINE_TREE_NAMES=0`: 2,717,352 commands, 21
dirty regions, 2,674 ms.

By cause:

| span from region | commands | dirty through |
|---:|---:|---|
| 353 | 1,216,701 | the edited line (19 onward), then the rest of `ch15.tex` |
| 364, 365 | 257,011 | `Rest`: `pdf.out`'s open object stream (a page's resources, which list other fonts now: real) and `ship.pdf_v` and `dest_list` (Task 14c's class (a)) |
| 485 | 360,212 | `Numbering` |
| 568 | 6,658 | `Written(2)` (the `.aux`), objects, `Dests` (the end) |

None is dirty through `Page` or through eqtb. The span from 353 is the
first thing to fix for this edit, and it is positions, not the fold.
After the deletion every later position in `ch15.tex` is two lines
earlier. A rebuild syncs only at a candidate whose position (`at()`,
which hashes `line`) equals an old region's entry, so none comes before
`ch15.tex` is closed. The rest of the chapter is re-run whatever its
page state is. The probe confirms it: its first matched boundary after
the edit is region 364, 48 steps earlier than in the old run. That is
where the fold would pay, once boundaries can be matched across moved
lines (phase 4's renaming of positions, applied to `at()`).

**Sanitizer.** `PARTEX_MACHINE_SANITIZE=1` on the course passed for
`space`, `word`, `footnote`, `enter` and `delete-par` (05:00–05:17), with
the same counts as the timed runs.

**Gate.** `scripts/sandbox cargo xtask check` passed at 05:12–05:17 in
5 min 18 s. It ran fmt, clippy, wasm32, the tests, and e2e 34/34 in both
modes, including `machine_edits`, `machine_edits_dvi` and
`modern_watch_sanitized`.

Found, not fixed:
- `Numbering`'s version includes every name. Any object still named
  by position makes every `Numbering` reader dirty: region 485, 360k
  commands, in `word`, `footnote`, `enter` and `delete-par`. A version
  that depends only on what `final_num` answers is the next step.
  Hashing the events without names is not exact: two objects that
  swap their order of creation would give the same version and
  different numbers.
- `cur_val` differs, in the probe, at many regions after a deletion
  while `Rest` is otherwise equal. The rebuild does not re-run those
  regions, and the sanitizer passes, so the difference is never
  observed. That is consistent with Task 14c's class (a) for `cur_val`.

Rejected:
- Naming links by a count of links: TeX defines none, and an added link
  would move every later one. They stay named by position until a
  boundary can be matched across moved text.
- Naming outlines by an ordinal: the same.

Time: 04:56–05:21, about 25 minutes.

### Task 6: the Flat commits by live prefix and high-water mark (Agent, branch cuts)

A snapshot's commit compared each of the five `Flat`s whole with its
shadow. `Flat::commit` refreshed the whole allocation whenever the
vector had been borrowed mutably since the last snapshot. The `Rest`
hash then compared the string pool again, segment by segment
(`Flat::segments`), to find the shared ones.
`PARTEX_CUT_TIMING` now reports each `Flat`'s commit, with its length
and live prefix in bytes at the last cut. The course, cold, at master
`7d05d43`:

| `Flat` | shape | bytes | live at the last cut | µs per cut |
|---|---|---:|---:|---:|
| `str_pool` | append-only, strings made below `str_start[str_ptr]` | 2,090,710 | 2,087,981 (`pool_ptr`) | 350 |
| `str_start` | append-only, entries to `str_ptr` | 713,592 | 713,576 (`str_ptr + 1`) | 107 |
| `buffer` | live prefix `max(first, last)` | 200,001 | 27 | 37 |
| `input_stack` | live prefix `input_ptr` | 240,024 | 0 | 59 |
| `param_stack` | live prefix `param_ptr` | 80,004 | 0 | 19 |

The string pool is all live, since it holds every name LaTeX made. So
for the strings the saving has to come from not comparing what cannot
have changed, not from the live prefix.

Built:
- **`Flat::commit_live(live, next)`** (`flat.rs`) captures only
  `[0, live)`.
  - The segments wholly below the `Flat`'s floor are taken from the
    shadow without a compare. In debug builds they are compared, and a
    difference fails an assertion.
  - The floor then becomes `next`, and the owner lowers it with
    `Flat::lower_floor` when it may write lower.
  - A checkpoint's segments hold the prefix; `Segments::len` keeps the
    length. `to_vec` and `thaw` pad the dead part with zeros, `get_all`
    and `range_all` read it as zeros, and a checkpoint's `prefix` ends
    at the captured prefix.
  - `Persist` now saves the length too, since the parts may be shorter.
  - A thaw sets the floor to 0, so the first commit after a restore
    compares everything once.
- **The owner** is `Tex::commit_flats` (`run.rs`). Its comment argues
  each prefix with WEB sections:
  - the pool to `pool_ptr`, with floor `str_start[str_ptr]`: made
    strings are never written, §40–§44, and only the string being made
    is changed in place, §517 and §260;
  - `str_start` to `str_ptr + 1`, with floor `str_ptr + 1`;
  - the buffer to `max(first, last)`: a line is read before it is
    scanned, §31 and §362; each level's line lies below `first`, §328
    and §331; and `show_context` reads each line to its `limit`, §318;
  - the input stack to `input_ptr`: §321 writes a level before
    `input_ptr` passes it, and §311 and §1335 store `cur_input` at
    `input_stack[input_ptr]` before reading it;
  - the parameter stack to `param_ptr`: §390 stores the arguments
    before it raises `param_ptr`.
- **Where `str_ptr` goes down**, `strings_reopened` (`strings.rs`)
  lowers both string floors. It is called from:
  - `flush_string` (§44);
  - `get_strings_started` (§47);
  - the three places in §517 that lower `str_ptr` without it
    (`files.rs`);
  - the overlay's replacement of the whole pool, which sets the floor
    to 0 (`overlay.rs`).

  Other writes to the pool were checked: `append_char` writes at
  `pool_ptr`; `intern_str` (`machine.rs`) and §260's move
  (`hash.rs:379`) write from `str_start[str_ptr]` on; `quote_range`
  writes inside the current string; the format load starts from a new
  store.
- **The `Rest` hash already read only the live prefixes**
  (`statehash.rs`, "strings", "input: stack", "input: buffer", "input:
  scanner"). `Flat::segments` now also takes a segment from the shadow
  uncompared when the vector is unwritten since the shadow (`clean`),
  which is the case right after the commit that precedes the hash.
- **Switch.** `PARTEX_FLAT_LIVE=0` (`set_flat_live`) brings back the
  whole commit and the hash's compare.
- **Unit test.** `a_live_commit_holds_the_live_prefix` covers four
  cases: a write above the floor, a write below it once the floor is
  lowered, the dead tail, and a resumed checkpoint's length and zeros.

Measured 05:08–05:16 on `~/code/tmp/course-ap-t4`, master `304c41c`
plus this change (A's Tasks 14–14b included), with several runs at
once, load average 8–10. The same binary was run with the switch on and
off (`footnote-on`, `footnote-off`, run side by side). µs per cut:

| piece | cold (570 cuts), off → on | footnote rebuild (142 cuts), off → on |
|---|---:|---:|
| `Flat` commits, the five | 489 → 39 | 369 → 43 |
| `str_pool` | 296 → 29 | 256 → 35 |
| `str_start` | 93 → 8 | 32 → 6 |
| `buffer` | 32 → 0.7 | 26 → 0.6 |
| `input_stack` | 52 → 0.9 | 43 → 0.7 |
| `param_stack` | 17 → 0.7 | 12 → 0.6 |
| `Rest` hash | 259 → 253 | 237 → 230 |
| `Snapshot::of` | 424 → 409 | 390 → 373 |

- The string pool's remaining 29–35 µs is walking its roughly 510
  segments of 4 KB, taking each `Arc`, and comparing the few above the
  floor.
- The `Rest` hash was 363 µs in Task 5b's round 3 and is about 255 here
  with the switch off too. That is because, in this binary, the `clean`
  shortcut in `segments` was not yet under the switch during these two
  runs; it is gated now.
- The token store (449 cold) and the `JVec` commits (1010 cold) did not
  change. The `JVec` commits are now more than half of a cold cut, which
  is T6b.
- The plain pdflatex pass (`perf stat`, the course) is 260.349 G
  instructions, against 260.347 G after Task 5c.

Rows and PDFs:
- `space` (as `space-r`): 133 ms, 32,870 commands, 9 cuts. With
  `PARTEX_FLAT_LIVE=0` (`space-off`) it is 138 ms with the same
  commands and cuts.
- `footnote`: 2535 ms on and 2575 off, 2,070,065 commands, 133 cuts.
  A's changes moved the commands from 1,813,966.
- The PDFs with the switch on and off are the same apart from `/ID`,
  for `space` and for `footnote`.

`PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word` and
`footnote`, run in parallel with no panic:
- `space` gave 32,870 commands and 9 cuts, and its PDF matches
  `space-off`'s apart from `/ID`;
- `word` gave 718,176 commands and 33 cuts;
- `footnote` gave 2,070,065 commands and 133 cuts, and its PDF matches
  `footnote-off`'s apart from `/ID`.

Rejected:
- Tracking the written range on every write, the paused `persist`
  branch's `Flat` change, which cost 2.7% of a plain pass. The floor
  costs nothing on the token path: it moves only where `str_ptr` goes
  down, and the prefixes are scalars the engine keeps anyway.
- A high-water mark alone for the pool, from the last snapshot's
  `pool_ptr`. It would miss a string flushed and made again below it,
  and the string being made, which §517 and `flush_char` rewrite in
  place.
- The full gate (`scripts/sandbox cargo xtask check`) passed on its
  second run: e2e 34/34 in both modes, and trip and etrip in both modes.
  The first run failed on one lint (`needless_range_loop` in the timing
  report).

Time: about 30 minutes of work (04:56–05:27), including two gate runs.

### Task 14e: the numbering guarded by its answers, stopped at the question (Agent, branch units)

Started from `0048ebb`, which merged Task 14d.

**Probe.** `ObjTab` now counts who observes the numbering in each
region: `final_num` calls, `of_final` calls, whole readings (`numbers`,
the job's end) and distinct arguments. `PARTEX_MACHINE_PARTS=9` prints
them after a cold build (`machinehost::dump_numbering_observers`); it is
a diagnostic and changes nothing.

On the course (cold build 05:28, release binary from `0048ebb` plus
this), nine regions observe it:
- Regions 6, 25, 26, 27 and 35 are in the preamble and front matter:
  3–15 `final_num` and 0–8 `of_final` calls each.
- Three regions, including the harness's 485 (360,212 commands),
  make one `final_num` call and one `of_final` call each. The other
  two cost 294,770 and 461,302 commands. That is the shape of pgf's
  shadings: `\pdflastxform` just after the form is made, and
  `\pdfrefxform` with that number later.
- The job's end (the harness's 568) makes 3 `of_final` calls and 2 whole
  readings (`pdf_finish_file`'s numbers and the statistics).

(This run's output directory had no `.aux`, so its region indices are
three lower than the harness's. The costs match.) So region 485 is not
a whole reader: two answers would guard it. The job's end is a true
whole reader and keeps the chain.

**The question: `partex-incr` cannot check such a guard today.** A
region finds itself dirty when its guards include a cell in D, and D
holds only cells that some re-executed region wrote with a new version
(or the edit's). The `Name` precedent works because the regions that
make a name write its `Name` cell, so it enters D. No region writes an
answer: an object created earlier shifts the number of every later one,
and there is no finite set of `FinalNum(k)` to write. The one lazy check
there is is the accumulating path of `rebuild_or_stop`: a region dirty
only through accumulating cells is replayed to its entry and its guards
are compared with `get` there. It compares the source cell's own
version, which for `Numbering` is the chain of names. What it would
need, as a proposal:

1. A hook `Machine::derived(c) -> Option<Cell>`: a guard on `c` asks
   about part of the source cell's value, and `get(c)` answers it on the
   state at the region's entry. For TeX: `FinalNum(k)` and `OfFinal(n)`
   → `Numbering`.
2. In `rebuild_or_stop`, a region that reads the source (it keeps its
   chain guard, so the readers index and `mark` find it as today) and
   has derived guards on it: when the source is in D, take the
   accumulating path. Replay to its entry, then check the derived guards
   by `get` instead of the source's version. If they hold, the region is
   clean for that cell. A region without derived guards (a whole
   reader) checks the chain as now.
3. In `Trace::compose_with`, if the first region wrote the source (it
   made objects), the second's derived guards do not hold at the
   composed entry. Drop them and guard the source at that entry
   (`entry(src)`, as for an accumulating read), so the composed region
   is a whole reader.
4. `Trace::holds` and the sanitizer's chain check need no change: every
   guard keeps its real version.
5. The stub machine gets a derived cell, so the property tests cover
   the new path.

On the TeX side, which is plain once 1–3 exist:
- `final_num`/`of_final` record their answers instead of forcing the
  numbering.
- An answer about an object that existed at the region's entry
  becomes a guard `FinalNum(k)` (version: the hash of the number) or
  `OfFinal(n)` (the hash of the id).
- An object the region made itself gets its number from the counters
  at entry (`sys`, `obj_ptr`, the open stream's `cur` and `idx`,
  `streams`), not from any name. That is a third derived cell,
  `NumState`.
- `get` computes all three from the numbering without forcing it.

This is exact because a virtual object's final number is fixed when it
is made (pdfTeX numbers objects in order of creation). The answer for
`k` changes only if the count of numbers taken before `k` changes: an
object added or removed before it, which the guard catches. The two
unit tests the coordinator asked for would check both directions.

Gate: `scripts/sandbox cargo xtask check` passed on the diagnostic at
05:30–05:34 (4 min 3 s): fmt, clippy, wasm32, the tests, and e2e 34/34
in both modes. No harness numbers change: nothing guards differently
yet.

Time: 05:21–05:35, about 14 minutes, stopped at the question as the
task said to.

### Merges of Task 14d and Task 6; the course sanitizer on six edits (coordinator, 05:20–05:39)

`cd63ce5` (Task 14d), with Task 14c's diagnostics `8c1fdd8`, was merged
into master as `0048ebb`. The LOG entries were put in order of end time
(14c 04:53, 5b 04:55, the 5b merge 05:05, 14d 05:21). `bbad2d6` (Task
6) was merged on top as `75523fb`. Both gates passed (e2e 34/34
identical in both modes). The course sanitizer now also runs `enter`
and `delete-par`, one process per edit in parallel (six processes, about
three minutes). It passed on both merges, with the same counts on both:

| edit | commands re-run | dirty regions / 569 | cuts |
|---|---:|---:|---:|
| space | 32,870 | 1 | 9 |
| word | 718,176 | 6 | 33 |
| footnote | 2,070,065 | 19 | 133 |
| label | 73,989 | 3 | 19 |
| enter | 588,495 | 9 | 52 |
| delete-par | 1,840,582 | 15 | 80 |

Task 6 changed no count, as expected: it changes what a cut costs, not
what is dirty.

*Task 14e's decision (05:37).* Agent A's probe found that the numbering
is read by one `final_num` and one `of_final` in each of three large
regions (1.1M commands; pgf's shadings, `\pdflastxform` then
`\pdfrefxform N`). Each is guarded on the whole numbering chain.
`partex-incr` cannot check a guard on a lazily computed cell, because D
holds only the cells some region wrote. So the task splits in two:
- 14e-1 adds derived guards to the runtime. A guard on a question about
  an accumulating cell is checked by `get` on the accumulating path, and
  the chain guard is kept for indexing. The stub machine gets property
  tests.
- 14e-2 adds the TeX side.

It stays in agent A's hands, although it touches the runtime, because A
holds the whole diagnosis and agent B is inside the checkpoint
internals.

### Task 14e-1: derived guards in `partex-incr` (Agent, branch units)

The coordinator accepted the proposal above at 05:37. This is its
runtime half; the TeX cells are Task 14e-2.

**What changed** (`partex-incr`):
- `Machine::derived(c) -> Option<Cell>`. A derived cell asks a question
  about an accumulating cell, its source. `get` answers it, no region
  writes it, and setting it does nothing. The default is `None`.
- `Trace::derived_guards(src)` lists a region's questions about `src`,
  and `Trace::answered(src)` says whether it has any.
- `rebuild_or_stop`, on the accumulating path. The path is taken for a
  region dirty only through accumulating cells, after `replay_span` has
  brought S to that region's entry. I checked this: either `pending` is
  replayed up to the region, or S is already at its entry (`synced`).
  At that entry, a cell of D the region reads is accepted in one of two
  ways:
  - its own guard holds (it caught up, as before), and it leaves D;
  - it fails, but the region has derived guards on it and all of them
    hold against `s.get`. Then it stays in D, for later readers.

  If every such cell is accepted, the region is clean. The region's
  source guard is then set to the source's value now (`refresh`, patched
  in `seq` after the check), so every guard keeps its real version. The
  store's chunk fingerprint includes the guards' versions, so a refreshed
  region is saved again.
- `Trace::compose_with`:
  - A question from the second region about a source the first one
    wrote is dropped. The source guard is then taken at the composed
    entry by the existing accumulating rule (`entry(src)?`).
  - The composed region keeps any question about a source only if every
    part that reads the source asked only questions, and the second
    asked none after the first added to it. Otherwise all its questions
    about that source are dropped, and it reads the source whole.
- `build::Config::derived_guards`, on by default, turns the rule off.

**The rule, in one sentence:** a derived guard can only make a region
cleaner than its source guard would, never dirtier.

**Rejected:**
- Keeping the source guard's old version on a region kept by its
  answers: the sanitizer's chain check (`check_replay`) then failed on
  the stub's first test, because the guard no longer held at the
  region's entry in the new run.
- Changing `check_replay` to accept answered sources: that would weaken
  the invariant instead of keeping it.

**Stub.** The new statement is `has g`, which outputs whether glyph `g`
was used so far. It is a question about the `Used` accumulator
(`Cell::UsedHas(g)`, derived from `Cell::Used`), answered at the
region's entry; what the region added before asking is its own. A region
that also runs `fonts` reads `Used` whole and records no question. The
oracle prints the same line. The generator's accumulator branch now
emits `has` a quarter of the time, so the random rebuild, stopped
rebuild and several-edit tests cover the path in every configuration,
with the sanitizer.

New tests (`tests/props.rs`), on the program `mark 1 / say a / has 2 /
say b` cut at every blank line:
- `a_derived_guard_that_holds_replays_the_region`: editing `mark 1` to
  `mark 3` re-executes 1 region with derived guards on and 2 with them
  off.
- `a_derived_guard_that_fails_reruns_the_region`: `mark 2` re-executes
  2.
- `composition_over_a_writer_falls_back_to_the_source_guard`: composing
  from the `mark` region to the `has` region leaves no question and a
  guard on `Used` that holds. Composing the regions after `mark` keeps
  the question.
- `derived_guards_keep_their_real_versions`: on 60 random programs,
  every trace holds at its entry on replay, and `check_replay` passes.

`cargo test -p partex-incr`: 3 unit tests and 17 property tests pass.

**Probe.** The `PARTEX_MACHINE_PARTS=9` probe from the Task 14e entry
above is part of this branch. TeX does not ask questions yet, so no
harness number changes.

**Gate.** `scripts/sandbox cargo xtask check` passed at 05:41–05:47 (5
min 13 s): fmt, clippy, wasm32, the tests, and e2e 34/34 identical in
both modes, including `machine_edits`, `machine_edits_dvi` and
`modern_watch_sanitized`.

Time: 05:37–05:48, about 11 minutes.

### Task 6b: the JVec commits (Agent, branch cuts)

eqtb, the hash table and the save stack are `JVec`s: 512-word chunks,
with a dirty bit per chunk set by every write. A commit walks the bits,
compares each dirty chunk with its base, and copies those that differ.
`PARTEX_CUT_TIMING` now splits each `JVec`'s commit into those three
passes (`JVec::commit_timed`, the same result in three passes). It also
reports the chunks, the dirty chunks per cut (mean and maximum), the
chunks copied, and `save_ptr`.

Before, on the course (master `bbad2d6`, the `footnote` row, µs per
cut):

| `JVec` | chunks | dirty per cut, mean (max) | copied | walk | compare | copy |
|---|---:|---:|---:|---:|---:|---:|
| eqtb, cold | 1231 | 580 (1231) | 187 | 9 | 488 | 466 |
| hash, cold | 1230 | 56 (1230) | 56 | 3 | 28 | 132 |
| save stack, cold | 391 | 2.5 (6) | 2.5 | 0.7 | 0.9 | 6 |
| eqtb, rebuild | 1231 | 207 (959) | 64 | 5 | 131 | 85 |
| hash, rebuild | 1230 | 1.5 (92) | 1.5 | 1.6 | 1.1 | 2.1 |

The commits are bound by the compare and the copy, not by the walk,
which is about 10 µs for 1,231 chunks. Most dirty eqtb chunks are
written back as they were: 580 dirty, 187 changed. That is a group's
local assignments undone at its end. The save stack is small (2.5 dirty
chunks a cut, `save_ptr` 0 at the last one), so a live prefix for it
would save nothing.

Built (`journal.rs`), from the paused `persist` branch's `JVec` part
(`1ad10cc`):
- A write sets a bit per 64-word block, a chunk's eight bits one byte
  of `dirty`. That is the same single OR as the chunk's bit it replaces;
  `index_mut` shifts the index by 6 instead of 9.
- A commit compares only the blocks written of a dirty chunk
  (`same_as_base`) and copies the whole chunk if one differs.
- `PARTEX_JVEC_BLOCKS=0` (`set_written_blocks`) compares whole chunks.
- The walk is not the cost, so there is no list of dirty chunks and no
  tracker constant here. The write path stays one OR in every mode.

After, the same binary with the switch on and off (run side by side at
05:46–05:49, with the sanitizer runs, load 8–10), µs per cut:

| piece | cold (570 cuts), off → on | footnote rebuild (142 cuts), off → on |
|---|---:|---:|
| `JVec` commits, the three | 1039 → 836 | 320 → 213 |
| eqtb compare | 393 → 178 | 195 → 75 |
| eqtb copy | 451 → 473 | 101 → 115 |
| hash compare | 29 → 15 | 1.3 → 0.7 |
| hash copy | 139 → 144 | 2.4 → 2.4 |

The copy is now the cost: about 2.5 µs per 4 KB chunk (473 µs for 187
chunks). That is well above a memcpy's 0.2 µs. The kept checkpoints
hold every copied chunk, so each copy is likely new memory (page
faults), but I did not verify that.

Tried and not kept:
- *64-word chunks* (`BITS = 6`, with 8-word blocks). The commits fell to
  300 µs cold (eqtb: compare 87, copy 99) and 84 µs in the rebuild. But
  `Snapshot::of` rose from 418 to 1064 µs and the `Rest` hash from 236 to
  293. The clone takes one `Arc` per chunk, 29,500 of them instead of
  3,700, and each is a cache miss on the chunk's count. Net, a cold cut
  cost 150 µs more. Smaller chunks pay off once the clone shares the
  chunk vector itself (T7).
- *A copy rebased on the base chunk*, with the written blocks copied
  over it: the same time as copying from the running vector (496 against
  501 µs), so it was dropped.

Gates for the kept form (05:46–05:53):
- The plain pdflatex pass is 260.352 G instructions, against 260.349 G
  after Task 6.
- `space` (as `space-r`): 123 ms, 32,870 commands, 9 cuts.
- `footnote`: 2672 ms on and 2690 off, 2,070,065 commands, 133 cuts.
  The PDFs on and off are identical apart from `/ID`.
- `PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word`
  and `footnote`. The sanitized `footnote` matches `footnote-off`, and
  `space` matches `space-r`, apart from `/ID`.
- The full gate (`scripts/sandbox cargo xtask check`) passed: e2e 34/34
  in both modes, and trip and etrip in both modes.

Time: about 25 minutes (05:29–05:54).

### Merges of Task 14e-1 and Task 6b (coordinator, 05:48–06:04)

`f1e2ece` (Task 14e-1) was merged into master as `9e1439c`, and
`d608751` (Task 6b) on top of it as `3cd17c2`. One gate and one
sanitizer run covered both merges. The gate passed (e2e 34/34 identical
in both modes). The course sanitizer passed on the six edits (06:03).
Every count is the same as on `75523fb`: neither task changes what is
dirty on the course yet. The derived guards have no TeX cells until
Task 14e-2, and Task 6b changes only a cut's cost.

Reviewed in Task 14e-1: the rule "a region with derived guards on a
source read it only through questions" is a contract on the machine
(`answered` is "has derived guards"). The TeX side must record none for
a region that also reads the numbering whole. Task 14e-2 asserts this
in `prepare_cut`. Reviewed in Task 6b: the only writers of a JVec's
live vector are `index_mut` and `copy_from`, and both mark their
blocks. `thaw` clears the block bits with the chunk bits' old sizing,
which is still right: one `u64` holds 8 chunks of 8 blocks.

### Task 14e-2: the numbering guarded by its answers, the TeX side (Agent, branch units)

Started from `9e1439c`, which merged Task 14e-1.

**What changed.**
- `ObjTab::final_num` and `of_final` record their answers in the log
  (`ObjLog::answers`: `Final(k, n)`, `Of(n, vid)`) instead of marking
  the numbering read whole.
- Only the whole readings set `forced`: `numbers()`, `pdf_finish_file`
  and the statistics at the end go through `numbering()`.
- With `PARTEX_MACHINE_NUM_ANSWERS=0` (`objtab::NUM_ANSWERS`,
  `machine::set_num_answers`), or with the machine's object log off,
  single numbers mark the numbering read whole, as before.

**The cells.** Three derived cells of `MCell::Numbering`
(`Machine::derived`), answered by `get` from the state without forcing
the numbering:
- `FinalNum(k)`: `ObjTab::peek_final_num`, which uses the cached
  numbering, a prefix of the log, plus the events after it (`Log::final_of`);
- `OfFinal(n)`: `peek_of_final` and `Log::vid_of`;
- `NumState`: the counters `sys`, `obj_ptr`, `cur`, `idx`, `streams`.
  The log now keeps them up to date with every event (`vnum::Counters`,
  stepped in `Log::push`), next to its running hash.

**The guards, in `prepare_cut`.** A region that asked for single
numbers keeps its chain guard on `Numbering` (how a rebuild finds it).
If it did not also read the numbering whole, it guards each answer as
it was at its entry:
- an object the region did not make: `FinalNum(k)` (hash of the
  number);
- the object numbered `n`, made before the region: `OfFinal(n)` (hash
  of its id);
- an object the region made, or a number past what was taken at the
  entry: `NumState`, the counters at the entry. Such a number depends
  on how many numbers came before, not on any name.

A region that also read the numbering whole records only the chain
guard. That is the implementors' contract, now in `Machine::derived`'s
doc comment, and a `debug_assert` checks it. The store saves the new
cells. `get` and `set` treat them as questions: setting does nothing.

**Exact because** pdfTeX numbers objects in order of creation: once an
object is made its number is fixed. It changes only when the count of
numbers taken before it changes, which the guard sees.

Unit tests (`pdf/vnum.rs`):
- `counters_and_answers_are_the_numberings`: on 39 random event
  sequences with streams, the log's counters equal the numbering's, and
  the answers read from any cached prefix equal those of the numbering
  caught up.
- `an_answer_changes_with_an_insertion_not_with_a_name`: an object
  inserted before `k` changes `k`'s answer and the counters (the region
  re-runs). An earlier object renamed leaves the answers and the
  counters equal while the chain differs (the region replays). The
  rebuild's use of the answers is covered by Task 14e-1's stub tests.

**Numbers.** Measured 06:00–06:10 with `bench/edits.sh` and
`PARTEX_MACHINE_PARTS=7` on the course probe copy. Conditions: release
binary from `9e1439c` plus this change; in process; one pass; load
average 4–11; the cold build took 55–60 s.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 117 | 32870 | 1 / 569 | 9 | 1, 24.9 | 64.6 |
| word | 490 | 357964 | 5 / 569 | 31 | 4, 48.4 | 62.9 |
| label | 222 | 73989 | 3 / 569 | 19 | 2, 33.9 | 62.3 |
| footnote | 2301 | 2070065 | 19 / 569 | 133 | 8, 97.1 | 63.0 |
| enter | 508 | 228283 | 8 / 569 | 50 | 3, 44.5 | 65.0 |
| word-local | 114 | 32870 | 1 / 569 | 9 | 1, 22.4 | 61.5 |
| delete-par | 1581 | 1480370 | 14 / 569 | 78 | 3, 45.2 | 65.7 |

With `PARTEX_MACHINE_NUM_ANSWERS=0`, `word` gave 718,176 commands, 6
dirty regions and 814 ms, which is Task 14e-1's count exactly. So region
485 (360,212 commands) went clean in `word`, `enter` and `delete-par`:

| edit | before (Task 14d) | now |
|---|---:|---:|
| word | 718,176 | 357,964 |
| enter | 588,495 | 228,283 |
| delete-par | 1,840,582 | 1,480,370 |
| footnote | 2,070,065 | 2,070,065 |

`label` is at 73,989 commands, the count the coordinator measured
after the merge of Task 5b–5c (list ids decide it), and unrelated to
this change.

**`word` now** (`PARTEX_MACHINE_PARTS=7`), 5 spans:

| region | commands | dirty through |
|---:|---:|---|
| 353 | 259,196 | the edited line |
| 357 | 32,870 | the reflowed paragraph's sealed lines, read at the ship |
| 358 | 33,933 | `Rest`: the page's link list (Task 14c's class (a)) |
| 359 | 25,318 | `Rest`: the same |
| 568 | 6,647 | `Glyphs`, `Numbering`: the end, a whole reader |

So what `word` costs next is the edited region's size, 259k (cut
placement, Task 13), and 59k of dead per-page scratch (Task 14c's class
(a)).

`footnote`'s 485 stays dirty through `Numbering`, and that is correct.
The footnote adds a destination and hyperref objects before it, so the
counters at 485's entry are one or more higher. The xform 485 makes
gets another number, and `NumState` fails. `footnote` is at 19 dirty
regions still, with the causes of Task 14b.

**Sanitizer.** `PARTEX_MACHINE_SANITIZE=1` on the course passed for
`space`, `word`, `footnote`, `enter` and `delete-par` (05:57–06:00, five
runs in parallel), with the counts above.

**Gate.** `scripts/sandbox cargo xtask check` passed at 06:10–06:14
(3 min 24 s). It ran fmt, clippy, wasm32, the tests, and e2e 34/34
identical in both modes, including `machine_edits`, `machine_edits_dvi`
and `modern_watch_sanitized`.

Time: 05:49–06:15, about 26 minutes.

### Merge of Task 14e-2 (coordinator, 06:15–06:26)

`3b7c69c` was merged into master as `3b0940c`. The gate passed (e2e
34/34 identical in both modes). The course sanitizer passed on the six
edits (06:25) at the agent's counts: 32,870, 357,964, 2,070,065,
73,989, 228,283 and 1,480,370 commands re-run.

Reviewed: the guards in `prepare_cut` are exact because an object's
final number is fixed at its creation. `FinalNum(k)` for an object that
existed at the entry has the same answer at the entry as when it was
asked. `OfFinal(n)` for a number taken before the entry likewise.
Anything the region made itself is guarded by the counters at the
entry, and its own creations are the same on a replay. The
`debug_assert` in that block cannot fire, because it sits inside
`if !whole`. The contract (a whole reader records no derived guards) is
enforced by that `if`, which is better; the agent will turn the assert
into a comment.

*Over the night* (the course, one rebuild after a cold build, commands
re-run): the reflowing word 1,457,463 → 357,964; Enter 1,727,637 →
228,283; `\label` 237,937 → 73,989; a space 76,119 → 32,870; a footnote
2,191,391 → 2,070,065. The footnote is still dirty through true eqtb
dependencies and the output routine's ship stops (Task 14c).

### Task 7: the clone by field (Agent, branch cuts)

A cut clones the engine (`Snapshot::of`, `tex.clone()`, `Tex`'s derived
`Clone`). `PARTEX_CUT_TIMING` now times that clone whole, then clones 21
field groups once more one at a time and times them
(`time_clone_groups`, `CLONE_GROUPS`). The groups follow DESIGN
§7.16.3's table, with the fonts-and-tables row split after a first
run. The groups are timed right after the whole clone, so with warm
caches: their sum (about 340 µs) is less than the whole (407 µs).

Before, on the course (master `b756f27` plus the timing), the `footnote`
row, µs per cold cut, the rebuild's in brackets:

| group | µs |
|---|---:|
| the clone, whole | 407 (314) |
| the font tables (`fonts`: twelve arrays of 9,001 slots) | 121 (≈120) |
| the `JVec`s (eqtb, hash, save stack: their chunk vectors) | 131 (129) |
| the token store and the `Flat`s | 55 (58) |
| `pdf.ship` | 17–41 (22) |
| the objects (`objs`, `xregs`, `xeq_level`) | 13 (10) |
| `prims` (its meaning map) | 8.6 |
| `hyph` | 4.6 |
| the host | 3.5 |
| `fontmap` | 3.1 |
| `pdf.objs` | 4 |
| `pdf.fontw` | 1.8 |
| the memo | 1.7 |
| `seals` | 1.8 |
| `cur_mark` | 1.0 |
| the output buffers (log, `\write`, dvi, effects, the logs) | 1.7 |
| the lists' records (the lists themselves are taken out) | 1.8 |

No bytes were measured per group: the types have no size accessor. The
font tables are big because a machine gives fonts slots by identity
(`Host::font_slot`), with `font_max` 9,000. Each of their twelve
vectors is cloned element by element, with an `Arc` bump for each
font's metrics and TFM.

Built (all in `partex-core`):
- `cow::Shared<T>`: an `Arc<T>`, with `Deref`, `DerefMut` by
  `Arc::make_mut`, `Clone` by one count, and `Persist`, `PartialEq`,
  `Hash` and `IntoIterator` through it. A mutable borrow copies the
  value the first time after a clone.
- *The font tables* (`fonts.rs`). `FontArrays` became a
  `Shared<FontData>` plus its scratch flag `order_read`, which stays
  outside, owned by each engine: shared, a flag set by one engine would
  be seen by another in a threaded round. The methods that set the flag
  (`in_order`, `number`, `last_loaded`) stay on `FontArrays`; the rest
  moved to `FontData` and are reached by `Deref`. The writes are rare:
  a font loaded, `\fontdimen`, `\hyphenchar`, a code table, and the
  interword glue cached once per font (§1042).
- *`pdf.ship.fonts`* is a `Shared<PdfFonts>`. A shipped page marks its
  glyphs there, so a region that ships copies it once.
- *The primitives' meaning map* (`prims.by_meaning`, rebuilt on demand)
  is an `Arc`.

After, same conditions (06:14–06:16), µs per cut:

| | cold | footnote rebuild |
|---|---:|---:|
| the clone, whole | 407 → 182 | 314 → 171 |
| `Snapshot::of`, all of it | 832 → 432 | 773 → 440 |
| the font tables | 121 → 0.2 | 120 → 0.2 |
| `prims` | 8.6 → 0.3 | |
| `pdf.ship` | 17–41 → 1.3 | 22 → 1.3 |

- The plain pdflatex pass is 260.666 G instructions, against 260.352 G
  after Task 6b (+0.12%). That is the `make_mut` behind each write to
  `pdf.ship.fonts` and the font tables, a check per glyph shipped.
- The machine's cold build is 305.007 G instructions, against 304.814 G
  (+0.06%): the copies on first write, once per region that writes.

Not done:
- *The `JVec` spines* (131 µs). A clone copies one `Arc` per chunk, each
  bump a cache miss on the chunk's count, about 46 ns. Sharing the spine
  (`Arc<Vec<Arc<chunk>>>`) would move that copy to the next commit,
  which must make a new spine beside the last snapshot's. Doing better
  needs a persistent tree of chunks that copies one path per changed
  chunk. It is not built.
- *The 64-word chunks* of Task 6b were therefore not retried: with an
  unshared spine they tripled the clone.

*The copy's 2.5 µs per 4 KB* (Task 6b), checked with `/usr/bin/time`
(`perf`'s software events are not available in the sandbox):

| run | minor page faults | peak |
|---|---:|---:|
| plain pass | 46,833 | 226 MB |
| machine cold build | 368,902 | 1.61 GB |
| machine cold build, Task 6b's binary | 386,008 | 1.67 GB |

The cold build copies about 140,000 `JVec` chunks (570 cuts × 245), 560
MB, all kept by the checkpoints. That is most of the 322,000 extra
faults, and the rest is the other copies the checkpoints keep. So the
guess holds in aggregate: a copied chunk lands on memory not touched
before. A free list of retired chunks would only help after coarsening
or eviction drops checkpoints. That is noted for later, not built.

Gates (06:16–06:27, on `~/code/tmp/course-ap-t4`):
- Rows, with the Task 6b binary for comparison where I ran it:
  - `space` 117 ms, 32,870 commands, 9 cuts;
  - `footnote` 2500 ms, 2,070,065 commands, 133 cuts;
  - `delete-par` 1943 ms, 1,840,582 commands, 80 cuts (1763 ms with
    Task 6b's binary, same commands and cuts; the times ran at different
    loads).

  The PDFs of all three match Task 6b's apart from `/ID`.
- `PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word`
  and `footnote`, and the sanitized PDFs match the plain runs apart from
  `/ID`.
- The full gate (`scripts/sandbox cargo xtask check`) passed: e2e 34/34
  in both modes, and trip and etrip in both modes.

Time: about 30 minutes (05:55–06:27).

### Merge of Task 7, and the link before the rest of the cuts (coordinator, 06:27–06:37)

`e299818` was merged into master as `4d0c69d`. The gate passed (e2e
34/34 identical in both modes), and the course sanitizer passed on the
six edits (06:36) with every count unchanged.

*The order changes: Task 27 (the link) before the JVec tree and Task
11.* Of a `space` edit's ~117 ms, the link is ~70, one restore ~25 and
nine cuts ~20. Every row pays 66–100 ms of link, although `space`
changes no PDF byte. `link.rs` relinks everything by design ("a fast
full link beats splicing"). That was right while a rebuild re-ran
hundreds of regions; now it re-runs 1–19 of 569. The JVec persistent
tree saves about 0.6 ms per cut (about 20 ms on `word`'s 31 cuts). It
comes next, then Task 11 (the `Rest` hash), then Task 12 (restore by
rebase, the ~25 ms).

### Task 14f: typing in the harness, and dead fields canonicalized (Agent, branch units)

Started from `3b0940c`, which merged Task 14e-2. At the coordinator's
note, the unreachable `debug_assert` in `prepare_cut` is replaced by a
comment: the branch that makes derived guards runs only without a whole
reading, so that branch is what keeps the contract.

**Part 1: typing.** Every row so far was one edit after a cold build,
the worst case. The engine already took successive edits: specs
joined by `;;` in `PARTEX_MACHINE_EDIT` are applied and rebuilt one
after another in one process, as a watch makes them. `bench/edits.sh`
now:
- checks each `from` of a sequence against the file as the edits
  before it left it;
- reports the last rebuild by default, and every rebuild with `ALL=1`
  (rows `NAME#1`, `NAME#2`);
- takes the regions before each rebuild from the one before it.

A slip found on the way: `set -o pipefail` made the new count of `;;`
fail on specs without one. The first sanitized runs of Part 2 lost
their rows, though not their logs, until it was fixed.

The new rows in `bench/edits/course.txt`:
- `word2`: `word`, then "The Hessian of the" → "The Hessian for the",
  three words on, same paragraph (line 32);
- `space2`: `space`, then a second extra space two words on (line 69);
- `enter2`: `enter`, then "at every iteration" → "at each iteration"
  in the new paragraph.

Measured before Part 2, 06:18–06:22, binary from `3b0940c`, load
average 8–18:

| edit | rebuild ms | commands re-run | dirty regions / total | cuts |
|---|---:|---:|---:|---:|
| word2#1 | 575 | 357964 | 5 / 569 | 31 |
| word2#2 | 278 | 88946 | 17 / 595 | 17 |
| space2#1 | 133 | 32870 | 1 / 569 | 9 |
| space2#2 | 43 | 1114 | 1 / 577 | 1 |
| enter2#1 | 610 | 228283 | 8 / 569 | 50 |
| enter2#2 | 79 | 6263 | 3 / 611 | 3 |

`word2`'s second rebuild, with `PARTEX_MACHINE_PARTS=7`:
- region 356: 7,752 commands, the edited line's fine region;
- region 366: 2,019 commands, the ship that reads the paragraph's
  sealed lines;
- regions 367–379: 13 fine regions, 78,931 commands, dirty through
  `Rest`. They are the page's ship stops and the regions after, where
  the class-(a) scratch of Task 14c differs;
- region 594: 244 commands, the end.

So a second edit already cost a quarter of the first. Most of the rest
was the dead scratch Part 2 removes.

**Part 2: the dead scratch canonicalized.** At every cut in machine mode
(`prepare_cut`, before the exit snapshot), `machine::canonicalize_dead`
sets each field to its initial value in the running engine, so the
snapshot and the run going on hold the same constants, and `Rest`
keeps hashing them. With each field, where it is written before it is
read:
- `ship.annot_list`, `link_list`, `dest_list`: emptied as a page ship
  begins (pdfTeX §752, `pdf_ship_box_out`), read only at that page's end
  (`write_page_object`, `write_page_marks`).
- `ship.pdf_v`: set by `pdf_set_origin` when text begins
  (`pdf_begin_text`, pdfTeX §727–§735). It is read only inside text
  (`pdf_begin_string`, `pdf_set_text_pos`), and a ship ends its text
  (`Draw::EndText`: `doing_text` is false after every ship). This is the
  one Task 14c had not shown; it is shown now.
- `out.scaled_out`: written by `pdf_print_bp` and `divide_scaled` and
  read by their callers right after (pdfTeX §690).
- `cur_val` and `glue_origin` (the lineage of `cur_glue`): the scanner's
  results. A command that uses one runs a `scan_…` routine first
  (§409–§413, §440–§463), so none is read between commands.

`PARTEX_MACHINE_CANON=0` turns it off. `PARTEX_MACHINE_POISON=1` sets the
fields to junk at every candidate (cut or not) instead of the initial
values at cuts:
- junk object ids in the three lists;
- `0x1234567` for `pdf_v`, `0x7654321` for `scaled_out`, `0x5eadbeef`
  for `cur_val`, and a junk lineage.

If any of them were read before being written, e2e would differ from
pdfTeX. No test flips these process-wide switches, so the `Switches`
lock is not needed.

Poisoned, e2e in machine mode (`PARTEX_MACHINE=1 PARTEX_MACHINE_POISON=1
cargo xtask e2e`, 06:23–06:26) is 34/34 identical, including
`machine_edits`, `machine_edits_dvi`, `modern_watch` and
`modern_watch_sanitized`.

**Numbers.** Measured 06:37–06:47 with `bench/edits.sh`, `ALL=1` and
`PARTEX_MACHINE_PARTS=7` on the course probe copy. Conditions: release
binary from `3b0940c` plus this change; in process; load average 6–11.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 111 | 32870 | 1 / 569 | 9 | 1, 23.4 | 61.7 |
| word | 447 | 298713 | 3 / 569 | 22 | 4, 48.8 | 66.1 |
| label | 299 | 40056 | 2 / 569 | 16 | 2, 40.5 | 98.0 |
| footnote | 2285 | 1977503 | 18 / 569 | 114 | 8, 98.8 | 65.5 |
| enter | 500 | 228283 | 8 / 569 | 50 | 3, 42.6 | 64.1 |
| word-local | 119 | 32870 | 1 / 569 | 9 | 1, 23.7 | 64.5 |
| delete-par | 1301 | 1223359 | 12 / 569 | 67 | 3, 44.0 | 65.6 |
| word2#1 | 437 | 298713 | 3 / 569 | 22 | 4, 49.5 | |
| word2#2 | 134 | 10015 | 4 / 588 | 4 | 5, 49.0 | 65.7 |
| space2#1 | 114 | 32870 | 1 / 569 | 9 | 1, 25.1 | |
| space2#2 | 33 | 1114 | 1 / 577 | 1 | 1, 19.5 | 68.6 |
| enter2#1 | 491 | 228283 | 8 / 569 | 50 | 3, 44.0 | |
| enter2#2 | 72 | 6263 | 3 / 611 | 3 | 2, 37.8 | 64.4 |

Against Task 14e-2:
- `word`: 357,964 → 298,713 commands. 358–359 are clean, as Task 14c
  predicted.
- `footnote`: 2,070,065 → 1,977,503. 486 is clean, as predicted.
- `delete-par`: 1,480,370 → 1,223,359. 364–365 are clean: their `Rest`
  difference was the class (a) of Task 14c, plus an open object stream
  that turns out to be equal once `scaled_out` is.
- `label`: 73,989 → 40,056 (2 regions). Its third region was dirty
  through the same scratch after the page ship.
- `enter` is unchanged.

**Typing.** `word2`'s second rebuild re-runs 10,015 commands in 134 ms,
down from 88,946 in 278 ms:
- region 356: 7,752 commands, the edited line's region, which the
  first rebuild recorded finely;
- region 366: 2,019 commands, the ship that reads the reflowed
  paragraph's sealed lines and an object;
- region 587: 244 commands, the end, which reads the numbering whole.

`space2`'s second rebuild re-runs 1,114 commands in 33 ms, and
`enter2`'s 6,263 in 72 ms. Of those 33 ms and 134 ms, about 20–50 ms is
the restore of an old state (`restores`, cause 3 of §7.16, phase 2's
checkpoints). The TeX work is 1–10k commands, a paragraph's worth.

**Sanitizer.** `PARTEX_MACHINE_SANITIZE=1` on the course passed for
`space`, `word`, `footnote`, `enter` and `delete-par`, twice:
- with the canonical values (06:23–06:27);
- poisoned (`PARTEX_MACHINE_POISON=1`, 06:48–06:50).

Both gave the counts of the table. Poisoned, `word` is at 298,713
commands and not Task 14e-2's 357,964, so the junk was applied, and
since the rebuild and the fresh build agree it was never observed.

**Gate.** `scripts/sandbox cargo xtask check` passed at 06:50–06:54 (3
min 27 s): fmt, clippy, wasm32, the tests, and e2e 34/34 identical in
both modes, including `machine_edits`, `machine_edits_dvi` and
`modern_watch_sanitized`.

Time: 06:16–06:55, about 39 minutes.

### Merge of Task 14f; where a keystroke's time goes (coordinator, 06:55–07:05)

`4019688` was merged into master as `2508421`. The gate passed (e2e
34/34 identical in both modes). The course sanitizer, now with `word2`
too, passed on seven edits (07:05). Counts: 32,870, 298,713, 1,977,503,
40,056, 228,283, 1,223,359, and 10,015 for `word2`'s second rebuild
(4 dirty regions of 588, 5 restores).

*A keystroke's time* (`word2`'s second rebuild, about 134 ms at load
6–11):
- TeX re-runs about 10k commands, about 10 ms;
- five restores cost 50–60 ms;
- the link costs about 70 ms.

So typing now waits on the checkpoint's restore (Task 12, restore by
rebase) and the link (Task 27, running), not on TeX. After Task 27,
agent B's order is Task 12 before the JVec tree and Task 11. A
restore's 10–12 ms each outweighs a cut's 2 ms when a keystroke
re-runs only 4 regions.

### Task 27: the link costs what changed (Agent, branch cuts)

The harness's "link ms" column (66–100 ms on every row, `space`
included) came from the in-process path of `PARTEX_MACHINE_EDIT`. After
the rebuild, that path linked every region in full (`effects::link`)
and wrote every file (`machinehost::link_and_write`), with no earlier
link to compare against.

A watch session already did better (`Session::write`). It took the last
link again when no region's effects had changed. `Build::splice_keeping`
compares the effects spliced out with those put in, which gives
`take_effects_changed`. Otherwise it resolved only the regions put in
(`link_cached`, `take_touched`) and wrote only the files whose bytes had
changed. So the column measured a path no user runs.

Measured first: the phases of a full link. The cold build's link below
resolves every region, which is a full link. The course, release
binary, `PARTEX_CUT_TIMING=1`, in process:

| phase | ms |
|---|---:|
| numbering (`numbering_of`, a pass over every effect) | 1.6 |
| resolve (renumbering every region) | 24.4 |
| layout, the pass over the effects | 1.7 |
| layout, the object streams rendered and compressed (the host's memoized deflate, first use) | 26.7 |
| layout, the cross-reference stream rendered and compressed | 4.5 |
| byte counts | 0.0 |
| copy into the buffer | 2.0 |
| the files hashed and written (3.7 MB; the PDF is 3,184,359 bytes) | 2.4 |
| all | about 63 |

Built:
- *`machinehost::Linker`*, the in-process path's link, now links as a
  watch does. The cold build's output is linked before the first edit,
  as a watch links it (reported as "the cold build's output", not in the
  harness's column). After each rebuild:
  - if no region's effects changed, the last link is taken again and
    nothing is written;
  - otherwise `link_cached` resolves the regions put in, and only the
    files whose bytes changed are written (by their `quick::hash`).
- *`effects::link_cached_timed`* and `LinkTimes`: `link_cached` with its
  phases timed, reported by `PARTEX_CUT_TIMING` ("link timing").
- *Exactness.* Under `PARTEX_MACHINE_SANITIZE=1`, and in debug builds,
  the output is compared with a full link's, files and terminal text,
  and a difference panics.
- *Switch.* `PARTEX_LINK_SPLICE=0` gives the full link and every file
  written, as before. A watch has its own switches:
  `PARTEX_MACHINE_LINK_CACHE=0` and `PARTEX_MACHINE_LINK_REUSE=0`.
- *Docs.* The module doc of `partex-incr/src/link.rs` said the link
  always relinks everything; it now says which link does what.

The seven rows, with the link and with `PARTEX_LINK_SPLICE=0`, run side
by side at 06:47–06:54 (master `92f1a48` plus this, load 8–10):

| edit | rebuild ms | commands | cuts | link ms, now | link ms, full |
|---|---:|---:|---:|---:|---:|
| space | 140 | 32,870 | 9 | 1.6 | 51.9 |
| word | 586 | 357,964 | 31 | 23.2 | 41.7 |
| label | 264 | 73,989 | 19 | 19.4 | 39.4 |
| footnote | 2392 | 2,070,065 | 133 | 42.0 | 50.4 |
| enter | 525 | 228,283 | 50 | 23.0 | 42.2 |
| word-local | 122 | 32,870 | 9 | 19.3 | 41.2 |
| delete-par | 1558 | 1,480,370 | 78 | 26.0 | 42.0 |

The seven PDFs match their full-link runs apart from `/ID`, which
depends on the output path.

Where the cached link's time goes (`word`: 33 regions resolved, 563
taken from the last link):
- numbering 1.6 ms;
- resolve 8.2 ms: the 33 regions, and every region's written numbers
  compared (`region_inputs`, a pass over its effects);
- layout 7.6 ms, of which the object streams took 1.0 (the deflate memo
  now warm), the cross-reference stream 4.5 (its offsets moved, so it
  is rendered and compressed again) and the pass 2.1;
- copy 1.8 ms;
- the PDF written, 2.5 ms.

`space` changes no effect, so it links nothing. Its 0.9–1.6 ms is
hashing the `\write` files (the `.aux` and others), which the host
keeps apart from the link. `word-local` changes a word, so its PDF
changes: 19 ms.

`footnote` resolves 330 of 683 regions again. The footnote's objects
shift the object numbers of everything after it, which changes those
regions' written numbers.

Not built:
- Splicing the output buffer. It would save at most the copy's 1.8 ms.
- What is left is O(regions) per link: the numbering, the comparison of
  written numbers, and the layout's pass. There is also the
  cross-reference stream, O(objects) and compressed each time. A layout
  cached per region, with offsets by prefix sums, would remove the
  pass. The numbering and comparison need numbers per region that do
  not shift, which is the stable-id work of fast mode (§7.15).
- The write (2.5 ms for 3.3 MB) is not worth designing around.

`PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word` and
`footnote`, run as `-s` rows with the full-link comparison on. Their
PDFs match the full-link runs apart from `/ID`.

The first full gate found a bug. `machine_edits` and `machine_edits_dvi`
differed in `term.txt`, because the in-process path printed the
terminal text at both links, the cold one and the rebuild's. Only the
last link prints it now (`Linker::finish`). The same run also failed
two lints (`too_many_lines`), fixed by moving the sanitizer's check and
the timing report into functions. With both fixed, the full gate
(`scripts/sandbox cargo xtask check`) passed: e2e 34/34 in both modes,
and trip and etrip in both modes. The rows and the sanitizer runs above
came from the binary before that fix, which differs only in what it
prints to the terminal.

Time: about 40 minutes (06:29–07:10).

### Merge of Task 27 (coordinator, 07:11–07:22)

`1a7ade6` was merged into master as `f43c0bd`, on top of Task 14f. The
merge was clean in `machinehost.rs`, but one interaction is left: the
`Linker` links after the cold build and at `finish`, so a `;;` typing
sequence does not link between its rebuilds. Its one link then covers
every edit's changes. The output is exact, but a typing row's link is
more than one keystroke's. Task 12 fixes it first. The gate passed (e2e
34/34 identical in both modes). The course sanitizer passed on seven
edits (07:21), with the counts unchanged.

Under `PARTEX_MACHINE_SANITIZE=1` the harness's link column includes the
full-link comparison (`check_full_link`): `word` shows 216 ms there.
Link times are to be read from unsanitized runs only.

### Task 20: sync across moved lines, the plan and boundary identity (Agent, branch units)

Started from `2508421`, which merged Task 14f.

**Step 1: the plan, and why the order changed.** The brief had
"boundaries first". Reading the rebuild with the old build's contents
shows why that cannot sync on its own: four things block a sync after
moved lines, and the first two only work together.
1. *Identity.* `TexMachine::at` hashed `line` into the key, so after a
   deletion no later position in the file equals an old one.
2. *Changed cells.* `changed_cells` (`machinehost.rs`) compares lines
   by index. For `delete-par`, 171 of the file's 204 lines differ by
   index, and 2 by `difflib`'s diff (checked on the course copy). Every
   later region of `ch15.tex` reads some of the 171 as `Line` cells in
   D, so even a boundary found by a renamed key would be followed by
   dirty regions.

   The diff alone cannot be used as the changed set either. An old
   region reading `Line(ch15, k)` would not be marked, would replay
   against new line `k`, which is another line, and would be unsound.
   The old build has to be renamed through the diff: keys, `Line`
   guards and writes, the readers' index, and `Positions`. Renaming the
   keys needs the line apart from the hash, which is what this task
   builds.
3. *Positions in `Rest`.* The part-1 switch has to be on
   (`PARTEX_MACHINE_RENAME=1`). The engine's copies (`mode_line`,
   `if_line`, `skip_line`, `pack_begin_line`: part 2) are constant at an
   outer clean point, with no conditional open. `pack_begin_line` is
   reset to 0 after every paragraph and alignment, in `linebreak.rs:142`
   and `align.rs:551`. So part 2 is needed for syncs inside paragraphs,
   not for the first syncs at clean points.
4. *Line numbers read as values* (`\inputlineno`, the warnings) are
   true dependencies, to become derived cells like the numbering's
   answers.

Also found: object names are seeded by the whole position, line
included. An object made past moved lines gets another name, held in
the page's link whatsits and in object entries. The seed should come
from the renamed position, or from the position without its line: the
naming rule of Task 14d, applied to lines.

The coordinator's expectation, that the differences left after a
boundary sync would be the line-number copies in `Rest`, is corrected.
The first blocker after identity is the index-based changed lines, and
the copies of part 2 come later, for syncs inside paragraphs.

The plan and the task split (T20–T25) are in DESIGN §7.16.5, "Sync
across moved lines: the plan".

**Step 2: the first piece.** `Machine::Boundary` for TeX is now
`machine::Pos { key, file, line }`:
- `line` is the innermost file's line (§304);
- `file` is the hash of that file's name (or of the file below the
  output routine, at a stop before `\shipout`);
- `key` is everything else `at` hashed before: the input stack's shape,
  the place in the line or token list, the pages shipped at a ship stop.

Equal positions are equal in all three, so the regions and the syncs
are the same as before. Object names are still seeded by the old single
hash (`position_hash`, unchanged), so no name changes. The store layout
is `partex machine build/11`: a trace's entry and exit are saved as
`Pos`. There is no switch, since there is no change in behaviour.

This piece moves no number by design: it makes the keys renamable, and
T22–T23 rename them. So the measurement below is the no-change check.

**Measurements.** Timed harness below. Sanitizer:
`PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word`,
`enter`, `delete-par` and `word2` (07:05–07:10), with Task 14f's counts:

| edit | commands | dirty / total |
|---|---:|---:|
| space | 32,870 | 1 / 569 |
| word | 298,713 | 3 / 569 |
| enter | 228,283 | 8 / 569 |
| delete-par | 1,223,359 | 12 / 569 |
| word2 (second rebuild) | 10,015 | 4 / 588 |

Timed, 07:10–07:20, `bench/edits.sh` with `PARTEX_MACHINE_PARTS=7` on
the course probe copy. Conditions: release binary from `2508421` plus
this change; in process; load average 6–11.

| edit | rebuild ms | commands re-run | dirty regions / total | cuts | restores (n, ms) | link ms |
|---|---:|---:|---:|---:|---:|---:|
| space | 109 | 32870 | 1 / 569 | 9 | 1, 22.8 | 63.3 |
| word | 448 | 298713 | 3 / 569 | 22 | 4, 48.9 | 67.7 |
| label | 195 | 40056 | 2 / 569 | 16 | 2, 34.4 | 65.1 |
| footnote | 2243 | 1977503 | 18 / 569 | 114 | 8, 99.5 | 66.5 |
| enter | 526 | 228283 | 8 / 569 | 50 | 3, 44.1 | 66.4 |
| word-local | 129 | 32870 | 1 / 569 | 9 | 1, 24.0 | 68.6 |
| delete-par | 1366 | 1223359 | 12 / 569 | 67 | 3, 43.8 | 68.3 |
| word2 | 168 | 10015 | 4 / 588 | 4 | 5, 63.7 | 70.9 |
| space2 | 33 | 1114 | 1 / 577 | 1 | 1, 19.5 | 70.2 |
| enter2 | 74 | 6263 | 3 / 611 | 3 | 2, 38.4 | 68.3 |

Every count is Task 14f's. With `PARTEX_MACHINE_PARTS=7`:
- `delete-par`: the span from 353 (1,216,701 commands, entered through
  19 `Line` cells from line 19 on), then 568 (the end);
- `enter`: the span from 357 (221,636 commands, 26 `Line` cells from
  line 68 on), then 568.

Both spans read the moved lines through index-based `Line` cells, as
step 1 says. T21–T23 are what changes them.

**Gate.** `scripts/sandbox cargo xtask check` passed at 07:21–07:26 (5
min 6 s): fmt, clippy, wasm32, the tests (the store round trip with
`Pos`), and e2e 34/34 identical in both modes, including
`machine_edits` (which inserts lines), `machine_edits_dvi` and
`modern_watch_sanitized`.

Time: 06:56–07:26, about 30 minutes.

### Merge of Task 20; a third agent for one mission (coordinator, 07:27–07:43)

`622ebf6` was merged into master as `4560346`, on top of Task 27. The
gate passed (e2e 34/34 identical in both modes). The course sanitizer
passed on seven edits (07:42), counts unchanged: Task 20 changes a
boundary's representation, not its equality. Agent A's question: Tasks
21 and 22 are one task. `changed_cells` by the diff is sound only with
the old build renamed, so both go in behind `PARTEX_MACHINE_RENAME=1`
(off), and Task 23 turns it on.

*Adaptive granularity.* The user asked why each lift out of `Rest` was
a special case, when a dynamic dependency tracker would find what the
page builder reads (the lines' heights) on its own. The answer, for the
record:
- `Rest` is the untracked remainder, compared by one hash. Tonight's
  lifts moved parts of it into tracked cells, chosen by a human reading
  probes.
- Field-level tracking everywhere would cost multiples of a plain pass;
  cell-level tracking already costs +16% (302 G against 260 G).
- The middle ground is hierarchical. Tracking is automatic inside chosen
  boundaries: the fold's `Page` should be versioned by the builder's
  tracked reads, not by a hand-written outline. Granularity is refined
  where a coarse dependency causes spurious re-runs.

The user approved a third agent (C, branch `granularity`) for one
mission only: a smoke test of that signal, a spurious-dependency audit
checked against tonight's five lifts with their switches off.

### Task 12: restore by rebase (Agent, branch cuts)

First, a fix to the typing rows, merged with Task 27. The in-process
path linked after the cold build and once at the end. A `;;` sequence
(`word2`, `space2`, `enter2`) therefore did not link between its
rebuilds, and the last link covered every edit's changes. The path now
links after each rebuild that ran to its end, as a watch does (a
rebuild stopped for a newer edit is not linked). `Linker::finish` only
prints the last link's terminal text. `bench/edits.sh` pairs each
rebuild with its own link line, so with `ALL=1` each row `NAME#k` shows
one keystroke's link.

A restore (`restore_rest`) makes a new engine from the snapshot
(`Snapshot::engine`, a clone) and thaws it. Before this task the thaw
rebuilt every running vector from the snapshot's chunks. For the token
lists, `TokStore::thaw_from` took the unchanged ones from the engine
being replaced, but moved every record into a new vector.
`PARTEX_CUT_TIMING` now times a restore by piece (`RESTORE_PIECES`, the
thaw by container: `Tex::thaw_timed`).

Before, master `1a7ade6` plus the timing, the course, in process, µs per
restore:

| piece | `footnote` (9 restores) | `word` (4) |
|---|---:|---:|
| the snapshot's engine cloned | 259 | 230 |
| eqtb's differences and the cells exported (`keep_eqtb`) | 1089 | 0.3 |
| thaw: the token store | 2315 | 3711 |
| thaw: the `JVec`s (eqtb 5 MB, hash 5 MB, save stack 1.6 MB, rebuilt) | 3683 | 3806 |
| thaw: the `Flat`s (3.3 MB rebuilt) | 1290 | 1279 |
| the cells imported, and the rest taken over from the old engine | 241 | 234 |
| the old engine dropped | 551 | 463 |
| all | 9427 | 9726 |

Built: the running engine's containers are rebased, not thawed anew.
- *`JVec::thaw_from(old)`* (`journal.rs`) takes `old`'s running vector.
  It copies a chunk only if `old`'s base chunk is not the snapshot's
  (`Arc::ptr_eq`), or if `old` wrote it since (a dirty block). It also
  reuses `old`'s dirty bits.
- *`Flat::thaw_from(old)`* (`flat.rs`) takes `old`'s running vector,
  resized to the snapshot's length. It copies a part only if `old`'s
  shadow part is not the snapshot's, or if `old` may have written it
  (neither clean nor wholly below its floor, T6). What lies past the
  captured prefix is dead and is left as `old` had it.
- *`TokStore::thaw_from`* rebases in place (`rebased`). `old`'s record
  vector is kept: a chunk it shares with the snapshot, unchanged, is
  left where it is, and so is a list equal to the snapshot's by content.
  Only the other lists are thawed. Before, every one of the 58k records
  was moved into a new vector.
- *`Tex::thaw_from`* does the three.
  - Two things that read the old engine's vectors after the thaw now
    read before it. These are the hash links of the cells taken over
    (`slot_link`), and the fonts' names by their characters: a new
    `Tex::font_names`, then `take_fonts_named`. They read the old
    engine's strings, which the rebase moves.
  - The first run without this panicked on the old engine's empty
    string pool.
- *Exactness.* In debug builds, each container's rebase asserts it holds
  the snapshot's elements. Under `PARTEX_MACHINE_SANITIZE=1`
  (`VERIFY_REST`), and in debug builds, `restore_rest` also thaws the
  snapshot in full and asserts that the rebased engine equals it
  (`Tex::same_thawed`). It compares the `JVec`s whole, the `Flat`s to
  their live prefixes, and the token lists with their records.
- *Switch.* `PARTEX_RESTORE_REBASE=0` (`set_rebase`) thaws every vector
  anew, as before.

After, same conditions (07:24–07:26), µs per restore:

| piece | `footnote` (9) | `word` (4) |
|---|---:|---:|
| the snapshot's engine cloned | 259 → 258 | 230 → 226 |
| eqtb's differences and the cells exported | 1089 → 1028 | 0.3 → 0.8 |
| thaw: the token store | 2315 → 1657 | 3711 → 3240 |
| thaw: the `JVec`s | 3683 → 1065 | 3806 → 2051 |
| thaw: the `Flat`s | 1290 → 220 | 1279 → 488 |
| the cells imported and the rest | 241 → 189 | 234 → 196 |
| the old engine dropped | 551 → 150 | 463 → 169 |
| all | 9427 → 4568 | 9726 → 6371 |

`word`'s restores gain less than `footnote`'s. They are `replay_exit`'s:
a restore of the old run's snapshot into an engine that re-ran regions,
so its chunks share less with that snapshot (`word`'s `JVec`s still
copy about half their chunks). The token store's cost is now its walk
over 914 chunks and 58k records: `shares_chunk`, and per record
`same_as` or a thaw where a chunk is not shared.

The ten rows, `ALL=1`, with the rebase and with
`PARTEX_RESTORE_REBASE=0`, run side by side at 07:31–07:40 with the four
sanitizer runs (load high), restores as (n, ms):

| edit | rebuild ms, rebase / thaw | restores, rebase / thaw |
|---|---:|---:|
| space | 136 / 126 | 1, 32.9 / 1, 25.6 |
| word | 463 / 499 | 4, 43.5 / 4, 56.1 |
| label | 235 / 245 | 2, 38.5 / 2, 43.5 |
| footnote | 2255 / 2273 | 8, 59.8 / 8, 100.0 |
| enter | 528 / 579 | 3, 37.3 / 3, 48.6 |
| word-local | 126 / 131 | 1, 25.5 / 1, 26.5 |
| delete-par | 1315 / 1323 | 3, 35.2 / 3, 45.7 |
| word2#1 | 433 / 448 | 4, 34.9 / 4, 47.9 |
| word2#2 | 118 / 130 | 5, 32.0 / 5, 46.7 |
| space2#1 | 154 / 160 | 1, 29.3 / 1, 32.0 |
| space2#2 | 48 / 47 | 1, 27.9 / 1, 28.4 |
| enter2#1 | 556 / 517 | 3, 36.7 / 3, 43.9 |
| enter2#2 | 73 / 79 | 2, 32.8 / 2, 39.6 |

- The restore column includes all of `replay_ns` (the cells patched by
  `set` too), not only `restore_rest`. So one restore still shows as
  about 25 ms for `space`, where `restore_rest` itself is about 5 ms.
  That column's remainder is the next thing to measure.
- In these runs `word2#2`'s link read 1,294 ms (1,054 with the thaw),
  at a load of seven heavy processes, four of them sanitized at over
  1.6 GB each. Alone, with `PARTEX_CUT_TIMING`, it is 20.3 ms: 4 regions
  resolved, 584 taken from the last link. The 1.3 s was memory
  pressure, not the link.
- The PDFs of all ten rows are the same with and without the rebase,
  apart from `/ID`.

Other gates:
- The plain pdflatex pass is 260.666 G instructions, against 260.666 G
  before.
- `PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word`,
  `footnote` and `word2`, with the rebase-equals-thaw check on at every
  restore. Their PDFs match the thaw runs apart from `/ID`. Under the
  sanitizer the restore and link columns include the full thaws and the
  full links it checks against.
- The full gate (`scripts/sandbox cargo xtask check`) passed on its
  second run: e2e 34/34 in both modes, and trip and etrip in both modes.
  The first run failed one lint (`machinehost::run` over 100 lines),
  fixed by moving the per-rebuild link into `Linker::after_rebuild`.
- The debug builds' checks ran in the unit tests: the per-container
  asserts and `same_thawed` at each restore of the machine tests.

Time: about 40 minutes (07:12–07:53).

### Task 21–22: the rename through the line diff (Agent, branch units)

Started from `4560346`, which merged Task 20 on top of B's Task 27. It is
behind `PARTEX_MACHINE_RENAME=1`, off by default: alone it is not sound
for regions that print a line number (Task 25), and Task 23 turns it on.

**`partex-incr`: `Build::rename(&Renaming)`.** A machine whose cells or
boundaries name places in an input maps them through the input's diff:
- `cell`: a cell's new name, the same content under it; `None` means
  gone.
- `boundary`: a boundary's new name; one inside what changed gets a
  name no new position has.
- `value`: a value that holds names, renamed.
- `guard`: a guard's version under its new name; the default keeps it.
- `state`: the final state renamed, where it holds names the rebuild
  does not patch.

It renames every trace's entry and exit, and every guard's and write's
cell:
- a renamed cell keeps its version;
- a guard on a cell whose value was renamed takes the renamed version
  of the write before it (its value at the region's entry, the chain);
- a write of a gone cell is dropped.

A region that read a gone cell is **forced**: it is dirty in the next
rebuilds until it runs again. `Build::forced` is a set of keys;
`splice_staged` removes a region from it when a splice replaces it, and
carries the set through a renumbering. The cells that differed in
earlier rebuilds and a stopped rebuild's frontier are renamed too. The
three indexes are made again.

**Stub.** `Program::renumbered()` numbers a program's lines by place,
as a file's lines are, and gives each old id's new one.
`Stub::renamed` renames a final state.

`renamed_rebuilds_match_scratch` covers every incremental config:
- 120 seeds, half with the accumulator;
- six random edits each, inserting, deleting and replacing lines, on a
  place-numbered program;
- after each edit, the program is renumbered and the build renamed: a
  line whose value the edit changed is gone (its text, or its
  successor, which an insertion or a deletion moves), every other line
  and boundary moves to its new place, and a renamed line's guard takes
  the new program's version (a stub line holds its successor's id);
- the rebuild's output and final state must equal a build from
  scratch, and the sanitizer configs check the chain.

`a_moved_line_replays_and_a_changed_one_reruns` covers the two cases
asked for:
- a line inserted at the top moves every other line, and the rebuild
  runs at most 3 commands: the new line's region; the rest replays
  renamed;
- a guarded line whose content changed makes its region and the
  reader of what it set run again.

Found while writing them: the final state holds names too (the stub's
program), hence `Renaming::state`. A region before a replaced line
must keep its exit named, so only a deleted line's boundary gets a new
name.

**TeX side** (`machine.rs`, `machinehost.rs`):
- `Pos::file` is now the hash of the file as the host serves it
  (`input_file[index].name`, §300; `machine::file_key`), so that a
  rename can find the boundaries in a changed file.
- `edited_with_maps` computes, with the switch on, one hunk per changed
  file from the common prefix and suffix of its lines (`line_hunk`).
  The changed cells are then the file and the hunk's new lines, not
  every line after it by index.
- `machine::rename_lines` renames the build before the rebuild
  (`LineMap`). An old line before the hunk keeps its number, and one
  after it moves by the difference. The hunk's lines are gone, and so
  is the line right after it: the region that read it went there from
  the hunk, and an insertion puts new lines in between. A line count
  inside the hunk (a boundary's `line`, `Positions`' counters and lines
  read) gets a number no new place has. `Positions` values take new
  versions.
- The watch loop does not rename yet; that is Task 23.

**Measured with the switch on** (07:43–07:45, `PARTEX_MACHINE_PARTS=7`,
course probe copy, release binary from `4560346` plus this change, two
sanitized runs in parallel):

| edit | rebuild ms | commands re-run | dirty regions / total | cuts |
|---|---:|---:|---:|---:|
| enter | 525 | 228283 | 8 / 569 | 54 |
| delete-par | 1357 | 1223359 | 12 / 569 | 71 |

The syncs now happen. Before, `enter` re-ran one span from 357 (221,636
commands) and `delete-par` one from 353 (1,216,701), because no old
boundary after the edit could be found. Now the rebuild finds each old
boundary and checks each region, and the spans are per region:

| edit | spans (commands) |
|---|---|
| `enter` | 357 (32,885), 358–363 (188,751 together), 568 |
| `delete-par` | 353 (259,070), 354–356 (736,009), 357–363 (221,622), 568 |

The regions after the edit are really dirty:
- `delete-par` 354: through `Rest`;
- `delete-par` 355–356: through `Rest` and `\@currenvline`, the line
  number LaTeX's `\begin` stores (`\on@line`: `\the\inputlineno`), a
  line read as a value, Task 25;
- `enter` 358–363 and `delete-par` 357–363: through `Rest`, `Page` and
  the sealed lines of the pages whose breaks moved.

So the counts are the same as with the switch off (228,283 and
1,223,359), and the reason changed: what is left is the page reflow
(the fold, Tasks 15–18), `Rest` (to diagnose; part 2's line-number
copies are the likely part), and `\@currenvline` (Task 25). The
coordinator's expectation for `enter` (357 clean after the rename) does
not hold, because the split paragraph moves the page breaks after it.

**Sanitizer with the switch on:** `PARTEX_MACHINE_SANITIZE=1` passed for
`enter` and `delete-par` (07:43–07:46), with the counts above. Every
region after the edit re-ran, so no reused region printed a stale line
number; that risk is Task 25's.

**With the switch off** (the default), 07:48–08:00, same conditions, load
average 9: all ten rows re-run the counts of Task 20:

| edit | commands |
|---|---:|
| space | 32,870 |
| word | 298,713 |
| label | 40,056 |
| footnote | 1,977,503 |
| enter | 228,283 |
| word-local | 32,870 |
| delete-par | 1,223,359 |
| word2 | 10,015 |
| space2 | 1,114 |
| enter2 | 6,263 |

The link times are lower than Task 20's (1–59 ms): B's Task 27, merged
underneath, and nothing of this task.

**Gates.**
- `cargo test -p partex-incr`: 3 unit tests and 19 property tests pass,
  the two rename tests included.
- `scripts/sandbox cargo xtask check` passed at 08:00–08:05 (5 min 8 s):
  fmt, clippy, wasm32, the tests, and e2e 34/34 identical in both
  modes, including `machine_edits`, `machine_edits_dvi` and
  `modern_watch_sanitized`.

**Next, for Task 23**, in order of what the numbers show:
1. Diagnose `Rest` at `delete-par` 354–356 and `enter` 358–363 with
   the switch on (`PARTEX_MACHINE_PARTS=4`): part 2's copies or the page.
2. Task 25's line value (`\@currenvline`), which makes 355–356 dirty
   and is a true dependency only where it is printed.
3. The fold, for the reflowed pages.

Turning the switch on by default needs Task 25 first, or the sanitizer
has to prove on the whole harness that no region reused past a moved
line printed one.

Time: 07:28–08:06, about 38 minutes.

### Adaptive granularity: the spurious-dependency audit (Agent C, branch granularity)

Started from master `4560346`. The question, from the user: could the
system find by itself the refinements of `Rest` that were made by hand
tonight (the page builder, the `\pdflast*` values, virtual names by TeX
identity, the numbering's answers, the dead scratch)? The proposal is to
start coarse and refine a dependency when it makes a region re-run
*spuriously*, that is, re-run and end exactly as it ended before. This
entry is the smoke test: an audit that computes that signal on every
rebuild, run once with the five lifts off (about the state at midnight),
to see if it points at what the lifts removed, and once with them on, to
see what it points at now.

**What was built.** `PARTEX_MACHINE_AUDIT=1` (off by default; a
diagnostic, and with it off nothing changes).
- `partex-incr` (`build.rs`): `Config::audit`. For each span a rebuild
  re-executes, `Build::audit` gets an `Audit` record: the first old
  region's index, the old regions' costs, the commands re-run, whether
  `S` was at the region's entry, the cells of `D` the region read with
  their values at its entry in both runs, the cells that differ at the
  span's end (a `Differ` each: in `D` at the entry already or not, both
  values at the end, and for an accumulating cell whether both runs
  added the same), and whether the effects are the same. `spurious()` is
  the strict test: nothing differs at the end and the effects are the
  same. The end is what `rebuild_or_stop` already compares after a
  re-execution: every cell either run wrote, at the old entry the run
  reached, by version, `Rest` included.
- `partex-core` (`statehash.rs`): `Tex::rest_field_differences`, the
  parts of `Rest` in which two states differ, by field where the
  existing comparisons tell: `rest.<section>`, `scalars.cur_val`,
  `pdf.<field>`, `pdf.ship.<field>`, `pdf.out.<field>`,
  `pdf.objs.head[<type>]`. The `\pdflast*` fields are left out when
  they are cells, and the ship's fonts always (their hash is empty).
- `partex-cli` (`machinehost.rs`): `dump_audit`, after each rebuild. One
  line per re-executed old region, then a ranking of causes.
- A property test (`the_audit_changes_nothing_and_counts_every_span`):
  with the audit on, the stub's statistics and output are the same, and
  the records account for every old region and command re-executed.

**A span is compared as a whole.** A dirty region runs until the machine
reaches the entry of a later old region, and the comparison is at that
point, so a span of several old regions has one verdict. The audit gives
each of the span's regions a line, and gives the later ones the first
one's cause ("in the span of region 353, its cause"). They re-ran
because the run had not met an old entry yet, not because a guard of
theirs failed. On these rows only the spans that begin at the edit
(`delete-par` 353, `enter` 357, `word2#2` 356) have more than one
region.

**The strict test is nearly blind with a lump.** The first run of the
audit had only the strict test. With the five lifts off it found one
spurious region on `label` and one on `footnote`, and none on `word`,
`enter`, `delete-par` or `word2`. The reason is that every region reads
and writes `Rest`. A live field that differs, such as `pdf.last_link`
after an edit to its left on the line, passes unchanged through every
region until something overwrites it, so every region's exit `Rest`
differs too, and the strict test calls each re-run real. A lumped cell
defeats "the same writes" by construction.

**The signal that works: carried differences.** So the audit has a second
class. A span *carried* a difference if its effects are the same and
everything that differs at its end differed at its entry already and
was not changed by either run in the span:
- for `Rest`: every field that differs at the end differed at the entry,
  and neither run changed it between its own entry and exit (the fields
  in which each run's entry and exit differ);
- for an accumulating cell (`Numbering`, `Glyphs`, `Dests`, `Written`):
  both runs added the same. The deltas each run wrote are added to the
  state at the span's entry, and the versions compared.

A carried span re-ran only because its entry differed in something it
passed on unchanged. Were that field a cell of its own, the span would
not have re-run for it, unless it read it; if it did read it, the
refinement would leave it dirty through the new cell, so the signal can
be acted on and checked. The rest is *real*.

The first version compared the accumulating deltas region by region.
That missed the numbering: a re-executed region is cut finer, and each
write's version is that of the whole accumulated value. Adding each
run's deltas to one entry state fixed it (region 485 below).

**Step 2: the signal against tonight's lifts.** Runs at 08:04–08:07,
release binary from `4560346` plus this change, in process, harness
`bench/edits.sh` with `ALL=1` on the probe copy `course-gran`, with
`PARTEX_MACHINE_PAGE_CELL=0 PARTEX_MACHINE_PDF_LAST=0
PARTEX_MACHINE_TREE_NAMES=0 PARTEX_MACHINE_NUM_ANSWERS=0
PARTEX_MACHINE_CANON=0 PARTEX_MACHINE_AUDIT=1`. The load average was
8–13, so no times are given. The counts are deterministic and are those
of the history in this log (word 1,454,185, enter 1,722,276, footnote
2,070,065, label 73,989, delete-par 2,717,352, word2#2 1,377,968).

| row | re-run | spurious | carried | real |
|---|---:|---:|---:|---:|
| word | 1,454,185 | 0 | 1,162,119 (7 regions) | 292,066 |
| enter | 1,722,276 | 0 | 1,170,278 (5) | 551,998 |
| footnote | 2,070,065 | 92,813 (1) | 548,905 (4) | 1,428,347 |
| delete-par | 2,717,352 | 0 | 794,137 (4) | 1,923,215 |
| word2#2 | 1,377,968 | 0 | 1,368,197 (29) | 9,771 |
| label | 73,989 | 33,933 (1) | 0 | 40,056 |

The lifts, with the waste each removed (the regions that became clean,
from the entries of Tasks 14–14f) and what the audit says about those
regions with all five off:

| lift | removed (row: regions, commands) | the audit, all lifts off | verdict |
|---|---|---|---|
| page builder (`PAGE_CELL`) | nothing on these rows (Task 14: counts equal with the cell on or off) | never names `rest.page` | correct negative |
| `\pdflast*` (`PDF_LAST`) | word 354–356: 736,009 | carried, sole cause `pdf.last_link`, 736,009; first in the row | hit, exact |
| | word2#2 358–370: 984,738 | carried, sole cause `pdf.last_link`, 13 regions, 984,738 | hit, exact |
| names by identity (`TREE_NAMES`) | enter 364–371: 1,133,781 | first in the row: `pdf.objs.head[2]` (the Pages list) and `pdf.ship.last_pages`; carried 364, 366–368 (810,066), real 365, 369–371 (323,715) | hit, 71% of it |
| | delete-par 366–371: 876,770 | the same cause first; carried 366–367 (400,093), real 368–371 (476,677) | hit on rank, 46% |
| numbering answers (`NUM_ANSWERS`) | word, enter, delete-par 485: 360,212 each; word2#2 514 and 598: 304,284 | carried, sole cause `Numbering` | hit, exact (with deltas compared by what they add) |
| dead scratch (`CANON`) | word 358–359: 59,251 | carried, sole cause `pdf.ship.link_list` | hit |
| | label 358: 33,933 | spurious, sole cause `scalars.cur_val` | hit, strict |
| | footnote 486: 92,562 | carried, sole cause `scalars.cur_val` | hit |
| | word2#2 375–387: 78,931 | carried, sole cause `pdf.ship.link_list`, 13 regions | hit, exact |
| | delete-par 364–365: 257,011 | 364 carried (33,832); 365 real (223,179) | partial |

So the top of every row's ranking is the lift that fixed it, and every
lift that removed waste on these rows is found. Of the 5,637,906
commands the lifts removed on these six rows, 4,614,335 (82%) are in
regions the audit flags spurious or carried; the strict test alone
flags 33,933 of them (label 358). The misses, 1,023,571 commands in
`enter` and `delete-par`, are mostly one kind. The regions that make
objects or ship pages (365, 369–371: `Obj`, `Dests`, `Numbering` differ
at their end), with names seeded by position, write deltas and effects
that differ in names only. The audit compares representations, so a
renaming looks like a real difference. The rest is dead scratch made
different: at the exits of `delete-par` 368 and of 370 the scanner's
`cur_val` differs, a new difference, where it did not at the entry. That is exactly what the
naming rule of Task 14d (§7.16.5) is for: it is not a split of a cell,
and a test of the representation cannot find it. Finding it
automatically would need the comparison modulo names (final numbers,
the link's resolution), which the audit does not do. The dead scratch
is found where the field differs at the entry and is overwritten or
passed on in the span (label 358, footnote 486, word 358–359). It is not
found where a region *makes* a new difference in it (`cur_val` at 368's
exit): a dead field that differs at the exit cannot be told from a live
one without its reads.

**Step 3: what is left on master.** The same six rows with the lifts on
(the defaults) and `PARTEX_MACHINE_AUDIT=1`, same conditions. The counts
equal master's (298,713, 228,283, 1,977,503, 1,223,359, 10,015, 40,056).

| rank | cause | row: regions | spurious + carried | real, same cause |
|---:|---|---|---:|---:|
| 1 | the save stack's saved values (`rest.tables`) and the destination list's head (`pdf.objs.head[5]`), together | footnote 354, 356 (carried; 458 with an eqtb cause too) | 414,165 (436,373 with 458) | 354,714 (355, 357) |
| 2 | `\@gtempa` (an eqtb cell, sole cause) | footnote 567 (spurious), 454 (carried) | 344,697 | |
| 3 | `Numbering` and `Glyphs` at the job's end | word, enter, word2#1: 568; word2#2: 587 | 6,647 each; 244 | |

Nothing else: `delete-par` and `label` have no spurious or carried
region, and every other re-run is real work. That is the edited lines
(`Line`), the page ships that read the reflowed paragraph's sealed lines
and objects (`Obj`, `Sealed`), the `.aux` (`Written`), and after a
footnote the regions that read its counters (an eqtb register above
255) and write different destinations. Candidate 1 is Task 14c's
save-stack case: the stops before `\shipout` inside the output routine,
about twenty groups deep, and the footnote's new destination.
Candidate 2 is a scratch macro read and then set to the same meaning
(its old value did not matter). A refinement of it would be a finer
read (the answer the region needed), not a finer cell. Candidate 3 is a
whole reader and would stay one.

**Step 4: a what-if on the top candidate.** `PARTEX_MACHINE_WHATIF_OMIT`
(`statehash::WHATIF_OMIT`, 0 by default, commented as unsound and for
measurement only). Bit 1 leaves the save stack's saved values out of
`Rest` (its pointer, level, group and boundary stay, so the group
context still guards). Bit 2 leaves the destination list's head
(`head_tab[dest]`) out of the PDF writer's hash, everywhere, so the
sanitizer's state comparison does not see it either, though its output
comparison does. Measured 08:10–08:20, same conditions, with
`PARTEX_MACHINE_SANITIZE=1` unless said:

| omitted | footnote commands | dirty regions | sanitizer |
|---|---:|---:|---|
| nothing (master) | 1,977,503 | 18 / 569 | passes |
| saved values (`=1`) | 1,977,503 | 18 / 569 | passes |
| destination head (`=2`) | 1,977,503 | 18 / 569 | passes |
| both (`=3`), without the sanitizer | 1,208,624 | 14 / 569 | |
| both (`=3`) | | | **fails**: the rebuild's final state differs from a fresh build's |

With `=3`, `word`, `enter`, `delete-par`, `word2` and `label` keep
master's counts and pass the sanitizer, since neither part differs
there. On `footnote` both parts must go for 354–357 to be clean
(768,879 commands, 39% of the row). That includes 355 and 357, which the
audit calls real, and the result is wrong. So the upper bound is 768,879
commands, and it is not reachable by leaving the parts out. The saved
values are read when the output routine's groups end (`unsave`,
§281–§283), and the new destination is a real object that the ship
(357) writes. This is Task 14c's finding again, now from the other side.
A refinement that would be sound is a cell per saved entry, read where
`unsave` restores it, so that the regions which only carry the entries
are clean and the one that ends the groups reads them. That is a design
task, not a what-if. The audit's carried verdict on 354 and 356 is
consistent with this: those two regions pass the entries on, and the
reader comes later.

**Conclusions.**
- The signal points at the right things, once it looks at the fields a
  lumped cell carries rather than at the lump. The strict test (same
  writes, same effects) flags 0.6% of what the lifts removed; the
  carried test 82%, and it ranks each row's lift first. So adaptive granularity looks feasible: start with `Rest` as
  one cell, and when regions re-run only to carry a field of it on, make
  that field a cell. The field-level comparisons it needs
  (`rest_field_differences`) exist for debugging already.
- It needs the fields' own resolution. Where the diff is coarse (the
  save stack is one section, `rest.tables`), "changed by the run" is
  true of the whole section, and the carried test fails. Footnote 355
  fails that way though 354 and 356 pass.
- It cannot find a renaming. Names seeded by position (Task 14d) differ
  in the representation and not in the output. Comparing modulo names
  would need the link's resolution in the audit.
- It cannot find dead scratch that a region makes different, only
  scratch that differs at the entry. Liveness needs reads, which is why
  Task 14f's poison test was the proof and not this.
- An acting system would refine a field when it is carried by regions
  whose commands add up (here, one row's re-runs are enough to rank),
  then check the refinement by the next rebuild's counts, which the
  harness does.

**Rejected.**
- Comparing a span region by region: the new run's cuts are not the old
  run's, so there is no pairing below the span; the first region's cause
  is used for all of them, as said.
- Comparing accumulating cells by their writes: partition-dependent, and
  the versions are of the whole value (the miss of region 485 above).
- Counting a span with several causes under one of them: each cause gets
  the span, and "sole cause" is counted separately, so a lift's upper
  bound can be read off.

The audit costs time when on: a machine clone and a `Rest` snapshot at
each dirty entry, and two clones per accumulating cell that differs at a
span's end. It is a diagnostic.

**Gate.** With the audit off nothing changes. The ten harness rows
(binary with this change, no audit, 08:10–08:12) give master's counts:
space 32,870; word 298,713; label 40,056; footnote 1,977,503; enter
228,283; word-local 32,870; delete-par 1,223,359; word2 10,015; space2
1,114; enter2 6,263. With the audit on, the six rows of step 3 give the
same counts, so the audit does not perturb the rebuild on the course
either. `scripts/sandbox cargo xtask check` passed at 08:21–08:28 (6
min 32 s): fmt, clippy (both feature sets), wasm32, the tests (with the
new property test), trip 7/7 and etrip 18/18 in both modes, and e2e
34/34 identical in both modes. On the way, two slips in the environment,
not in the change:
- The fresh worktree had no `upstream/` and no `refs/tex/trip`. I
  fetched the first with `scripts/sandbox --net scripts/fetch-upstream.sh`
  and made the second with `cargo xtask oracle --engine tex --filter
  trip`.
- Running `etrip` and `etrip --machine` side by side while
  `refs/pdftex/etrip` was missing made both write the oracle there at
  once, and ctrip differed. Regenerated alone, it is 18/18.

Time: 07:41–08:30, about 49 minutes.

### Merge of Task 12 (coordinator, 07:54–08:08)

`906a8be` was merged into master as `97d78c2`, on top of Task 20. The
merge was clean. The gate passed on the merge (e2e 34/34 identical in
both modes). The course sanitizer passed on seven edits (08:08), with
the rebase-equals-thaw check (`same_thawed`) on at every restore and
the counts unchanged: space 32,870, word 298,713, footnote 1,977,503, label 40,056, enter 228,283, delete-par 1,223,359 and word2 10,015 commands, 0 panics.

What a keystroke waits on is now to be measured, not estimated. Task
12 found that the harness's restore column is all of `replay_ns`, so
`space`'s one restore shows as about 25 ms where `restore_rest` is about
5 ms. Task 12b (Agent B, briefed 07:55) profiles one keystroke in phases
that sum to its wall time, explains that remainder and fixes the
largest piece that is not TeX. The target in the original queue, a
restore under 0.5 ms, needs the token store's walk to cost what differs
(914 chunks and 58k records are walked today) and the `JVec`s as a
persistent tree; both are queued after Task 12b.

### Task 12b: a keystroke's profile (Agent, branch cuts)

`PARTEX_CUT_TIMING=1` now prints, after each rebuild of the in-process
path, one line: the keystroke's timeline from the edit to the files
written ("keystroke timeline, ms"). It is built from:
- the edit's time (`edited`, the edit applied to the served files and
  the changed cells found);
- the rebuild's `Stats`: `run_ns`, `replay_ns`, `compare_ns`, and
  `other_ns` with its parts (start, splice, old states dropped);
- the cuts' sums;
- the restores' pieces;
- a new split of `replay_exit` (`REPLAY_PIECES`: `restore_rest`; the
  positions and the span's accumulating cells set; the patched cells
  set; the tracker reset and the snapshot taken);
- the link and the write (`Linker::timed`).

The cuts taken inside replays are taken out of the "TeX re-run" cuts.
The line checks itself: the pieces leave 0.6–1.1 ms unaccounted, under
2% of the wall. The idle work after the keystroke (`between_edits`'s
coarsening, 1.4–1.6 s) is reported beside the wall, not in it, as a
watch does it after writing.

The timing's own costs moved to `PARTEX_CUT_TIMING=2`
(`CUT_TIMING_DETAIL`). These are a second clone of the token store per
cut and the engine's field groups cloned again per snapshot. With `=1`
the timing only reads the clock, so a rebuild's wall time is its own
(`word2#2` 118–121 ms with `=1`, against 118 ms in Task 12's rows
without timing).

Measured 08:22–08:38 on `~/code/tmp/course-ap-t4`, master `906a8be`
plus this. `space2`, `word2` and `enter2`, three runs each, one at a
time. The other agent's runs were active in another worktree, which hit
the first of each three, so the table takes the median. The second
rebuild of each row, ms:

| piece | `space2#2` | `enter2#2` | `word2#2` |
|---|---:|---:|---:|
| edit applied and diffed | 1.0 (0.9–6.7) | 1.0 (0.9–1.5) | 0.9 (0.8–1.8) |
| start: the initial state, changed cells marked | 0.2 | 0.1 | 0.1 |
| TeX re-run, without its cuts | 5.7 | 28.2 | 68.2 |
| its cuts (1, 4, 4) | 0.6 | 2.6 | 3.5 |
| replays (1, 2, 5): `restore_rest` | 12.6 | 16.3 | 20.6 |
| replays: the rest of `replay_exit` | 7.5 | 9.6 | 11.4 |
| other replay (exit patches, single regions) | 0.0 | 5.0 | 0.0 |
| compare | 0.2 | 1.5 | 6.3 |
| splice | 1.8 | 4.3 | 7.7 |
| other (the final cutoff) | 5.4 | 0.2 | 0.4 |
| link | 0.0 | 11.5 | 17.9 |
| write | 0.2 | 2.3 | 2.4 |
| wall | 35.7 (35.7–219.9) | 83.3 (81.4–96.1) | 139.7 (137.3–204.0) |

*`space`'s restore, explained.* The harness's restore column is all of
`replay_ns`. For `space2#2` one `replay_exit` took 20.2 ms (the
`space2-rp` run):
- `restore_rest` 12.5 ms: the token store 7.2, the `JVec`s 3.9, the
  `Flat`s 0.9, the clone 0.2;
- the span's accumulating cells set, 2.5 ms: `Glyphs` over the PDF font
  table, which the set copies once since it is shared with the snapshot,
  and the `FontOrder`, `Dests`, `Numbering` and `Written` cells;
- the tracker reset and the snapshot at the restored point, 5.2 ms: a
  cut, its `Rest` hash computed in full on the restored state.

This restore costs 12.5 ms, not Task 12's 4–6 ms average, because it is
the rebuild's first. It restores into the rebuild's start state (a clone
of the job's initial state), which shares no chunk with a snapshot in
the middle of the document, so the rebase copies nearly everything. The
later restores of `word2#2` are 4.2 ms each.

*TeX re-run is not TeX's speed.* `word2#2` re-runs three spans (old
region index, commands, ms): (356, 7,752, 24–25), (366, 2,019, 14.5)
and (587, 244, 32). The cold build runs about 1.2M commands a second,
0.86 µs each. These run at 3, 7 and 130 µs a command.
- The last one is the job's end region. Its time is not the font
  writer's: `writet1` is under 0.1% of a 20-keystroke profile.
- So re-running a region costs a fixed amount besides its commands, and
  that amount is not accounted for yet.

Not built, proposals in order of size:
1. *The fixed cost of a re-run region* (up to 30 ms per keystroke on
   `word2`). It needs a timer inside `record_span`: the first steps after
   a restore, the region's end (reads and writes to versions), and the
   job's end region.
2. *The first restore of a rebuild* (12–13 ms of `space2#2`'s 36). Start
   the rebuild's working state from an engine that shares chunks with
   the snapshots it will restore, for example the previous final state,
   instead of a clone of the initial state. Or make the token store's
   thaw lazy per list, copy-on-write from the frozen tokens: 58k thaws
   are 7 ms of it.
3. *The snapshot at the end of `replay_exit`* (5 ms): its `Rest` hash in
   full. `known` is false after a rebase that moved input files, and the
   hash memo does not cover the restored state.

Gates (08:39–08:52; all runs in parallel, so the times are loaded;
correctness only):
- The ten rows, `ALL=1`: the same commands, cuts and restore counts as
  Task 12's. The PDFs are the same as Task 12's thaw runs apart from
  `/ID`.
- The plain pdflatex pass is 260.666 G instructions, as before.
- `PARTEX_MACHINE_SANITIZE=1` on the course passed for `space`, `word`,
  `footnote` and `word2`.
- The full gate (`scripts/sandbox cargo xtask check`) passed: e2e 34/34
  in both modes, and trip and etrip in both modes. `machinehost::run`
  stayed under clippy's 100 lines by moving a keystroke's rebuild, link
  and idle work into `Keystroke::rebuild`, and the part dumps into
  `dump_parts`.

Time: about 60 minutes (07:55–08:53), over the 45-minute box. The first
set of timelines ran three processes at once and was discarded; so was a
profile of 20 keystrokes whose call graphs did not resolve (no frame
pointers).

### Merge of Task 21–22 (coordinator, 08:10–08:31)

`b7e7c01` (A's rename through the line diff, with two doc fixes of
mine: `index`'s comment back above `index`, and a stale link to
`edited`) merged as `d3b7a70`. The gate failed on clippy alone: after
the merge, `machinehost::run` was 101 lines of 100, because A's rename
call landed on top of Task 12's trimming (e2e was 34/34 identical).
`76e29e6` moves the rename inside `edited`, which now takes the build
mutably and renames it before returning the edits; behaviour is the
same. Gate on `76e29e6`: exit 0 (e2e 34/34 identical, machine mode).
Course sanitizer (seven edits, `PARTEX_MACHINE_SANITIZE=1`, rename off
by default): no messages, 0 panics, counts unchanged: space 32,870,
word 298,713, footnote 1,977,503, label 40,056, enter 228,283,
delete-par 1,223,359, word2 10,015.
### Task 23a: what keeps the regions after the edit dirty, with the rename on (Agent, branch units)

Started from `b7e7c01` (Task 21–22, with the coordinator's two fixes).
Diagnosis only; I stopped at the table, as the task allows.

**Method.** A new diagnostic, `PARTEX_MACHINE_PARTS=10`, runs after a
rebuild:
- it takes the regions the rebuild replaced (the garbage);
- at every old boundary the re-execution passed, it compares the new
  run's `Rest` there with the old run's, part by part as a machine
  hashes `Rest` (`Tex::rest_hash_parts_served`, the served scope);
- it prints details for each: the engine's line-number copies
  (`Tex::line_copies_difference`: the nest's `mode_line` §213, the
  conditions' lines §489, `skip_line` §493, `pack_begin_line` §661,
  `line`), the scanner's scalars, the save stack and the PDF writer.

Run on the course with `PARTEX_MACHINE_RENAME=1`, release binary from
`b7e7c01` plus the diagnostic, 08:14–08:19.

**The table** (new-build region indices; old ones in the Task 21–22
entry):

| field | where | class |
|---|---|---|
| `skip_line` (§493), in `input: conditionals` | every boundary, both edits | (a) a line copy. Also dead at a clean point: `pass_text` sets it before the only reader, §336's "all text was ignored after line N", reads it |
| the nest's `mode_line` (§213), in `lists` | boundaries inside a paragraph (`enter` 382; `delete-par` 358–369, 395) | (a), live: printed in "in paragraph at lines N--M" and by `\showlists` |
| e-TeX's group lines in the save stack (§274: `saved(-1)`), in `tables` | `delete-par` 358–369, inside the output routine's groups | (a) |
| `seal_at` (the sealed lines' key seed: file, level, **line**, count), in `scalars` | every boundary | (a), a name. The keys of every paragraph's sealed lines are hashed with its line, so after lines move, every later paragraph's `Sealed` cells and the page boxes that carry the keys (`Page`) differ |
| `obj_ptr` (the last object's name, seeded with the line, Task 20) | some boundaries | (a), a name: T25's seed without the line |
| `ship.last_stream`, `pdf_h`, `tj_start_h`, `delta_h`, `tm_a`, `pdf_f`, `last_f` | every boundary | (c) the ship's per-page scratch, of the same kind as Task 14f's: dead between ships |
| `\reserved@b`, `\reserved@c` in the save stack; `marks`; `total_pages`, `last_page`; the open object stream's resources | `delete-par` only | (b) the reflow: a paragraph fewer, other pages |

**The deciding experiment: a line split with no reflow.** `split-line`
turns "the solution first. \doc{ePlace} adjusts" into two lines at the
space. That moves every later line of `ch15.tex` by one and changes
nothing in the output.

| edit | switch | commands re-run | dirty / total | dirty through (`PARTS=7`) |
|---|---|---:|---:|---|
| split-line | on | 228,268 | 8 / 569 | 357 by its lines; 358–363 by `Rest`, `Page`, `Sealed`; 568 by `Numbering` |
| split-line | off | 228,268 | 8 / 569 | the span from 357 |
| enter | on | 228,283 | 8 / 569 | the same regions, the same cells |

So `enter`'s regions after the edit are not dirty through the reflow.
A pure shift of the lines costs the same, and the `Rest` differences
(`PARTS=10`) are only `skip_line`, `mode_line` inside paragraphs,
`seal_at`, `obj_ptr` and the ship scratch. `Page` and `Sealed` differ
through `seal_at`: the sealed lines are named by the line they were
broken at.

Is (a) most of it?
- For `enter`, yes: (a) with (c) is everything.
- For `delete-par`, (a) and (c) are everywhere, and (b) is real as well
  (a paragraph fewer: marks, page count, resources).

But 3ae550d's copies (`mode_line`, the conditions' lines, `skip_line`,
`pack_begin_line`, e-TeX's group lines) would not make one region clean
by themselves. What makes `Page` and `Sealed` differ is `seal_at`, the
sealed lines' names, which 3ae550d does not touch. So I did not start
T24 as scoped, and I ask before re-scoping it.

**Proposed T24, re-scoped, in this order:**
1. Sealed lines named without the line. By Task 14d's rule, a name held
   across regions must not come from a position. A sealed line's key
   could be structured (the file, the line apart, the count), so that
   the rename maps it, as `Pos` does. That means renaming the `Sealed`
   cells in the traces and the key in every box that carries one: the
   page list, the nest's lists, the snapshots. Or the key could be
   seeded from the paragraph's content and an occurrence count, which
   needs a count that crosses regions, itself a cell. The first is
   exact and fits the rename.
2. `skip_line` canonicalized at cuts (dead at a clean point, §336, §494),
   and the ship's remaining scratch (`last_stream`, `pdf_h`, `tj_start_h`,
   `delta_h`, `tm_a`, `pdf_f`, `last_f`) added to Task 14f's list with the
   poison test.
3. `mode_line` and e-TeX's group lines into `Positions`, from 3ae550d's
   tags.
4. T25: object names seeded without the line, and line numbers read as
   values (`\@currenvline`, "at lines N--M") as derived cells.

After 1–3 the `split-line` row should re-run only its own region, and
`enter` only its own region and the pages whose breaks moved. That is
the check for T24.

**Gate.** No behaviour changes: the diagnostic runs only with
`PARTEX_MACHINE_PARTS=10`, so the ten rows with the switch off are
Task 21–22's by construction. The `split-line` row with the switch off
re-ran the same 228,268 commands as with it on.

The first gate run (08:20–08:28) failed on clippy's `too_many_lines` in
`machinehost::run`. The post-rebuild dumps for `PARTS=5/6/7` moved into
`dump_after_rebuild`. `scripts/sandbox cargo xtask check` then passed
(08:29–08:34, 4 min 47 s): fmt, clippy, wasm32, the tests, and e2e 34/34
identical in both modes, including `machine_edits`, `machine_edits_dvi`
and `modern_watch_sanitized`.

Stopped here at the coordinator's word (08:29): no T24, and the user
has said to launch no new work. The re-scoped T24 above waits for the
coordinator.

Time: 08:09–08:35, about 26 minutes.

### The third design: dynamic SSA (coordinator with the user, 08:30–08:55)

DESIGN §7.17 written and approved: the build as one graph of calls
over persistent values, stores forwarded to loads in memory, the cycle
as a φ iterated to the fixed point, names for lookup and content for
verification. It supersedes §7.16's targets; §7.16's numbers stay as
the baseline and its harness as the method.

**What prompted it.** The user's questions over the night, in order:
run exactly what needs running, with no "two passes then biber then
one pass"; link the backedges without `.aux` or any file; the
hierarchy is natural with proper SSA, so no adaptive granularity; SSA
takes functions, so `hpack` is not special; stable ids for lines
instead of a rename through the diff; a persistent token IR rather
than the literal input; hashing by content is what the parallelism is
for. Each is now a paragraph of §7.17. The evidence from the week:
Task 23a (a split line that changes nothing costs what Enter costs,
228k commands, because of state named by lines in `Rest`) and the
audit (the strict spurious test finds 0.6% of the waste while `Rest` is
a lump, a test by field 82%).

**Decisions.**
1. Fixed-point semantics: iterate from the previous build's stores,
   which is latexmk's from the files on disk; five trips at most; a
   fresh build starts from empty stores.
2. Identity: a call's name is `f` and its inputs' versions, nothing
   about where it is; a set of records per name, told apart by their
   reads. Positions are values, never keys. This retires the rename of
   Tasks 20–24 rather than extending it. (First written at 08:50 as
   "the line object and offset, content as the fallback"; the user
   caught the special case at 08:58 with a doubled space: a retyped
   line would get a new id though its tokens are the same. Corrected.)
3. The first step is the runtime on the stub (values, records,
   verified reads, names, the φ), then the engine's state as values
   type by type, then the routines as recorded calls, then the cycle,
   then speculation (§7.17.10).

**Rejected, and why** (§7.17.9): files as the channel; adaptive
granularity as a policy (the audit's ranking is not a policy; in
§7.17 every value is a value); calls below the engine's routines (a
record costs more than an addition); guessing the fixed point (two
fixed points); identity by position renamed through the diff.

**Corrections to what was said earlier tonight.** "+16% recording cost,
so granularity is adaptive" argued from the cost of tracking bolted
onto a mutable engine; in §7.17 the cost is per record and the grain
is a measurement. "SSA must go inside `hpack`, as a reduction tree"
had the direction wrong: `hpack` is one call, memoized whole; its
additions are never nodes.

**Rules from the user.** One Opus 5.5 agent at a time from now on, in
thirty-minute tasks; no new work was launched between 08:29 and the
approval (A stopped after Task 23a's diagnosis; C's audit was merged
with its unsound what-if removed and got no new task; B finishes
Task 12b).


### Merges of Task 23a, the audit and Task 12b (coordinator, 08:35–09:06)

Three reports merged into master in a row, each gated: A's Task 23a
(`dd392c4` on units, merged `b4557f7`), C's audit (`27b8fa4` on
granularity, then `82231f8` removing its unsound what-if switch and
reframing its DESIGN note as a diagnostic that orders the cells, merged
`4fae5bb` → amended `b0327e2`: my resolution of `props.rs` had dropped
the rename test's closing brace, which git had put in the common tail,
and `cargo fmt` caught it), and B's Task 12b (`66f50ea` on cuts,
merged `097f5c4` by hand: B's `Keystroke::rebuild` with A's
`dump_after_rebuild`, A's `PARTS=10` dump inside the keystroke after the
link, B's duplicate `dump_parts` dropped). LOG conflicts resolved with
each agent's entry placed by its start time.

Gates: `xtask check` exit 0 on `b0327e2` (08:46) and on `097f5c4`
(09:00), e2e 34/34 identical in both modes. Course sanitizer, seven
edits, `PARTEX_MACHINE_SANITIZE=1`, on both binaries: no messages, 0
panics, counts unchanged (space 32,870; word 298,713; footnote
1,977,503; label 40,056; enter 228,283; delete-par 1,223,359; word2
10,015). DESIGN §7.17 (`e0cfa7b`, corrected `52cb90a`, and 7.17.11
`2ab7058`) rides on top.

B's proposals (profile inside `record_span`; the rebuild's first
restore) are not pursued: both costs are the lump's, and §7.17's path
deletes `Rest`, the restores and the snapshots rather than trimming
them. C gets no new task; one agent at a time from here.


### §7.17.10 step 1: the runtime, `crates/partex-ssa` (Agent, master, 09:13–09:58)

A new crate, `no_std` + `alloc`, no dependencies, generic over a
`Machine` trait, built on a stub language, with no code from
`partex-incr` or the engine. It is the clean design of §7.17 from its
first line; the old runtime is not extended.

**What is built.**

- *Values* (`hash.rs`, `value.rs`, `pvec.rs`, `pmap.rs`, `pstack.rs`).
  `Version` is a 128-bit stable content hash (our own two-lane
  multiply-fold hasher). `Value::version()` is cached when a value is
  made, and `Value::field(i)` gives a subtree for field-level reads. The
  persistent vector is a chunked tree (32 per node, path copying).
  Every node's version is a polynomial hash over two lanes mod 2⁶¹−1,
  which combines associatively (`h(ab) = h(a)·Bˡᵉⁿ⁽ᵇ⁾ + h(b)`). So a
  node's version is made from its children's, and **equal content has
  equal versions whatever shape the edit history gave the tree**: a
  property test checks this after random inserts, removes and sets.
  The map is a HAMT whose per-node version is the wrapping sum of one
  hash per entry, which is independent of order and shape and costs
  O(1) per level on a write. The stack is a shared list with a version
  per node. Rejected: hashing the children's versions in order, which
  makes the version depend on the shape, so two equal sequences built
  differently would not compare equal.
- *State, records* (`runtime.rs`). The state is a `PMap<Addr, Val>`. A
  write of an equal version does not touch the map (backdating), but it
  is still logged, because a record must carry it: replayed where the
  slot differs, it has to set the value. A `Record` holds the name, the
  args' versions, the result, the reads `(Loc, Version)` in order, the
  net writes, the items (effects, stores and children in program
  order), the cost, and a content version. The arena deduplicates by
  content. The memo maps a name to its set of records. Records that no
  root of the last `keep` builds reaches are collected once the arena
  has doubled since the last collection.
- *Evaluation.* A name is `hash(f, args' versions)` and nothing else. A
  lookup takes the first record in the name's set whose reads verify
  in order against the current state; on a hit it puts the writes in
  place, pushes the child id and reuses the subtree unseen. Otherwise
  the body runs in a recording frame. **A record's reads are the
  external reads of its whole subtree**, so a hit verifies once and
  never descends. Internal reads (of a slot the subtree wrote first)
  and repeated reads are dropped, and they are found by serials, not
  per-frame sets. Every read and write takes the next serial, a frame
  keeps its start serial, and each slot keeps its last write serial.
  A read entry keeps the slot's last write serial and the location's
  previous read serial, so at a child's end each entry is filtered for
  the parent in O(1): it is internal if `w ≥ parent.start`, and a
  duplicate if `r ≥ parent.start && r > w`. The oracle computes the
  same sets naively, with `BTreeSet`s, and exactness compares the two.
- *Stores, loads, trips.* A store (and an `open`) is an item, not a
  write, so appending never reads the stream. A load is a read of
  `Loc::Phi(stream)` from a φ map that is fixed during the trip: the
  previous trip's stores, or on trip 0 the previous build's final ones
  (empty for a fresh build), so the order is Jacobi. A stream that
  does not exist reads as `ABSENT`. After each trip the items are
  flattened in program order into effects and streams. The build has
  converged when every stream loaded in the trip (by a performed load
  or a verified `Phi` read of a hit) has the version its φ had. The
  limit is five trips, and the output is the last trip's effects. Each
  trip is reported with the streams that changed (lines, lines
  differing, loaded or not), the calls that re-ran, hits and misses.
- *Text form* (`trace.rs`), LLVM-IR shaped, as the user asked
  mid-task: `%vN = call @f(%vA, %vB) ; name=#… hit|new|miss=@<first
  differing read>`, with the reads, writes, effects, stores and nested
  calls indented beneath. Versions are named `%vN` in order of first
  appearance, with `%vN=#hash` defined in the line's comment. A load is
  `%vN = phi [%v<prev build>, %v<trip k−1>|undef] @stream:aux`, and a
  build is `trip k { … }` with `converged|changed @stream lines=N`
  notes. `Runtime::trace()`, `Trace::to_text()` and `Trace::parse()`
  round-trip. A hit's subtree is printed as `hit` from its record.
- *Statistics and hooks.* Hits, misses, fresh names, reads verified,
  cost re-run, trips, collected records; `Config::clock` times the
  trips; `Config::on_effect(EffectKind::Store | Output)` is the
  slicing hook of §7.17.11, unused.
- *The stub* (`stub.rs`). A source of lines; `main` calls `tokenize`
  per line (a comment and runs of spaces vanish) and then `para` over
  each paragraph's tokens. There are variables as slots, `work`,
  `emit`, `alloc` (numbered at the link), groups on a `PStack` save
  value where `end` reads the frame it restores, `lineno` as a slot
  that `main` writes, and `setbox`/`wd` (a field read of the width).
  `page` is a fold call per box (inputs: the accumulator, the height)
  that breaks at 4 lines, and `main` calls `ship` per page, which
  stores `L p` into `aux`. `ref` loads `aux` and prints roman numerals,
  so a width depends on the number, and `cite`/`biber`/`printbib` form
  a bcf → bbl chain. The oracle interprets the same bodies through its
  own `Cx` over `BTreeMap`s, iterates whole trips and traces each
  call's name and external reads with sets. Only the evaluation is
  independent; the language's semantics are shared.

**Tests** (`tests/props.rs`, 12, all passing, 0.34 s in release):

- (a)+(b): 300 seeds × 8 random edits (insert, delete, change, retype
  with a doubled space, comment, move a paragraph, push a label
  along). Output, streams, trip count and convergence equal the
  oracle's, and **the list of calls re-run in each trip equals,
  element for element, the oracle's calls whose (name, reads) no
  earlier call had**.
- (c): `set X N` → `set X N-1 add X 1` re-runs only that tokenizer and
  that paragraph.
- (d): a moved paragraph runs no tokenizer, and in text-only programs
  no paragraph.
- (e): a doubled space re-runs exactly one tokenizer call (besides the
  root). An oscillating program iterates its five trips again, all hits.
- (f): the biber chain takes 3 trips. A bistable program (46
  characters and `ref X`: with `??` or `ii` it is 4 lines and the
  label goes to page 2, with `i` it is 3) reaches `ii` from a fresh
  build in 2 trips and `i` from streams holding `X 1` in 1 trip, as
  the oracle does.
- (g): streams nothing loads cause no trip.
- (h): recording off gives identical output, streams and trips.
- (i): the text form round-trips.
- The containers against models, and field reads cutting off (`wd`
  re-runs only its paragraph).

**Bench** (`examples/bench.rs`, release, warm, this machine; run by
`scripts/sandbox cargo run --release -p partex-ssa --example bench
--features std`). 2,500 one-line paragraphs, 11,262 calls. Cold build
29.3 ms. Rebuild after one changed line: median 9.0 ms over 40 edits,
with 11,195 hits and 67 misses (the line's height changes, so page
steps re-run until the accumulator is equal again), which is **about
800 ns per evaluated call, against the 100 ns target**. Conditions:
release profile, one thread, Intel Xeon E5-2673 v3 at 2.40 GHz, warm
(same process, 40 rebuilds after a cold build, median), GC amortized
(it runs when the arena doubles; no collection in the 40 rebuilds).
The hits are about 5,000 `tokenize` (2,500 lines of text and 2,499
blank lines, none with reads), 2,500 `para`, 2,480 `page` and 1,220
`ship` (two paragraphs of two lines to a page).

Where the 800 ns go (perf, inclusive, the whole run including the
cold build; costs are inlined into `Rec::call`, 23% in all):
- lookup, the memo probe and the verification of reads: 11% (the
  probe alone is 4%);
- applying a hit: the reads classified into the parent frame, the
  writes put in place, the child pushed: 7%;
- the stub's `main` re-forming each paragraph's token vector and
  versions in its plain loop (`extend` of cloned tokens, `PVec` leaf
  `fix`, `Val::version` for the arguments): about 16%;
- growing the root record's vectors (reads, items; `realloc` and page
  faults): about 15%;
- the rest: hashing addresses (4%), `field` (4%), `flatten` at the end
  of the trip, and `intern` of `main`'s record.
The same program built with recording off takes a median 9.4 ms, as
long as the recorded rebuild. The stub's bodies are cheaper than their
records, and `main`'s plain loop, which runs in both, builds the
values. The runtime's own share of a hit (lookup, verification,
applying) is about 18% of the samples, roughly **140 ns per hit**, so
the 100 ns target is close for the runtime itself; the rest of the
800 ns is the stub making values.
Not optimized in this task, by the coordinator's decision. The next
levers come from this profile: build the token list as a concatenation
of the lines' lists by `Poly::then` instead of a new vector; no `Vec`
for the argument versions; hash each address once; size `main`'s
vectors from its previous record.

Answers for the review. The memo keeps a **set** of records per name
(`Table<Version, Vec<RecId>>`) and takes the first whose reads verify
in order (7.17.2). A second record for the same name is added only
when none verified; equal content is deduplicated to one id. Read
deduplication **keeps the first occurrence, in execution order**: a
child's external reads are appended to its parent when the child ends,
which is program order because the parent is suspended while the child
runs, and a later repeat is dropped. So a record's reads are verified
in the order the body made them.

### §7.17.10 step 2 (first cut): TeX on the runtime, with check mode (Agent, master, 09:48–10:33)

**What.** The runtime opened to an engine that owns its state, the
engine's tracker as its recorder, the paragraph and the tokenizer as
the first recorded calls, and check mode. Uncommitted on `master`.

- `partex-ssa/src/open.rs`: the state store is a trait (`Store`: the
  version of a `Loc`, get, set; `set` of an equal value keeps the
  version), `MapStore` is the persistent map as one. `Runtime::call(
  store, f, args, body)` is re-entrant: the body gets the store and the
  runtime back. For a call that spans the engine's main-loop steps
  (a paragraph) there is the pair `begin`/`end`, and the recorder is
  driven from outside: `note_read(loc, ver)`, `note_write(addr)`,
  `note_effect`, `note_store`, `note_open`, `note_cost`. `lookup` and
  `probe` (a lookup that counts and notes the status for the trace)
  verify against any `Store`; `replay` puts a hit's writes in place;
  `begin_quiet` runs a probed hit's body again with its children's
  statuses kept out of the trace. `open_trip`/`close_trip` bracket a
  trip, and the trace (`Trace::to_text`) is the same code as the
  stub's. The records, memo, interning and collection are
  `runtime.rs`'s, shared (only `Frame`, `ReadE`, `intern`, `flatten`
  and a new `keep_roots` became `pub(crate)`). The stub's own path
  (`Runtime::build` with its `Rec` context) is unchanged; porting it
  onto `Store` is left (it would remove one copy of the frame logic).
  Tests: `tests/props.rs` unchanged and passing, `tests/open.rs` new
  (hit replays a write into a store where the slot differs; an outer
  miss with an inner hit; a probed hit re-recorded equal interns to
  the same record; backdating).
- `partex-core/src/ssa.rs`: `SsaTracker` (`Tracker` with `VALUES`),
  a `RefCell<Recorder>` holding the runtime and the slots' versions.
  Addresses are `Slot(family, index)`, the families of `track::Cell`
  (eqtb, hash, hash link, font, font table, read and write streams, the
  random generator, the strings by hash), never a name. An `eqtb`
  slot's version is its content (`Tex::cell_content`), cached in an
  array parallel to `eqtb` and dropped at each write, so a read costs
  an index but the first read after a write, which hashes once. The
  other families have revisions tagged with the build.
- `PARTEX_SSA=1` (CLI `run_ssa`): the root call, a `paragraph` call
  from each `CleanPoint::Outer` to the next (the engine's
  `set_stop_at_candidate` and `clean_point`, nothing of the region
  machinery), named by the token value of the line it starts on and the
  offset in it; that value is the result of a `tokenize` call over the
  line's bytes whose reads are the catcodes and `\endlinechar` it read
  through the accessors. `PARTEX_SSA_REBUILD=<cmd>` runs an edit and
  builds again in the same process with the same records;
  `PARTEX_SSA_TRACE=<file>` dumps the last build's trace.
- **A hit is not replayed yet**: the body runs either way. Check mode
  (`PARTEX_SSA_CHECK=1`) compares, at each probed hit, the state a
  replay would leave (the entry state plus the record's slot writes)
  with the state the body left, by `rest_hash_parts`; each part that
  differs is state the recorder does not cover. Replaying a hit into
  the engine is blocked until that list is empty.

**Measured** (`tests/e2e/pages.tex`, plain format, release binary,
one process, cold build then rebuild; machine loaded by a concurrent
`xtask check`, so times are rough):

| | calls | hits | misses | paragraphs (hits) | tokenize (hits) | commands |
|---|---|---|---|---|---|---|
| cold | 29 | 8 | 21 | 14 (4) | 14 (4) | 16514 |
| rebuild, no edit | 28 | 22 | 6 | 14 (8) | 14 (14) | 16514 |
| rebuild, doubled space | 29 | 21 | 8 | 14 (8) | 14 (13) | 16514 |
| rebuild, `\hsize` 3in→3.1in | 29 | 20 | 9 | 14 (7) | 14 (13) | 16514 |

Every rebuild's DVI is byte-identical to the plain engine's (the body
always runs). The rebuild's misses are all first differing reads of
revision families (`hash:1218`, `hash:7472`, `str:…`): revisions do
not compare across builds, so a paragraph that reads the hash misses.
Check mode, 8 hits checked on the no-edit rebuild, 10 uncovered
parts: `input: stack` 8, `scalars: current token` 7, `input: buffer`
4, `input: files` 4, `scalars: cur_val` 3, `input: scanner` 1,
`scalars: file names` 1, `scalars: printing` 1, `strings` 1,
`tables: hash` 1. The input stack, buffer and file position are the
source position (7.17.4's reads of the source, not built);
`cur_val` and the current token are scratch (7.17.11's escape
analysis should drop them); the hash and strings are the families
still on revisions.

Cold builds with `PARTEX_SSA=1` against the plain engine, same
binary and input state (plain format): the DVI and the log are
byte-identical on `pages`, `math`, `align`, `tokens`, `texxet`,
`cutoff`, `incr` (from the same `incr.toc`; the document does not
converge between plain passes), `effects`, `readback` (second pass),
`verbatim` and `edits-dvi`. Paragraph calls on the cold build, and
hits among them (the same first line and verified reads within one
build): texxet 1372 (467), edits-dvi 281 (239), cutoff 95 (78),
incr 80 (51), effects 69 (63).

Check mode on no-edit rebuilds: `cutoff`, 84 hits checked, 12
uncovered parts, top: `input: stack` 84, `scalars: current token` 84,
`input: buffer` 42, `input: files` 42, `scalars: cur_val` 41,
`scalars: printing` 41, `input: conditionals` 40, `input: scanner` 2,
`page` 1, `hyphenation, output files, dvi` 1. `texxet`, 884 hits
checked, 15 parts, top: `input: stack` 852, `scalars: current token`
838, `input: buffer` 496, `input: files` 491, `input: scanner` 315,
`scalars: cur_val` 56, `scalars: printing` 25, `scalars: file names`
22, `tables` 16, `input: conditionals` 11. Check mode sees only what
`rest_hash_parts` hashes; the eqtb slots are outside it, so a write
to `eqtb` that bypasses the accessors would not show (a check of the
record's slot writes against the eqtb cells that changed is next).

The course (`course-master` copied to `~/code/tmp/ssa-course`, its
`_out` aux files, the native pdflatex format, release binary in the
sandbox, one process, `bench/edits.sh`'s `space` edit applied by
`sed` between the builds; no check mode):

| | time | calls | hits | misses | paragraphs (hits) | tokenize (hits) | commands |
|---|---|---|---|---|---|---|---|
| cold | 151.1 s | 103457 | 63299 | 40158 | 51728 (29525) | 51728 (33774) | 66063920 |
| rebuild, `space` | 152.8 s | 103457 | 92563 | 10894 | 51728 (40835) | 51728 (51728) | 66063920 |

From the same aux files, a plain build and a `PARTEX_SSA=1` build of
the edited course make the same PDF (`scripts/pdfcheck same`: same
content; the 56 differing bytes are the `/ID`, which hashes the
output directory's name, `out-plain` against `out-ssa1`) and the same
log but for that name and the string-pool count it lengthens. Plain
took about 36 s, the SSA build 161 s, run side by side: the recorder
on costs about 4.5× on the course, none of it removed yet (the stop
at every candidate with its `flush_outputs`, a `RefCell` borrow and a
table update per read, the root call's record of every read outside
paragraphs).

Every tokenizer call hits on the rebuild: the line with the doubled
space tokenizes to the tokens it had. But the counts expose the gap
that check mode names first: 65.5M of the 66.1M commands lie in
paragraphs that "hit", i.e. one call from a clean point near the
start of the body to the next spans nearly the whole job and shares
its name and verified reads with a short one recorded earlier. The
source position is not a read yet (7.17.4), so a paragraph's name
and slot reads do not fix what input follows; until it is, a hit is
not sound to replay, which is why none is replayed. The 51728 calls
are mostly the preamble's lines (each line read in outer vertical
mode is a clean point).

**Cost when off.** `PARTEX_SSA` unset runs `Tex<_, Untracked>` as
before: the engine's code is unchanged but for the new module, and
the CLI pays one environment lookup. On (no check), `pages.tex` takes
about 0.3 s against 0.1 s plain on a loaded machine: the stop before
every command read from a file (`set_stop_at_candidate`, with
`flush_outputs` at each) and a content hash at the first read of each
eqtb slot after a write; not measured apart yet.

**Decided.** (1) §7.17.1's revisions are equivalent to versions only
within one store's life; partex starts every build from a fresh
engine, so across builds a revision says nothing. The fix, used for
`eqtb`: the parallel array holds the *content* version, computed at
the first read after a write; revisions remain only where no content
hash exists yet, tagged by the build so they never compare equal
across builds (a false miss, never a false hit). DESIGN §7.17.1
updated. (2) An engine's call does not fit a closure when it spans
main-loop steps; `begin`/`end` is the primitive and `call` the
closure form over it.

**Rejected.** A revision counter shared across builds (a paragraph
that re-ran with one more write shifts every later revision, so every
later read misses; and with a counter reset per build, equal
revisions of different content would hit falsely). Keeping the
runtime inside the engine's tracker and calling `Runtime::call` with
`&mut Tex` (the body would need the tracker borrowed twice); the
tracker holds it in a `RefCell` and the driver borrows it between
steps, the hooks skipping reads while it is held.

**Not built** (next): `ship_out` and `write_out` calls, the tokenizer
per line (only the line a paragraph starts on), loads as the φ, the
source position as reads, the course measurement, and the replay of a
hit.

### §7.17.10 step 2b: the source as reads, the hash by content (Agent, master, 10:17–11:02)

**What.** Items 1 and 2 of the task "hits replayed, soundly, and the
recorder cheap"; items 3–6 (replay, `ship_out`/`write_out`/loads as
calls, the recorder's cost, the stub's `Rec` on `Store`) are not
built but for a first lever of 5. Uncommitted on `master`;
`partex-ssa` changed only by `Machine::dense` (below).

- *The source a call consumes is read, relative to the call's start*
  (`partex-core/src/ssa.rs`, `Src`, `Level`). When a paragraph call
  opens, the recorder takes the input levels open then: for each, the
  file's data, where its next line begins and its line number. Each
  line read into the buffer of level `j` is its `k`-th since the start,
  and when the line is done (`Tracker::line_end`, the engine's `LINES`
  hook, now on for `SsaTracker`) it is the read `source:j.k/c`: its
  version is the line's tokens (`line_tokens`) under the codes `c` it
  was read with, interned by the recorder, so a doubled space or a
  line moved to another place with the same tokens verifies. The
  address carries `c` because verification must tokenize the new line
  under the codes the recorded run saw, not the codes at the call's
  start (a call that says `\makeatletter` and then reads a line). If a
  catcode or `\endlinechar` changed while the line was in the buffer
  (the recorder's catcode generation moved), the line is read by its
  bytes (`c` = 0): `\obeyspaces a  b` on one line tokenizes the rest
  differently, so tokens under the line's first codes would be a false
  hit. The line a call starts on is its name (its tokens and the
  offset), and is a bytes read only in that case. The line still in the
  buffer when the call ends is read by its bytes. A level whose file
  ends in the call reads `source:j.(k+1)/EOF` = absent. A line of a
  file the call opened is the file's (below); a line of a file neither
  open at the start nor opened in the call (a `\read` of a stream
  opened earlier) is an `unknown` read, which never verifies across
  builds.
- *Where the call ended is its result*: the lines each level consumed,
  whether its file ended, the depth, and the offset in the last line.
  Nothing replays it yet.
- *Line numbers are reads of the source position*
  (`Tex::print_line_no`, `Tracker::line_number`): `\inputlineno`
  (§424), the `l.N` of an error context (§313), `file:line:` (§73),
  box reports' "at line" and "at lines A--" (§660, §674), the
  conditionals' lines (§336, e-TeX's `\showifs` and group warnings).
  The read is `line:j±d`, the number less level `j`'s line at the
  start, with the number as its version; verification adds level `j`'s
  line now. A number whose level is not known (a paragraph's first
  line, `if_line`) is read against every level open at the start. So a
  moved paragraph verifies unless it printed a line number, and then it
  must not. `mode_line`, `skip_line` and `if_line` are still copied
  from `line` when set; only their use is a read.
- *Loads.* `\input` (§537) and `\openin` (§1275) tell the tracker the
  name and the contents found (`Tracker::load`): a read `load:id` of
  the contents' version, with names interned. At each build's start
  the driver asks the host for every name again. A name the build
  opened for writing (`Tracker::store_open`, §1374) reads as never
  equal: that is the φ of §7.17.5, not built.
- *The hash by content* (`Tex::name_content`, `Tracker::NAMES`). A
  `text` slot's version is the name's characters, cached like
  `eqtb`'s; a `next` slot's is the link, which is the table's layout. A
  lookup (`id_lookup_chain`, §259) no longer reads `Cell::Str` of the
  characters (a revision, so every lookup missed across builds); it
  reads `name:id` (the interned name) with the slot it found, or 0, and
  verification looks the name up again (`Tex::peek_lookup`, untracked).
  The chain walk before an insertion reads each slot's name and link
  by content. `search_string` (the file-name reuse of web2c's
  `\input`) still reads `Cell::Str` as a revision, so a call that
  `\input`s a file misses across builds; fonts, the font table, the
  streams and the random generator stay on build-tagged revisions.

**Why the layout, not the name, for a link.** A reader of `next(p)`
goes on to the slot it names, and its record's later reads and writes
are addressed by that slot (`eqtb[q]`, a `\def` of the name there).
Versioning the link by the name it leads to would verify where the same
names are laid out differently, and a replay would then write the old
slot. The layout is the same wherever the same names were entered in
the same order, which is every rebuild that did not add a colliding
name; where it differs, a miss.

**Measured** (release binary, one process per document, cold build then
rebuild, `--compat=tex`, plain format, in the sandbox; the machine was
loaded by `xtask check` and two course runs, so times are rough):

| document, rebuild after | paragraphs (hits) | calls: hits / misses | reads verified | DVI, log vs plain |
|---|---|---|---|---|
| `pages`, cold | 14 (4) | 8 / 21 | 89 | identical |
| `pages`, no edit | 14 (11) | 25 / 4 | 1750 | identical |
| `pages`, a doubled space | 14 (11) | 25 / 4 | 1750 | identical |
| `pages`, a word in `\words` | 14 (8) | 21 / 8 | 1586 | identical (vs plain of the edited file) |
| `cutoff`, no edit | 95 (85) | 180 / 11 | 4917 | identical |
| `texxet`, no edit | 1372 (1337) | 2709 / 36 | 257404 | identical |

Step 2's `pages` rebuild with no edit hit 8 paragraphs of 14, missing
at `hash:` and `str:` revisions; it now hits 11, and the doubled space
hits the same 11 (the line verifies by its tokens). The word edit
misses first at `source:1.3/0` (the paragraph that read line 4 of the
`\def`) and then at `eqtb:4077` (`\words`' meaning) for the paragraphs
after it, as it should. The remaining misses on the no-edit rebuild
are the root (`str:`, from `search_string`) and paragraphs whose first
differing read is a revision family.

Check mode still lists, on `pages`' no-edit rebuild, 11 hits checked
and 12 parts: `input: stack` 11, `scalars: current token` 10, `input:
buffer` 7, `input: files` 7, `scalars: cur_val` 5, `input: scanner` 3,
`scalars: printing` 2, `strings` 2, `tables: hash` 2, `input:
conditionals` 1, `page` 1, `scalars: file names` 1. The input parts are
now read (the call's reads fix what it consumes, and its result says
where it ended), but a replay that advances the input is not built, so
check mode, which compares a replay's state (the entry plus the slot
writes) with the body's, still sees them; they are the replay's first
job, not uncovered reads. The rest is item 3's list.

The course (`~/code/tmp/ssa-course` with `ch15.tex` reset to
`course-master`'s, its `_out` aux files, the native pdflatex format,
`--compat=pdftex`, batchmode, `SOURCE_DATE_EPOCH` fixed, release binary
before the dense arrays below, in the sandbox, one process per edit:
a cold build, then `PARTEX_SSA_REBUILD` applying `bench/edits.sh`'s
edit by `sed`; no check mode; the `space` run, the `word` run, a plain
build and `xtask check` ran side by side, so times are loaded):

| | wall | calls | hits | misses | paragraphs (hits) | tokenize (hits) | commands (in hit paragraphs) |
|---|---|---|---|---|---|---|---|
| plain cold | 61.8 s | | | | | | |
| SSA cold | 187.0 s | 103457 | 59341 | 44116 | 51728 (25567) | 51728 (33774) | 66063920 (4944894) |
| rebuild, `space` | 155.1 s | 103457 | 102582 | 875 | 51728 (50854) | 51728 (51728) | 66063920 (5288878) |
| rebuild, `word` | 156.5 s | 103457 | 102582 | 875 | 51728 (50854) | 51728 (51728) | 66063920 (5288878) |

The false hit is gone: step 2's `space` rebuild counted 65.5M of the
66.1M commands in paragraphs that "hit"; now 5.3M are, and the long
call that shared a short one's name and reads misses, as it must.
The `space` rebuild's PDF has the same content as the plain build's
(`scripts/pdfcheck same`). But `space` and `word` give the same counts:
both edits make the same 875 calls miss, and since nothing is replayed
every command runs either way (**commands re-run: 66.1M**, of which a
replay would skip the 5.3M in hits). The first differing reads of the
`space` rebuild's misses: `source` 794 (737 of them `source:1.1/1`, a
line of `course.tex`), `str` 40, `font` 15, `eqtb` 14, `name` 8, `out`
2, `fonttable` 1. The reported read is the first differing one of the
*first candidate* under the name, and a blank line's name has
thousands of records, so it is not the reason the right record missed.
A rebuild with **no edit** (the dense-array binary below, `sed
's/x/x/'`) misses the same 875 calls with the same counts, so the edit
is not what they miss on: they read something that never verifies
across builds (the revision families: `str` from `search_string` at
each `\input`, `font`, `out`, `fonttable`; a load of a name the build
writes; an `unknown` line), and each such call is long, since 60.8M
commands lie outside hit paragraphs. Which reads, call by call, is the
next measurement (the trace's first differing read is not enough, as
said); the obvious suspects are the `\include`d chapters (a `\write`
to `\@mainaux` reads `out:`, a revision, and `\input` reads `str:`).
No-edit rebuild with the arrays: 141.2 s, 102582 hits, 875 misses.

**The recorder's cost, first lever.** A profile of the recorder on
`texxet` (release, `perf record -g`, one run, no check mode) put 24.5%
of the samples in the write-serial table's `swap_by` (`note_write`) and
11.4% in `last_write` (a probe per read): hash tables keyed by `Slot`.
`Machine::dense` now gives a slot a family and an index, and the open
recorder keeps those slots' write and read serials in arrays per
family (`eqtb`, `text`, `next`, fonts, streams); the other addresses
keep the tables. Samples on the same run: about 1.0–1.9k before, 837
after, with neither table in the top entries; `cell_content` (the
content hash at the first read after a write, 8.9%) leads now.
`texxet`: plain 0.065 s, recorded 0.19 s (build 0: 160 ms), unloaded
but for `xtask check`. On the course the lever does not show: with the arrays, a
recorded cold build took 151.3 s against a plain one's 49.0 s (the
recorded one beside `xtask check`, the plain one beside the recorded
run's rebuild and the end of `xtask check`), about 3.1×, against 3.0×
without them under heavier load (187.0 s and 61.8 s above) and step
2's 4.5× (161 s and 36 s). The target, 1.3×, is not met, and these
loaded pairs cannot rank the levers: an unloaded profile of the course
is the next step (the stop at every candidate with its
`flush_outputs`, the content hash, a `RefCell` borrow per read).

**Scratch at a clean point.** Nothing was canonicalized yet: that is
item 3's comparison. The parts check mode lists and that are dead at a
clean point are `scalars: cur_val`, `scalars: current token`,
`scalars: printing` and `scalars: file names`.

**Not built**, in the task's order: 3 (replay: the vertical list, the
page builder, marks, insertions, `\prevdepth`, the PDF tables and the
pool's additions as recorded writes with values, and the input advanced
by the result), 4 (`ship_out`, `write_out`, loads as the φ), 5 beyond
the first lever (a `RefCell` borrow per read remains, and the root's
reads outside paragraphs are recorded though the root always runs),
6 (the stub's `Rec` on `Store`).

### §7.17.10 step 2c: TeX's state by inventory; the method corrected (Agent, master, 10:56–11:20)

**What prompted it.** The task began as step 2b's continuation (hits
applied soundly, scratch canonicalized, check mode silent, `ship_out`
and the φ). Two messages from the user changed it during the box: a hit
is nothing but stores of the record's output versions (O(1) per
written slot, values shared, never copied or thawed; `replay` renamed
`apply`), and then the method itself: **steps 2a–2b diverged from the
design.** They bolted a recorder onto the mutable engine and used check
mode to discover what it missed, a worklist found by probing, which is
the retrofit §7.17 rejects. Step 2 as agreed (§7.17.10, 7.17.1) is the
engine's state *as values*, by inventory, converted family by family;
check mode is then a test that must be silent, and a difference is an
error in the inventory. This entry returns to the design.

**Built.**

- DESIGN §7.17.12 "TeX's state": every field of `Tex` (218) and of the
  structs it owns (`ListStateRecord`, `AlphaFile`, `Objs`, `FontArrays`,
  `DviState`, `HyphState`, `Builder`, `AlignState`, `PdfState`, the
  token store, the registers above 255), one row per field or group of
  fields, with the class (value, scratch, effect, configuration), for a
  value its value type and address family, for scratch one clause on
  why it is dead at a call boundary, the family that converts it
  (tables, groups, tokens, lists, page, output) and whether it is
  converted. A script checked that every field name of `Tex` appears in
  the table. Above the table, the conventions the six family agents
  share: what a call boundary is (a clean point, `Tex::clean_point`;
  a call at another boundary re-checks the scratch rows), a version
  made at the write (persistent structures, `Arc`-shared values with
  the version they were made with, version arrays beside the tables,
  backdating; the lazy content cache is the stopgap it replaces), a
  read recorded by the accessor through the `Tracker` hooks into
  `Runtime::note_read`, a hit as stores plus the position as the
  result plus the effects again, and check mode as a field-by-field
  test over the value rows.
- `partex-ssa`: `Runtime::replay` → `Runtime::apply` (doc comments and
  the test's names too). No other code changed: a first conversion of
  the page to a shared value (`Shared<Builder>`) was built and then
  reverted, because the page family converts it (and it computed no
  version at the write).

**Found while classifying** (the rows that a probe would have missed):

- `param_stack` is a value, not scratch: LaTeX's `\include` reads its
  file from inside a macro, so a macro's rest and its parameters wait
  on the input stack below the file level at a clean point.
- `after_token`, `align_state`, `last_badness`, `open_parens`,
  `long_help_seen`, `term_offset`/`file_offset` (where the next print
  breaks its line), `history`/`error_count`, `split_discards`,
  `pdf.last_match` are values live across a clean point; check mode's
  "scalars" parts had them mixed with scratch.
- Scratch at a clean point, each with its reason in the table: the
  current token (the `get_x_token` after the stop overwrites it), the
  scanners' results (`cur_val`, `cur_glue`, `glue_origin`,
  `cur_val_level`, `radix`, `cur_order`), the file-name scanner's
  results, printing's work areas (`dig`, `tally`, `trick_*`,
  `old_setting`, `font_in_short_display`, `depth_threshold`,
  `breadth_max`), fields at their reset value at every boundary
  (`deletions_allowed`, `set_box_allowed`, `no_new_control_sequence`,
  `scanner_status`, `force_eof`, `expand_depth_count`,
  `name_in_progress`, `is_in_csname`, `cancel_boundary`,
  `pack_begin_line` — 0 after `line_break`/`fin_align`, checked in
  `linebreak.rs` and `align.rs` —, `adjust`, `cur_box`, `arg_list`,
  `preamble_list`), and the caches and §7.16 machinery (`skip`,
  `cs_cache`, `memo`, the seal and cell logs, the stop switches).
- Fonts and hyphenation belong to none of the six families named; the
  table puts them under *tables* (both are written by assignments,
  §1252, §1257). `sys_time`… and `epoch` are values read from the
  host's clock (`Clock`), not configuration: without
  `SOURCE_DATE_EPOCH` they differ between builds.

**Not measured**: nothing ran but the gate; no document numbers this
step.

**Gate.** `scripts/sandbox cargo xtask check` passed (rc 0; e2e 34/34
cases identical in machine mode) on the tree with the table and the
rename; the last comment-only edits to `ssa.rs` were followed by
`cargo fmt --check` and `clippy -D warnings` on `partex-core` and
`partex-ssa`, both clean.

**Open questions.** (1) Fonts and hyphenation: which family agent owns
them (the table says *tables*)? (2) The scratch clauses hold at a clean
point; when `macro_call`, `hpack`, `build_page` steps and `ship_out`
become calls, the current token, the scanners' results, `adjust` and
`cur_box` are live at some of those boundaries: re-classify per
boundary in the table, or keep the calls' boundaries at clean points
and record the finer routines as children whose boundaries the parent
covers?

### §7.17.10 step 1b: one evaluator, and the cost of a hit (Agent, branch ssa-tokens, 11:32–12:05)

**What.** Three items, all inside `crates/partex-ssa`, on master
`3a693e2`, uncommitted; the crate's public API is unchanged but for two
additions (`Runtime::note_read_with`, `Table::entry_by`), so
`partex-core/src/ssa.rs` builds as it is.

- *One evaluator.* The stub's recording context (`Rec` in
  `runtime.rs`, a copy of the frame logic: lookup, hit, finish, the
  serials) is gone. A recorded trip now runs through the open
  evaluator the engine uses (`open.rs`: `Runtime::call`, `apply`,
  `begin`/`end`, `note_read`/`note_write`), over a `MapStore` that holds
  the trip's state and its φ. The stub's side is `Eval`, a `Cx` of
  three references (the machine, the runtime, the store) whose methods
  are one line each: a read is `store.get` then `note_read`, a write is
  `Store::set` then `note_write`, a call is `Runtime::call` with a
  closure that runs the body under a new `Eval`. What only the stub's
  trips needed moved into the recorder: the streams a trip loaded
  (`apply` notes a hit's φ reads as loaded, as `Rec::hit` did), the
  calls whose bodies ran (`begin` pushes them unless the frame is
  quiet), and `note_open` calls the effect hook as the other effects
  do. `Plain`, the recording-off context, stays: it is the reference
  mode the tests compare against.
- *The trace is unchanged, byte for byte.* A throwaway program printed
  `Trace::to_text` and the effects of 40 random stub programs (all
  features: variables, groups, labels and refs, cites, line numbers) over
  6 builds each, with an edit between builds, against the crate at
  `3a693e2` built separately: 53,562 lines, identical. That check threw
  out one optimization (below).
- *A hit's cost.* In the order they paid:
  1. The paragraph's token list is no longer rebuilt. `main` kept a
     `Vec` of cloned tokens and made a new `PVec` of it per paragraph
     (an atomic increment per token, a leaf per 32, the polynomial
     hash again). A paragraph is now the lines' token sequences as
     they came from the tokenizer, and its version is their
     polynomial hashes combined (`Poly::then`, the same version the
     flat sequence has, so names do not change): `Val::Cat`, flattened
     only by a body that runs. A one-line paragraph is the tokenizer's
     own sequence.
  2. `apply` reads the record in place instead of taking it out of the
     arena and putting it back (two copies of a 170-byte struct per
     hit), by splitting the borrows of the arena and the recorder.
  3. The arguments' versions are on the stack for up to four
     arguments; the name is hashed once (it was hashed again in
     `begin` on a miss).
  4. The memo keeps a name's first record inline (`Cands`): the
     profile's hottest instruction was the load through the `Vec` of
     ids, a cache miss per lookup.
  5. The link skips subtrees with no effects or stores: `inert`, a
     `bool` per record, set when it is interned, so `flatten` does not
     read the 10,000 records that only return values.
  6. Page texts are `Arc<str>` (the link copied each page's string out
     of its record); the frame of a function with many items is sized
     by its last frame; the status vector of the last build is reused.
  7. The recorder's reads (item 3 of the task, below).

  Rejected, because the trace diff caught them: (a) keeping one write
  entry per slot in a frame (a write to a slot already written in the
  frame keeps the first entry). It saves `main` 5,000 entries and the
  filter's lookups at its end, and it is exact (the argument is the
  one for reads below), but it orders a record's writes by their first
  write instead of their last, so the records' content versions and
  the trace's `write` lines change. (b) A sequence's version as the
  `PVec`'s own (already tagged) instead of hashed once more: it changes
  every version the trace prints. The two together were worth about
  0.5 ms of the rebuild (fastest run 3.8 ms with them, 4.4 ms without).
  The way to have (a) and the trace is a tombstone: the slot's stamp
  keeps its entry's frame and index, and a rewrite clears the old entry
  in place.

**The recorder's reads, for the engine.** A dense slot
(`Machine::dense`) has one stamp, its last write and last recorded read
serials side by side (they were two arrays). `note_read_with(loc, ver)`
returns at once, before the version is made and before any serial is
taken, when the read is internal (the slot was written in the frame,
`w ≥ start`) or a repeat (read since that write in the frame, `r ≥ start
&& r > w`): one array access, no map probe, and the version closure is
not called. Such a read changes nothing: every frame open now contains
the innermost, so its answer would be the same, and a frame opened
later starts after both serials. (The old code took a serial and
updated the read stamp on every read.) A hashed slot's two lookups use
one hash of the address. `note_read(loc, ver)` is `note_read_with` with
the version made.

**Measured.** Release, warm, one thread pinned to one core
(`taskset`), this machine (Intel Xeon E5-2673 v3, 24 threads) loaded by
other builds (load average about 4), so medians move by 10–20% between
runs and the fastest of the 40 rebuilds is given too. Before is the
crate at `3a693e2` built on its own with the same bench.

`examples/bench.rs` (2,500 one-line paragraphs; a rebuild after one
changed line: 11,262 calls, 11,195 hits, 67 misses, 2,499 reads
verified, 1 trip):

| | median | fastest | per call (median) |
|---|---|---|---|
| before (`3a693e2`) | 7.31–8.60 ms | 6.19–6.76 ms | 649–764 ns |
| after | 4.82–5.66 ms | 4.29–4.59 ms | 428–503 ns |

(Six runs of each, interleaved. Recording off, the same program builds
in a median 7.2–8.2 ms: the recorded rebuild is now well under a plain
build, where it was level with it.)

Profile (perf, 15 kHz, DWARF call graphs; the run includes the cold
build and the bench's `source()` per edit, about a tenth of the
samples; self time):

- before (the bench at `3a693e2`, which also runs the 20 plain builds,
  so its shares are lower): `Rec::call` (lookup, hit, finish, inlined)
  10.6%, `free` 4.3%, `PVec` leaf `fix` 3.9%, `Val::version` 3.8%,
  `Version::of::<str>` 3.3%, `malloc` 3.1%, the token vector's
  `extend` of clones 3.1%, `PVec` iteration 2.5%, `Addr::hash` 2.1%,
  `Version::node` 2.0%, `name_of` 1.9%, `PVec::from_vec` 1.8%;
- after (`profile` mode, the rebuilds only but for the cold build):
  `Eval::call`, which inlines `Runtime::call`, the lookup and `apply`,
  18.2% (its hottest instructions are the loads of the memo's slot and
  of the record, and the reference count of the result, in that
  order), `Val::version` 4.4%, `Addr::hash` 4.0%, `Version::node`
  3.8%, `name_of` 3.0%, `Version::of::<str>` 2.9% (the bench's
  `source()`, outside the timed rebuild), `PMap::get` 2.6%, `free`
  2.2%, `Val::field` 2.0%; the kernel (page faults on the new root
  record's memory, mostly the cold build) 11.8%, libc 11.7%. The
  token copies, `fix` and `from_vec` are gone from the list.

`note_read` as the engine calls it (the bench's `reads` mode: `calls`
calls, each making `reads` reads drawn from `slots` slots, a write
every 64 reads; ns per `note_read`, best of 5 rounds; the same code
against `3a693e2` with `note_read`):

| calls × reads of slots | dense before | dense after | hashed before | hashed after |
|---|---|---|---|---|
| 100 × 10,000 of 64 (repeats) | 12.5–13.2 | 5.4–6.1 | 32.3–32.5 | 16.1–16.7 |
| 1,000 × 1,000 of 64 | 14.5 | 7.6–10.0 | 44.2–44.3 | 33.7–34.0 |
| 1,000 × 1,000 of 1,000 (mostly first reads) | 40.2–42.5 | 35.4–36.1 | 71.9–73.8 | 59.8–66.9 |

A repeated dense read costs about 5 ns with the bench's loop; a first
read costs what recording it costs (the entry, and at the call's end its
share of interning the record, which hashes each read), about 35 ns.

**Where a hit's time goes now.** The target of 100 ns per call is not
reached: the rebuild is about 390–450 ns per call. The runtime's own
part (the lookup, the verification, applying, the name) is the top
line of the profile, and it is memory: three dependent cache misses
per hit (the memo's slot, the record, the result's reference count),
about 110 ns. The rest is the stub's `main`, which runs in full on
every rebuild (it is one call whose source changed): its loop over
5,000 lines, the `PMap` writes of `lineno` and the variables (a path
copy each), the addresses' string hashes (three per written variable),
the integers' versions (a hash each time), the 11,262-item record it
makes again (page faults on fresh memory), and dropping the last
build's. The next levers: the write tombstone above; an address's hash
kept in the address (`Addr::Var` hashes its string on every map
operation); integer versions without the hasher; and the record's hot
fields (reads, writes, result, cost) in one cache line apart from the
rest.

**Tests.** `cargo test -p partex-ssa --release`: 12 property tests and
3 open tests pass unchanged, the exact re-run sets among them; clippy
(pedantic) clean with and without `std`; the crate builds for
`wasm32-unknown-unknown`. `scripts/sandbox cargo xtask check` in the
worktree: fmt, the lints (clippy `-D warnings`, with and without
`partex-cli/trace`), the wasm check and the workspace's tests passed,
and e2e is 34/34 identical, plain and in machine mode; trip and etrip
(both modes) did not run: "No such file or directory", because the
worktree's `upstream` and `refs` are symlinks to the main checkout,
which the sandbox does not bind. Nothing here is reached by them
(`partex-ssa` runs only under `PARTEX_SSA=1`).

**DESIGN.** Not changed: §7.17's text holds. §7.17.10 step 1's "Built"
note records the bench as it was (800 ns, 140 ns of it the runtime's);
this entry has the new numbers.

### §7.17.10 step 2d: the tables as values, versioned at the write (Agent, master, 11:32–12:32)

**What prompted it.** The task was 7.17.12's whole inventory as values
in one design (tables, structures, a hit as stores, `ship_out`,
`write_out` and loads as calls, check mode silent). At the interim
(12:02) the coordinator cut the box to the tables, the recorder's
cost and check mode by row; the structures and applying a hit are the
next box, in the same design. The three stopped family agents' diffs
(`~/code/tmp/partex-ssa-{tables,lists,page}`) were read for pitfalls
only; nothing of them is merged. Uncommitted on `master`.

**The two conventions** (DESIGN 7.17.12, restated so the next box
starts from them): *tables* keep a version array beside their entries,
set by the writing accessor after the store from the content written,
an equal write giving the same version; *structures* are persistent or
`Arc`-shared values carrying the version they were made with, from
their parts. This step builds the first for its rows.

**Built.**

- *Versions made at the write* (`track.rs`: `Tracker::wrote`;
  `equiv.rs`: `set_eqtb`, `modify_eqtb`, `set_xeq_level` end in
  `wrote_eqtb`; `hash.rs`: `set_text`, `set_next`). The accessor stores
  the value, then gives the slot's content version (`cell_content`,
  `name_content`: the word, its level, the objects it names; a name's
  characters; a link). `ssa.rs`'s `Versions` keeps four arrays (eqtb
  below the registers above 255, those registers by their offset from
  `EXT_BASE`, `text`, `next`); a read is `known(slot)`, an index. The
  lazy cache (`Versions::content`, the first read after a write) and
  the `write_value` invalidation are gone. A slot no writer versioned
  reads as a build-tagged revision (never equal: a miss, never a false
  hit); none is left in practice.
- *Bulk writers version what they store* (`Tex::version_tables`): the
  engine as `Tex::new` made it (called when `ssa::run` starts) and a
  format's load (`load_fmt_file`, after `undump`), over every eqtb slot,
  every register above 255 when e-TeX's are on (their defaults
  included), and every hash slot. A control sequence's word that names
  no object has the content of its bits, so each distinct word is
  hashed once (most are undefined); an empty hash slot's name and link
  are two constants.
- *The pool's search by content.* web2c's `search_string` (a file
  name's reuse, `\input`, `\font`) told the tracker `Cell::Str` of the
  characters, a revision; it now tells `Tracker::string_search(name,
  found)`, recorded as `search:<id>` (interned name) with the string it
  found, and verified by `Tex::peek_search` (the index, then the
  strings made since it was last extended, newest first). `Cell::Str`
  reads are no longer recorded.
- *A read noted once per call* (`SsaTracker`): a stamp per table slot
  (`Vec<Cell<u32>>`, four arrays sized at the start) holds the call
  generation it was last noted in; `SsaTracker::boundary` moves the
  generation at every call's begin and end. A repeated read in a call
  is a compare of two integers: no `RefCell` borrow, no map, no
  closure. Equivalent to the runtime's own rule (a read after a read or
  a write in the same frame is dropped), so records are unchanged.
- *Check mode by row* (`Tex::value_rows`, `statehash.rs`): the value
  rows of 7.17.12 other than the tables, each hashed on its own
  (scratch rows left out: the current token, the scanners' results,
  printing's work areas, the file-name scanner), compared at each
  probed hit between the call's entry plus its table writes and the
  body's exit. And the array test: in check mode every table read also
  hashes the slot's content and counts a version that differs
  (`stale`), with `lost` for a write whose version could not be stored.

**Found by the array test.** On `pages.tex` one slot read stale, 135
times: eqtb INT_BASE+49, `\newlinechar`. The versions its writes made
were right; its content changed between them with no write: the
format's load (`undump`, §1299–§1329) stores eqtb, `xeq_level`, the
registers and the hash wholesale, past the accessors, after the first
line's `print`s had written `\newlinechar` through them. So a format's
load is a writer of the tables, and versions them (above); 7.17.12's
eqtb row is corrected to say so. After the fix: stale 0 and lost 0 on
`pages`, `texxet`, `cutoff`, `incr`, `readback`, `edits-dvi` (cold and
rebuild); the course below.

**Measured.** Release binaries in the sandbox, one process per run, a
cold build then `PARTEX_SSA_REBUILD` with `bench/edits.sh`'s `space`
edit on `ch15.tex` (reset to `course-master`'s first), the course's
`_out` aux files, the native pdflatex format, `--compat=pdftex`,
batchmode. *Before* is HEAD (3a693e2)'s binary, run 11:36–11:43 while
this step's first release build ran; *after* is this tree at 12:07
(before the bulk writers' fast path), run 12:08–12:12 beside release
builds and the e2e checks. Both are loaded; the ratios are rough.

| course | before | after |
|---|---|---|
| plain cold | 45.0 s | (same code: `Untracked` compiles the hooks out) |
| recorded cold | 133.9 s (3.0×) | 95.8 s (2.1×) |
| rebuild, `space` | 135.6 s | 101.3 s |
| calls / hits / misses | 103457 / 102578 / 879 | 103457 / 102664 / 793 |
| paragraphs (hits) | 51728 (50850) | 51728 (50936) |
| commands re-run | 66063920 (5288874 in hits) | 66063920 (5301429 in hits) |

No hit is applied yet, so every command runs on the rebuild either way
(66.1M); the target (the edited paragraph and what its outputs change)
needs the structures' convention and `apply`. The rebuild's misses by
first differing read: `source` 751 (696 of them `source:1.1/1`, a line
of `course.tex`, as in step 2b), `font` 15, `eqtb` 14, `name` 7, `out`
3, `fonttable` 2, `hash` 1; `str` 0 (40 in step 2b: the pool's search
now verifies by content). As step 2b noted, the first differing read
is the first candidate's, not the reason the right record missed. An
excerpt of the rebuild's trace around a miss:

```
      read @eqtb:27866 = %v75128 ;
      read @eqtb:27754 = %v75132 ;
    %v224160 = call @paragraph(%v224142, %v223162) ; name=#97972ed114f8c03f8861e7de4e5f5c7b miss=@eqtb:2378 %v224160=#7a5c1ff9d75be50edc8732a983b68e47
      read @eqtb:29305 = %v75299 ;
      read @eqtb:29325 = %v221023 ;
      read @eqtb:27833 = %v74992 ;
      read @eqtb:27846 = %v75138 ;
```

`texxet` (plain format, best of five, wall, beside a course run):
plain 0.08 s; recorded 0.21 s before, 0.34 s at 12:07, 0.29 s with the
bulk writers' fast path. On a small document the fixed cost of
versioning the tables twice (the engine as made, then the format:
about two million slots with the default `hash_extra`) and of hashing
every write's content (a `\def`'s tokens are hashed at the write; a
token list carrying its own version, the structures' convention, makes
that a combine of versions) outweighs the stamps; on the course the
stamps and the pool's search win. Neither pair is unloaded; an
unloaded profile is the next measurement.

**Check mode by row** (`PARTEX_SSA_CHECK=1`, e2e, cold then rebuild;
the rows that differ between a hit's entry plus its table writes and
the body's exit, with the hits among the rebuild's): `pages` 11 hits:
the input stack 11, buffer 7, files 7, `line` 7, the pool 2, `history`
2, conditionals, `last_badness`, `page`, `term_offset` 1 each. `texxet`
1339 hits, `cutoff` 85, `incr` 61 (input stack 61, files 37, `line`
37, `page` 29, buffer 21, the pool 6, conditionals 4, `cur_list` 4,
offsets 4, marks 2, DVI, the group stacks, `hash_used`, names,
`mag_set`, the file-name stacks, `write_file`, `write_open` 1 each),
`readback` 86 (input 86/43/43, conditionals 40, offsets 40, buffer 11;
DVI, group stacks, `log_file`, `page`, `selector`, `write_file`,
`write_open` 1), `edits-dvi` 40 (input, buffer, `line`, `align_state`,
the save stack and its levels, `history` 40 each). Every row listed is
a structure or scalar row not converted yet, which a hit's stores will
cover once they are values; none is a table row. DVI and log
byte-identical to the plain engine on `pages`, `texxet`, `cutoff`,
`readback` and `edits-dvi` (cold and rebuild); `incr`'s rebuild reads
the `.toc` its cold build wrote, and is identical to a third plain pass
(the document does not converge between plain passes, step 2).

**The course in check mode** (this tree at 12:13, `space`, the
conditions above, beside `xtask check`): the cold build took 133.8 s
with the checks; the array test found no stale read and no lost write
(0 of every table read noted, 25567 hits checked). The rows that differ
at the hits, by count: the input stack 22336, the open files 21570,
`line` 21568, the buffer 17346, the conditionals 11601, the pool 8963,
the hash's allocator (`hash_used` …) 8855, the save stack 301, the
offsets 263, the group levels 236, the file-name stacks 217, e-TeX's
per-file stacks 149, `param_stack` 143, `align_state` 64, the PDF
writer's tables (`objs` 23, `xform_count`, `last_xform`, `out`, `ship`
17 each, `tounicode` 9, and single ones), `read_file` 19, the pseudo
files 6, fonts 5, `page` 5, `cur_list` 4, `dead_cycles` 4; 47 rows in
all, every one a structure or scalar row of the next box, none a table.
The rebuild after `space`, in check mode: 145.4 s, 103457 calls,
102664 hits, 793 misses (as without check mode), 50936 hits checked,
50 rows differing, the same rows led by the input stack; no stale read,
no lost write. So the array test is silent on the course, cold and
rebuilt; the row comparison is not, and cannot be before the
structures are values (a hit's stores are the table writes only).

The check-mode rebuild's PDF and a plain build of the edited course
(48.9 s, beside the end of `xtask check`) have the same content by
`scripts/pdfcheck same` but for `/CreationDate` and `/ModDate`
(`SOURCE_DATE_EPOCH` was not fixed in these runs).

**Gate.** `scripts/sandbox cargo xtask check`, started 12:12 on this
tree: rc 0; fmt, clippy, the wasm build and the tests passed; trip 7/7
and etrip 18/18 identical, both modes; e2e 34/34 identical, plain and
in machine mode. Comment-only edits to `ssa.rs` followed; `cargo fmt
--check` and `cargo clippy --workspace --all-targets -D warnings` on
the final tree are clean.

**Not built** (the next box, in 7.17.12's second convention): the
nest's lists and boxes, token lists with their versions, the save
stack, the page builder by field, marks and insertions, the input state
as a value with the position as the call's result, the PDF and DVI
writers' tables, the streams and loads as the φ, fonts, the font table
and the random generator (still build-tagged revisions); then a hit
applied as stores, and `ship_out`, `write_out` and loads as calls.

### §7.17.10 step 1c: the levers of 1b, measured (Agent, branch ssa-tokens, 12:05–12:35)

**What.** The five levers step 1b listed, on `bc86018` (the before),
uncommitted, all in `crates/partex-ssa`. The check for each is the
throwaway trace diff of 1b (40 random all-feature stub programs, 6
edited builds each, `Trace::to_text` and the effects, 53,562 lines)
against `bc86018` built on its own, plus the 12 property tests (exact
re-run sets) and 3 open tests, which pass unchanged.

1. *One write entry per slot, in last-write order* (`Open::note_write`,
   `net`, `adopt`). A slot's write stamp says where its last write's
   entry is (the frame's depth and the index in its writes, `Pos`). A
   rewrite in the same frame updates that entry's serial in place; a
   write in a call nested inside the frame holding the entry kills that
   entry (serial 0) and adds one to the call's frame. At a call's end
   its live entries sorted by serial are its net writes, and the parent
   adopts them (their positions move to the parent's list). Sorting by
   serial gives exactly the old order: a frame's list was always in
   serial order (a child's entries are appended at its end, and all
   their serials lie between the parent's entries before and after), so
   the old survivors were in the order of their last writes. `main`'s
   frame now holds 8 entries instead of 5,000, and a call's end does no
   lookup per write. **Trace: byte-identical.** The positions live
   apart from the read stamp, so a read still touches 16 bytes per
   dense slot.
2. *The address's hash kept in the address.* The stub's names
   (`Addr::Var`, `Reg`, `Stream`) are a `Name`: the characters and their
   hash, made once; `Hash` writes the one word, equality compares the
   hash first, order is by the characters (streams and the trace sort
   by it). `Addr::hash` (4.0% of the samples in 1b) is gone from the
   profile. **Trace: byte-identical** (checked with this lever alone).
   The internal hashes of addresses change, so records' content
   versions (their identity in the arena, never printed) change value.
3. *Integer versions without the hasher* (`int_version`): the number
   after a tag through a multiply by an odd 128-bit constant and a
   xor-shift, twice. Both steps are bijections of 128-bit words, so
   distinct integers have distinct versions; against other values' the
   odds are those of any 128-bit hash. **Trace: identical but for the
   hash literals** (30,897 masked); the `%vN` numbering, which follows
   which versions are equal, is identical too.
4. *The record's hot fields in one cache line*: **not kept.** What a
   hit touches (the cost, the reads and writes as boxed slices, the
   result) is 8 + 16 + 16 + 48 bytes for the stub, whose `Val` is 48
   bytes (a `PVec` carries its 128-bit version inline), so it cannot
   fit in 64. Tried anyway with `#[repr(C, align(64))]` and the hot
   fields first (records of 256 bytes, two lines touched instead of an
   unaligned two or three): fastest runs 373–404 ns per call against
   360–379 ns without, i.e. no gain inside the noise; reverted. The
   engine's value (`SVal`, a version, 16 bytes) makes it 56 bytes,
   which fits: the lever belongs with the engine's records.
5. *A first read's hash fed as it is noted*: a read entry carries its
   location's hash, made where the recorder has it anyway (the table's
   hash of the address, or a mix of a dense slot's family and index),
   and each frame feeds a running hasher as an entry is pushed (its own
   reads when noted, a child's surviving ones when it ends), so
   interning hashes no location again. The content version's formula
   changes (not printed). **Trace: identical to lever 3's.** It did not
   pay: a first read costs what it did (below), because the location
   hash was not the cost. What a first read costs is memory: the
   64-byte entry (a 16-byte-aligned version inside), converted in place
   to the record's 32-byte entry (the allocation stays 64 per read),
   and the records' fresh memory (the micro-bench makes 20 MB of reads
   per round). The next step there is a compact entry: the record's
   `(Loc, Version)` plus the serials in a parallel array that is
   dropped at the call's end.

**Measured.** Release, one thread pinned (`taskset`), warm. The machine
was loaded by another agent's builds (load average 5.5 to 11 during the
runs), so the wall times below are rough and the instruction counts are
the steadier measure.

The rebuild bench (`examples/bench.rs`, 11,262 calls: 11,195 hits, 67
misses): six interleaved pairs. The three pairs under the heaviest load
(load average 11) ran 10.8–12.1 ms for both; the three calmer ones:

| | median | fastest |
|---|---|---|
| before (`bc86018`) | 4.86–6.26 ms | 4.26–4.99 ms |
| after | 4.38–5.63 ms | 3.94–4.33 ms |

Per rebuild, from the difference between 80-edit and 40-edit runs
(edits 40–79; `perf stat`, 3 runs each; includes the bench's `source()`
per edit, which is outside the timed rebuild): **23.2 M instructions
after, 25.2 M before (−8%)**; cycles 10.6 M against 14.5 M (loaded, so
rough). That is about 2,050 instructions per evaluated call, a part of
them the bench's own `source()`.

Profile after (perf, 15 kHz, DWARF; self time; the run includes the
cold build): `Eval::call` (the lookup, `apply`, the name, inlined)
21.1%, `Val::version` 4.1%, `name_of` 3.6%, `PMap::get` 3.0%,
`Version::node` 2.9% (tuples and sequences, the stub making values),
`Version::of::<str>` 2.8% (the bench's `source()`), `Val::field` 2.7%,
`free` 2.1%, `flush` 2.0%, `PMap` insert 1.9%. `Addr::hash` is gone.

`note_read` (the bench's `reads` mode; ns per call, best of 5 rounds;
two runs each, interleaved):

| calls × reads of slots | dense before | dense after | hashed before | hashed after |
|---|---|---|---|---|
| 100 × 10,000 of 64 (repeats) | 5.6–5.7 | 5.6–7.1 | 16.4–16.8 | 16.5–18.8 |
| 1,000 × 1,000 of 64 | 8.1 | 7.4–9.1 | 33.0–33.3 | 32.5–37.0 |
| 1,000 × 1,000 of 1,000 (mostly first reads) | 34.5–37.4 | 33.7–38.5 | 58.1–59.6 | 55.4–59.8 |

No change beyond the noise: the recorder's read path is where 1b left
it.

**Where it stands.** About 370–480 ns per evaluated call on this bench
(fastest to median, loaded), 100 ns the target. The runtime's own part
is the top line and is three dependent cache misses per hit (the memo's
slot, the record, the result's reference count). The rest is the stub:
`main` runs again in full on every rebuild (5,000 lines, a tuple per
page step, a `PMap` path copy per written variable).

**Gate.** `cargo test -p partex-ssa --release`, clippy pedantic with
and without `std` (`-D warnings`), and `scripts/sandbox cargo xtask
check` in the worktree (rc 0): fmt, the lints, the wasm check, the
workspace's tests, trip 7/7 and etrip 18/18 in both modes, e2e 34/34
plain and in machine mode.

**DESIGN.** Unchanged: no number here changes what §7.17 says.

### §7.17.10 step 2e: token lists and the pool as values; the next box's brief (Agent, master, 12:26–13:28)

**What.** The task was 7.17.12's structure rows, all of them, as values
under the structures' convention, then the hit applied, `ship_out`,
`write_out` and loads as calls. At the interim (12:44) I reported that
the box could hold only the first part. The coordinator chose to
measure, gate and write up, with check mode's exact list of the rows
not yet values, and with this entry's brief for the rest. **Converted:
token lists, the string pool, the hash's allocator. The hit is still
probed and its body runs.** Uncommitted on `master` (from 0af03c1).

**Built.**

- *Token lists* (the structures' convention, `tok.rs`). Each `TokList`
  carries `poly`, a polynomial of its tokens mod 2⁶¹−1 (`poly_step`,
  `poly_of`), kept by every write for a tracker that keeps versions
  (`Tracker::VALUES`). An append (`tok_push`, `pooled_push`,
  `copy_into`, `tok_append`, `tok_from`) is one step per token and
  keeps the prefix's. An insertion (`tok_insert`), an edit in place
  (`\uppercase`'s case change, §1289) or a group's braces removed from
  an argument (§400) makes it again at that write (`repoly`). A release
  resets it, and a format's lists are versioned in bulk (`version_all`,
  from `Tex::version_tables`). `TokStore::version(id)` is the
  polynomial, the length and the `\protected` flag: 16 bytes, no token
  read. An eqtb entry that names a list (a macro, a token parameter or
  register) combines that version (`Canon::list_versions`) and no
  longer hashes the tokens at the write. Check mode tests the lists
  too: its content for an entry is made with each list's version from
  its tokens (`eqtb_content_by_tokens`), so a write that bypassed the
  polynomial shows as a stale read.
- *The pool and the allocators* (the tables' convention; `track::Row`,
  `Tracker::row_read`/`row_wrote`; `ssa::Fam::Pool`, `Fam::Alloc`).
  String `n` is versioned by its bytes when it is made (`make_string`,
  and web2c's raw `str_ptr` moves in `end_name`, §517). `str_ptr` and
  `hash_used` (and `hash_high`, web2c's extra area, whose row the
  course's check showed was the one changing: 19447 hits) are scalar
  slots whose version is their value
  (`track::scalar_version`, shifted clear of the recorder's mark bit so
  no two values share one). They are read and written by
  `make_string`, `flush_string`, `end_name` and `id_lookup`'s
  insertion (§260, `insert_cs_after`, `new_name_at`). `version_tables` versions all of them after a
  format's load. Both rows left check mode's row comparison, and the
  array test covers them.
- *Check mode names the rows not yet values*
  (`SsaReport::not_values`, printed as `ssa check: N rows not values
  yet: …`). There are 70 on `pages.tex`: `value_rows`, i.e. every value
  row of 7.17.12 that is not a table, since no record holds a write of
  them.
- *A frozen list keeps its polynomial* (`Frozen::poly`), so the old
  runtime's checkpoints neither lose nor recompute it (a persisted one
  makes it again when loaded, like its hash).

**Measured** (release, in the sandbox, the conditions of step 2d:
`bench/edits.sh`'s `space` on `ch15.tex` reset to `course-master`'s,
the `_out` aux files, the native pdflatex format, `--compat=pdftex`,
batchmode; the machine at load 4–5, from the coordinator's gate in
another worktree and my own builds, so times are rough):

| course | step 2d (12:08) | step 2e (12:43) |
|---|---|---|
| recorded cold | 95.8 s | 108.4 s |
| rebuild, `space` | 101.3 s | 110.9 s |
| calls / hits / misses | 103457 / 102664 / 793 | 103457 / 102664 / 793 |
| commands re-run | 66.1M | 66.1M (no hit applied) |

The misses by first differing read are unchanged: `source` 751, `font`
15, `eqtb` 14, `name` 7, `out` 3, `fonttable` 2, `hash` 1. The version
changes did not move a record: the same records verify, with the
versions now made at the writes. The times are not comparable across
the two columns (the load differed); an unloaded pair is still owed.
`texxet` recorded, best of five, the step 2d and 2e binaries run in
alternation at load 9 (12:55): 2d 0.26 s and 0.29 s, 2e 0.32 s and
0.32 s (build 0: 252 ms against 288 ms); plain 0.08 s. So step 2e is
slower on a small document, and the `\def` hashing that went is not
where texxet's recorded time was. Unprofiled; the suspects are the
per-push step (every token pushed, argument lists included, now costs
a multiply mod 2⁶¹−1 that a `\def`'s one hash at the write used to
cost once) and the stamps' 4 MB for the pool, allocated per build. A
profile is the next box's first measurement.

The course in check mode (12:47–12:53, the binary before `hash_high`
was converted, loaded as above): cold 126.5 s, 25567 hits checked, 46
rows differing; the `space` rebuild 147.4 s, 50936 hits checked, 49
rows differing, led by the input stack 44837, the files 36613, `line`
36601, the buffer 31800, the conditionals 20450, `hash_top, hash_high`
19447 (converted since, above), the save stack 1262, the group levels
524, the offsets 450, `objs` 348. The pool's row, 19618 in step 2d, is
gone. No stale read and no lost write, cold or rebuilt.

e2e with `PARTEX_SSA_CHECK=1` (cold, then a rebuild): DVI and log
identical to the plain engine on `pages`, `texxet`, `cutoff`,
`readback` and `edits-dvi`, and `incr` against its third plain pass;
no stale read and no lost write on any of them.

**The next box's brief: each remaining row, its versioned type, its
writers, and the expected cost.** Site counts are references in
`partex-core` outside the machine, sanitizer, persistence and
state-hash code.

- *Nest's lists and boxes* (`cur_list.list: Vec<Node>`, 42 references
  in 9 files; 54 appends, `tail_append` and direct pushes, in 8 files). The version goes on
  the node: `partex-engine`'s `Node` gains nothing per variant, but a
  list becomes a sequence that keeps a running version beside it (a
  `NodeList { nodes: Vec<Node>, ver: Vec<u64> }` prefix polynomial, the
  token lists' scheme), so an append costs one combine and shares the
  prefix. `BoxNode` gains `ver: u128`, set at packing (`partex-engine`'s
  `pack::hpack`, `vpack`, `vpack_natural`: from the list's running
  version, the dimensions and the glue setting), and a node's version
  is a small hash of its fields, with a box's taken from its `ver`.
  `objs.boxes` then combine the box's version into eqtb's content
  instead of hashing the tree (today's cost of a `\setbox`). Expected:
  a day's worth of sites; the per-append cost is one hash of a node's
  scalar fields. A clean point has an empty contribution list, so what
  a paragraph call writes here is the page builder's list (below) and
  the box registers.
- *The save stack* (`save_stack: JVec<MemoryWord>`, `save_ptr`,
  `cur_level`, `cur_group`, `cur_boundary`: 160 references in 14
  files, 41 direct writes of the pointers). It is a table: the entries
  are versioned by the accessors `set_save_type`, `set_save_level`,
  `set_save_index` and `eq_save` (§268–§274, `save.rs`), an entry
  holding an eqtb word combines its content version as eqtb's does, and
  the four scalars become scalar slots behind setters. Reads: `unsave`
  (§281) reads the entries it restores. Expected: about an hour.
- *The page builder by field* (`Builder` in `partex-engine/src/
  builder.rs`; every access already goes through `Tex::page` and
  `Tex::page_mut`, `page.rs`, 23 calls in 4 files). The fields become
  slots: `so_far[0..8]`, `contents`, `max_depth`, `least_cost`,
  `best_break`, `best_size`, `ins`, `insert_penalties`, `last`,
  `discards`, and the list as the node list above. The writers are
  `Builder::build` (§994), `fire_up` (§1012), `start_new_page` (§987)
  and `release_held`, which return the fields they changed, so `page_mut`
  is replaced by field setters that make each field's version at the
  write. `\pagegoal` and friends (`scan.rs`) read one field. Expected:
  about an hour, all inside `builder.rs`'s four functions.
- *Marks* (`cur_mark: BTreeMap<i32, [Option<Tokens>; 5]>`, 4
  references in `page.rs`; `Tokens = Arc<[i32]>` in `partex-engine/src/
  node.rs`, also the whatsits' `\write` bodies). `Tokens` becomes a
  struct `{ toks: Arc<[i32]>, ver: u128 }` made by `take_tokens` from
  the list's `version` (already made at its writes), so a mark node, a
  `\write` whatsit and `cur_mark` carry the version they were made
  with. Expected: an hour, mostly the node type's uses in
  `partex-engine` (display, persistence, hashing).
- *The PDF and DVI writers' tables* (`PdfState`, 947 references in 19
  files; `DviState`, 30 in 2). These are the largest rows. The design
  keeps them as persistent maps per table (§7.16's `Dests`,
  `Numbering`, `Glyphs`, `FontOrder`, `Written`, and the `last_*`
  scalars as scalar slots). Their writers are `ship_out` and the
  `\pdf…` commands, so they become `ship_out`'s writes once `ship_out`
  is a call whose result is the page's effect. Expected: the whole of a
  box for the PDF writer; the DVI writer's `total_pages`, `max_v`,
  `max_h`, `max_push` and fonts defined in an hour.
- *The streams* (`write_file[16]: OutFile`, 6 references in 3 files;
  `write_open`; `read_file[16]`, `read_open`, 6 in 2). A stream is a
  persistent sequence of lines (`partex-ssa`'s `PVec`), appended by
  `write_out` (`dvi.rs:807`, §1370) as `Runtime::note_store`. `\openout`
  (§1374) is `note_open`, and the file's bytes an effect. `\openin`
  (`open_or_close_in`, §1275) and `\read` (`read_toks`, §482) of a
  stored name read the φ (`Loc::Phi`) through the runtime's trip loop,
  the rest as a load by content. Expected: an hour, since the runtime
  has the φ and the trips already.
- *The input state as the call's result* (`cur_input`, 199 references
  in 12 files and the hot path; `input_stack` 27; `input_file` 9;
  `AlphaFile { data: Arc<[u8]>, pos, lines, name, … }`). The call's
  result already says, per level open at its start, how many lines it
  consumed and whether the file ended, the depth, and the offset
  (`close_source`). A hit advances level `j`'s `AlphaFile` by `k` lines
  (`pos` by `line_bounds`, `lines += k`), loads the last into the buffer
  at `start..limit` with `loc` at the offset, and pops ended levels. A
  level opened during the call (an `\input` inside the paragraph) is in
  the result as the load's name and its lines consumed. The hit opens
  it through the host as `start_input` would (§537: a load by
  content), pushes its level and advances it. `line`, `line_stack`,
  `open_parens`, the file-name stacks and e-TeX's per-file stacks
  follow from the same result. Token-list levels below the file
  (`\include`) are values (their list's version, their `loc`) and a
  call that ends at a clean point cannot have changed them without
  reading them. Expected: two hours; this is what check mode's top rows
  on the course are (the input stack 44837, the files 36613, `line`
  36601, the buffer 31800).
- *The conditionals* (`cond_stack` 22 references in 8 files, `if_limit`,
  `cur_if`, `if_line`, `skip_line`). A persistent stack of records;
  `if_line` and `skip_line` store the line as data today (7.17.4: a
  position is a read of the source, so they store the level and the
  offset from the call's start instead). Expected: an hour.
- *Fonts* (`fonts` 135 references in 21 files; `read_font_info`,
  `tfm.rs:77`). A font's load is a read by content, `font:<name>` =
  the file's version, and the loaded font an `Arc`-shared value carrying
  it. `\fontdimen`, `\hyphenchar`, `\skewchar` and pdfTeX's codes are
  table slots per font under the tables' convention (their writers
  already call `Tracker::write(Cell::Font(f))`: `fontexp.rs`, `scan.rs`,
  `tfm.rs`, `pdf/ship.rs`), and the table of loaded fonts is a
  sequence. Hyphenation's exceptions (`hyph.rs`, 107 references to
  `self.hyph` in 5 files) are a table by word under the same
  convention; the patterns are fixed once packed (configuration after
  `\dump`). Expected: two hours.
- *The scalar rows*, the tables' convention with `scalar_version`,
  behind setters. Write and reference sites: `mag_set` 1/4,
  `dead_cycles` 4/8, `last_badness` 4/5, `error_count` 4/5,
  `after_token` 2/6, `long_help_seen` 1/2, `shown_mode` 3/3,
  `output_active` 2/10, `open_parens` 5/6 (a few minutes each); then
  `history` 12/23, `interaction` 7/23, `term_offset` 12/21,
  `file_offset` 7/15 (the printer's column: every `print_char`, where
  the stamp makes a repeated read a compare), `align_state` 45/67 (the
  brace count, `get_next`'s hot path), `selector` 63/102. About two
  hours for all, the last three the costly ones.
- *Then the hit*: `Runtime::apply` with `Store::set` for every family
  above. `SVal` becomes a version with the shared value (an `Arc` or a
  persistent root), and the record's writes hold those. Apply is one
  pointer store per written row and the position as the result, and
  `ship_out`'s page is re-emitted as its effect. Only then does check
  mode's row comparison become the test that must be silent, and a hit
  skip its body. The record's hot fields in one cache line (step 1c) are
  taken when apply's profile shows the record's load.

**Gate.** `scripts/sandbox cargo xtask check` on the final tree
(12:55–12:58): rc 0; fmt, clippy with and without `partex-cli/trace`,
the wasm build and the tests passed; trip 7/7 and etrip 18/18 identical
in both modes; e2e 34/34 identical, plain and in machine mode. A first
run (12:46) failed `machine_edits` and `modern_watch_sanitized` in
plain e2e ("partex watch stopped before `Watching`"). I had rebuilt
`target/release/partex` under it at 12:49–12:51. The rerun on the
final tree passed both. That tree also changed between the runs
(`Frozen::poly`, which the old runtime's thaw uses, and `hash_high`),
so the first failure is most likely, not certainly, the binary
replaced under the test.

### §7.17.10 step 2f: boxes, the conditionals and the marks as values (Agent, master, 13:01–14:01)

**What.** The task was the list rows in order: (1) the nest's lists
with a running version and boxes versioned at packing, (2) the page
builder by field, (3) marks and `\write` bodies, (4) the save stack,
(5) the conditionals. At the interim (13:22) I reported what fits, and
the coordinator confirmed the stop. **Converted: boxes (as shared
values carrying their version), the conditionals and the marks (each
as one slot versioned at the write from its contents). The hit is
still probed.** Not built: the node lists' running version and
`cur_list`'s scalars, the page builder by field, the save stack, and
`Tokens { toks, ver }`. Uncommitted on `master` (from 3974c9b).

**Built.**

- *Boxes* (`partex-engine/src/node.rs`). `BoxNode` gains `ver: u128`,
  set with `node::VERSIONS` (an SSA build sets it; plain and machine
  runs leave it 0 and pay nothing).
  - *Made when shared:* the version is made when the box becomes a
    shared value, `BoxNode::share` (`Arc::new` with the version),
    which replaces every site that wrapped a box in an `Arc` (38, some
    in tests; packing
    results, `\vsplit`'s rest and split, `\box255`, inserts, display
    math, alignments, the format's boxes). That is one rule where a
    version per packing function would have needed every other maker
    too.
  - *From its parts:* the version (`parts_version`) is made from its
    dimensions, glue setting, subtype, list and seal, with each box in
    the list by its own version. So a box costs what its own list
    holds, and `Hash` of a versioned box is its version. Equality
    (`PartialEq`) stays by content.
  - *Changed in place:* a box changed after it is made (`\wd`, §1247;
    `box_end`'s shift, §1076; the shift reset of `\lastbox`, §1081;
    `\vcenter`, §736) is made again (`reversion`), and the format's
    boxes are versioned in bulk (`version_tables`).
  - *In eqtb:* a box register's eqtb content combines the box's
    version instead of hashing the tree (`Canon::boxed` under
    `list_versions`), and check mode's content remakes it from the
    parts, so a box changed without `reversion` shows as a stale eqtb
    read.
- *The conditionals* (`conds.rs`). The stack, `if_limit`, `cur_if` and
  `if_line` are one slot (`track::Row::Cond`, `Fam::Alloc` 3), versioned
  by its contents at each write (`cond_wrote`: `push_cond` §495,
  `pop_cond` §496, `change_if_limit` §497, `common_ending` §498).
  Every reader reads it: the `\fi` test (§510), e-TeX's
  `\currentiflevel`/`\currentiftype`, `\showifs`, the group and
  condition warnings, the file end's check, the runaway error, the
  final cleanup, `\tracingifs`, and `start_input`'s `if_stack` entry;
  11 sites. `skip_line` is scratch, not a value: `pass_text` sets it
  before its only reader, the runaway error inside the same skip
  (§494). That corrects 7.17.12's row. `if_line` stores the number the
  `line:` read gave at the push, as data (the coordinator's ruling,
  DESIGN's `cond_stack` row). Until the source is a persistent
  sequence with the rank derived, a move therefore misses the calls
  that read an open conditional's stack, never a false hit.
- *The marks* (`page.rs`). `cur_mark` is one slot (`Row::Marks`,
  `Fam::Alloc` 4), versioned by its tokens at each write (`vsplit`
  §977, `fire_up` §1012), read by `\topmark` and friends and by
  `fire_up`. That is once per page and per `\vsplit`.
  `Tokens { toks, ver }` is the same value with a cheaper write, for
  when a profile shows the write.
- *Bulk versions are not writes* (`Tracker::row_made`): the pool's
  strings, the allocators, the conditionals and the marks versioned by
  `version_tables` at a format's load set their versions without
  noting a write of the running call. Runs of equal eqtb words (most
  are undefined control sequences) look up their content once.

**The texxet profile** (the 2e debt; `perf record -F 5000 -g`,
release, recorded build, 1870 samples, load 4). About 35% of the
recorded run is fixed startup:

| share | where |
|---|---|
| 8% | `Versions::set` (the version arrays' stores of the bulk versioning: about two million slots, twice) |
| 7% + 1.4% | sorting `(Slot, serial)` pairs: the root call's writes at its end |
| 4% + 4% | `close_frame`, `note_write` (the same writes, most likely the engine's initial tables, written through `set_eqtb` before the format replaces them) |
| 5% | `version_tables` |
| 4% | the kernel zeroing the arrays' pages |
| 4% | kpathsea's `cnf_get` (plain runs pay it too) |

The document's own work is a small rest: `texxet` is 1788 commands.
With `row_made` and the equal-word runs, recorded `texxet` takes
0.30–0.31 s, best of five, against the step 2d binary's 0.26 s run
alternately (13:14, load 4); plain 0.08 s. The remedy is the design's:
the format as a value whose dump is the write, its versions dumped
with it (7.17.12's `format_ident` row), and the engine's initial
tables as configuration rather than root writes.

**Measured.**
The course, `space` (release, in the sandbox, 2e's conditions, run
13:24–13:29 beside this step's `xtask check`, so loaded):

| | step 2e (12:43) | step 2f |
|---|---|---|
| recorded cold | 108.4 s | 146.3 s |
| cold calls: hits / misses | 59341 / 44116 | 55375 / 48082 |
| rebuild, `space` | 110.9 s | 127.2 s |
| rebuild: hits / misses | 102664 / 793 | 102643 / 814 |
| reads verified (cold) | 222.1M | 495.9M |
| commands re-run | 66.1M | 66.1M (no hit applied) |

The rebuild's misses by first differing read: `source` 767, `font` 15,
`eqtb` 14, `name` 10, `out` 3, `alloc` 3 (the conditionals' slot 2,
the pool's end 1), `fonttable` 2. The cold build has 3966 more misses
within itself and verifies twice the reads. My reading, not
established by a breakdown (the trace keeps only the last build): the
marks and the conditionals are now read where no call recorded them
before. A `	opmark` in a page's output routine, or a paragraph that
tests an open conditional, now misses a record made under other marks
or another stack. Such hits were unsound before, and the unsound
share would have shown only once a hit is applied. The times are
beside the gate and not comparable with 2e's.

The course in check mode (13:29–13:34, after the gate): no stale read
and no lost write, cold or rebuilt. 68 rows are not values yet. The
rebuild checked 50915 hits, with 47 rows differing (49 in 2e). The
conditionals' row (20450 in 2e) and the marks' are gone. The rest is
led by the input stack 44816, the files 36592, `line` 36581, the
buffer 31779, the save stack 1262, the group levels 524, the offsets
440, `objs` 348, the PDF writer's `out` 339, the file-name stacks
327.

e2e with `PARTEX_SSA_CHECK=1` (cold, then a rebuild): DVI and log
identical to the plain engine on `pages`, `texxet`, `cutoff`,
`readback`, `edits-dvi`, and `incr` against its third plain pass; no
stale read and no lost write on any. Check mode's list of rows not yet
values has 68 rows (70 in step 2e): the conditionals and the marks
left it. Boxes live inside eqtb's row and `objs`, so the list does not
count them.

**What remains** (the next boxes, in this order):
1. The node lists' running version (the prefix scheme of the token
   lists) and `cur_list`'s scalar fields as slots behind setters
   (about 90 direct write sites: `mode` 16, `mlist` 12, `pg` 6, the
   rest one to four each).
2. The page builder by field (`Builder`'s four functions).
3. The save stack as a table (the entries through their accessors;
   `save_ptr`, `cur_level`, `cur_group`, `cur_boundary`, about 60
   pointer writes).
4. Then 2e's brief: the PDF and DVI tables, the streams, the input
   state as the call's result, fonts, the scalar rows, and the hit.

**Gate.**
`scripts/sandbox cargo xtask check` on this tree (13:24–13:28):
rc 0; fmt, clippy with and without `partex-cli/trace` (`-D
warnings`), the wasm build and the tests passed; trip 7/7 and etrip
18/18 identical in both modes; e2e 34/34 identical, plain and in
machine mode.

### §7.17.10 step 3 (first box): two measurements, the node list, and no call yet (Agent, master, 13:36–15:00)

*In progress, handed off at 14:38 (the tree does not compile; see `~/.claude/jobs/77fe9c17/tmp/engine-handoff.md`).*

**What prompted it.** The box began as step 2g (the list rows) with a
measurement first. At 13:55 the coordinator redirected it to step 3
(the routines as recorded calls) after the user's revision of 7.17.10.
At 13:58 he corrected the brief: 7.17.2's full list of routines; the
accumulator rule for `build_page`; the node list as 7.17.12's row says
(a persistent sequence, not a `Vec` with prefix versions); and the
revised order, under which a routine becomes a call only once the rows
it reads and writes are values. From then on the rule is that a brief
restates DESIGN and any other form is a question. Uncommitted on
`master` (from 9d41c78).

**Measurement 1: which family's verified reads grew from step 2e to
step 2f** (the course cold build, recorded, the same aux files, one
after the other, loaded by builds; the step 2e tree in a worktree,
`~/code/tmp/partex-2e`, and the step 2f tree, both with the same
counters: `RecState::noted` and `RecState::verified` by family, the
allocators' slots apart, printed as `ssa reads noted/verified by
family`).

| family | noted, 2e | noted, 2f | verified, 2e | verified, 2f |
|---|---|---|---|---|
| eqtb | 5,578,498 | 5,578,498 | 205,322,466 | 452,417,318 |
| hash (`text`) | 882,268 | 882,268 | 11,667,605 | 26,422,881 |
| source | 199,528 | 199,528 | 4,218,109 | 10,105,554 |
| name | 405,545 | 405,545 | 2,285,508 | 6,312,827 |
| conditionals (2f's slot) | — | 21,274 | — | 2,045,985 |
| marks (2f's slot) | — | 310 | — | 1,108 |
| font (revisions) | 24,372,832 | 24,372,832 | 50,118 | 50,118 |
| total verified | | | 222,055,062 | 495,857,273 |

The noted reads are the same in every family but the two new slots,
so the recorder dedups (a call's reads are a set, and the new slots
add 21,584 reads). What grew is verification: every family's verified
reads grew about 2.2×, while the conditionals' slot itself accounts
for 2.0M. Cold hits fell from 59,341 to 55,375 and misses rose from
44,116 to 48,082. A miss walks every record under its name, each
record's reads in order until the first that differs. A paragraph
record's reads are mostly eqtb reads, so a miss verifies long eqtb
prefixes. The records that differ only at the conditionals' state
used to verify as equal (those hits were not sound to apply) and now
miss after their eqtb prefix. So the growth is real: records now hold
a row they did not. It is also 7.17.2's warning about a set per name
that grows with the document, which the routines' grain (small
records, names by inputs) is meant to end. The 24.4M font reads are
revisions noted without a stamp (`Tracker::read`), each a borrow of
the recorder.

**Measurement 2: the rebuild's `source` misses** (the `space` edit,
the step 2f code, rebuilt 13:44–13:49 with the trace). The trace's
`miss=@source:1.1/1` (713 of 767) is the first differing read of the
*first candidate* under the call's name, not the reason the record
made at the same place missed. So I classed the 813 missed calls by
what their own reads hold instead:

| missed calls | read a revision family |
|---|---|
| 443 | `font` and `fonttable` |
| 255 | `font`, `fonttable` and `out` |
| 17 | `out` |
| 17 | `font` |
| 2 | `fonttable` |
| 79 | none |

734 of 813 (90%) read a build-tagged revision (the fonts, the font
table and the `\write` streams), which never compares equal across
builds. They are one class: **every call that reads a font or a
`\write` stream misses on every rebuild**, because those rows are
still revisions (7.17.12's font and stream rows). The other 79 are
paragraphs whose first differing reads are a level's end and the
buffer's line at levels 3–6 (the files the chapter reads), names and
eqtb. They are the next measurement once the fonts are values. The
design's expectation, about one miss for this edit, needs the fonts'
and streams' rows first.

**Built.**

- The counters above (`ssa::count_ix`, `count_name`, printed per build
  by the CLI): counts, not a mechanism.
- `NodeList` (`partex-engine/src/nodelist.rs`), in the form the
  coordinator's 2g brief gave: the nodes with prefix versions, a
  polynomial mod 2⁶¹−1 of the nodes' versions, so an append is one
  combine. It is read through `Deref<[Node]>` and changed only through
  its methods: `push`, `pop`, `truncate`, `take`, `split_off`,
  `insert`, `remove`, `last_mut` (a guard that versions the last node
  again), `edit_tail` and `edit` (guards that version again from a
  position). It is kept only with `node::VERSIONS`. The nest's lists
  (`ListStateRecord::list`) and the page builder's list
  (`Builder::list`, `build`, `fire_up`, `release_held`) use it. Check
  mode tests each list's running version against its nodes at every
  call boundary (`Tex::stale_lists`): 0 stale on `pages`, `texxet`,
  `cutoff`, `incr`, `readback` and `edits-dvi`, whose DVI and log stay
  identical to the plain engine. **This is not the design's form.**
  7.17.12's `cur_list` row names a persistent sequence in which an
  append shares the prefix. A `Vec` copies in O(n), so a list cannot
  be passed or returned as a shared value. The coordinator's
  correction says not to continue it; whether it stays until the
  persistent sequence replaces it is his question to the user.
- `hpack` and `vpack` as recorded calls were built (13:55–13:59) and
  **reverted**. `hpack` reads the fonts, which are revisions, not
  values. Both write `last_badness` and, when they report, the
  printing rows (`selector`, the offsets, the log's bytes), which are
  not values. I had moved the report out of the call, which is a form
  7.17 does not state. By the revised order, neither is a call until
  those rows are values. The patch is kept at
  `~/code/tmp/step3-all.patch` for reference.

**Where step 3 starts.** No routine of 7.17.2's list reads and writes
only values yet. The nearest is `vpack` (§668). Its rows are the
parameters (eqtb, values), its arguments (the list, the spec, the
depth), `last_badness` (a scalar), the printing rows its report
touches (`selector`, `term_offset`, `file_offset`, `output_active`),
and the log's and terminal's bytes, which are effects a hit
re-emits. Then `hpack` needs the fonts; `line_break` needs the fonts,
hyphenation and `cur_list`'s fields; `build_page` the builder's fields
with the page-so-far as its argument; `macro_call` and `expand` the
input state.

### §7.17.10: the fonts and hyphenation as values (Agent FONTS, branch ssa-fonts, 16:40–17:23; corrected 17:25–17:41)

**What prompted it.** Step 3's measurement, above: on the course's
rebuild after one edit, 734 of 813 misses read a build-tagged revision
(the fonts, the font table and the `\write` streams). A revision never
compares equal across builds, so every call that read a font missed on
every rebuild, whatever the edit. The coordinator split 7.17.12's
remaining rows among three agents (ENGINE, WRITERS, FONTS). This entry
covers the FONTS agent's two rows, in the forms the rows give:
`fmem_ptr, font_ptr, fonts` and `hyph`. Uncommitted, on branch
`ssa-fonts` from `575990a`.

**The fonts.** A font slot `f` has 16 address slots, `Font(16f + k)`,
one per field (`track::font`):

| k | field | version, made at the write |
|---|---|---|
| 0 | the metrics (TFM tables, name, size, `\fontname`) | the font's identity |
| 1 | the parameters (`\fontdimen`, §578) | the parameters' contents |
| 2, 3 | `\hyphenchar`, `\skewchar` | the value |
| 4 | the expansion parameters and the font's chain of expanded fonts | their contents |
| 5 | the space glue cache (§1042) | its contents |
| 8–15 | pdfTeX's code arrays `\lpcode` … `\knaccode` | a sum of per-character terms |

A font is an immutable shared value, so its metrics are versioned by
the identity it was loaded under (`FontData::idv`), not by a hash of
its tables. That identity is what `Host::font_slot` already keys fonts
by: a TFM font's is the hash of its bytes, name, area and size, an
expanded font's is made from its base's and its ratio, a copy's from
its source (`FontIdent::content`). It is computed once, at the load,
so a read of a metric costs an array load and hashes nothing. Three
writers reshape a font in place: letterspacing (`pdf/vf.rs`), and
pdfTeX's `\tagcode` and `\pdfnoligatures`. They mark the font
`remade`, and a remade font's metrics version is made from their
contents at that write. The code arrays keep, per font and code, a sum
of a hash of each character's non-default value (`FontData::code_sum`),
so `set_code` updates one term and a read is an index. The table of
loaded fonts (`FontTable`, one slot) is `FontOrder`, a `PVec` of
(slot, identity) in load order, versioned with `font_ptr` and
`fmem_ptr`. It is read by whatever depends on which fonts exist: a
load's search (§1260), the room test, `\fontdimen`'s growth of the last
font (§580) and the statistics.

**Hyphenation.** Two slots of the `Hyph` family, and a family for the
words (as corrected at review, below).
- *The patterns* (`Hyph(0)`) are one slot. `\patterns` is legal only
  in INITEX before `init_trie` packs the trie, and a packed trie is
  never changed, so a paragraph reads the patterns once. Each part
  `\patterns` stores (a pattern's op after the duplicate test, the
  hyphenation codes `\savinghyphcodes` stores with the patterns,
  `init_trie`'s pack) folds into a running version
  (`HyphState::pat_ver`). After a format's load it is the packed trie's
  contents.
- *The exceptions* (`Hyph(1)`) are `Exceptions`, a `PMap` from a word
  with its language (`exception_key`) to its positions, whose version
  is the map's, together with `hyph_count` and `hyph_next`. This is
  the table as a whole, which an insertion (§940), the format's dump
  and the statistics read. The dump walks the map in sorted order (a
  HAMT iterates in its hashes' order).
- *A word* is its own address, `Fam::HyphWord`, the way a control
  sequence's name is (`Fam::Name`) and web2c's string search is
  (`Fam::Search`). The word (`exception_key`: its `\lccode`s and
  language) is interned in the record, and its version is what the map
  holds for it: the positions' version, made once when they are
  entered (`Positions`), or `NO_EXCEPTION`. `line_break`'s lookup
  (§930) tells the recorder through `Tracker::hyph_word_read(key,
  version)`, taking the version from the entry it found, so the read
  hashes nothing but the map's key. A check verifies it by looking
  again (`EngineView::hyph_word`). An insertion (§940) reads the
  table's slot and the word's, and writes both
  (`Tracker::hyph_word_wrote`). §939's hash (`hyph_word`, `hyph_link`)
  is kept for tex.web's overflow behaviour and the format, but
  addresses nothing.
`hc`, `hyf` and `cur_lang` stay scratch.

**Recording.** Each accessor records the field it uses, and only that
field.
- `Tex::font_read(f, k)` is used by `char_exists`, `char_metrics` and
  the warnings, main control's glyph and ligature paths, `\iffontchar`,
  `\fontname`, font selection, display and the math environment.
  `TrackedFonts` wraps the fonts for `hpack`, the alignments and the
  display box, recording the metrics per font it is asked for.
  `line_break`'s environment records the metrics, `\hyphenchar`, the
  expansion parameters, the codes, the patterns and the word's bucket.
- A writer calls `font_wrote(f, k)` after the store: `\fontdimen`,
  `\hyphenchar`, `\skewchar`, the code arrays, pdfTeX's expansion and
  letterspacing, the glue cache.
- A load calls `font_made(f)` for all the font's fields and the table.
- The engine as made and a format's load version everything
  (`version_fonts`, `version_hyph`, from `version_tables`).
`SsaTracker` now ignores `Cell::Font` and `Cell::FontTable`. The row
hooks carry fonts in SSA mode, and machine mode's cells (`MCell::Font`,
`order_hash`, `retagged`) are unchanged.

**Rejected.**
- *One version per font.* A `\fontdimen` write (`\fontdimen2\font=…`,
  the idiom that changes a font's interword space) would move every
  read of the font's metrics. The row gives each settable field its
  own value.
- *The metrics by a content hash made at a read*, like step 2's lazy
  cache. A read must be an array load. The load already has the
  identity.
- *One slot for all the exceptions.* One `\hyphenation` would miss
  every paragraph that hyphenated any word.
- *§939's 607 buckets as slots* (this entry's first form, rejected at
  review). They mirror web2c's hash layout, which AGENTS.md rules out,
  and a change to one word makes every other word in its bucket miss.
  A word addressed by itself costs one interning per lookup, as a
  name's lookup does.
- *A slot per character for the codes.* That is 256 × 8 per font for
  arrays that microtype sets in the preamble. The sum keeps both a
  write and a read at O(1).

**Measured** (plain TeX documents from `tests/e2e`, the release
binary, a format made by each binary, a first plain pass for the aux
files, then an SSA cold build and a rebuild with the same records
through `PARTEX_SSA_REBUILD`; `master` at `575990a` against this branch;
wall times are omitted because the gate ran alongside). The rebuild
with no edit:

| document | misses, before → after | paragraphs hit | commands in hit paragraphs |
|---|---|---|---|
| `pages` | 4 → 2 | 11 → 13 of 14 | 32 → 16,478 of 16,514 |
| `texxet` | 41 → 11 | 1,332 → 1,362 of 1,372 | 1,511 → 1,690 of 1,788 |
| `cutoff` | 51 → 11 | 45 → 85 of 95 | 48 → 6,211 of 6,225 |
| `edits-dvi` | 242 → 2 | 40 → 280 of 281 | 40 → 3,967 of 3,995 |

The misses by the family of their first differing read:

| document | before | after |
|---|---|---|
| `texxet` | font 10, fonttable 3, source 27, alloc 1 | source 9, out 1, alloc 1 |
| `cutoff` | eqtb 42, font 5, out 3, alloc 1 | eqtb 7, out 3, alloc 1 |
| `edits-dvi` | font 241, alloc 1 | line 1, alloc 1 |

No miss on this branch has a font, the font table or hyphenation as
its first differing read. `edits-dvi`'s rebuild verified 1,411,192 reads,
against 3,074,494 before, because a miss walks each record's reads
until the first that differs. `pages` after a word edit inside
`\words` (`consequat. }` → `consequat, dolor. }`) has 10 misses both
before and after: alloc 2, eqtb 4, source 1 as first differing reads.
The edit changes every paragraph that uses `\words`.

**Check mode** (`PARTEX_SSA_CHECK=1`, cold and rebuild, on `pages`,
`texxet`, `cutoff`, `readback`, `edits-dvi` and `incr`):
- The DVI and log are identical to the plain engine's (`incr` against a
  third plain pass, since its rebuild is a third pass; `pages`' log
  differs only in the minute on its first line).
- Lost writes 0, and no table read whose version was not the content.
- The "not values" list has 48 rows, and the fonts and hyphenation are
  not among them (`statehash.rs` no longer lists them).
- INITEX `\input plain \dump` gives an identical format and log.
- The microtype document (pdfTeX, `--compat=pdftex`, expansion,
  protrusion and letterspacing) gives an identical PDF and log with a
  fixed `SOURCE_DATE_EPOCH`.

**Found: marks written without being read.** On `pages`' word-edit
rebuild, check mode reports one write that differs:

```
alloc:4 (Row::Marks) hit #cf90… body #f01f…
```

This is at the paragraph `\setbox0=\vbox{…}` (line 22), whose call
goes on to `\vsplit`. The hit's and the body's 56 reads are equal.
`vsplit` (§977) clears the split marks of every class in `cur_mark`
and writes the marks row. The version it writes is a hash of all of
`cur_mark`, including the top, first and bottom marks, which the call
never read and the edit changed. The write is a read-modify-write
recorded as a write only. The fix belongs to the marks row
(`page.rs`, step 2f): a read of `Row::Marks` before `vsplit`'s two
updates (§977, §979), as `fire_up` already makes before its own
(§991). Before this branch the paragraph always missed on a font
revision, so check mode never compared it. The coordinator gives it to
the engine agent, whose fix is 7.17.12's form for the marks: shared
token lists by class, so a write depends on nothing it did not read.

**Corrected at review** (17:25–17:41, the coordinator's three points).
- *A word by itself, not by bucket.* The first form above addressed
  §939's 607 buckets (`Hyph(2 + h)`, with a per-bucket sum
  `HyphState::word_sum`). Both are gone. A word is now `Fam::HyphWord`,
  as described under **Hyphenation**. This adds two `Tracker` hooks
  (`hyph_word_read`, `hyph_word_wrote`), the interned words in
  `RecState`, the view's arm and `EngineView::hyph_word`, all in
  ENGINE's `track.rs` and `ssa.rs`. `partex-engine`'s `exception_hash`
  is removed, and `Exceptions` gains `entry` and `word_version`.
- *The times* in this entry's title, which first said 16:38–17:45.
- *DESIGN's family for fonts and hyphenation*: the conventions said the
  groups family, against the table and convention 1. It is the tables
  family.

Measured after the correction (the release binary, check mode on).
`hyph.tex` is a scratch document of the agent's, not committed: plain
TeX, `\hsize=1.1in`, `\pretolerance=-1`, two `\hyphenation`s
(`tab-u-la-tion`, `rep-re-sen-ta-tion`), four paragraphs that
hyphenate `tabulation`, `representation` and `manuscripts`, a short
one and a `\showhyphens`.
- Rebuilt with no edit, it has 2 misses of 43 calls.
- After `tab-u-la-tion` → `tabu-la-tion` it has 9 misses (3 fresh). The
  edited line misses on its source. The three calls that look
  `tabulation` up (two paragraphs and `\showhyphens`) miss on
  `hyphword:0`. The following `\hyphenation{rep-re-sen-ta-tion}`
  re-runs, because it reads the table as a whole. `\bye` misses on
  the streams' revision. The paragraphs of `representation` alone, of
  `manuscripts` and the short one hit.
- The DVI is identical to a plain run of the edited file.
- With no edit, check mode is silent on `hyph`, `pages`, `texxet`,
  `cutoff`, `readback`, `edits-dvi` and `incr`. INITEX `\input plain
  \dump` (14 exceptions) gives an identical format and log.

### Plain cold regression, part 1: the clones on the token path (Agent ENGINE, master, 16:28–17:59; commit df2ea56)

*What prompted it.* Making the objects the entries' values, the scalar
rows slots and the node lists persistent (42b576a, 833d498) slowed
plain cold on the course from 45–48 s (5ae2807) to 60–68 s. About
three quarters came from the objects and the scalar slots. A row's form
was not to change; the cost was to come out of the code around it.

*What changed.* `push_input` no longer clones the level's `Arc`, and
is inlined with its overflow path cold. `macro_call` clones its
meaning once instead of twice and keeps its arguments in a reused
vector, not a `[Option; 9]`. The bulk runs and the skip cache borrow
the level's list instead of cloning it. `str_toks`, `text_toks` and
`tok_from` take their lists from the pool. `null_list` and
`omit_template` are made once, and `take_def` keeps `def_ref`'s
buffer. A `PVec` append, and a set of its last element, are O(1): the
head hash is kept per node. `NodeList::into_vec` moves nodes out of
trees nobody else shares instead of cloning them. `eqtb_obj` is a flat
`Vec`.

*Measured.* Plain cold, the course, `meas.sh` (the binary's own
format, two passes each), load 6–7, back to back:

| binary | run 1 | run 2 |
|---|---:|---:|
| before (575990a) | 62.4 s | 64.7 s |
| after (df2ea56) | 51.4 s | 54.4 s |
| 5ae2807 | 48.5 s | 47.8 s |

*What is left*, from `perf stat`: instructions are within 4 % (271G
against 261G), but atomic lock operations are 649M against 31M and L1D
refills 1.53G against 0.84G. The rest of the gap is the `Arc`
reference counts on token lists: a clone and a drop per macro call,
per parameter level (`#n`) and per pooled list's `get_mut`, about 4 s
of cycles. The levers the agent names, queued: parameter levels read
`param_stack` by index instead of cloning, and a macro level reads its
table entry, the list moving into the level only when the macro is
redefined. The task was wrapped here, gate-clean (`xtask check` exit 0
at 17:52), because the user put hits and evaluation first.

### Coordinator, 16:30–18:00: three agents by family; the paragraph boundary; a rule withdrawn

*Three agents, split by the design.* At 16:35 the user allowed up to
three agents on disjoint work, citing Conway's law: a system's design
copies the structure of the team that builds it, and implementation
speed was not to compromise quality. So the split follows the design's
own seams, 7.17.12's families, not convenience. One agent (ENGINE)
owns every shared mechanism: the runtime, the tracker, the record
format and the store. Each other agent owns a family's files and only
adds that family's variant, its version arm and its accessors. FONTS
took the fonts and hyphenation (a6ffdb0), and WRITERS the DVI and PDF
writers' tables. Every brief restates the same two conventions, and
each branch is reviewed against them before it merges.

*The review of FONTS.* The first version read a hyphenation exception
by web2c's hash bucket (§930's `h`, one of 607). That copied tex.web's
table layout, against the rule to port behaviour rather than layout,
and it made a change to one word a miss for every word sharing its
bucket. The word is now its own address, interned in the record the
way a control sequence's name is (`Fam::HyphWord`).

*The paragraph boundary* (owed since step 3's first box). The first
calls were the paragraph and the tokenizer, and both start and end at
§7.16's clean points. The paragraph was kept as the unit of work
because the machine mode's regions were cut there, not because §7.17
names it: 7.17.2 makes every routine a call, and the paragraph is
only the call that contains `line_break`, the `hpack`s and the page
steps. Keeping it as the grain was the old seam coming back. On
2026-09-28 the user asked "why are you only splitting by paragraph?",
and the routines became the work: `unsave` first (575990a), then
`hpack`, `vpack` and `line_break` (FONTS, in progress).

*Hits and evaluation first* (17:40). No hit was applied yet: a record
held only its writes' versions, so a hit was found and verified but
the body still ran. The user: "are you doing SSA hit/eval? Please."
ENGINE's task became hits applied: each record's writes hold their
values, the store puts each one back, the effects are replayed and the
body is skipped. The remaining state is split into three lanes by
family: tokens with ENGINE, lists and the page with FONTS, output with
WRITERS.

*A rule withdrawn* (17:50). I asked the user to approve a rule for a
record whose write cannot be put back yet, because its state was not
yet converted: evaluate it as a miss. The user approved it, then
withdrew it: "I hate how you feel like that current code should change
our decisions … give me clean design." The rule existed only because
the conversion was unfinished. The design has no such case: every
piece of state is a value, and a hit puts every write back. So the
rule is in neither DESIGN nor the code. Hits are tested in check mode,
and they become the default when the whole design is in and check mode
is silent on the course. A lesson for the book: an unfinished
conversion is a fact about the plan, never a design decision.

### §7.17.10: hpack, vpack and line_break as recorded calls (Agent FONTS, branch ssa-fonts, 17:44–18:10)

**What prompted it.** 7.17.2 lists the engine's routines that are
calls, and 7.17.10's revised order makes each one a call as soon as the
rows it touches are values. `unsave` (575990a) built the mechanism:
`Tracker::call_begin(f, name, view)` probes the record set for the
name, `call_end` closes the frame, and check mode compares a probed
hit's recorded writes with the writes its body makes again. The
coordinator gave this agent the three routines of the lists family:
`hpack` (§649), `vpack` (§668) and `line_break` (§815), through that
mechanism as it is. Hits stay probed: ENGINE makes them apply, and a
record only needs its writes recorded the way `unsave`'s are.
Uncommitted, on branch `ssa-fonts` from a6ffdb0.

**The boundary rule, applied** (7.17.12: what is live at a routine's
boundary is its argument or its result, never a re-classified field).
The scratch rows were checked against a pack's boundary:
- `pack_begin_line` is live inside `line_break` and `fin_align` (§815,
  §800): a report says "in paragraph at lines 3--7" or "in alignment at
  lines …". Only its *sign* changes which words are printed, so the sign
  is part of the name. The lines are printed through `print_line_no`,
  which already records a position read (`line:j±d`, 7.17.4). A pack
  that prints no report therefore does not depend on the lines.
- `adjust` (§655) is live inside `hpack` when the caller gives it a
  list to migrate into. Whether it is given is part of the name, and
  what migrated is part of the result.
- `cur_box` is not involved: a pack returns its box, and the box is the
  result.
- `font_in_short_display` stays scratch: a report sets it before
  `short_display` (§663).
- `last_badness` (§646) is a value, a scalar slot since 42b576a. Each
  report writes it through `scalar_wrote`, so it is in the record.

**The names.** Each name is `Version::node(tag, parts)`:
- `hpack`: the list (`Version::of(&[Node])`, each box by the version it
  carries), the spec (`exactly` or `additional`, and the size), and a
  triple: whether `adjust` is given, the packer, and `pack_begin_line`'s
  sign. The packer is §649's, or pdfTeX's `hpack_line` with the fonts
  expanded to fit (`\pdfadjustspacing`, a line of §889).
- `vpack`: the list, the spec, the depth limit `l` (§668) and the sign.
- `line_break`: the paragraph's list (`NodeList::version`, O(1)) and
  whether a display follows (§815's `d`).

Everything else is a read, recorded where the body reads it: the
parameters (`\hbadness`, `\hfuzz`, `\overfullrule`, TeXXeT and the
rest), each font's fields through `TrackedFonts`, and for `line_break`
the paragraph parameters, the patterns and the exceptions by word.

**The results.** A result is a write to a slot of its own, with a
version made from content: `track::scalar::HPACK_RESULT` (88),
`VPACK_RESULT` (89) and `LINE_BREAK_RESULT` (90).
- `hpack` writes a version of the box's parts, the migrated material
  and the glue totals by order. §796 keeps the stretch in an alignment
  entry, and §1199–§1201 test a display's shrink.
- `vpack` writes the box's parts and the totals.
- `line_break` writes the version of `just_box`, the last line
  (`parts_version`), which §1146 measures for a display. The lines it
  appends to the enclosing vertical list are writes of that list's
  `List` row, as any append is.

A record with its writes needs nothing new from the runtime. A result
parameter on `call_end` was rejected: the mechanism was to be used as
it is, and a result slot is already compared by check mode's write
comparison with no new code.

**line_break's children.** A paragraph's record should hold its
`line_break`, and the `line_break` record should hold one `hpack` per
line (7.17.2's example). So the engine's breaker no longer packs.
- `post_line_break` (§877–§890) returns each line as
  `Item::Line { list, width, shift }`: the hlist, the width §889 packs
  it to, and the shift.
- `Item::Migrated` is gone.
- The core prints the break's events first (`\tracingparagraphs`, the
  hyphenation and expansion errors), which is TeX's order: every one of
  them happens during the search, before §877.
- The core then packs each line as an `hpack` call (`pack_line`). With
  `\pdfadjustspacing > 0` the call's body is `expand::hpack_line`, with
  `get_expand_font`'s errors surfacing as the call's `Err`. The box gets
  its shift (§889) and is appended; the migrated material follows it
  (§888).

The other §649 and §668 calls of main control go through the calls
too: `\hbox`, `\vbox` and `\vtop` (§1086), §796's cells and §799's
rows, and §1199's display box. The packs inside a routine's own work are
not recorded yet:
- `mlist_to_hlist`'s quiet packs (engine);
- `fin_align`'s prototype (§804) and rules (§806);
- `vsplit`'s box (§977), made inside `vpage::vsplit`, which is the page
  family's.

Two alternatives were rejected:
- `pack_begin_line`'s value in the name. Every pack of a paragraph would
  then depend on the paragraph's start line, so an edit that moves a
  paragraph by a line would miss every line's `hpack` even though none
  prints a report.
- A name that leaves the kind out. A pack in a paragraph and the same
  list packed in an alignment print different reports, and they would
  share a record whose effects differ.

**The counters.** `ssa::RoutineCount` counts calls, probed hits and new
records per `Func`, and the CLI prints them after each SSA build line
(`partex: ssa build N routines: hpack X (hits Y, records Z), …`). The
three new `Func` variants and the counters are in ENGINE's `ssa.rs`,
the result slots in `track.rs`, and the printing in `partex-cli`.

**Measured** with the release binary, plain TeX (`--compat=tex`), a
second pass after one plain pass, `PARTEX_SSA=1 PARTEX_SSA_CHECK=1`,
and the no-edit rebuild (`PARTEX_SSA_REBUILD=true`). Each cell is
calls / probed hits / records made:

| document | `unsave` | `hpack` | `vpack` | `line_break` |
|---|---|---|---|---|
| pages | 220 / 220 / 23 | 1052 / 1052 / 80 | 120 / 120 / 47 | 68 / 68 / 29 |
| texxet | 55 / 55 / 28 | 42 / 40 / 41 | 10 / 10 / 8 | 4 / 4 / 4 |
| cutoff | 320 / 320 / 8 | 400 / 400 / 88 | 160 / 160 / 82 | 80 / 80 / 80 |
| readback | 280 / 280 / 7 | 120 / 120 / 81 | 160 / 160 / 82 | 40 / 40 / 40 |
| edits-dvi | 598 / 598 / 9 | 251 / 251 / 247 | 20 / 20 / 12 | 240 / 240 / 240 |
| incr | 248 / 248 / 24 | 390 / 380 / 102 | 44 / 42 / 28 | 86 / 86 / 38 |
| hyph | 8 / 8 / 8 | 20 / 19 / 20 | 5 / 5 / 5 | 6 / 5 / 6 |
| microtype (pdfTeX) | 27 / 27 / 20 | 141 / 122 / 99 | 4 / 4 / 4 | 25 / 14 / 25 |

- *Records* counts the records the build adds (a record for every
  distinct name and read set), so a document that repeats a paragraph,
  as `pages` does, has fewer records than calls.
- `incr`'s rebuild is its third pass, compared with a third plain pass.
- `microtype` is a scratch document of the agent's: pdfTeX with
  `\pdfadjustspacing=2`, expanded cmr10 and a few loose paragraphs, run
  with `--compat=pdftex`, `SOURCE_DATE_EPOCH=1700000000` and
  `FORCE_SOURCE_DATE=1`.

On every document, cold and rebuilt, the output (DVI, or microtype's
PDF) and the log are identical to a plain run (microtype's log past its
banner line), with 0 lost writes, 0 stale reads and no "write differs".
The gate: `scripts/sandbox cargo xtask check` exit 0 (e2e 34/34 in
both modes).

**Why a call misses on a no-edit rebuild.** Most misses are misses of
the caller, not of the pack.
- A texxet `hpack` inside a paragraph missed on `line:1+7`: an
  underfull line printed "in paragraph at lines 1--8".
- Line reads are recorded relative to the *paragraph's* start (the
  `src` base of 7.17.4), but `ssa::View` verifies them against the
  level's current line (`level_line(j)`). For a child probed in the
  middle of a paragraph, they agree only when `d = 0`.
- This is sound and conservative. Verifying against the paragraph's base
  instead would be unsound: the same hbox twice in one paragraph would
  hit with a stale line.
- The fix is a base per open call, with the runtime rebasing a child's
  position reads into its parent. That belongs to ENGINE (`ssa.rs`), so
  it is reported, not changed here.
- `line_break` reads `mode_line` (`ml`, §815's `pack_begin_line`) as the
  `List` row, versioned by its absolute value. A paragraph moved by one
  line therefore misses `line_break`, though only a report prints the
  line. The form 7.17.4 gives a position is `line:j±d`, which is also
  ENGINE's.

**What it costs.** A pack's name hashes its list (`Version::of`: O(n)
in the list's nodes; a box inside it is hashed by the version it
carries, O(1)), and its result hashes the box's parts again, so every
pack in SSA mode walks its list twice. `line_break`'s name is the
list's running version, O(1). With recording off (`T::VALUES` false)
the calls are the bodies. Measured as wall time, the minimum of 7 to 10
runs, with the release binary, the second pass, and cold (no session),
comparing a6ffdb0 (base) with this tree (new):

| document | plain, base | plain, new | SSA, base | SSA, new |
|---|---|---|---|---|
| pages | 137 ms | 139 ms | 346 ms | 340 ms |
| edits-dvi | 78 ms | 77 ms | 346–376 ms | 373–391 ms |

The spread between repeated runs of one binary is about 20 ms. The
calls' cost is inside it, and SSA's cold overhead lies elsewhere.

### §7.17.12, the output family: the DVI and PDF writers' tables are values (Agent WRITERS, branch ssa-writers, 16:40–18:15)

**What prompted it.** Four rows of 7.17.12 were still hashed whole at
every probed hit by check mode's by-row comparison (`Tex::value_rows`):
`dvi`, `pdf`, `tounicode`, and `fontmap, fonts_mapped`. On the course,
step 2f's check listed the PDF writer's `objs` (348 hits) and `out`
(339) among the rows that differed. The coordinator's brief was to
make them values in the rows' own forms: a field per table, each a
persistent or shared value carrying its version (the structures'
convention), and the bytes as effects.

**What changed.**

- `pdf/val.rs` (new) holds the structures and the writer scope.
  - `VMap` and `VSet` wrap partex-ssa's `PMap`. The map's version is
    the sum of one hash per entry, so an insertion versions only what
    it changed. They hold the object table's eleven lookup trees
    (avlstuff.c), `font_attr`, `nobuiltin_tounicode`, the encodings
    read (writeenc.c), `\pdfglyphtounicode`'s table and the TFM names
    whose map entries were used.
  - `VTab` is a table by index, stored in shared chunks of 64. A clone
    shares every chunk; a change copies one. Each element is stored
    with the version it was made with, and the table's version is the
    sum of `contribution(i, v)` over its elements. `get_mut` marks an
    element dirty, and `settle` versions each dirty element once. It
    holds the object table (pdfTeX §695's `obj_tab`, §7.16's
    `Numbering`), the destination names (`Dests`), and the per-font
    state with the glyphs used (`Glyphs`).
  - `Val<T>` is a record. It is owned while it changes, and shared
    (`Arc`) with its parts' versions cached once its scope ends.
    `DerefMut` takes it back, copying only while a record of the
    runtime still holds it. It holds `out` (the writer), the `*_toks`,
    `space_font_name`, the stacks, `fontw`, `last_match`, the shipping
    state (the order of first use, `FontOrder`, is one of its parts),
    and the DVI writer. The DVI writer has three parts: the fonts
    defined, the totals (`total_pages`, `max_v`, `max_h`,
    `max_push`), and where the file is.
- The fields: 36 for the PDF writer (`Row::Pdf(f)`, `pdf::val::field`)
  and 4 for the DVI writer (`Row::Dvi(f)`). `ssa::Fam::Pdf` and
  `Fam::Dvi` are dense families with a stamp per field, and each has
  its arm in `View`'s `Store::version`. The counts and the `\pdflast…`
  values are versioned by their values.
- The writer scope (DESIGN 7.17.12's new convention bullet, in
  7.17.2's terms: each routine that changes the tables is a call, its
  reads the fields at its start, its writes the fields it made,
  versioned when the call ends). `ship_out`'s two backends (§640, pdfTeX §750) run in one. So do
  `\immediate\pdfxform` and `\pdfobj` writes, `\pdfximage`'s image
  writes, a `\pdf…` command (pdfTeX §1537–§1599, with the fields per
  command in `pdf_extension_fields`), `\pdfmapfile` and `\pdfmapline`
  (mapfile.c's `process_map_item`), `\pdfglyphtounicode`
  (pdfTeX §1587), `\pdfcolorstackinit`, and the job's end (§642,
  pdfTeX §794).
  - At entry, the scope notes a read of each declared field that no
    open scope has written yet.
  - At the outermost end, each written field is settled: its dirty
    elements versioned, its record shared, and `Tracker::row_wrote`
    given the version made from its parts.
  - `\pdfmatch`'s result, the `\pdflast…` values and `\pdfoutput`'s
    fixing are read and written at the access.
  - Plain runs (`!T::VALUES`) run the body alone.
- Effects. A recorded run pushes to `Emitted` the bytes handed to the
  host (`pdf_write_pending`, `flush_dvi`) and each page given to the
  DVI sink (the page's persisted bytes). `ssa.rs` notes them as the
  open call's effects (`Runtime::note_effect`) when the call closes.
- Loads: map files, encoding files, font files, images, virtual
  fonts, and the files `\pdffilesize`, `\pdfmdfivesum` and
  `\pdffiledump` read are each a `Tracker::load`.
- Removed:
  - the state hash's `tounicode` memo: a persistent map's hash is its
    version;
  - from `value_rows`: the DVI state, the PDF state's parts, the
    tounicode table and the font map. Check mode now covers them by
    the record's writes and the stale-read test.

**Call sites in other owners' files** (each minimal):

- `track.rs`: `Row::Pdf`, `Row::Dvi`.
- `ssa.rs`: the families, the stamps and `note_writer_effects`.
- `equiv.rs`: `version_tables` calls `version_writers`.
- `tex.rs`: the field types, and `dvi_writer` through `as_deref`.
- `format.rs`: the maps' dump through `sorted`, the load through
  `clear` and `insert`.
- `hashmemo.rs` and `statehash.rs`.
- `machine.rs`: the destination names by iterator.
- `display.rs` and `maincontrol.rs`: `pdf_obj_aux`.
- `run.rs`: `pdf_output_fixed`.
- partex-engine's `dviout.rs`: `DviWriter::part_version`.

**Why this form, and what was rejected.**

- *A version made at every change at the access* (each `obj_tab`
  store, each glyph's bit). Rejected: one page changes an element many
  times, and versioning each change is the cost the scope avoids. A
  scope versions each changed element once, when the routine that
  made it is done, which is when another call can first see it.
  Nothing is hashed at a read.
- *partex-ssa's `PVec` for the tables.* Its `set` replaces an element
  and versions it with its path at each change. The writers change
  entries in place many times in one routine (an object's offset when
  it is written, a font's glyphs per character). `VTab` keeps the
  dirty elements instead, and versions them once.
- *Hashing a table whole at the scope's end.* That is O(table) per
  page, and the object table grows with the document. `VTab`'s sum
  changes by the dirty elements' contributions only.
- *One scope with every field for every command.* Rejected: a command
  that touches only the color stacks would read and write every table.
  `pdf_extension_fields` gives each command its own fields.

**Measured.**

- *SSA e2e in check mode.* `PARTEX_SSA=1 PARTEX_SSA_CHECK=1`, a cold
  build then a `PARTEX_SSA_REBUILD` with no edit, in the directory of
  a plain build's two passes. Release binary, in the sandbox.
  - Documents: `pages`, `math`, `align`, `cutoff`, `edits-dvi` and
    `incr` (plain format, DVI); `readback`, `cutoff-pdf` and
    `effects` (plain format, PDF); `texxet` (`-ini -etex`, DVI);
    `microtype` (`-ini`, PDF).
  - The DVI or PDF and the log are byte-identical to the plain
    engine's first pass (cold) and second pass (rebuild) on all 11.
  - No stale table read and 0 lost writes, cold and rebuilt. No
    `writes: pdf` or `writes: dvi` difference.
  - Check mode's list of rows not yet values went from 50 (the base
    575990a on `pages`) to 16. The 34 that left it are `dvi`,
    `tounicode`, `fontmap, fonts_mapped` and the PDF state's 31 parts.
    The 16 left are the other families' rows, plus `log_file.id`,
    `write_file`, `read_file` and `random`. None of the writer rows
    is among the uncovered rows at any hit.
  - The five DVI documents gave the same result with
    `PARTEX_DVI_THREAD=0`, where the DVI writer runs in the engine.
- *The formats.* `pdflatex.fmt` made by the base (575990a) and by this
  tree (`-ini -jobname=pdflatex -translate-file=cp227.tcx
  *pdflatex.ini`) are byte-identical, 15,551,983 bytes. The maps'
  dump order is unchanged.
- *Plain cost on the course.* A copy of `course-master` with its
  `_out` aux files and the format above, one process, batchmode,
  `--compat=pdftex`, release, in the sandbox. The machine was loaded
  (load average 5–9: other agents' builds).

  | binary | round 1 | round 2 |
  |---|---|---|
  | base 575990a | 63.9 s | 62.7 s |
  | this tree | 62.6 s | 65.3 s |

  The difference is within the noise. The PDFs have the same content
  (`scripts/pdfcheck same`), except for `/CreationDate` and `/ModDate`,
  because the sandbox clears `SOURCE_DATE_EPOCH` when it is set
  outside. The logs differ only in the output directory's name and
  the banner's minute.
- *The course in check mode* (both binaries side by side, a cold
  build then the `space` edit): neither finished its cold build in
  1800 s, the base's (575990a) no more than this tree's, and both were
  stopped there. Check mode runs every hit's body twice and compares
  the whole table of rows at each, so the course needs a longer run
  than a report's; the next entry measures SSA mode without it.

**Gate.** `scripts/sandbox cargo xtask check` on this tree
(17:46–17:53): rc 0. That covers fmt, clippy with and without
`partex-cli/trace` (`-D warnings`), the wasm build and the tests;
trip 7/7 and etrip 18/18 identical in both modes; e2e 34/34
identical, plain and in machine mode. The first run failed on three
clippy lints in the new code, fixed: `type_complexity`, a truncating
cast, and a missing semicolon.

### §7.17.10: the alignment state as values, and every pack a call (Agent FONTS, branch ssa-fonts, 18:13–18:50)

**What prompted it.** The coordinator's lane for this agent is lists and
the page. Its second piece is three rows of 7.17.12's lists family in
their table forms: `align` (`cur`, `stack`, columns, tabskips),
`last_badness` and `shown_mode`. After the first piece was committed
(4f4b7fd), the coordinator added the packs that piece left inside other
routines. The grain rule makes "`hpack` over a list one record" wherever
`hpack` is called, so `mlist_to_hlist`'s packs and `fin_align`'s §804
and §806 packs belong here. `vsplit`'s (§977) and `fire_up`'s (§1017)
go with the page family. Uncommitted, on branch `ssa-fonts` from
4f4b7fd.

**Math's packs.** The engine's `mlist_to_hlist` packed with a private
set of parameters, `quiet()`: `\hbadness` infinite, `\hfuzz` at its
maximum, no overfull rule, TeXXeT off.
- Its `Env` now has `hpack(list, spec)` and `vpack(list)`, and the core
  answers them with the recorded calls. `MathEnv` holds the engine
  mutably, as `BreakEnv` does for line breaking.
- A pack's confusion is kept as the `Jump` the core raised, so it is
  printed once, where it happened, as before.
- `quiet()` is gone. The calls read the real parameters, and that
  changes no output:
  - math packs a list only to its natural width, or, in `rebox` (§715),
    to an exact width with `\hss` glue on both sides;
  - a natural pack returns before any report (§658: `x = 0`);
  - an exact pack with infinite glue sets its order to `fil`, and
    §658–§667 report only at order `normal`;
  - TeXXeT's LR check needs math nodes in the packed list, and math's
    lists have none, because e-TeX appends an LR math node only in
    horizontal mode (etex.ch, "Cases of `main_control` for
    `hmode+valign`").
- TeX's `hpack` and `vpack` begin with `last_badness:=0` (§649, §668),
  but the quiet packs never set it. `\badness` after a formula was
  therefore wrong. After `\setbox0\hbox to 100pt{a}$x^2$`, pdfTeX and
  TeX print 0 and a6ffdb0 printed 10000. The calls set it, so it is now
  0. `tests/e2e/math.tex` gains three `\message{[\the\badness]}` lines,
  and the base binary fails them.

**`fin_align`'s packs.** The engine's `align::fin_align` did §801–§806
in one pure function, packing the preamble and the shifted rules itself.
It is now three functions, with the packs between them the caller's:
- `align::preamble` does §801–§803: the widths, and the tabskip glue
  that §802 zeroes after an unused column. It returns the preamble list
  for §804.
- The core packs that list with `pack_begin_line := -mode_line`, as §804
  does. `\halign` uses an `hpack` call with `Packer::NoRule`: TeX stores
  0 in `\overfullrule` around this one call, and the call does not read
  the parameter. `\valign` uses a `vpack` call.
  `align::prototype_widths` then turns a `\valign` prototype's heights
  back into widths.
- `align::set_rows` does §805. §806's rules in a shifted display are
  packed through a closure the core passes, `hpack(rule, natural)`.
- The prototype's report is now printed before §805, where it was
  printed after. §805 prints nothing, so the log is unchanged.
- The special case that set `last_badness` to 0 after the rules
  ("§806 packs the rules") is gone, since the rule packs set it.
- `tests/e2e/align.tex` gains a hanging-indented display with `\halign
  to 200pt` and an `\hrule`, then `\message{[\the\badness]}`.

**The `align` row.** Seven fields, `Row::Align(f)` (`track::align`), in
the lists family. Each is versioned by the value itself:

| field | tex.web | value | version |
|---|---|---|---|
| `COLUMN` | `cur_align` (§770) | an index | its value |
| `SPAN` | `cur_span` | an index | its value |
| `LOOP` | `cur_loop` | an index | its value |
| `ADJUST` | the row's `cur_head` list (§796) | a `NodeList` | the list's, O(1) |
| `COLUMNS` | the preamble's alignrecords (§769) | a `PVec<AlignRecord>` | the vector's, O(1) |
| `TABSKIPS` | the tabskip glue (§778) | an append-only list | a running polynomial |
| `STACK` | the alignment stack (§772) | levels with the stack's version up to each | O(1) at a push or pop |

- An `AlignRecord` carries a version made when it is made or changed
  (`remade`): its templates by the versions they carry, its
  `extra_info`, its width and its spans.
- Every access goes through an accessor (`cur_align`, `column`,
  `edit_column`, `edit_cur_column`, `push_column`, `tabskip`,
  `push_tabskip`, `row_adjust_mut`). A read notes the field's version,
  and a change reads the field and writes it, as the nest's accessors
  do.
- `push_alignment` and `pop_alignment` (§772) read and write every
  field.
- §789 in `get_next` (inserting `v_j`) reads `cur_align` and edits the
  current column's `extra_info`, so a tokenizer or `macro_call` call
  inside an alignment reads exactly those two.
- The ENGINE files gained the plumbing: `Row::Align` and `track::align`
  in `track.rs`; in `ssa.rs`, the slots after the nest's in `Fam::List`
  (13 to 19), the view's arm and `EngineView::align_ver`. The value
  stamps grew from 16 slots to 32 so that the new slots are noted once
  per call.

Rejected:
- *One slot for the state.* Each cell's width update (§797) would
  change what §789 reads, so every token-level call in an alignment
  would depend on every column's width.
- *Versions made at the read*, as the nest's `nest_version` is. That
  would hash every column at the first read in each call, and 7.17.12's
  second convention makes the version at the write.
- *A `PVec` for the tabskips.* The list is only appended to while it
  lives: §802 edits a copy after the level is taken. A running
  polynomial is the same rule without a `Value` wrapper for glue.

**`last_badness` and `shown_mode`.** Both became scalar slots in
42b576a. `shown_mode` had accessors already. `last_badness` was written
and read with inline `scalar_wrote`/`scalar_read` calls. It now has
`last_badness`/`set_last_badness` in the `scalar_rows!` table, used by
the two reports, `fire_up`'s and `\badness`. 7.17.12 marks all three
rows converted.

**Measured** with the release binary, plain TeX, a second pass after a
plain pass, `PARTEX_SSA=1 PARTEX_SSA_CHECK=1`, and the no-edit rebuild.
Each cell is calls / probed hits / records. The `math` and `align`
rows are the e2e documents with the lines above.

| document | `unsave` | `hpack` | `vpack` | `line_break` |
|---|---|---|---|---|
| pages | 220 / 220 / 23 | 1052 / 1052 / 80 | 120 / 120 / 47 | 68 / 68 / 29 |
| texxet | 55 / 55 / 28 | 42 / 40 / 41 | 10 / 10 / 8 | 4 / 4 / 4 |
| cutoff | 320 / 320 / 8 | 400 / 400 / 88 | 160 / 160 / 82 | 80 / 80 / 80 |
| readback | 280 / 280 / 7 | 120 / 120 / 81 | 160 / 160 / 82 | 40 / 40 / 40 |
| edits-dvi | 598 / 598 / 9 | 251 / 251 / 247 | 20 / 20 / 12 | 240 / 240 / 240 |
| incr | 248 / 248 / 24 | 511 / 501 / 134 | 68 / 66 / 30 | 86 / 86 / 38 |
| hyph | 8 / 8 / 8 | 20 / 19 / 20 | 5 / 5 / 5 | 6 / 5 / 6 |
| align | 153 / 153 / 51 | 137 / 132 / 115 | 15 / 15 / 15 | 4 / 4 / 4 |
| math | 311 / 311 / 90 | 270 / 266 / 192 | 39 / 39 / 35 | 2 / 1 / 2 |
| microtype (pdfTeX, INITEX) | 27 / 27 / 20 | 141 / 122 / 99 | 4 / 4 / 4 | 25 / 14 / 25 |

- Three documents have new counts. `incr` is 390 → 511 hpacks and 44 →
  68 vpacks, because its math packs are now calls. `math` and `align`
  were not measured before.
- On every document, cold and rebuilt, the output (DVI, or microtype's
  PDF) and the log are identical to a plain run, with 0 lost writes,
  0 stale reads and no "write differs".
- `incr` is compared at pass 3, whose rebuild is its pass 3.
- Run later, with the cold build at pass 3 and the rebuild at pass 4,
  `incr` reports one "write differs" on `read:1`, in the paragraph of
  `\newwrite\toc`. The `\openin` streams are on build-tagged revisions
  (7.17.12's `read_file` row, the output family), so a revision never
  equals across builds. a6ffdb0 reports the same difference, so it is
  not this change's, and it is not this lane's.
- The gate: `scripts/sandbox cargo xtask check` exit 0, e2e 34/34 in
  both modes, with the new `math` and `align` lines identical to the
  oracle.

Cost, as wall time, the minimum of 9 runs, cold, a6ffdb0 against this
tree:

| document | plain | SSA |
|---|---|---|
| math | 57 → 58 ms | 238 → 239 ms |
| align | 61 → 63 ms | 244 → 238 ms |

Both differences are within the noise.

### §7.17.10: the page builder's state by field, and the page's packs as calls (Agent FONTS, branch ssa-fonts, 18:52–19:23)

**What prompted it.** The third piece of this agent's lane is the page
family of 7.17.12: the page builder's state (`Builder`, §980) "per
field so `\pagegoal` reads one field's version", with `dead_cycles`,
`output_active` and `split_discards`. When phase 2 was committed
(d85d880), the coordinator added `vsplit`'s pack (§977) and
`fire_up`'s (§1017, §1021) as calls, the packs phase 2 had left inside
the page builder. The marks (`cur_mark`) are ENGINE's, done first as
marks by class, and are not touched here. Uncommitted, on branch
`ssa-fonts` from d85d880.

**The rows.** `dead_cycles` and `output_active` were already scalar
slots (42b576a), so 7.17.12 only marks them converted. The builder and
`split_disc` are 22 slots, `Row::Page(f)` (`track::page`), in a family
of their own, `Fam::Page`:

| slot | tex.web | value | version |
|---|---|---|---|
| `CONTENTS` | `page_contents` (§980) | empty, inserts only, box there | its value |
| `LIST` | the page so far, `page_head`…`page_tail` | a `NodeList` | the list's, O(1) |
| `SO_FAR` + k | `page_so_far[k]` (§982), eight | a dimension | its value |
| `MAX_DEPTH` | `page_max_depth` | a dimension | its value |
| `LEAST_COST` | `least_page_cost` (§974) | an integer | its value |
| `BEST_BREAK` | `best_page_break` | a position in the list | its value |
| `BEST_SIZE` | `best_size` | a dimension | its value |
| `INS` | the page insertion records (§981) | a few records per page | hashed at the read |
| `INSERT_PENALTIES` | `insert_penalties` (§982) | an integer | its value |
| `LAST_GLUE` … `LAST_NODE_TYPE` | `last_glue`, `last_penalty`, `last_kern`, e-TeX's `last_node_type` (§982) | four fields | their values |
| `DISCARDS` | e-TeX's `page_disc` | a `NodeList` | the list's, O(1) |
| `SPLIT_DISCARDS` | e-TeX's `split_disc` (§977) | a `NodeList` | the list's, O(1) |

- The family is its own because the lists family is full. Its slots are
  the nest's and the alignment's (0 to 19), and the value stamps that
  note a slot once per call are 32 wide. `Fam::Page` has its own
  stamps.
- Each version is made at the read, as the nest's is. Every field is a
  scalar, a list that carries its version, or a few small records, so
  a read costs O(1) or a hash of those records. The alignment's columns
  needed versions made at the write (phase 2); the page's fields do
  not.
- `page()` and `page_mut()` are gone. Every access goes through an
  accessor that names its field, and each access still tells a machine
  (`page_access`, `MCell::Page`). The readers and writers outside the
  builder each touch one field:
  - `\pagegoal` and the other five `\page…` dimensions (§421) read
    `page_contents` and `SO_FAR+k`, and assigning one (§1245) writes
    `SO_FAR+k`;
  - `\insertpenalties` reads (§421) and writes (§1246) its own;
  - `\lastskip`, `\lastpenalty`, `\lastkern` and `\lastnodetype` in
    vertical mode with nothing contributed (§424) read `LAST_*`;
  - `\unskip` in vertical mode (§1105) reads `LAST_GLUE`;
  - `\pagediscards` and `\splitdiscards` take `DISCARDS` and
    `SPLIT_DISCARDS`;
  - §1026 (the end of an output routine) writes `INSERT_PENALTIES`,
    `LIST` and `DISCARDS`;
  - `its_all_over` (§1054) reads `LIST`;
  - `\showlists` (§986) reads the fields it prints.
- `build_page` (§994–§1008) and `fire_up` (§1012–§1022) run the engine's
  `Builder` methods on the whole state. The core takes the builder out
  through `page_all_mut`, which notes every field read and written.
  Phase 4 makes each step of `build_page` a call. That call's name will
  be the builder's fields (`page_version`, already written) and its
  results the fields it changes. The whole-builder read here is what
  that name will cover.

**The packs.**
- `vsplit` (§977) now packs in the core, in TeX's order, as calls:
  - first the rest into box n, `vpack(q, natural)`;
  - then the split, `vpackage(p, h, exactly, split_max_depth)`.

  The engine's `page::vsplit` returns the two lists. Before, it packed
  the rest itself and handed the split to `report_vpack`.
- §1021's and §1022's insertion boxes, `vpack(…, natural)`, are calls
  through a closure the core passes to the engine's `Builder::fire_up`.
  If a pack raises a jump, the jump is kept and returned after the
  builder is put back.
- §1017's box 255 is packed in the core. TeX stores `inf_bad` in
  `\vbadness` and `max_dimen` in `\vfuzz` around this `vpackage`. The
  call therefore reads neither parameter, and its name says it is quiet
  (`Tex::vpack_quiet`, a flag in `vpack_with`'s name), as
  `Packer::NoRule` does for §804's `hpack`.
- The pack sets `last_badness` itself (§668). `Fired::badness`, and
  the `set_last_badness` that copied it, are gone, and so is the
  engine's private `quiet()` parameter set.
- A natural pack never reports, so §1021's packs now use the ordinary
  parameters without a change in output.
- The engine's `release_held` is gone. §1023 puts the held-over
  insertions back in front of the contributions itself, through
  `page_list_mut`.

Rejected:
- *One slot for the builder.* `\pagegoal`, `\pagetotal`, `\lastskip`
  and `\insertpenalties` would then depend on the whole page. A macro
  that reads `\pagetotal` would miss whenever any paragraph was
  contributed, even one that left the total alone. 7.17.12 asks for
  fields.
- *Noting fields inside the engine's `Builder`*, by passing a tracker
  into `partex-engine`. The engine's routines are values in and values
  out, and the engine knows nothing of tracking. The step calls of
  phase 4 name what a step reads, so the fields read inside a step are
  covered by its name without the engine knowing.
- *`split_discards` in another family* (as a list row). 7.17.12 puts it
  with the page ("→ `Page` (field)"), and it is e-TeX's page material.

**Measured** with the release binary, plain TeX, a second pass after a
plain pass, `PARTEX_SSA=1 PARTEX_SSA_CHECK=1`, and the no-edit rebuild.
Each cell is calls / probed hits / records. `incr` is compared at pass
3 (its rebuild), as before. Microtype is pdfTeX INITEX.

| document | `unsave` | `hpack` | `vpack` | `line_break` |
|---|---|---|---|---|
| pages | 220 / 220 / 23 | 1052 / 1052 / 80 | 177 / 175 / 96 | 68 / 68 / 29 |
| texxet | 55 / 55 / 28 | 42 / 40 / 41 | 12 / 12 / 10 | 4 / 4 / 4 |
| cutoff | 320 / 320 / 8 | 400 / 400 / 88 | 200 / 200 / 122 | 80 / 80 / 80 |
| readback | 280 / 280 / 7 | 120 / 120 / 81 | 200 / 200 / 122 | 40 / 40 / 40 |
| edits-dvi | 598 / 598 / 9 | 251 / 251 / 247 | 25 / 25 / 17 | 240 / 240 / 240 |
| incr | 248 / 248 / 24 | 511 / 501 / 134 | 81 / 78 / 40 | 86 / 86 / 38 |
| hyph | 8 / 8 / 8 | 20 / 19 / 20 | 6 / 6 / 6 | 6 / 5 / 6 |
| align | 153 / 153 / 51 | 137 / 132 / 115 | 16 / 16 / 16 | 4 / 4 / 4 |
| math | 311 / 311 / 90 | 270 / 266 / 192 | 40 / 40 / 36 | 2 / 1 / 2 |
| microtype (pdfTeX, INITEX) | 27 / 27 / 20 | 141 / 122 / 99 | 9 / 9 / 9 | 25 / 14 / 25 |

- Only the `vpack` column changed from phase 2. Every document gains a
  call per page shipped (box 255) and per insertion box. `pages` also
  gains two per `\vsplit`: 120 → 177.
- Two of `pages`' `vpack` calls do not hit on the no-edit rebuild. Both
  are `\vsplit`s that report "Underfull \vbox (badness 10000) detected
  at line 21" (and 23). A report reads `line`, which is not a value yet
  (7.17.12's `line` row, the input family). The probe fails on that
  read, and the call ends in the same record. This is the same class as
  phase 1's texxet `hpack` misses.
- "page" is no longer among the check's "rows not values yet".
- On every document, cold and rebuilt:
  - the output (DVI, or microtype's PDF) and the log are identical to a
    plain run (microtype's log past its banner line);
  - 0 lost writes, 0 stale reads and no "write differs".
- The gate: `scripts/sandbox cargo xtask check` exit 0, e2e 34/34 in
  both modes (machine mode's `machine_edits` runs its sanitizer).

Cost, as wall time: the minimum of 9 to 15 runs, cold, the second pass.
It compares a6ffdb0 (before phase 1) with this tree, alternating the
two binaries:

| document | plain | SSA |
|---|---|---|
| pages | 142 → 142 ms | 346 → 346 ms |
| cutoff | 86 → 87 ms | 265–292 → 270–300 ms (five alternations) |

`cutoff` ships 40 pages, each now with a box 255 call and the
builder's 21 fields noted per `build_page`. It is at most about 5 ms
slower, inside the spread between runs of one binary.

### §7.17.12, the output family: the streams, the printing state and the small tables (Agent WRITERS, branch ssa-writers, 18:16–19:30)

**What prompted it.** This is the coordinator's second piece after
2afde54. The first two items are the streams in their 7.17.12 forms
(`write_file`/`write_open` → `Out(n)`, `read_file`/`read_open` →
`Read(n)`) and the printing state with the small tables. After
2afde54, check mode still listed `log_file.id`, `write_file`,
`read_file` and `random` among its rows that were not values yet.
Their families were still on build-tagged revisions. The FONTS agent
also found a symptom: when incr's cold SSA build is its third pass,
check mode reported "write differs read:1" in the paragraph of
`\newwrite\toc`, because the `\openin` stream's revision changed with
the build.

**What changed.**

- `streams.rs` (new) holds the rows.
  - `Row::Out(n)`: the file `\write` stream `n` stores to, the name
    `\openout` gave (§1374), or none. `Out(LOG)` (n = 16) is whether
    the log is open (§534).
  - `Row::Read(n)`: `\openin` stream `n`, versioned by its contents'
    version, made at the load (§1275), together with its position
    (`pos` and the line counters, §483). The version is made again at
    each line read and at the close.
  - `Row::Random`: pdfTeX's generator (pdfTeX §110), versioned by its
    state.
  - `Row::Clock`: see below.
  - In `ssa.rs` they are the table families 8, 9 and 10, so their
    content versions sit in `Versions`' arrays with a stamp each, and
    `View` answers `known`. A format's load and the engine as made
    version them wholesale (`version_streams`, called from
    `version_writers`).
- *Outputs are effects, attributed to the call that made them.*
  - `Tracker::output(kind, bytes)` is called by every output:
    - the terminal and the log (`wterm`, `wlog`, `term_bytes`);
    - each `\write` stream (`write<n>`, `stream_byte`);
    - the DVI bytes and pages (`dvi`, `dvi start`, `dvi finish`, the
      page sink's pages);
    - the PDF writer (`pdf`).
  - The recorder keeps one pending run per kind. It notes the runs as
    effects of the innermost call at every call boundary: a call's
    begin and end (`call_begin`/`call_end`), the tokenizer's probe and
    end, and the paragraph's begin and end.
  - This replaces 2afde54's `Emitted` buffer, with its
    `take_writer_effects` and `note_writer_effects`. That buffer
    gathered the bytes and noted them only when the paragraph closed,
    so bytes a child call made were counted as the paragraph's.
  - The first version kept one run per *change* of kind. On readback
    that made 527 runs each for the terminal and the log, because they
    are written byte by byte in turn. The outputs are separate files,
    so only each one's own order matters: one run per kind.
- *A stream's lines are stores* (7.17.5).
  - `Tracker::store_open(stream, name)` makes the name's `Load`
    address the stream's and notes the open (`note_open`).
  - `Tracker::store_line(stream, line)` notes a store of each line
    ended (`note_store`), in program order with the effects before it.
  - A load of a name the build writes still reads a revision that no
    probe finds equal, as before.
  - Rejected: letting a writer read the lines to append to them. An
    early line that changed would then make every later writer miss.
    Also rejected: addressing a stream by its number. The φ is by
    name, since `\input` names a file, not a stream.
- *`\read`* (§485, §486): `read_line`'s result is kept, so the
  stream's write is noted on every path. The stream still closes only
  when the read reports the end, as the original `?` did: an error
  goes up without closing. `norm_rand`'s loop moved to `norm_draw`, so
  the generator's write is noted after the draw.
- *The host's answers that are not loads are `Clock` reads.*
  - They are the clock for `\pdfelapsedtime`, `\pdfcreationdate`,
    `\pdffilemoddate`, the info dictionary's dates and the trailer's
    ID (pdfTeX's `/ID` from the creation date), and the terminal's
    line in `term_input` (§71).
  - Each is `value_read(Row::Clock, answer)`. `View` answers the slot
    with a revision, so a probe never finds it equal, and a call that
    asked runs again in every build.
  - Rejected: letting `View` ask the host again at a probe. That is a
    second question, and its answer (the clock) need not be the
    first's.
  - The rows' own form holds for the job's start and the timer
    (DESIGN's `sys_*` and `epoch`: "read from the host's clock →
    `Clock`"). The log's banner (§536) and `\dump`'s (§1328) make a
    `Clock` read of the date where they print it. The timer's start is
    a `Clock` read where it is asked: at the job's start and at
    `\pdfresettimer` (pdfTeX §1582). A first draft kept the date in
    the `sys_*` scalar slots and marked the row *Corrected*. The
    coordinator sent it back: the code as it stands never changes a
    design decision.
- *The printing state* was already a set of scalar slots (42b576a).
  This piece routes the accesses that bypassed the accessors through
  them:
  - `\dump`'s banner (§1328): `selector`, `job_name` and
    `interaction` (its date is the `Clock` read above);
  - `print_nl`'s column test (§62), which read `term_offset` and
    `file_offset` directly;
  - the job's start, which now writes `Random` after pdfTeX's default
    seed.
- *Check mode compares effects too*: a hit's and its body's `Out`,
  `Open` and `Store` items, their children's included, in order, by
  version (`ssa::outputs`).
- `statehash::value_rows` drops `log_file.id`, `write_file`,
  `read_file` and `random`. `save_state` persists `Streams` (the
  names and the contents' versions).
- Call sites in files other owners hold, changed minimally:
  `track.rs`, `ssa.rs`, `tex.rs`, `lib.rs`, `save_state.rs`,
  `statehash.rs`, `files.rs`, `run.rs`, `prefixed.rs`, `toklists.rs`,
  `conds.rs`, `random.rs`, `expr.rs`, `maincontrol.rs`, `format.rs`
  and `input.rs`. The machine's cells (`Cell::Out`, `Cell::Read`,
  `Cell::Random`) are read and written where they were.

**Open item: one capture point for the outputs.** `Tracker::output`
is a new hook in the tracker, which is ENGINE's. ENGINE's ab6e7d4
(hits applied, not in this tree) captures the printed bytes at the
print sinks (`effect_bytes`, the log and the terminal) and replays
them. The two overlap because the print code went to this piece and
"effects hold their bytes" went to ENGINE. The coordinator's ruling:
this piece keeps its capture for now. At the merge, ENGINE, which
owns the recorder, makes them one capture point. The behaviour to
keep is this piece's:

- one run per kind, flushed as the innermost call's effects at every
  call boundary;
- typed effects widened to every output (log, terminal, `\write` n,
  DVI, PDF), so an applied hit can put each back through its sink;
- check mode's walk comparing `Open` and `Store` items as well.

**Measured.**

- *SSA e2e, check mode* (release, sandbox, `SOURCE_DATE_EPOCH` fixed,
  11 documents, a cold build then a rebuild with no edit, from the
  inputs alone):
  - outputs byte-identical to the plain builds;
  - no write or effect differs;
  - no stale table read, no lost write;
  - the rows not values yet are down from 16 to 12, all other
    families' (`line`; `buffer`; `split_discards`; the input stack;
    `in_open`…; `grp_stack`…; `pseudo_files`; the file-name stacks;
    `param_stack`; the fonts; hyphenation exceptions; `page`);
  - the uncovered parts at the hits are only those rows.
- *The same as passes 3 and 4* (two plain passes first, so the job
  reads back what it wrote): the same result. incr checks 274 hits in
  its cold build and 605 in the rebuild, and none differs, so the
  FONTS agent's "write differs read:1" is gone. math's pass-4 log
  differed once, in the banner's minute: the plain run and the SSA run
  fell on either side of 18:55. A rerun was identical.
- *`PARTEX_DVI_THREAD=0`*: the seven DVI documents were identical and
  silent.
- *SSA mode on the course* (no check; a cold build, then the `space`
  edit in ch15; base 575990a, then this tree, one after the other):
  started at 18:57, with a limit of 1800 s per binary and a load
  average of 7 from other agents' builds. The base's cold build took
  1,544 s:

  | calls | hits | misses | reads verified | records |
  |---|---|---|---|---|
  | 3,203,512 | 1,615,805 | 1,587,707 | 38,251,232,400 | 1,588,792 |

  The 1800 s limit stopped the base in its rebuild. That cost is the
  probes' verification, 38 billion reads, and not the writers'. This
  tree's run began at 19:27, and its numbers go in the next entry. Plain runs pay nothing new: every addition sits behind
  `T::VALUES`, a constant that is false for the plain tracker.

**Gate.** `scripts/sandbox cargo xtask check` (18:45–18:55): rc 0.
After the `Clock` fix (the date and the timer), it ran again
(19:20–19:30): rc 0, with the same counts, and the SSA e2e in check
mode was again identical and silent on all 11 documents.
It covers fmt, clippy with and without `partex-cli/trace`, the wasm
build and the tests, plus trip 7/7 and etrip 18/18 identical in both
modes and e2e 34/34 identical, plain and in machine mode. One earlier
run was stopped by hand to add the `files.rs` change, so it was not a
failure. Clippy, run before the gate, found a collapsible `if` in
`\read`, an unevenly grouped hex literal and an unsized
`clock_read` argument; all three were fixed.

### Machine mode: the footnote edit's sanitizer failure; eqtb's objects compared apart (Agent ENGINE, branch engine-fix, 19:11–20:30)

**The failure.** The coordinator found that the machine-mode course
sanitizer fails on the course's `footnote` edit:
`sanitizer: region 513's guard fails on replay: Eqtb(42422) was …, is …`
(`Build::check_replay`, `partex-incr/src/build.rs`). He bisected it,
each commit with a format made by its own binary: 5ae2807 passes
(1,977,503 commands re-run), and 42b576a and every commit after it fail
with exactly the same two versions. So 42b576a introduced it (the
objects became the entries' values).

**Finding it.** For one run I added a diagnostic at the panic (since
removed). It listed every region that guards or writes the cell, with
the values written. Eqtb(42422) is `\@gtempa`, a list of one entry per
counter with its value. Region 485 (recorded by the rebuild) writes
exactly the version region 513 guards, and no region in between writes
the cell. Yet the replayed state at 513's entry held a list that
differed from 485's write in one counter's digit, `0` where 485 has
`1`: the cold build's value. So a state change set the cell without a
write, and that change is `Rest`'s. Setting `Rest` (`restore_rest`)
takes a snapshot's whole engine and keeps this state's eqtb cells
where the two differ, as `Tex::eqtb_differences` finds them
(`adapter.rs`). That function compared the words of the chunks the two
states do not share. It compared the object a word names only when the
word's `equiv` was positive, which before 42b576a meant that the
`equiv` was the id of its list, glue, box or shape.

**Cause.** Since 42b576a an entry's object sits beside its word
(`eqtb_obj`). The word no longer names it: its `equiv` is null, or a
macro's `\protected` flag (`macro_flag`). A `\gdef\@gtempa{…}` with a new
body therefore writes back a word equal to the old one. When the next
snapshot finds the chunk written back unchanged, `JVec` shares it
again, and an unprotected macro's `equiv` is 0 anyway. Both tests said
"the same", and the restore kept the snapshot's `\@gtempa`. The premise
of 7a9f120's rule ("chunks they share name the same objects: stored
objects never change") stopped holding when the objects left the
words. The rebuild's own guard checks passed, because a trace carries
the version the live engine had at the cut. Only a replay of the values
themselves, the sanitizer's, found the snapshot's list. A rebuild's
restores go through the same `restore_rest`, so the live rebuild could
continue from the wrong value too.

**Fix.** `eqtb_differences` compares the words by chunk as before (a
shared chunk has the same words). It also compares the objects apart,
every one: an object that is the very value in both states (the same
`Arc`, or equal glue) is the same without a look, and any other pair
goes to the content test that already filtered the words
(`Tex::cell_content`, the cell's version). `JVec::differences_at`, used
only by the old test, is gone. Rejected alternatives:
- keep the word test and give objects a nonzero `equiv` again: the
  `equiv` would become an id, which 7.17.12's form removed;
- compare objects only where the words are object kinds: that reads
  every word of the shared chunks as well, for no gain in what it
  finds.
The scan reads both states' `eqtb_obj`, one `Option<Obj>` per eqtb word
(`hash_extra` = 600000 on the course).

**Measured.** First the footnote edit alone, on a course copy whose
format ab6e7d4's binary made (the fix does not touch the format):
the sanitizer passes, with 1,977,503 commands re-run, 18 of 569
regions dirty and 114 cuts, as at 5ae2807. Then the ten edits of
`bench/edits/course.txt` with `PARTEX_MACHINE_SANITIZE=1 bench/edits.sh`,
on ~/code/tmp/course-ef1 with a format made by the fixed binary, one
process per edit. The rebuild times include the sanitizer's fresh
build and replays, and the first three edits ran beside `xtask check`,
so they are not timings of a rebuild. The commands re-run are the
measure:

| edit | commands re-run | 5ae2807 | dirty / total | cuts | restores (n, ms) |
|---|---:|---:|---:|---:|---:|
| space | 32,870 | 32,870 | 1 / 569 | 9 | 1, 69.8 |
| word | 298,713 | 298,713 | 3 / 569 | 22 | 4, 232.7 |
| label | 40,056 | 40,056 | 2 / 569 | 16 | 2, 117.1 |
| footnote | 1,977,503 | 1,977,503 | 18 / 569 | 114 | 8, 685.6 |
| enter | 228,283 | 228,283 | 8 / 569 | 50 | 3, 177.3 |
| word-local | 32,870 | 32,870 | 1 / 569 | 9 | 1, 81.2 |
| delete-par | 1,223,359 | 1,223,359 | 12 / 569 | 67 | 3, 187.2 |
| word2 | 10,015 | 10,015 | 4 / 588 | 4 | 5, 265.6 |

All eight passed the sanitizer: every guard held on replay, and each
rebuild's final state and output were those of a fresh build. The run
was stopped at 20:27, during `space2`, with `space2` and `enter2` not
yet run. The coordinator stopped it because at 20:15 the user decided
that machine mode (§7.16's machine, with its checkpoints, restores and
guards) is to be removed, so the run no longer decides anything.

Gate (`scripts/sandbox cargo xtask check`, exit 0): fmt, clippy with
and without `trace`, the workspace tests, trip 7/7 and etrip 18/18 in
both modes, and e2e 34/34 in both modes.

### §7.17.10: the page builder's steps and the output routine as calls (Agent FONTS, branch ssa-fonts, 19:25–20:10; sent back and redone 20:15–20:35)

**What prompted it.** The last piece of this agent's lane. Each step
of `build_page` (§994) becomes a recorded call with the page so far as
its argument, and the output routine (§1025) becomes a call too.
DESIGN 7.17.4 gives the goal: after an edit that keeps a paragraph's
heights, "every `build_page` step that read them hits, the page's
`ship_out` misses". The coordinator approved the plan after phase 3
(9a0e479) with one constraint. ab6e7d4 (ENGINE's marks by class and
applied hits) has not reached this branch, so two things wait for it:
the marks code in `fire_up`, and the `SValue`/`Store::set` arms for
this lane's families (the page, the alignment's slots, the pack
results). Uncommitted, on branch `ssa-fonts` from 9a0e479.

**The engine.** `Builder::build` becomes `Builder::step`, one node:
§996–§1008 for the first contribution `p`.
- Its `Step` result says what became of `p`:
  - `Page(p)`: it goes on the page (§998);
  - `Discard(p)`: it is discarded, and given back when
    `\savingvdiscards>0` (§999);
  - `TopSkip(glue, p)`: it goes back behind the `\topskip` glue
    (§1001);
  - `Kern(p)`: a kern that ends the contributions stays (§1000);
  - `FireUp(p)`: the page is complete (§1005).
- The caller links `p` in. A step never changes the page's list or its
  discards; it reads the list only by its length and
  `tail_precedes_break`.
- `After` is what follows a kern (§1000), the one thing a step reads
  past its node.
- The `Params` struct is gone. The `Registers` trait became `Env`,
  which also reads the parameters, each where TeX reads it:
  - `\vsize`, `\maxdepth` and `\tracingpages` at a freeze (§987);
  - `\topskip` at the first box (§1001);
  - `\savingvdiscards` at a discard (§999);
  - `\tracingpages` at a breakpoint (§1005) and a split (§1011).

  Before, `build_page` read all five before every batch of
  contributions.
- `build` remains, as the steps over a list, for the engine's test.

**The core's `build_page`.**
- It takes the contributions into a deque while the steps run. For
  each node:
  - `page_step(p, after)` runs;
  - then `build_page` links the node into the page list or the
    discards;
  - then it puts the step's nodes back in front.
- On a fire it puts the contributions back first and then runs the
  output routine's call.
- Holding the contributions outside the list is safe because nothing a
  step does reads them:
  - no command runs inside `build_page`;
  - a machine stops only at main control's top (`Jump::Checkpoint`);
  - the default output routine's `ship_out` runs after the
    contributions are back, inside the output routine's call. It can
    see them: a `\write` it expands can read `\lastpenalty` through
    `tail`, §424.

**What a step reads, and what it writes** (`builder::Access`,
`step_name`, `page_put`).
- The engine's step reports what it read and what it assigned, field
  by field, as TeX's step does them:
  - a field is *read* if the step used its value before assigning it;
  - a field is *assigned* if the step stored into it, whether or not
    the value changed.

  `builder::field` numbers the fields, and `track::page` takes its
  numbers from there. `LIST_LEN` (the index the node takes, §1005,
  §1008) and `LIST_TAIL` (whether the last node precedes a break,
  §1000) are two new slots, views of the list written whenever the list
  is (`page_changed(LIST)`). `split_disc` moves to slot 23.
- `page_put` notes the reads, at their versions before the step, and
  then one write per assigned field. No field is written because its
  version changed, and none is read because the step might assign it.
- Reads that depend on a condition come in only on the paths that make
  them. §1007's badness reads the stretch of the orders past `normal`,
  or the shrink, depending on whether the page is short. §1003 reads the
  total only when the depth is over its limit. §1005 reads the index
  its node takes, the goal for `best_size`, and the insertion records
  only for a new best break.
- The name is the fields the step reads on every path from its start,
  then `p`'s version, then `after`. The path is known from those,
  `page_contents` and `LIST_TAIL` (`step_name`):

| node | page | fields in the name |
|---|---|---|
| box, rule | box there | contents, total, depth, `MAX_DEPTH` (§1002, §1003) |
| box, rule; glue, kern, penalty | empty or inserts only | contents (§1001, §999) |
| whatsit, mark | any | depth, `MAX_DEPTH` (§1003) |
| glue | box there | contents, `LIST_TAIL`, its stretch's order, shrink, total, depth, `MAX_DEPTH`, and if the last node precedes a break, goal, `INSERT_PENALTIES`, `LEAST_COST` |
| kern | box there | contents; followed by glue: goal, total, `INSERT_PENALTIES`, `LEAST_COST`, depth, `MAX_DEPTH`; by anything else: total, depth, `MAX_DEPTH` |
| penalty | box there | contents, depth, `MAX_DEPTH`, and below 10000, goal, total, `INSERT_PENALTIES`, `LEAST_COST` |
| insertion | empty | contents, `INS` (the freeze assigns the depth and its limit before §1003 reads them) |
| insertion | inserts only, box there | contents, `INS`, depth, `MAX_DEPTH` |

- A debug assertion checks that a step that does not fire read every
  field of its name. A fire reads the whole page, so it covers the
  rest. A debug build runs the ten plain documents without tripping it.

The form was built three times.
1. *First:* reads by node kind, and writes only the fields whose
   versions changed. Check mode on `cutoff` and `readback` reported
   "write differs page_step: page:13", `best_size`. In the rebuild, a
   step assigned `best_size` the value it already had, so the body's
   writes lacked a field that the record had.
2. *Second:* the step also read every field it might assign
   (`BEST_BREAK` and `BEST_SIZE` for any breakpoint, and so on), so
   that its writes followed from its reads. Check mode went silent.
   This was the reported form, and the coordinator sent it back. It
   hides the rule that DESIGN 7.17.2 already has, "a write of an equal
   value is kept in the record although it leaves the state alone:
   replayed into a state where that slot differs, it must set it". It
   also costs hits for no reason: a breakpoint misses because an
   earlier break left a different `best_size`, which the breakpoint
   never reads.
3. *The form now:* writes are what the step assigns, and reads are
   what TeX's step reads, both reported by the engine where they
   happen. Check mode is silent on every document, cold and rebuilt.

**The fire and the output routine.**
- The step that fires does §1012–§1022 inside it. `fire_up` now takes
  the break node as the contributions (`front`) and puts the rest of
  the page in front of it. It returns the classes of the insertion
  boxes it filled. The fire reads and writes every field
  (`page_all_mut`).
- The output routine is the next call, `Func::Output`.
  - Its name is `\box255`, `\outputpenalty` and those insertion boxes,
    as 7.17.4 says. It reads `\output`, `\deadcycles` and
    `\maxdeadcycles` inside.
  - The user's routine runs from §1025 until §1026.
    `Tex::output_end` then ends the call, with the nodes that go in
    front of the contributions (`OUTPUT_RESULT`, the held-over list's
    version) as its result, and the caller puts them there.
  - The default routine (§1024's error, then §1023's ship-out) is the
    call's whole body.
- The calls nest.
  - The step has ended before the output routine's call begins.
  - While `output_active` holds, `clean_point` is `None`, so no
    paragraph call ends inside the routine.
  - A job that ends inside a routine, stopped by a fatal error, has its
    open calls ended before the last paragraph closes
    (`SsaTracker::end_open_calls`, `ssa::run`).
- In `ssa.rs`: `Func::PageStep` and `Func::Output`, the routine counts
  (9 of them), and `end_open_calls`. In `track.rs`: the two views and
  the result slots `PAGE_STEP_RESULT` (91) and `OUTPUT_RESULT` (92).

Rejected:
- *The whole page as the step's argument*, the form first proposed. A
  step then depends on the list's contents, so after an edit every
  later step on the page misses. Measured on the edit below: 322 of
  374 steps hit, against 371 with the fields a step reads.
- *The output routine's call opened inside `fire_up`*, where TeX
  begins it (§1025 inside §1012 inside §994). The step would end
  before the routine does, and the calls would not nest.
- *The fire as a call of its own.* The fire is where the step's node
  is a break, and 7.17.4 makes the routine, not the fire, the call over
  box 255. A fire in its own call would read the contributions to put
  the rest of the page back.
- *Reading the five parameters at each step*, as the old `Params` did.
  Every step would depend on `\vsize` and `\topskip`, which TeX reads
  once a page.

**Measured** with the release binary, plain TeX, a second pass after a
plain pass, `PARTEX_SSA=1 PARTEX_SSA_CHECK=1`, and the no-edit rebuild.
Each cell is calls / probed hits / records.

| document | `unsave` | `hpack` | `vpack` | `line_break` | `page_step` | `output` |
|---|---|---|---|---|---|---|
| pages | 220 / 220 / 23 | 1052 / 1052 / 80 | 177 / 175 / 96 | 68 / 68 / 29 | 889 / 889 / 527 | 18 / 18 / 18 |
| texxet | 55 / 55 / 28 | 42 / 40 / 41 | 12 / 12 / 10 | 4 / 4 / 4 | 51 / 51 / 48 | 2 / 2 / 2 |
| cutoff | 320 / 320 / 8 | 400 / 400 / 88 | 200 / 200 / 122 | 80 / 80 / 80 | 1081 / 1081 / 146 | 40 / 40 / 40 |
| readback | 280 / 280 / 7 | 120 / 120 / 81 | 200 / 200 / 122 | 40 / 40 / 40 | 321 / 321 / 129 | 40 / 40 / 40 |
| edits-dvi | 598 / 598 / 9 | 251 / 251 / 247 | 25 / 25 / 17 | 240 / 240 / 240 | 752 / 752 / 474 | 5 / 5 / 5 |
| incr (pass 3) | 248 / 248 / 24 | 511 / 501 / 134 | 81 / 78 / 40 | 86 / 86 / 38 | 995 / 968 / 871 | 8 / 0 / 16 |
| hyph | 8 / 8 / 8 | 20 / 19 / 20 | 6 / 6 / 6 | 6 / 5 / 6 | 55 / 55 / 55 | 1 / 1 / 1 |
| align | 153 / 153 / 51 | 137 / 132 / 115 | 16 / 16 / 16 | 4 / 4 / 4 | 74 / 74 / 73 | 1 / 1 / 1 |
| math | 311 / 311 / 90 | 270 / 266 / 192 | 40 / 40 / 36 | 2 / 1 / 2 | 53 / 53 / 50 | 1 / 1 / 1 |
| badness | 17 / 17 / 13 | 12 / 11 / 12 | 7 / 7 / 7 | 1 / 1 / 1 | 8 / 8 / 8 | 1 / 1 / 1 |
| microtype (pdfTeX, INITEX) | 27 / 27 / 20 | 141 / 122 / 99 | 9 / 9 / 9 | 25 / 14 / 25 | 269 / 269 / 264 | 5 / 5 / 5 |

- The four older columns are phase 3's. `badness` is the phase-2 test
  of `\badness` after math's packs (`target/ssa-e2e/badness.tex`).
- Against the second form, the steps' records fell on pages (609 →
  527), texxet, cutoff, readback, incr (939 → 871), align, math and
  microtype. A step no longer reads fields it only assigns, so states
  that differ only there share a record.
- Steps share records across pages. `cutoff`'s 1081 steps have 146
  records: the lines of its pages are alike, so the states repeat. The
  cold build already hits on them: 935 of cutoff's steps and 362 of
  pages' hit in build 0, and check mode compares those writes too.
- `incr` is a real change, not a no-edit case: its pass 3 reads the
  table of contents that pass 2 wrote.
  - 968 of its 995 steps hit.
  - Its 8 output routines miss on `out:0`. Each ships a page whose
    `\write\toc` reaches the output streams, which are not values yet
    (7.17.12's output family, WRITERS').
- On every document, cold and rebuilt:
  - the output (DVI, or microtype's PDF) and the log are identical to a
    plain run;
  - 0 lost writes, 0 stale reads and no "write differs".

An edit that keeps the heights. `target/ssa-e2e/edit.tex` is plain
TeX with `\hsize=4in \vsize=3in`, a first paragraph "First paragraph
xxxx." and two sentences, then 20 paragraphs of three sentences: 7
pages. The edit changes `xxxx` to `zzzz`. The rebuild runs 374 steps
and 371 hit (87 records). The three that miss are the edited
paragraph's changed lines and page 1's fire, which reads the list. Of
the 7 output routines, 6 hit: page 1's misses, because its box 255
changed. Check mode is silent, and the rebuild's DVI and log are
byte-identical to two plain passes over the edited file.

Cost. The machine was loaded (load average 9 to 11), so wall times
varied by up to 40%. The count is user-space instructions instead:
`perf stat -e instructions:u -r 5`, the mean of 5 runs, the second
pass, cold. It compares 9a0e479 (phase 3, built from `git archive` in
its own target directory) with this tree:

| document | plain | SSA |
|---|---|---|
| pages (889 steps, 18 routines) | 677.6 → 677.9 M | 1821.5 → 1837.8 M (+0.9%) |
| cutoff (1081 steps, 40 routines) | 307.8 → 308.2 M | 1389.1 → 1408.2 M (+1.4%) |
| edits-dvi (752 steps, 5 routines) | 323.3 → 323.5 M | 1998.8 → 2002.6 M (+0.2%) |

- A step costs about 18 thousand instructions in SSA mode: the name
  (the versions of every page slot, then those of its fields and the
  node's), the probe, and the reads and writes noted. The second form
  cost the same within 0.1%. `Access` replaces the diffing it did at
  the end, and `Access` costs no more.
- The engine's `Access` bookkeeping is two bit masks per step. In
  plain mode `page_step` is the body, and plain mode is unchanged
  (+0.05%).
- With recording off (`T::VALUES` false), `page_step` is the body, and
  plain mode is unchanged.

The gate: `scripts/sandbox cargo xtask check` exit 0, e2e 34/34 in both
modes (machine mode's `machine_edits` runs its sanitizer).

Left for when ab6e7d4 reaches this branch:
- the conflict in `fire_up` and `vsplit` with the marks by class (this
  change leaves the marks code as it was);
- the `SValue`/`Store::set` arms for this lane's slots, so that hits
  can be applied:
  - `Fam::Page`;
  - the alignment's slots in `Fam::List`;
  - the results `HPACK_RESULT`, `VPACK_RESULT`, `LINE_BREAK_RESULT`,
    `PAGE_STEP_RESULT` and `OUTPUT_RESULT`, each holding its value (a
    box, a step's nodes) for the caller to take after an applied hit.
- The user's output routine, applied, needs its end state made
  settable, as a paragraph's position is (`Position::settable`), when
  ab6e7d4 lands.

### §7.17.10: ship_out and write_out as recorded calls (Agent WRITERS, branch ssa-writers, 19:42–20:30)

**What prompted it.** This is the third piece of the output family
(the coordinator's list). `ship_out` (§638) and `write_out` (§1370)
become recorded calls with 4f4b7fd's machinery (`call_begin` and
`call_end` with the engine as the view, `Func`), on 351cc86. The box
is `ship_out`'s argument, its effects are the page's bytes, and its
writes are the writers' tables. The coordinator: "the scope becomes
the recorded call itself, and a child call inside it reads through the
same rule as any other." Applied hits (ab6e7d4) are not in this tree,
so a hit still runs its body, and check mode compares the two.

**What changed.**

- `Func::ShipOut` and `Func::WriteOut` join `Func::ALL`, so the SSA
  report counts their calls, hits and records per build.
- `ship_out` (dvi.rs) is a call named by the box.
  - The name uses the version the box carries once shared
    (`BoxNode::share`). A box built for the page and never shared is
    versioned from its parts.
  - The body is the old routine, unchanged.
  - Its scope is the call, through `Tex::scoped_call` (pdf/val.rs):
    `call_begin`, an outermost writer scope that declares nothing,
    the body, the scope's end, then `call_end`.
  - The scopes of its parts (`fix_pdfoutput`, `pdf_ship_out`, §640's
    `ship_box_out`) nest in the call's scope. Their reads are noted in
    the call's frame, and the fields they wrote are versioned when the
    call ends: the call's writes.
- `write_out` (dvi.rs) is a call named by the stream and the version
  the `\write` node's token list carries. A list with no version,
  because it was made before versions were on, is versioned from its
  tokens.
  - It runs from `out_what` in both places: inside `ship_out`
    (§1374) and at `\immediate` (§1375).
  - Its reads are `Out(n)`, the expansion's reads and the printing
    state. Its effects are the line's bytes, and its store is the
    line.
- *The child rule* (`Tex::writer_child_begin`).
  - A call begun inside an open writer scope first versions the
    fields the scope wrote so far (`writer_settle`, whose `row_wrote`
    lands in the parent's frame, before the child's `call_begin`).
    The child then starts with no scope open.
  - When the child ends, the parent's scope is reopened with the same
    fields marked, so its end versions them again as they are then.
  - Without this rule, a `write_out` inside `ship_out` would note a
    writer field at the version from before its parent's change: the
    old scope skipped the fields "the call itself wrote", and after
    the change that was no longer the same call.
  - Rejected: a hook in `SsaTracker::call_begin` to settle every open
    scope. The tracker cannot change the engine's fields, and the
    tracker is ENGINE's. Every call that can begin inside a writer
    scope is `write_out` (an expansion opens no other call), and
    `scoped_call` is the one way in.

**Measured.**

- *SSA e2e, check mode* (release, sandbox, 11 documents, a cold build
  then a rebuild with no edit, from the inputs alone): all outputs
  byte-identical, with no write or effect differing in `ship_out`,
  `write_out` or any other routine except the one below, and no
  stale read. The rebuild's calls:

  | document | `ship_out` hits | `write_out` hits |
  |---|---|---|
  | pages | 18/18 | — |
  | math, align | 1/1 | — |
  | cutoff | 40/40 | 9/9 |
  | edits-dvi | 5/5 | — |
  | readback | 0/40 | 3/3 |
  | cutoff-pdf | 59/60 | — |
  | effects | 29/30 | 1/1 |
  | incr | 0/8 | 19/29 |
  | texxet | 2/2 | 33/33 |
  | microtype | 3/5 | — |

  readback and incr read back what they wrote. Their second pass is a
  different document, so its pages miss, as they should. As passes 3
  and 4, from a fixed point, readback hits 39/40 and incr 8/8, with
  `write_out` at 3/3 and 29/29.
- *Why a PDF's first page misses* (a PARTEX_SSA_TRACE trace, then a
  temporary trace of the loads, since removed).
  - cutoff-pdf's first `ship_out` misses on `@load:2`, which is
    `pdftex.map`, read at the first page.
  - At each build's start, `ssa::run` versions every name in the
    loads table again with `host.read_file(name, FileKind::Tex)`. A
    map file, a `.vf`, a `.pfb` or an encoding is not found as a TeX
    file, so it becomes absent. Every call that loaded one then misses
    in the next build, though the file is the same.
  - The loads table would need each load's kind: `Tracker::load`
    given the `FileKind`, and the new versions read with it. The hook
    and the start of the run are ENGINE's, so this is reported, not
    changed. It costs hits only, never output.
- *The same as passes 3 and 4*: identical outputs, with the same one
  finding below.
- *`PARTEX_DVI_THREAD=0`*: the seven DVI documents were identical and
  silent, with the same hits.
- *Finding, not this piece's*. In microtype check mode, two `hpack`
  hits' effects differ from their bodies'. The report says "in
  paragraph at lines 42--42" in the record and "43--43" in the body:
  the same paragraph set again one line later hits the first one's
  record.
  - The record read `line:1+1` as `b1e6`, and the body in the rebuild
  read it as `ebb9`, but the probe passed. So the line read's address
  does not tell the two lines apart at the probe.
  - The 351cc86 binary, built from an export without this piece, shows
  the same two differences. The line family and the packs' report
  reads are FONTS'.
  - Effects have been compared since piece 2, so this is the first
  time the difference shows. With applied hits it would print the
  wrong line numbers.
- *The course* (piece 2's tree, SSA mode without check): this tree's
  cold build did not finish within 1800 s, at a load average of 11 to
  13. The base's took 1,544 s at 7. The two are not comparable at
  those loads, so there is no number for piece 2's cost on the course.

- *Cost* (SSA mode without check, release, sandbox, a cold build then
  a rebuild with no edit, alternating this tree with a build of
  351cc86, 11 rounds, medians; the machine was loaded by other
  agents' builds, load average 6 to 8, with single runs up to three
  times the median):

  | document | this tree cold | 351cc86 cold | this tree rebuild | 351cc86 rebuild |
  |---|---|---|---|---|
  | edits-dvi | 438.6 ms | 404.8 ms | 431.0 ms | 372.0 ms |
  | cutoff-pdf | 337.7 ms | 353.6 ms | 255.6 ms | 253.1 ms |
  | readback | 345.1 ms | 337.1 ms | 247.7 ms | 247.8 ms |

  A second series on edits-dvi gave 439.0 against 425.8 cold, and 430.3
  against 407.1 rebuilt. The PDF documents are equal within the noise.
  edits-dvi pays 13 to 60 ms for five `ship_out` calls.
  - It is the name. A build that computes the name and then runs the
    body with no call (15 rounds) gave 452.1 against 414.4 ms cold and
    439.9 against 371.4 ms rebuilt, the same cost as the call.
  - Plain's output routine ships a `\vbox` just packed, which carries
    no version. So do its lines, which are not shared. The name hashes
    the page from its parts, every character node included. `vpack`'s
    call has just made that same version for its result.
  - DESIGN 7.17.10's step 2 says a box's version is made at packing.
    If the packs (FONTS') gave each box the version they compute,
    `ship_out` would read it and the second hash would go. A change to
    the box after the pack would then version it again, as a shared
    box's change already does.

**Gate.** `scripts/sandbox cargo xtask check` (20:05–20:16): rc 0.
fmt, clippy with and without `partex-cli/trace`, wasm, the tests,
trip 7/7 and etrip 18/18 identical in both modes, e2e 34/34 identical,
plain and in machine mode. An earlier run (19:53–20:02), also rc 0,
had a temporary trace line in `ssa.rs` for part of its test step, so
it was run again on the clean tree.

### Coordinator, 20:15–21:00: machine mode to go; one agent again; applied hits merged into the combined line

**Decisions (the user).**
- 20:15: machine mode (§7.16's machine: units, checkpoints, restores,
  guards, the sanitizer) is removed. The user asked whether the engine
  checkpoints; SSA does not (7.17.7: "no checkpoints and no restores"),
  but machine mode was still the default rebuild, and one SSA change
  (42b576a, the objects beside the entries) had broken its restore, so
  an hour went to the old system. §7.17 keeps nothing of it for
  compatibility; its numbers stay in this log as the baseline. The
  removal is a task of its own on the merged line, with DESIGN and
  AGENTS.md changed first.
- 20:25: back to one agent once the three running finish their tasks.
  ENGINE, which owns the runtime and hits, continues; FONTS' and
  WRITERS' open items go into its queue.
- 20:20: the coordinator resolves the applied-hits merge itself. It had
  waited since 18:00 in the main checkout, half merged with an older
  line.

**The merge.** The combined line (351cc86: the fonts and hyphenation
values, the packs, alignment and page builder by field, the writers'
tables, the streams) with ab6e7d4 (hits applied, switchable) and
e8265e2 (the machine-mode fix), in a worktree; the main checkout's
half-done merge with b954e91 was aborted (all its changes were the
merge's own) and master fast-forwarded to the result.
- `page.rs`: the marks by class, each mark written as it is set
  (ab6e7d4), with the page family's accessors and the split's packs
  as calls (FONTS).
- `print.rs`, `ssa.rs`, `track.rs`: **one capture point for effects.**
  ab6e7d4 captured the log and the terminal (`effect_bytes(stream)`,
  one pending run, a typed `Effect(u8, bytes)` it replays); WRITERS'
  piece 2 captured every output (`output(&str, bytes)`, a run per
  output, an `Effect(String)` for check mode). Kept: WRITERS' hook
  and runs, ab6e7d4's typed effect and replay, with the kind a closed
  enum, `track::Output` (log, term, `Write(n)`, the DVI file's bytes,
  the page sink's start, page, page-now and finish, the PDF file's
  bytes). `replay_effects` matches it exhaustively, so an output
  cannot be dropped at a hit by a name it does not know; each has a
  raw sink (`log_bytes_raw`, `term_bytes_raw`, `write_bytes_raw`,
  `dvi_bytes_raw`, `pdf_bytes_raw`, `page_sink_raw`, which loads the
  saved writer or page back). Check mode compares a hit's and its
  body's bytes output by output, each joined (runs can split at
  different boundaries). `wterm_cr` goes through `wterm`, so the
  newline is one effect, not two. The `capture` flag went: the
  recorder's `on` gates the runs.
- A collision the merge made: ab6e7d4 gave the conditionals the
  per-call read stamp 15 of the list family's array, which FONTS had
  since given to an alignment slot (`Row::Align` is 13–19). A read of
  `cur_loop` would have hidden a later read of the conditionals in the
  same call. The conditionals take `COND_STAMP`, after the list
  family's slots; the marks have none, as in ab6e7d4.
- `run_ssa` passed clippy's 100 lines with both sides' report lines;
  check mode's report became `report_check`.

**The SSA cold measurement, corrected.** At 19:34 the coordinator
mailed that the 1,544 s cold build (575990a, 38.3 billion reads
verified) needed "a parent checks the child's name, not its reads
again". DESIGN has no such mechanism, and it was withdrawn in the next
mail. The cause is the half-built state: a record holds its children's
reads (`close_frame` copies them up, 7.17.2's form, so that a hit
reuses its children unseen), and on 575990a a hit is probed, not
applied, so every call checks its reads, runs its body anyway, and
each child checks the same reads one level down. With hits applied
each read is checked once, at the highest call that hits.

**The dependency graph (the user, 20:45–20:55).** Asked what applied
hits would bring, the coordinator answered that after a word edit the
root misses and its children, every paragraph, are each checked by
their reads. The user: a paragraph's effect is its outputs (a counter,
a `\ref`, a box's size), so why would 5,000 paragraphs care, and
"if we built a correct dependency system, then this … wouldn't even
need to be described": the dependency system is broken. It is, in
three places. `close_frame` copies each child's reads into its
parent's record, so the root depends on the whole document; the
runtime keeps no readers, so a rebuild can only verify every record
from the root; and the input state is not a value, so it is a
dependency no record sees. §7.17's first sentence already said "a call
runs again if and only if a version it read changed"; the coordinator
had written §7.17.3 as "evaluated from its root" (e0cfa7b, 08:50) and
accepted the copied-up reads in the runtime's first cut (52795cc,
10:16). Both sentences are corrected in DESIGN (§7.17.2: a record
holds its body's own reads, a child's stay in the child and the parent
depends on the child by its result; §7.17.3: every value keeps its
readers and a rebuild runs again the readers of what changed, then the
readers of what their re-runs wrote differently, and nothing else).
The dependency graph is ENGINE's task, with the input family's rows;
machine mode's removal follows it.

### Coordinator, 21:00: three more conceptual breaks, corrected in DESIGN

Asked what else was broken conceptually, the coordinator checked
three suspicions against the code; all held, and the user approved the
forms (21:00):
1. *No state to restart a call from.* §7.17.1 made values persistent
   but kept the slots "in the engine's arrays"; `eqtb` is a flat
   `JVec` with the machine's change journal and `eqtb_obj` a `Vec`.
   A rebuild by readers starts a call in the middle of the document,
   which needs the state as it was there; with flat arrays that means
   walking from the start. Now: a table is a persistent vector of
   shared chunks, and a record keeps the state it started from.
2. *Output replayed by walking.* The 20:30 merge made a hit emit its
   runs again into the files (`replay_effects` through the raw
   sinks), so the files need every record visited, and a rebuild by
   readers would leave out every page it skipped. §7.17.2 and 7.17.11
   already said the link orders the records' effects; now the streams
   bullet says so too, and a record not visited keeps its bytes.
3. *The top level cut by the machine.* The SSA driver steps the engine
   to §7.16's clean points through the machine's `Step::Checkpoint`
   and wraps each region as a "paragraph" call. Now: the top level is
   a fold of steps from clean point to clean point, named by the
   tokens each consumed; the root reads nothing and depends on no
   step's result, so a rebuild never runs it; the engine ends a step
   itself. §7.16's machine is removed (the user, 20:15), with what
   replaces each use written into 7.17.9.
Items 1 and 2 join ENGINE's running task, since a rebuild by readers
needs both; item 3, with the machine's removal, is its next and last.
After it the work halts (the user).

### §7.17.3 built: the fold rebuilt by readers; the course's word edit in 2.6 s (Coordinator, master, 2026-09-28 21:09 – 2026-09-29 02:40; on b390f39)

At 21:09 the user stopped the agents ("No agents, no timer, just focus
on the core"), and the coordinator wrote §7.17's dependency core
itself. This entry records what exists, why each piece has its form,
and what it measures. Every form below was written into DESIGN 7.17.3
before its code, dated there.

**The fold** (`partex-ssa/src/fold.rs`). The top level is a sequence of
steps, each from one outer clean point to the next
(`Tex::clean_point`: outer vertical mode, nothing on the contribution
list, no token list on top of the input). A step has a key that orders
it (`insert_after` numbers a new step between two, `renumber` when the
gap runs out). Each slot keeps its definitions in program order, each
with the step that made it, and every definition keeps its readers:
the steps that read it from outside themselves (`readers_between`).
Nothing is snapshotted. The state at a point is each slot's reaching
definition, and after each step the engine's arrays hold the latest
definitions again. That is the user's correction of the evening ("Why
do we still snapshot when we have freaking SSA"): the first cut kept
entry states and restored them, and it is gone.

**The rebuild** (`partex-core/src/ssa/rebuild.rs`, `ssa::rebuild`), in
the process that built the job:
1. The files loaded are looked up again (each as the kind it was
   loaded as). A file read by lines gets an edit per data; a file
   read whole makes its loads' readers dirty.
2. The dirty steps run in key order. Each is placed at the definitions
   that reach it: its last run's reads predict, its end validates, and
   a read of a later definition drops the run (`abort_step`) and runs
   it again with that slot placed too.
3. A definition that changed marks its readers up to the slot's next
   definition. An equal one changes nothing: the cutoff.
4. A step that ended elsewhere runs on until it meets an old step's
   end (`meet`, 64 steps ahead), and the steps passed over are retired
   with their definitions.
5. The output is linked from the steps' records afterwards
   (`effects::link` over `ssa::step_effects`). The `\write` files are
   written as the build goes too, since the job reads them back.

**The forms that made the edit harness pass**, in the order the
failures showed them (the harness is below):
- *A load reads the store, not the file* (`readback`). A stored name's
  value at a point is its appends since its last open, and before any
  open the φ, what the last trip stored. Each step keeps its store
  events and its loads (the version found, whether it read the φ).
  Inside a rebuild a load is served from the steps' stores, never from
  the host's file, which a step re-run out of order may have truncated.
  A changed φ makes dirty the loads that read it. A step that stored
  differently makes dirty the later loads that read the build's own
  store.
- *A load is looked up as its kind* (`cutoff-pdf`, a panic at object
  196). Every file was re-probed as a TeX source, so the font map and
  the Type 1 fonts were "gone".
- *A stream's version is its file* (`incr`'s `.toc` empty after its
  third rebuild). `Out(n)` is versioned by its name and its `WriteId`,
  and the job's end, which closes the open `\write` files, writes it.
- *A family's stamps are its own* (`incr`'s DVI short). `Alloc`, `Pdf`
  and `Dvi` shared one dense array of per-call read stamps, so a read
  of one hid the next read of another in the same call.
- *Glyph appends* (item 5; the course). Every ship read and wrote each
  font's glyph set, so the word edit re-ran every ship after `pdf:30`,
  a chain through the rest of the book. Now each `pdf_ship_out` appends
  the glyphs it used (`Row::Glyphs(n)`), the job's end reads the union
  (`glyphs_union`), and a font's version leaves its glyphs out.
- *A `\read` stream is mapped* (`verbatim`'s last edit). `Read(n)`'s
  value holds the file's data and position, and the step reading after
  `\openin` re-ran on the data `\openin` had found. A stream's value is
  now mapped through the edits like the input's files, each edit
  applying to the data it replaced (`Arc::ptr_eq` on `Edit::old`).
- *A query is asked again* (`effects`: a touched file kept its old
  `\pdffilemoddate`). Each step keeps the host queries it asked
  (`track::Query`: a file's date, the clock, the timer, the terminal).
  A rebuild asks them again, and a step whose answer changed is dirty.
  DESIGN's older line "a call that asked runs again in every build"
  contradicted §7.17's first sentence and was fixed there. The
  timer's start at the job's start is left as the first trip's; it
  joins "Not built" with the dirty first step.
- *Lines are of a data, and an edit is of a data* (`edits-dvi`: a
  panic in `unsave`). LaTeX reads its `.aux` by lines twice: the φ at
  `\begin{document}` and its own store at `\end{document}`. The lines
  of both were kept as one file's, by position. Now every load's
  contents is a `Data` (its name, whether it came from the file, its
  lines read), and an edit turns one data into another. Edits are made
  at the rebuild's start for data from the files (the φ now, for a
  stored name). Inside the rebuild, a step whose re-run left a file
  level holding other data than its old run gets the edit from the one
  to the other (`data_edits`).
- *Edits found by lines* (`edits-dvi` again: new steps inserted for the
  whole `.aux`). The first run's `.aux` differs from the second's in
  almost every line (the page numbers). The byte diff was one span,
  bytes 75 to 5608, so no step's old end inside it could be mapped,
  and the rebuild ran the whole `.aux` as new steps. Now the common
  prefix and suffix are cut at line boundaries and the lines between
  are diffed by Myers' O(ND) algorithm, capped at 1,000 changes. In a
  run of changed lines the old and new lines are paired in order, one
  hunk each, so the place between two changed lines maps.
- *A dropped run leaves no bytes waiting* (`edits-dvi`'s `.toc` had an
  extra first line). A new step's first run read `\tf@toc` at a later
  definition, so `\@writefile` wrote a line into stream 3's buffer
  while the stream's handle was `None`. It was never flushed, and the
  `\openout` of the `\tableofcontents` step later wrote it into the
  new file. A run's start now clears the `\write` buffers, with the
  dropped run's effects. The alternative, putting the buffer in
  `Out(n)`'s value, was rejected: a consistent run never leaves bytes
  there at a step's end (`take_effects` flushes every stream with a
  file), so the buffer is scratch at a boundary.
- *The same bytes under another identity are an edit with no hunks*
  (`edits`: the `.toc` kept old page numbers). The steps reading the
  `.aux` at `\end{document}` had their lines under the cold build's
  data. A re-run of the load step found a copy with the same bytes, so
  no edit linked the two, and the next rebuild's edit went from the
  copy. Those lines were never reached.
- *A line number read is kept by position* (`edits`' last edit, an
  empty line inserted: later box reports kept `at lines 206--207` for
  207--208). A step that read a line number keeps, in the data its
  level was reading, where the level was at its start. An edit that
  changes the number of lines before that point makes the step dirty.

**The edit harness** (this session's `ssa-edits.py`: each e2e edit
sequence built cold by SSA, then each edit rebuilt in the same process,
against plain partex run on the same sources in the same order;
release build with debug info, sandboxed, `SOURCE_DATE_EPOCH` and the
files' dates fixed; compared: the DVI or PDF, the log with the
statistics masked as `xtask/src/mask.rs` does, and the files the job
writes). Every stage of every sequence is byte-identical:

| sequence | cold | rebuilds, ms (steps run) |
|---|---|---|
| cutoff (plain, DVI) | 537 ms | 39.5 (39), 21.7 (22), 39.1 (39), 9.5 (11), 22.5 (22) |
| cutoff-pdf | 626 ms | 32.8 (1), 3.5 (1), 3.4 (1), 3.5 (1) |
| incr | 499 ms | 82.4 (77), 33.5 (67), 28.4 (22), 31.9 (42), 32.6 (38) |
| readback | 560 ms | 73.9 (57), 51.3 (50), 50.8 (52) |
| effects | 601 ms | 6.9 (3), 3.8 (3), 2.7 (1) |
| verbatim | 504 ms | 3.8 (6), 2.9 (8), 0.7 (1), 2.2 (4) |
| edits (pdflatex) | 1,276 ms | 606 (367), 311 (61), 573 (129), 22 (1), 434 (154), 366 (150) |
| edits-dvi (latex) | 912 ms | 415 (247), 319 (39) |

The first rebuild of each LaTeX sequence re-runs most of the job, since
its `.aux` came from a first run (every page number moved). The 11
cold e2e documents built by SSA match plain partex too.

**The course's one-word edit** (`ch15.tex`, "That limit is expensive."
to "dear."; the book built cold by SSA, then rebuilt in the same
process; the oracle is plain partex on the edited source from the same
auxiliary files; release binary, sandboxed, load 1.7):

| | cold SSA build | rebuild |
|---|---|---|
| before the glyph appends | 177–187 s, 66 M commands, 51,728 steps | 29.5–30.5 s: 123 steps, 11.2 M commands, 366 definitions changed, 242 readers marked, 686,874 reads checked |
| now | 185.0 s, 66.06 M commands, 1,669,761 records | **2,650 ms** (link 70 ms): 4 steps, 1.01 M commands, 14 definitions changed, 4 readers marked, 21,944 reads checked, 5,248 slots positioned, 13,196 restored |

The PDF has the same content as the oracle's (`pdfcheck same`), and
the log is the same with the statistics masked. Plain partex builds
the book in 52 s, so the cold SSA build costs 3.6 times that.

**What is left**, in the order it costs:
- The 4 steps are 1 M commands: a step is as long as the stretch
  without an outer clean point. Step size is the next target.
- The link is O(document): 70 ms on the course.
- A step that ends inside a changed line (its `loc` moved) runs on as
  new steps and passes over old ones (`edits-dvi`: 3 new, 3 passed
  over). Handing its end to the next old step, which is dirty by the
  same line, would run it in place.
- The edits list is never pruned: a step's result taken g edits ago is
  mapped through every edit since.
- Not built: a dirty first step, and so the timer's start; the fonts',
  font tables' and hyphenation's values are not positioned.
- Machine mode's removal (7.17.9) is still to do.

## 2026-09-29 — A candidate inside `big_switch`'s fetch; the page's list as appends

**Why.** After 54d8af9 the course's one-word edit (`ch15.tex`,
"expensive." to "dear.") rebuilt in 2,650 ms, and nearly all of it was
one step: 996,361 of the rebuild's 1,014,325 commands, from ch15's
line 26 to line 65 (three paragraphs and a pgfplots figure). Two
things kept the step that long and kept it dirty.

**No paragraph end in LaTeX is a candidate.** Since LaTeX's paragraph
hooks (2021), `\par` is a macro that ends
`…\tex_par:D \hook_use:n {para/after}\@kernel@after@para@after`, and
its last token expands to nothing. `big_switch` fetches with
`get_x_token` (§380): it expanded that token, popped the exhausted
lists (§357) and read the next paragraph's first letter from the file,
all without coming back to `big_switch`. The candidate test was at
`big_switch`, and when it ran the `\par` list still held that token.
So no step boundary ever fell between two paragraphs.
- The form (DESIGN §7.16.1, "A candidate is also taken inside
  `big_switch`'s fetch", written before the code). `big_switch` now
  fetches with `get_command` (`expand.rs`), which is §380's loop. After
  an expansion, once the lists it pushed are exhausted and popped and
  a file is on top, it stops as at `big_switch`: the command about to
  be fetched comes from a file with no list above it. §380's loop is as
  clean as `big_switch` each time it returns to `restart`: no local is
  live there. A stop resumes at `big_switch` with the command already
  counted, and the fetch starts again.
- Fetches elsewhere (a scanner's `get_x_token`, the main loop's
  lookahead) have their caller's state live, so they never stop.
- Measured on the course, cold SSA build: 58,207 steps against 51,728.
  The rebuild's long step split. The paragraph with the edit became
  step 53674 (967 commands), and the figure became step 53675 (991,705
  commands).

**The figure's step was still dirty, through the page's list.**
`build_page` put each node on the page by changing the list, which
reads the whole list (`page:1`). So every step that contributed to a
page read the list the edited paragraph had appended to.
- The form (DESIGN 7.17.3 item 5, written before the code). Each node
  the page receives is an append, `Row::PageNode(k)`. The list's
  length and whether its last node precedes a break (`LIST_LEN`,
  `LIST_TAIL`, §1000) are slots of their own.
  - A page step reads only those two slots. Putting its node on the
    page (`page_push`, §998) reads the length and writes the node, the
    length and the tail.
  - The fire, the output routine's end, the default output and
    `show_page_status` read the whole list (`page_list`).
  - What changes the list whole writes the length, the tail and every
    node it leaves (`page_list_wrote`).
- To place a step, the rebuild puts the length first: the engine's
  list is made that long, padded with stand-ins
  (`Penalty(0)`). Then it puts the tail, as a stand-in of the right
  kind if needed, and then the nodes the step reads (`put`, ordered by
  rank).

**Two bugs this found, both fixed.**
1. `incr`, `verbatim`, `edits` and `edits-dvi` panicked (index out of
   bounds, `journal.rs:364`) or differed. A node past the list's
   current length is held by no engine array. So the rule "a slot with
   no definition after the step's key is already in place" is false
   for page nodes. The fix: `later()` is always true for
   `Fam::PageNode`, so a node the step reads is always placed. That
   fixed `verbatim` and `edits-dvi`.
2. `incr`'s first edit (the `.toc` from the cold build, read now, so
   every page moves) shipped page 5 short: "Underfull \vbox (badness
   10000) has occurred while \output is active", glue set 66.4. Found
   with a temporary trace of each fire's list (its length, a content
   hash and the kinds of its nodes), in the cold build and in the
   rebuild. Step 16 fired three times across its runs:
   - its second run's page matched the cold build's;
   - that run was dropped because it read a slot that was not placed;
   - its fire had already cut the list to the page's remainder. Putting
     the dropped run's writes back set the length again, which padded
     the list with stand-ins over nodes the run had placed;
   - the third run placed only the slot it had missed, so the fire
     packed stand-ins.

   The fix (DESIGN item 5, the new last sentence): when the length is
   among the writes put back after a dropped run, the nodes the step
   has placed are put back with it. The same bug was the cause of
   `edits`' panic. The trace was removed.

**The edit harness** (as on 2026-09-28: each e2e edit sequence built
cold by SSA, then rebuilt in the same process after each edit, against
plain partex on the same sources; release binary, sandboxed). Every
stage of every sequence is byte-identical, logs masked:

| sequence | cold | rebuilds, ms (steps run) |
|---|---|---|
| cutoff (plain, DVI) | 507 ms | 40.3 (39), 22.4 (22), 41.8 (39), 9.8 (11), 23.2 (22) |
| cutoff-pdf | 576 ms | 32.5 (1), 3.5 (1), 3.5 (1), 3.4 (1) |
| incr | 517 ms | 84.3 (77), 42.3 (66), 35.0 (14), 56.7 (40), 31.6 (31) |
| readback | 611 ms | 86.7 (59), 48.0 (51), 49.0 (51) |
| effects | 578 ms | 6.5 (3), 3.4 (3), 2.5 (1) |
| verbatim | 477 ms | 3.8 (6), 2.8 (8), 1.2 (1), 3.2 (4) |
| edits (pdflatex) | 1,398 ms | 1,347 (647), 554 (209), 445 (155), 25 (5), 568 (252), 147 (40) |
| edits-dvi (latex) | 1,037 ms | 672 (527), 177 (45) |

The LaTeX sequences got slower (`edits`' first rebuild: 606 ms over
367 steps before, now 1,347 ms over 647). The steps are smaller, so
an edit that moves every page re-runs more of them, and each costs
about 2 ms beyond its commands. That rebuild is LaTeX's second pass:
the cold build rewrote `edits.aux` in 121 places and `edits.toc` in 40,
so it re-runs 83,095 of the cold build's 96,367 commands. It checks
2.83 M reads, about 4,400 per step. The per-step cost is next after
the deferred fire; it has not been profiled yet.

**The course's one-word edit, now** (same conditions as 2026-09-28;
cold SSA build 188.5 s, 66.06 M commands, 58,207 steps; release
binary, sandboxed, load 3.2): **2,728 ms** (link 71 ms), 5 steps,
1,005,936 commands, 42 definitions changed, 30 readers marked, 20,905
reads checked. The PDF has the same content as the oracle's
(`pdfcheck same`), and the log is the same with the statistics masked.

It is no faster yet. The figure's step (991,705 commands) is still
dirty, now through the page's nodes. `\end@float` puts
`\penalty\@floatpenalty` (at most −10000) in vertical mode, so the
page builder fires the output routine (`\@specialoutput`) inside the
figure's step, and the fire reads the whole page, including the
edited paragraph's lines. The deferred fire (DESIGN §7.16.1, "before
a fire") ends the figure's step before `fire_up`, so the edit then
reaches only the fire's step. That is the next task.

## 2026-09-29 — The deferred fire: the course's word edit in 261 ms

**Why.** After 270870b, the course's word edit re-ran the step that
holds ch15's pgfplots figure: 991,705 of 1,005,936 commands.
`\end@float` puts `\penalty\@floatpenalty` (at most −10000) in vertical
mode, so the page builder fired `\@specialoutput` inside the figure's
step. The fire reads the whole page, and the page held the edited
paragraph's lines.

**The form** (DESIGN §7.16.1, "The deferred fire's form", written
before the code):
- The page step that decides to fire does not fire. Its node goes back
  in front of the contributions, a fire is pending, and `build_page`
  returns.
- At the next `big_switch` the pending fire is a candidate and a clean
  point of its own (`CleanPoint::Fire`). It is taken before the
  exhausted token lists are popped: the output routine goes above
  them, as in tex.web, where §311 shows them in an error inside the
  routine.
- The step that begins there begins with the fire (`fire_deferred`):
  `fire_up`, then the output routine, or the default output and the
  rest of `build_page`'s loop. Then `big_switch`'s candidate test is
  made again.
- The pending fire is part of the step's place (`InputState::fire`),
  not a slot. No step reads it, so a deferral that appears or goes away
  moves a step's end.
- A fire step is named by the line of its topmost file level, under
  the token lists, and by the fire.
- It is on in SSA builds (`Tex::set_defer_fire`) where the other
  candidates are (`machine::clean_cuts()`). Machine mode does not
  defer.
- DESIGN named §1145, a display's start, as an exception, with
  `build_page` followed by `push_math`. That was wrong: tex.web's §1145
  calls `push_math` first and `build_page` last, as every call site
  does. It is corrected.

**Two bugs the edit harness found, both fixed.**
1. `incr`: "stopped: a step that read nothing". The loop
   `\loop \partA … \repeat` fires the page inside its token lists, and
   the input stack at two of its fires is the same: the same lists at
   the same positions over the same file position. So a fire step could
   end at the place it began, having run 1,523 commands.
   - Before the deferral, a step ended only at a command read from a
     file, so places did not repeat.
   - Reusing a place is sound: the input is the same, and everything
     else the next step depends on is in slots, which its reads check.
   - So the guard now stops only a step that made no progress: one that
     ran no commands, did not begin with a fire, and ended where it
     began.
2. `edits`' second edit panicked in `unsave` (index out of bounds,
   `journal.rs:364`). The deferral at a display's start (§1145) made a
   new step begin inside the display's group. Its `unsave` read the
   group's save entries, which no old run had predicted, at their
   latest definitions, laid out by a later group. A saved value was
   read as an entry's word, and gave an eqtb index past the table.
   - A dropped run works only if the later value is one the engine can
     run on, and the save stack is a layout (§268: an entry's word,
     then the value it saves).
   - The rule (DESIGN 7.17.3, "The save stack is placed whole below its
     pointer"): before a step runs, each entry below `save_ptr`, and
     the stack's other slots, take the values that reach the step
     wherever a later definition holds the arrays
     (`save_stack_whole`).

The rebuild's trace now notes each step as it begins, so a run that
panics shows where it began.

**The edit harness** (same conditions as the entry before). Every stage
of every sequence is byte-identical, logs masked:

| sequence | cold | rebuilds, ms (steps run) |
|---|---|---|
| cutoff (plain, DVI) | 547 ms | 22.1 (40), 12.3 (23), 21.4 (40), 11.4 (19), 13.2 (23) |
| cutoff-pdf | 570 ms | 32.9 (2), 3.6 (2), 3.5 (2), 3.6 (2) |
| incr | 514 ms | 89.4 (85), 41.1 (72), 29.5 (20), 61.8 (48), 31.4 (36) |
| readback | 616 ms | 74.6 (64), 43.4 (54), 44.8 (54) |
| effects | 557 ms | 6.9 (3), 3.6 (3), 2.6 (1) |
| verbatim | 542 ms | 3.5 (6), 2.7 (9), 0.9 (1), 2.2 (4) |
| edits (pdflatex) | 1,400 ms | 1,415 (698), 558 (237), 433 (168), 29 (6), 552 (277), 144 (47) |
| edits-dvi (latex) | 1,036 ms | 689 (575), 136 (46) |

`cutoff`'s rebuilds take half the time, since its page-moving edits now
re-run fire steps rather than the steps that hold a fire. `edits-dvi`'s
second edit went from 177 to 136 ms. The LaTeX sequences' first
rebuilds, LaTeX's second pass, have more steps and are slightly slower.

**The course's one-word edit** (same conditions; cold SSA build
183.7 s, 66.06 M commands, 58,709 steps; release binary, sandboxed,
load 2.4): **184.9 ms** against 2,728 ms before, then the link, 76.2 ms:
**261 ms** in all. (Corrected in the entry after this one: the report
printed the link's time beside the rebuild's, not inside it, and this
entry first read 184.9 ms as the whole.)
- 6 steps, **4,450 commands** against 1,005,936;
- 44 definitions changed, 32 readers marked, 6,599 reads checked,
  1,753 slots positioned, 3,640 restored.
- The PDF has the same content as the oracle's (`pdfcheck same`), and
  the log is the same with the statistics masked.

The steps: the edited paragraph (967 commands), its fire (57), the
output routine that ships the page (2,844), and three others (564, 11
and 7 commands; the last is the job's end, which reads the glyphs the
page used).

**What is left**, in the order it costs on the course:
- The link, 76 ms, is O(document).
- The rebuild, 185 ms for 4,450 commands.
- The per-step overhead on the LaTeX sequences, about 2 ms a step.
These are not profiled yet; perf comes next.

## 2026-09-29 — The SSA link as a watch's link: 76 ms to 12 ms

**A correction first.** The rebuild's report printed
`N ms (link L ms)`, where N is taken before the link runs. The entry
before this one read N as the whole, and it was not. The course's word
edit took 184.9 ms to rebuild, and then 76.2 ms to link: **261 ms**. (It
was 2,728 + 70 ms before the deferred fire.) The report now prints the
total first, then its parts by name:
`N ms (the rebuild R ms, the link L ms)`. The entry before is
corrected.

**Why.** Of the 261 ms, the link took 76. Profiled with perf (flat, 60
warm rebuilds), it cost:
- deflate, 18% of the samples: the SSA link compressed every object
  stream again, with no memo;
- `step_effects`, 12%: it walked every record of every step, 1.67 M
  records, to find the chunks of effects among their items.

The machine's linker (§7.16.6) had solved both: deflate memoized by
content, a cached link by region, and only the files whose bytes
changed written. The SSA link had not been given them.

**The form** (DESIGN 7.17.3, "The link after a rebuild is a watch's
link", written before the code):
- Each step keeps its chunks with their versions, by step id, as its
  run ends (`Steps::effects`, filled in `step_closed`, which every
  step's close goes through). Gathering the chunks costs the live
  steps, not their records.
- The link is `effects::link_cached`, a chunk per call of a step that
  made effects, keyed `step << 32 | k`. A chunk is touched when its
  version is not the one the last link had under its key.
- Deflate is memoized by content (`StableHasher` of level and data). The
  memo keeps what the last link used, so it does not grow.
- A file is written when its bytes changed, or when a step run since
  the last link opened it.
- `PARTEX_LINK_SPLICE=0` links in full and writes everything, as
  before.
- In a debug build, each cached link is checked against a full link.

**A bug the edit harness found.** The first version wrote a file only
when its bytes changed. `edits`, pdflatex with four outputs, then had
its `.aux`, `.toc`, log and PDF differ at stages 1–6. The reason is the
host: `NativeHost::open_write` is `File::create`, which empties the
file. A re-run step's open, such as `\openout` or the PDF's at the
first ship, left the file empty on disk while its bytes, and their
hash, stayed what the link had last written. The host's ids only grow,
so a file whose id is above the last link's highest was opened since.
Such a file is written whatever its hash.

**The course's word edit doesn't resolve numbers.** The link reports
"0 of 2,580 chunks resolved": the course's PDF has no virtual object
numbers (`numbering_of` is `None`), so the link goes straight to its
layout, and the region cache is never used. The time went down
because of the deflate memo and the O(steps) gathering:

| link phase, course word edit | before | now |
|---|---|---|
| object streams | (not timed) | 0.8 ms (23.8 cold) |
| cross-reference | | 4.4 ms |
| layout in all | | 6.5 ms |
| copy | | 1.9 ms |
| files written | every file | 2.2 ms, 2 files, 3.3 MB |
| **link** | **76.2 ms** | **12.0 ms** |

**The fold's definitions found by key.** A step's close put each write
in the slot's list of definitions. To find the step's own entry (a later
record defining the slot again), it scanned the list from its end, and
all of it on the step's first write of the slot. A slot that every step
writes has a list as long as the fold (58,709 steps on the course), and
dead runs stay in it until `renumber`. The list is sorted by key, and
the step's entries sit at its key, so a binary search to the key finds
the same entry (`Fold::close`).

**Measurements** (release binary, sandboxed):

The course's one-word edit (`ch15.tex`, "expensive" to "dear"; cold
SSA build 181.1 s; the oracle is plain partex on the edited source;
load 2.5). The PDF has the same content as the oracle's
(`pdfcheck same`), and the log is the same with the statistics masked.
The rebuild's counts are unchanged: 6 steps, 4,450 commands, 44
definitions changed, 32 readers marked, 6,599 reads checked, 1,753
slots positioned, 3,640 restored.

| | the rebuild | the link | total |
|---|---|---|---|
| first rebuild, before | 184.9 ms | 76.2 ms | 261.1 ms |
| first rebuild, now | 190.2 ms | 12.1 ms | **202.4 ms** |
| 60 warm rebuilds (edit, revert), before, mean | 152.6 ms | 76.7 ms | 229.3 ms |
| 60 warm rebuilds, the new link, before the fold fix, mean | 153.3 ms | 14.0 ms | 167.3 ms |
| 60 warm rebuilds, now, mean | 145.4 ms | 12.1 ms | **157.5 ms** (median 156.5, 149.0–201.1) |

The fold's lookup by key took 8 ms off the warm rebuild.

**The edit harness.** Every stage of every sequence is byte-identical,
logs masked. The times are the totals, rebuild and link:

| sequence | cold | rebuilds, ms in all (steps run) |
|---|---|---|
| cutoff (plain, DVI) | 541 ms | 21.7 (40), 11.8 (23), 21.0 (40), 11.1 (19), 12.4 (23) |
| cutoff-pdf | 574 ms | 34.6 (2), 4.1 (2), 4.0 (2), 3.7 (2) |
| incr | 488 ms | 90.9 (85), 41.1 (72), 29.6 (20), 60.8 (48), 31.0 (36) |
| readback | 560 ms | 73.1 (64), 42.0 (54), 46.0 (54) |
| effects | 609 ms | 6.9 (3), 3.7 (3), 2.5 (1) |
| verbatim | 504 ms | 3.6 (6), 2.5 (9), 0.5 (1), 1.4 (4) |
| edits (pdflatex) | 1,372 ms | 1,419 (698), 512 (237), 429 (168), 27 (6), 527 (277), 139 (47) |
| edits-dvi (latex) | 1,054 ms | 684 (575), 138 (46) |

The small documents' links were under a millisecond before and are
now. The totals are near the last entry's rebuild times.

A first run had `cutoff-pdf`'s first two links at 243 and 126 ms, all
of it "files written" for 26 KB (the disk), and `edits-dvi` at 1,016
and 379 ms. Run again, they were 0.5 ms links and 684 and 138 ms, as
before.

**Where the rest goes.** A frame-pointer build profiled the 60 warm
rebuilds (`-C force-frame-pointers=yes`, the `profiling` profile;
DWARF unwinding had failed). Of the samples inside
`ssa::rebuild::rebuild`:
- **31%, the job's end step** (`close_files_and_terminate`): 7
  commands, which write every font again, reading the font files (14%,
  and 7% in kpathsea's searches), deflating them, making the
  `ToUnicode` maps and the object streams. It re-runs because the page
  shipped read and appended a different glyph set of one font
  (`glyphs:179`), and the end reads every ship's glyphs.
- 22% deflate inside the engine (fonts, the page's stream, object
  streams).
- 9.5% the page shipped again.
- 9% `Fold::close` (measured before the fix above).
- 4.6% `Version::of`.

The end re-runs whole because a step is the unit that runs again. The
glyphs "dear" uses are all in the font's union already, so no font's
bytes change. The general fix is 7.17's own: calls inside a re-run
step that are hits are applied, not run (task 10, its form still to be
written in DESIGN). Each font written is then a call. Its reads are the
font's file and the union of its glyphs, a child it reads by result.
When neither changed, the call is a hit and its effects are taken
again.

## 2026-09-29 — A rebuild's file checks cost the files that changed

After `ea2caf0` (the link as a watch's link), the frame-pointer
profile of the course's warm rebuilds put 13% of `ssa::rebuild` in
`read_file`, called from the rebuild's own start. To find its edits,
the rebuild made every load of the job again: a kpathsea lookup (7%)
and a full read (6%) for each of the 852 names the job had loaded. It
did this for every package, font and chapter, and for each of the 365
names LaTeX had asked for and not found.

**The form** (DESIGN 7.17.3, "A rebuild's file checks cost the files
that changed", written before the code). One host call,
`Host::unchanged(loads)`, says for each load (a name, its kind, and the
contents it gave, or none) whether `read_file` would give the same
again. The default says false, and the load is made again and compared,
as before. The native host answers with the watch's rule (§7.4, "Seeing
an edit") and the restart's stamps, and it keeps each lookup too:
- *A file found keeps its stamp* (size, modification and change times,
  inode, device), taken just before its contents were read. Its
  contents are then compared with the load's, since two names can find
  one file (`preamble` and `preamble.tex`: 39 of the course's loads).
- *A lookup keeps its trail*: the candidates it tried and did not find,
  by directory, each directory with its stamp after the search.
  - `Kpse::find_file_trail` reports the candidates kpathsea's search
    tried on disk. That includes a database's candidate that failed
    `readable_file`, though the databases themselves are read once per
    process, as kpathsea reads them.
  - The host adds its own: the output directory's candidates and the
    formats directory's.
  - A directory that still has its stamp holds none of its candidates.
    In one that changed, each candidate is looked at, and must still not
    be a file.
- A stamp is kept only if its times were over 2 s old (git's racy
  entries, `quick.rs`'s rule).
- One call answers every load, so each directory is looked at once.
- `PARTEX_STAT_CACHE=0` makes every load again.

**A first form that was wrong.** The first draft stamped only the
directories on a trail. `sed -i`, and every editor that saves by
renaming (vim's default), creates a file in the document's directory.
That changes the directory's times at each save, and the directory is
on nearly every lookup's trail (`.` comes first in `TEXINPUTS`). So
each save would have made almost every lookup again. The candidates are
kept for that case.

**Tests.** The edit harness never creates a file. So a plain document
that runs `\openin1=extra` and `\input story` was rebuilt three times in
one process:
1. `extra.tex` was created: the `\openin` found it.
2. `./story.tex` was created: it shadowed the TeX tree's `story.tex`.
3. `./story.tex` was removed: the tree's was found again.

Every stage of the eight edit sequences is byte-identical, logs masked.
On `edits`, 23 of 26 loads are taken as they were and 3 are made again:
the edited file and the job's own `.aux` and `.toc`, rewritten by the
link.

**The course's word edit** (same conditions; cold SSA build 183 s;
release binary, sandboxed). The PDF is the same content as the
oracle's, and the log is the same with the statistics masked:

| | loads made again | the rebuild | the link | total |
|---|---|---|---|---|
| first rebuild, after `ea2caf0` | 852 | 190.2 ms | 12.1 ms | 202.4 ms |
| first rebuild, file stamps only | 408 | 177.1 ms | 12.2 ms | 189.3 ms |
| first rebuild, stamps and lookups | **5** | 167.7 ms | 12.0 ms | **179.6 ms** |
| 60 warm, after `ea2caf0`, mean | 852 | 145.4 ms | 12.1 ms | 157.5 ms |
| 60 warm, file stamps only, mean | 408 | 141.2 ms | 15.7 ms | 156.9 ms |
| 60 warm, stamps and lookups, mean | 5 | 129.8 ms | 12.8 ms | **142.6 ms** (median 138.5) |

**Next**: the job's end step. It re-runs 7 commands, but they write
every font again: 31% of the rebuild in the last profile, about 45 ms.
Task 10 (hits applied inside re-run steps), with each font written as a
call.

## 2026-09-29 — Hits applied inside a step that runs again: the fonts as calls

**Why.** After `22465ee` (a rebuild's file checks by stamp and lookup),
the course's word edit spent about a third of its rebuild in the job's
end step. That step re-runs 7 commands, but they write every font again:
each Type 1 file is read, subsetted and deflated, and each `ToUnicode`
map and object stream is made again. The glyphs "dear" uses were all in
the fonts' union already, so no font's bytes changed. The fix is 7.17's
own evaluation, made inside the rebuild. A step that runs again
evaluates each call its body makes, and a call that is a hit is applied,
not run.

**The form** (DESIGN 7.17.3, "Hits applied inside a step that runs
again", written before the code):
1. *The probe.* The call's records are looked up by name, and their
   reads are compared with the engine's state as the step's run has
   placed it.
2. *The hit.* Its writes are stored and its bytes printed again. It
   becomes the running call's child, and its body does not run. Its
   effects in the link's form must come back too, so a step's effects
   are cut into chunks at the boundaries of the calls that apply:
   - at the call's start, the step's effects so far are a chunk;
   - at its end, before its writes are versioned, its body's effects
     are its own chunk, an effect of its record;
   - an applied hit gives the step the chunks in its record's subtree;
   - the step's end cuts the last chunk.

   The link took several chunks per step already (keys `step << 32 | k`).
3. *Its reads are the step's.* Each read of the hit's subtree from
   outside it is a read of the step too, unless the step wrote the slot
   before. So the step is the slot's reader in the fold, and the run's
   check covers the hit. A probe that compared a value a later
   definition holds (one the run did not set) drops the run, which is
   made again with that slot set.
4. *Which calls apply.* Only those whose record holds everything the
   call makes. That means `unsave` of a group with no token to insert,
   and the fonts. The others run; they probe only in check mode.
5. *The fonts are calls:* `font_file` (a descriptor with its font file
   stream), `encoding`, and `font_dict` (with its `ToUnicode`). Each is
   named by its tree's key and the entry. Each is a writer call whose
   scope names its fields:
   - all three read the font trees (`FONTW`);
   - `font_file` also writes them (the subset tag and the object
     number);
   - `font_dict` also reads `\pdfglyphtounicode`'s table, the
     no-builtin set and the font attributes;
   - all three write the objects and the writer (`WRITING`).

   The font file is a load, and the parameters are read through the
   tables.

**The code.**
- `Runtime::apply` walks the hit's subtree as `verify` does
  (`outside_reads`). It notes each outside read as the open step's
  (`Open::note_step_read`, once per step, by the same serials as a
  body's read), before its writes make the slots the step's own.
- `Func::applies` names the routines that apply. `call_begin` probes a
  routine only if it applies and hits are applied, or in check mode.
  So `tokenize` and `step` now probe only in check mode.
- A rebuild applies hits unless `PARTEX_SSA_APPLY=0`. A cold build
  applies them only with `PARTEX_SSA_APPLY=1`, as before.
- `Tex::applied_call` is the font calls' wrapper, and `ssa::cut_chunk`
  makes the cuts. `Steps::cur_chunks` holds the open step's chunks,
  and `step_closed` takes them; it no longer walks the step's records.
- Each rebuild reports its calls by routine
  (`ssa rebuild N routines: ...`), and the hits applied with the
  commands they stand for.

**A bug the harness found: `unsave` of an `\aftergroup`.** Until now a
rebuild never applied a hit (`apply` was set false in it); only a cold
build with `PARTEX_SSA_APPLY=1` did. With hits applied, `incr`,
`edits` and `edits-dvi` differed from their first rebuild on, and
`edits` ran without end. `incr`'s cold build with hits applied showed
why. After page 1 came "You can't use `\end` in internal vertical
mode". Plain's `\footnote` ends its group with `\aftergroup\@foot`.
`unsave` puts those tokens in the input (§326), and `back_input` may
first end finished token lists. No family holds the input, so the
record did not hold it, and the applied hit lost `\@foot`. Such an
`unsave` is now a routine of its own, `unsave_after`. It is named as
`unsave` is and chosen by the entries in the name, and it runs. The
DESIGN rule it breaks was already written (item 4: a record must hold
everything the call makes). The code had applied every `unsave`.

**A second bug, found by the course: output twice.** The course's
first rebuild with font hits had the same content by `pdfcheck`, but
qpdf had to recover a stream's length. The PDF was 3,707,250 bytes
against 3,184,369, and the log named each font file twice. The first
form replayed a hit's bytes through their outputs *and* gave the step
its chunks. With effects in the link's form on, every output is such an
effect: the log, the terminal, `\write` files, the DVI and the PDF all
go through `out_write`/`out_term`. So the chunks held the bytes already.
The form is now uniform (DESIGN item 2):
- a hit's output comes back only as its chunks;
- the bytes a record keeps by output are check mode's;
- a build with no effects in the link's form prints them again;
- the step's effects are cut at every call that applies, `unsave`
  included, and an empty cut makes no chunk.

The eight sequences had passed with the doubled output: none of them
applied a font hit (`edits` misses all 14 font calls in every rebuild,
since each edit adds glyphs, and `cutoff-pdf` never runs its end
again). A seventh stage was added to `edits`: `99` on an early page,
digits the fonts have from other pages' folios. The job's end then runs
again with the same fonts, and all 14 font calls are hits. (The
harness, `ssa-edits.py`, is not in the repository yet; it moves into
e2e with task 6, SSA as the gate's second mode.)

**Tests.**
- Every stage of the eight edit sequences, and `edits`' new stage 7,
  is byte-identical to plain partex (logs masked), with the default
  (rebuilds apply) and with `PARTEX_SSA_APPLY=0`.
- `incr` and `edits-dvi` are also the same with hits applied in the
  cold build.
- The gate (`cargo xtask check`) passes: e2e 34/34 identical, machine
  mode's edits and sanitized watch included.

**Measurements** (release binary, sandboxed).

The course's word edit (`ch15.tex`, "expensive" to "dear"; cold SSA
build 188.3 s; the oracle is plain partex on the edited source). The PDF
has the same content as the oracle's (`pdfcheck same`, no qpdf
warnings), and the log is the same with the statistics masked. The
counts are unchanged: 6 steps, 4,450 commands, 44 definitions changed,
32 readers marked, 6,599 reads checked. Of the rebuild's 264 calls,
256 hits were applied:
- `unsave`: 199 of 207;
- `unsave_after`: 63, which run;
- the fonts: all 57 (19 `font_file`, 9 `encoding`, 29 `font_dict`).

The link has 3,427 chunks, from 2,580.

| | the rebuild | the link | total |
|---|---|---|---|
| first rebuild, after `22465ee` | 167.7 ms | 12.0 ms | 179.6 ms |
| first rebuild, hits applied | 150.1 ms | 13.9 ms | **164.0 ms** |
| 60 warm (edit, revert), after `22465ee`, mean | 129.8 ms | 12.8 ms | 142.6 ms (median 138.6) |
| 60 warm, hits applied, mean | **94.9 ms** | 21.2 ms | 116.1 ms (median **106.3**) |

The warm link's mean rose because of the disk, not the link. Its own
work is 9.4 ms mean (layout 6.7 ms), as before. "Files written" has a
median of 2.2 ms but reached 198 ms. Without the writes, the warm total
is 105.5 ms mean. (A first try with the doubled output measured 165.5
ms; it does not count.)

The edit sequences, the rebuilds' own time summed over each sequence
(the same binary, one after the other):

| sequence | hits applied (default) | `PARTEX_SSA_APPLY=0` |
|---|---|---|
| cutoff | 77.6 ms | 76.1 ms |
| incr | 255.6 ms | 268.3 ms |
| edits | 3,183.0 ms | 3,154.1 ms |
| edits-dvi | 838.5 ms | 882.0 ms |

On these small documents, applying `unsave`'s hits neither pays nor
costs: a hit's lookup and walk cost about what the body they skip does.
`edits`' stage 7, where the fonts hit, went from 21.8 to 16.8 ms.

**A label no one refers to** (asked by the user: ideally one pass). A
`\label` with no `\ref` was added, then two rebuilds with no edit
followed. Each stage matched plain partex run the same number of times
(pdf same, log same but for the output directory's path, `.aux` same).

| | `edits.tex` (cold: 96,367 commands) | the course (cold: 66.1 M commands) |
|---|---|---|
| the label added | 17 steps, 1,467 commands, 24.3 ms | 241 steps, **4.82 M commands**, 51.2 s (traced) |
| next rebuild, no edit | 11 steps, 437 commands, 12.0 ms | 15 steps, 24,902 commands, 280 ms |
| the one after | 0 | 0 |

There are two rebuilds because a load reads what the last rebuild
stored. LaTeX's `\end{document}` compares the labels it reads back
with those read at `\begin{document}`, and asks for a rerun. On the
course, the first rebuild re-ran several steps of about 1 M commands
each (step 53964: 991,637 commands). They were marked by slots the
label's step changed, one of them with 31 readers. Two gaps with
DESIGN show here:
- the unit of re-entry is still the step, where it should be the call,
  down to the command;
- a `\write`'s store goes to the next rebuild, where it should be
  forwarded to its load in the same one.

They are the next work: the evaluation graph, written into DESIGN next.

## 2026-09-29 — DESIGN 7.17.13: evaluation as one flat graph, run to convergence

Written with the user after the label test above. 7.17 already said
that every operation is a call and that a job's cycle is iterated in
memory (7.17.5). The code is behind it in two places the test showed:
- the step between clean points is still the unit of re-entry;
- a store is read by the next rebuild, where it should be forwarded to
  its load in the same one.

7.17.13 states the evaluation at the grain 7.17 implies:
- **Nodes.** Every command of `main_control` and every call it makes is
  a node. The hierarchy is intrinsic, and a hit skips its subtree.
- **Re-entry at any command.** TeX is iterative, so a command boundary
  is a place a node can begin once the input stack is a value. Clean
  points go.
- **No cursor is read as a value**, so no chain of commands forms:
  - the current list is appended to;
  - the input position is read relative to the node;
  - allocators name what they allocate by identity.
- **Functions are values.** A meaning is a value in its slot, a call
  reads it, and a redefinition is a new definition, with memo keys by
  content. A call depends on the whole body for now; the value format
  keeps a body's spans addressable so that a call can later depend only
  on the spans it consumed (agreed with the user).
- **Edges**: data, φ (a store to its loads, in the same evaluation:
  there are no passes), ordering (effects), and sources asked again.
- **Memo** of *k* past (operands → results) per node.
- **Evaluation**: a worklist by priority until it is empty, with a
  bound on oscillation. On a keystroke, the viewer's page comes first
  and is painted from its node, and the rest converges behind it.
  Target: 16.7 ms per edit to the page on screen (the user: 60 frames a
  second).
- **The oracle**: plain partex run until its stores settle, latexmk's
  rule.

**Measured for reference** (the course, same machine, same auxiliary
files, load about 2.6):

| run | time |
|---|---|
| pdfTeX 1.40.29 | 57.6 s |
| plain partex | 55.1 s |
| partex SSA cold (recording) | 178–188 s |
| partex SSA word edit, first rebuild | 164.0 ms |
| partex SSA word edit, 60 warm rebuilds | 106.3 ms median (116.1 mean) |

The work is listed in 7.17.13's order. The first item is the input
stack as a value.

## 2026-09-29 — DESIGN rewritten; the evaluation's representation and threads decided

**DESIGN.md rewritten** (`dede86e`). The 6,251-line document had become
the record of designs since replaced. It moved whole to
`DESIGN_ARCHIVE.md`, kept as history only. The new DESIGN.md states the
design as it is, self-contained enough to implement again from tex.web.
Its chapters are 1–6, so no citation collides with the old 7.x numbers
that code comments cite; appendix A maps each of those. From now on a
design change rewrites its section in place, and its history goes here.

**Then, with the user, the representation of the evaluation and its
threads** (DESIGN 3.2, 3.6, 3.9, 3.10, 4). The questions and what they
decided:
- *Why does `unsave` give the old version back?* Today it writes the
  saved entry, which is a new definition whose version only happens to
  equal the old one. So readers after a group read `unsave`'s
  definition, and a change of the outer definition reaches them only
  through the save entry and `unsave`'s re-run. Decided: **a local
  definition has a scope** ending at its group's end, after which the
  outer definition reaches again, the same node. The save stack becomes
  the stack of **group frames** (kind, boundary, the `saved(k)`
  context, `\aftergroup` tokens, pointers to the local definitions);
  TeX's undo log of copies goes.
- *Why a physical restore at all?* Only because flat arrays cache the
  latest definitions for fast reads. *Doesn't one cache stop threads?*
  It would. Decided: **no working copy**. Every read resolves (address,
  timestamp) through the **definition index**, the address's
  definitions sorted by timestamp: the current-definition map of SSA
  construction (Braun et al. 2013), kept alive because construction
  never ends, stored as a "fat node" persistent array (Driscoll et al.
  1989). Fast paths: one definition; the last one, at the frontier. It
  removes placing a step, dropped runs, the save stack placed whole and
  the page list's stand-ins, which all existed to position one mutable
  engine. Risk: the token path's read cost, measured first (a plain run
  of the course through the index; chunked eqtb cost 2–3% before).
- *What is a slot?* The code's word for an **address**, what a read
  computes; the graph has nodes, values and edges, and the index is how
  a read finds its edge.
- *Functions as values.* A meaning is a sum: a macro, a primitive
  (versioned by the binary), a character token, undefined, or an
  address (`\countdef`). `\let` is an edge to the same value node; a
  call is an apply node keyed by the function's and operands' versions.
- *Like a sea of nodes?* Yes in its edges (Click; C2, Graal): no
  blocks, nodes constrained only by edges, memory split by address,
  effects on ordering edges, memo as GVN. It is dynamic and has a
  hierarchy. *How would we do it like V8?* V8 left the sea of nodes for
  JavaScript ("Land ahoy") because nearly every node sat on the effect
  chain. Decided: sea of nodes in its edges, **a trace in its layout**
  (self-adjusting computation's dynamic dependence graph: program
  order, struct-of-arrays chunks as Turboshaft stores operations,
  nested spans for the hierarchy, hierarchical Dewey timestamps so a
  re-run inserts children inside its own interval without a global
  lock). The no-cursor rules are what keep TeX's nodes off one chain,
  and the SSA report is to print the longest such chains. ("Tape" was
  my word first; the precise term for this use is Acar's trace.)
- *Isn't that single-threaded?* No: the layout is not the order. **One
  shared worklist**, a relaxed concurrent priority queue by (viewer's
  page, timestamp); any worker runs any node; publish, then validate
  its reads (a changed resolution re-runs it); old definitions of a
  node about to re-run are **estimates** readers wait on. This is
  Block-STM's execution of ordered transactions over a multi-version
  memory (Gelashvili et al. 2022), and correct in any order because a
  node runs again iff a version it read changed (chaotic iteration).
  Effects commit by timestamp; the settled prefix streams to the link;
  the page on screen paints when its node is settled.
- *Is it cleaner than the current design?* Yes: DESIGN 4.1 lists each
  current mechanism, why it exists and what replaces it. Where it is
  harder: the read path, memory at command grain (66 M commands on the
  course), and concurrency, which stays inside the index and the
  scheduler.

**The work reordered** (DESIGN 4.2): the input's value; the definition
index (read cost first); the command as the node (bytes per node
first); scoped definitions; appends and results as values; the φ;
function values; the memo of *k*; the worklist; threads last, with one
worker until then. Expected numbers per kind of edit are in DESIGN 5.1,
as estimates to be replaced by measurements.

## 2026-09-29 — The input's value, shared (DESIGN 4.2 item 1)

Commit 62174c7.

**What changed.** Where a step leaves the input was a copy: the input
stack, the parameter stack, the whole buffer and the file arrays,
copied at every step's end and mapped at the next rebuild. The input
is now a shared value (DESIGN 3.5, `input::InputValue`):
- the levels and the parameter stack are chains (`input::Chain`), each
  entry holding the value of the entries below it. `input::Prefixes`
  keeps each prefix's chain while it holds, and a push or pop at a level
  forgets that level's chain. So taking the value makes only the levels
  pushed since the value was last taken.
- The buffer below the top file level's start is frozen while that
  level is open, because every write to the buffer lands in the top
  line or above `first` (§355, §362, §363, §372, §538). It is one
  `Arc<[u8]>`, made when a file level opens or closes, plus a copy of
  the top line.
- The file levels below the top (their files, lines, `\everyeof` flags,
  names, and e-TeX's group and condition depths) are one shared
  `input::FileLevels`, made at the same times. e-TeX's `group_warning`
  and `if_warning` write into lower levels, so they clear it.
- A `\scantokens` pseudo file is its shared lines and the index of the
  next line (`input::PseudoFile`), not a queue popped as it is read.
- `rebuild::InputState` holds an `InputValue` and the top level's own
  entries (`FileTop`). `step_closed` receives the state, taken before
  the record is borrowed.

**Why this form.** Item 1 is measured by its cost at every command,
since the command becomes the node (4.2 item 3). The cost was probed
temporarily: the value was taken after every command in `main_control`
on the course (66,063,925 commands) in a cold SSA build, with the
target/trace binary, warm cache:

| taken after every command | cold build | per command |
|---|---|---|
| nothing | 203.1 s, 201.8 s | — |
| the old copy | 329.8 s | ≈ 1.9 µs |
| levels and buffer shared | 254.8 s | ≈ 0.78 µs |
| plus FileLevels and parameters shared | 223.8 s | ≈ 0.33 µs |

On average a command's buffer prefix is 114 bytes and its top line 77
bytes, and the input stack is 22.9 levels deep. What remains is mostly
the top line's copy and the small allocations per value. The target is
under 0.1 µs, and it is taken up with item 3, where a node's operands
are laid out in the trace's chunks. The probe was removed before the
commit.

A rejected alternative was a persistent vector (an RRB tree) for the
whole buffer. The frozen prefix makes that unnecessary: the part that
changes is the one line on top.

**Measured** (the release binary, SSA, the same process):
- The course word edit (ch15, "expensive" → "dear"): 145.2 ms (the
  rebuild 132.6 ms, the link 12.6 ms), before 164.0 ms. Calls 264
  (fresh 6, hits applied 256), readers marked 32, reads checked 6599,
  definitions changed 44, commands 4450. The PDF is the same content
  as plain partex's and the log is the same, with statistics masked.
- The cold build is 191.5 s (before 188.3 s, within noise at load 2.9).
- The label test: every pass has the same PDF and aux as the oracle.
  The log difference is unchanged from before.
- `ssa-edits.py`: every sequence is the same.

## 2026-09-29 — The definition index measured; the event log

Commit 87652e8 (the event log, `scripts/events.py`, DESIGN 3.2 and 4.2).

**The question** (DESIGN 4.2 item 2): the definition index was to be
measured before it replaced the flat arrays, on a plain course build
with eqtb and the hash read through it, the read path's cost to decide
its representation.

**A prototype first.** A fat-node index (each address's last
definition inline, the older ones in a side vector, reads by binary
search) ran as a shadow of eqtb, eqtb's objects and the hash, with the
command counter as the timestamp. It resolved 1.52 G eqtb reads, 146 M
object reads and 14 M hash reads, and none differed from the arrays.
On an 8-CPU machine with nothing else running, three alternating runs
each of the plain course: 37.1–38.4 s without it, 50.5–51.6 s with it,
and 6.1 GB peak memory against 172–188 MB. `perf stat` on the
workstation: 303.5 G → 364.5 G instructions (+20%), 159.3 G → 214.4 G
cycles (+35%), cache misses 51 M → 224 M. The extra cycles are the
history missing the cache, not the read path. (A first timing, +40%,
counted reads with an atomic increment, a locked instruction on every
read; `perf` would have shown it at once.) Reads in the middle: 51.2 M
binary searches in 1.6 s, about 31 ns each.

**Memory, not the read path, decides.** Kept per command the index
holds 51 M eqtb definitions and 50 M object definitions; their token
lists alone are 2 GB (34 M lists). The busiest addresses (`\toks0`,
`\pgfmathresult`, `\pgfmath@parse@next`) have 2–3.4 M definitions each:
pgfmath's temporaries. For comparison, today's SSA cold build of the
course peaks at 24.0 GB (141 s, 1.68 M records, 1.51 M of them
`unsave`'s): the records are the larger memory problem, and are next.

**The event log.** The prototype was edited for each new question, and
each question cost a build. Instead, a tracker observes a plain build
through the engine's own hooks and writes a table of every definition
and every command (`PARTEX_EVENTS=DIR`, `partex-cli/src/eventlog.rs`;
DESIGN 5.2). The engine gains one hook, `Tracker::command`, the
observer's clock (the command's number, input level, line, group level,
and whether it begins at an outer clean point, computed from the fields
themselves so that no read of the command's is added). Queries are
offline: `scripts/events.py DIR summary|setup|grain`. The course's log
is 2.4 GB and takes 108 s to write.

**The course's log** (66.1 M commands; 62.7 M definitions made by
commands, 1.4 M held from the format):
- lifetimes, commands to the cell's next definition: 52% under 16, 85%
  under 256, 94.5% under 1,024, 99.4% under 65,536; 170 K never
  redefined; 9.5 M (15%) never read.
- the setup, the build before the first `\shipout` (the cover page):
  4.27 M commands (6.5%), 4.32 M definitions, 150 K of them net (the
  body's view of the setup), 43 K read by the body. 98% of the
  addresses (620 K of about 630 K) are never defined after it. Two
  earlier boundaries failed: the first contribution to the page and
  the first box on a page both come from `book.cls`'s `\onecolumn`,
  which clears the page (`\vbox{}` and a discarded empty page).
- kept per span (a definition is internal if the cell's next one is
  made in the same span):

  | spans | kept | crossing reads |
  |---|---|---|
  | a command each (66.1 M) | 62.7 M (100%) | 52.0 M |
  | windows of 16 | 41.3 M (65.9%) | 29.9 M |
  | windows of 256 | 16.2 M (25.9%) | 12.1 M |
  | windows of 4,096 | 3.8 M (6.1%) | 2.9 M |
  | windows of 65,536 | 942 K (1.5%) | 668 K |
  | steps, outer clean point to the next (30,721) | 703 K (1.1%) | 556 K |
  | the setup, then steps (2,121) | 638 K (1.0%) | 476 K |

  28.6 K of the 30.7 K outer clean points lie in the setup: the body's
  steps are long, 29 K commands on average.

**Decided** (DESIGN 3.2): the index keeps a span's net definitions. The
frontier stays in the engine's tables, the older definitions are kept
per span, and a span that runs holds its own in a local log and
publishes them at its end; a node inside a span is re-entered at the
span's start. Spans are the setup and then windows of a few thousand
commands, priced by the table. Steps (outer clean points) are not spans:
they are the old machinery's seam, and too sparse in a body.

**How the session went wrong, for the record.** Item 1 was tuned (0.78 →
0.33 µs a command) inside the old steps, and item 2 was measured alone
on an engine that still has TeX's undo log: many of the 62.7 M
definitions are likely `unsave`'s restores, which scoped definitions
(item 4) remove, and 1.5 M of the 1.68 M records are `unsave`'s. So
these numbers price the old machinery as much as the design. Items 2–5
depend on each other and are built as one component, behind a switch,
then measured whole (DESIGN 4.2). Rejected: the index per command, compacted (16 bytes a
definition plus the token lists, about 3.6 GB, and 1.5 G read edges);
starting SSA at `\begin{document}` (keyed on a name, and a setup edit
would be a cold build, where the setup as a span re-runs 6.5% and wakes
only what changed).

## 2026-10-02 — The SSA edit harness, `scripts/ssa-edits` (agent gate)

Commit 2d589d3 (the harness); DESIGN 1.4, 4.3 item 8 and 5.2.

**Why.** The window work (DESIGN 4.3) changes how a rebuild re-enters,
what it records and how it links. Every agent working on it needs one
command that says whether SSA mode's rebuilds still give plain partex's
bytes at every stage. The harness that answered this before,
`ssa-edits.py` (LOG 2026-09-29), lived outside the repository and
pointed at the old tree.

**What it does.** `scripts/sandbox scripts/ssa-edits [--case NAME]...`.
For each case, in `target/ssa-edits/<case>/`:
- `o/`: the oracle, plain partex (the same binary, no `PARTEX_*`
  variable), run once per stage. Each stage's edit is made before its
  run, and the files the runs before wrote (`.aux`, `.toc`, `\write`
  streams) stay, as e2e's incremental cases have them.
- `p/`: ONE process, `PARTEX_SSA=1`, whose `PARTEX_SSA_REBUILD` lines
  are the edits. Each line first saves what the build or rebuild before
  it wrote, then makes the next edit. The last stage is saved when the
  process exits.
- Every file of each stage is compared: PDF, DVI and every written file
  byte for byte, logs without their first line (banner and date) and
  with `xtask/src/mask.rs`'s statistics masked.
- A table per case: each stage's result (identical, or the files that
  differ with a log's first differing line), its ms (rebuild, link), the
  steps run and the commands. Exit 1 on any difference, or when the
  SSA process failed (a panic, a `stopped:` rebuild, a timeout). Every
  number goes to `target/ssa-edits/results.json`.

The cases, 10 sequences of 61 stages:
- the 7 incremental cases e2e builds with `-watch` (`verbatim`,
  `effects`, `readback`, `cutoff_pdf`, `incremental`, `cutoff`,
  `tokens`). They are read from `INCREMENTAL` in `xtask/src/e2e.rs` by a
  small reader of Rust constant expressions, so a case or an edit added
  there runs here too. `tokens` (13 edits) and `incremental`'s two
  stages with no edit were not in the old harness.
- `machine_edits` (`edits.tex`, pdflatex) and `machine_edits_dvi`
  (`edits-dvi.tex`, latex): e2e's arguments, with
  `-output-directory=out`. Their edits are written out in the script,
  since e2e builds one of them with `format!`. `machine_edits` has a
  seventh edit, `99` on an early page (LOG 2026-09-29). It is the only
  stage where the job's end runs again with every font call a hit (7
  `font_file`, 7 `font_dict`).
- `label`: `edits.tex` given a `\label{new}` nothing refers to, then two
  rebuilds with no edit (LOG 2026-09-29, "A label no one refers to").

**Decisions, and why.**
- *One pass per stage* on the oracle's side, because an SSA rebuild is
  one pass today: a `\write`'s store is read by the next rebuild. e2e's
  `-watch` cases run every build to its fixed point. `--fixpoint` makes
  the oracle do the same, for item 5 (`aux-loop`). Its rule is e2e's
  `fixpoint`: run again while a file the job wrote, other than its log,
  DVI and PDF, changed, at most 5 runs. latexmk's rule (the files read
  back) would need a recorder, which partex does not have. The two rules
  give the same outputs: a file that is only written depends only on
  the sources and on what its run read back, so e2e's rule makes at
  most one more run, and that run is identical.
- *The LaTeX cases are seeded as e2e's `machine_edits` is*: the `.aux`
  and `.toc` of two plain runs of the original text, on both sides. So
  stage 0 is converged, and each stage measures its own edit. The old
  harness seeded with one run, so its first rebuild was LaTeX's second
  pass (1,419 ms, 698 steps).
- *Edits as e2e's `edit_file` makes them*: the edited text goes to a
  temporary file, which is dated and renamed over the file. Inputs are
  dated 1758800000 and edit *k* 1758800000 + 1000 *k*, so
  `\pdffilemoddate` agrees on both sides. The rebuild's file checks see
  what an editor's save gives them: a new inode in a changed directory.
  Each stage's text is made once, and both sides copy it. An empty
  marker is a rebuild with no edit; a marker equal to its replacement
  only dates the file (`effects`' third edit).
- *The masks* are `mask.rs`'s patterns, applied to bytes. On a log they
  mask what `mask.rs` masks and never more, since `\d` matches only ASCII
  digits here. Bytes are compared rather than lossily decoded text. So
  the comparison is at least as strict as e2e's. Checked on the
  harness's own outputs: a pdflatex log masks 8 lines, and the oracle's
  and the SSA process's raw logs differ only in "strings out of" and
  "string characters out of".
- *Formats* are made once per binary, keyed by its SHA-256, in
  `target/ssa-edits/formats/`. pdflatex's takes about 14 s.
- *No `PARTEX_*` variable* reaches the oracle or the format runs, which
  are plain partex. The harness says which variables it ignored.
  `--env K=V` passes a variable to the SSA process, and `--check` adds
  `PARTEX_SSA_CHECK=1`.
- *Every process has a cap*: virtual memory (`--mem-gb`, 8) and a
  timeout (`--timeout`, 600 s). An SSA bug that loops (LOG 2026-09-29:
  `edits` "ran without end") then fails its case and not the machine.
  The harness refuses to run outside the sandbox.

**Measured.** Two binaries, both sandboxed:
- main's release binary (dc849cf, built in the main tree and copied
  into the worktree);
- this branch's own release build of the same source.

Every stage of the 10 cases is identical on both binaries. On this
branch's binary it is also identical with `--check`, with
`--env PARTEX_SSA_APPLY=0`, and with `--jobs 1`. Runtimes:

| run | wall | load (1 min) |
|---|---|---|
| all cases, 8 at a time, formats cached | 7.0 s | 23 |
| the same, formats made first | 21–31 s | 20–41 |
| `--jobs 1` (each case's oracle, then its SSA process) | 33.7 s | 9.6 → 7.5 |

Three negative tests:
- `--fixpoint` makes `incremental` differ at stages 0, 1 and 4–6, where
  the oracle runs again to settle the `.toc`. It makes `label` differ at
  stage 1, LaTeX's "Label(s) may have changed" run.
- `--timeout 3` kills `machine_edits`' process in its third rebuild. The
  harness reports that stage as not reported, with the process killed,
  and the later stages as not built.
- A format that cannot be made stops the run with its directory named.

The numbers with `--jobs 1` (this branch's binary; ms is the rebuild and
the link together):

| case | cold | rebuilds, ms (steps run) |
|---|---|---|
| verbatim (plain, DVI) | 539 ms | 3.8 (6), 3.1 (9), 1.4 (1), 3.5 (4) |
| effects (PDF) | 543 ms | 2.9 (3), 1.0 (3), 1.1 (1) |
| readback (PDF) | 549 ms | 70.0 (64), 41.4 (54), 43.5 (54) |
| cutoff_pdf | 566 ms | 32.4 (2), 1.9 (2), 2.8 (2), 3.4 (2) |
| incremental (DVI) | 532 ms | 91.0 (85), 200.7 (64), 2.9 (2), 38.6 (11), 85.2 (20), 93.6 (48), 36.1 (36) |
| cutoff (DVI) | 563 ms | 22.3 (40), 12.9 (23), 25.0 (40), 15.9 (19), 23.1 (23) |
| tokens (DVI) | 514 ms | 28.1 (1), 1.2 (1), 1.6 (1), 5.3 (7), 3.8 (4), 5.1 (4), 4.3 (6), 5.4 (4), 2.3 (1), 4.2 (2), 3.1 (1), 2.5 (1), 6.7 (7) |
| machine_edits (pdflatex) | 1,336 ms | 63.7 (3), 541.4 (237), 468.6 (168), 20.5 (6), 596.9 (277), 146.4 (47), 16.6 (3) |
| machine_edits_dvi (latex) | 1,024 ms | 136.1 (42), 148.2 (46) |
| label (pdflatex) | 1,336 ms | 66.7 (17), 10.5 (11), 1.3 (0) |

`incremental`'s second rebuild spent 169.4 ms in its link, writing
files to disk; its rebuild took 31.3 ms. The step counts are those of
LOG 2026-09-29 wherever the sequences are the same. `label`'s 17 steps
and 1,467 commands, then 11 and 437, then none, are that entry's
`edits.tex` numbers.

**Seen in passing**, for `rebuild-cost` (not profiled): in some
sequences a process's first rebuild costs about 25–30 ms more than a
later one that does the same work.
- `tokens` re-runs 1 step of 73 commands in 28.1 ms, then the same step
  in 1.2 ms.
- `cutoff_pdf` re-runs 2 steps in 32.4 ms, then 1.9 ms. The second run
  has the same calls by routine.
- `effects` (2.9 ms) and `verbatim` (3.8 ms) do not show it.

**Next** (part 2): `cargo xtask ssa-edits` in `xtask check`; the
course's edits as rebuilds of one process (`bench/ssa-course.sh`), with
JSON in `bench/results/`.

## 2026-10-02 — The gate runs ssa-edits; the course's edits in one SSA process (agent gate)

Commits 36359a8, baf5ad6, a877ebc, 93092dc and this entry's; DESIGN 1.4,
4.3 item 8 and 5.2.

**`cargo xtask ssa-edits`** builds the release binary and runs
`scripts/ssa-edits` with its arguments. `cargo xtask check` runs it
beside e2e, on the binary its release build made, with `--brief`: a line
for each identical case, and a table for a case that differs. The
harness stays one Python script, which xtask only calls. Machine mode's
steps stay in the gate.

**`bench/ssa-course.sh [--warm N] [--edits NAME,...] [--rebuild-timeout S]
[COURSE [AUX]]`** measures the course's edits as the rebuilds of one SSA
process.
- The course (`COURSE`, default `~/code/tmp/np-course`) is copied afresh
  into `target/ssa-course/run`, with the aux files of a full build
  (`AUX`, default `COURSE/_out`).
- One process builds it cold. Then come `word` and its revert, N times
  (default 20): the warm numbers, first. Then, for each edit of
  `bench/edits/course.txt` in its order (`--edits` keeps those named):
  the edit, its rebuild, the revert to the original text, and that
  rebuild. A `;;` spec's steps are rebuilt one by one before the revert.
  Each rebuild is one pass.
- `--rebuild-timeout S`: a watchdog started by each rebuild's line ends
  the process if the rebuild takes over S seconds, and the numbers so far
  are kept. The warm pairs come first so that a slow edit cuts only the
  tail. `--report DIR` prints the table and writes the JSON again from a
  run's saved directory, such as the one saved from a cancelled job.
- Here it runs through `scripts/heavy`. In an accl job, `scripts/heavy`
  passes through, and the allocation caps the memory.

Per rebuild it records:
- the ms (rebuild, link), which depend on the machine's load;
- the counts, which do not: steps run, commands, reads checked, readers
  marked, definitions changed, and records made (summed over the
  rebuild's routines line);
- `instructions:u`, the user-space instructions of the phase.

For the process it records the cold build's ms, commands, records and
instructions, the peak RSS (GNU time inside the sandbox, around the
process), and whether the final outputs are the cold build's.

**How the instructions are counted by phase.** The user asked for
numbers that do not depend on the load. A `perf stat` counts a process,
not its phases, so each phase gets its own count:
- *The cold build*: a `perf stat` around the process, with a control
  FIFO. The first rebuild line sends `disable` and waits for the ack, so
  the count ends where the cold build's link ended.
- *Rebuild K*: its line makes the edit, then starts
  `perf stat -p <the process> -D -1` with a control FIFO of its own. It
  sends `enable` and waits for the ack before it returns, so the count
  starts before the rebuild does. The next line stops it with SIGINT.
  The next line's shell is the process's child, made while the phase's
  counter was on, so a phase includes that shell, about a million
  instructions.
- *The last measured rebuild*'s counter is stopped by one more line. Its
  rebuild, with no edit, is not reported. A perf still attached when the
  sandbox ends would be killed before it wrote its count.

**The final outputs against the cold build's.** Every edit is reverted,
and the warm `word` pairs leave the text as it was. With one pass per
rebuild, an `.aux` that a `\label` or `\footnote` changed has settled
again long before the end. So the final PDF, `.aux`, `.toc` and `.out`
must be the cold build's byte for byte, and the log must be the same
with the statistics masked. That is an exactness check on the course,
and a cheap one: it needs none of the oracle's plain builds of a minute
each.

**The format** is made by the binary measured, at the jobs' fixed time,
with the course's recipe (`-ini -jobname=pdflatex
-translate-file=cp227.tcx *pdflatex.ini`). It differs from the course
copy's `_fmt/pdflatex.fmt` in 2 of its 15,551,983 bytes: the dumped
`\time` (630 minutes against 419). TeX sets the date and time again when
a job starts, after loading the format (§1337, `fix_date_and_time`), so
the outputs are the same. The fixed time makes the format reproducible.

**accl.** The machine's load made local timings meaningless, and the
heavy lock was held by three other agents' course runs, so the baseline
ran on the accl cluster:
`scripts/accl/accl run cmd sh -c 'bench/ssa-course.sh "$w/course" "$w/aux"'`.
In a job, the course is `$w/course` and its aux files are `$w/aux`.
job.sbatch's `cmd` task copies them there, as its `course` task does
(c22e989). Before, it copied neither, and the container cannot see the
home directory. The JSON comes back in the run's `bench-results/`.

**A bug the first, stopped run found.** I killed a run while it waited
for the heavy lock. It reported the peak RSS of the run before it, read
from a stale `time.txt`. The outputs of an earlier run are now cleared
first, and a run whose cold build did not finish writes no JSON.

**The gate** (local, at 36359a8): `cargo xtask check` passes. e2e is
34/34 identical, so is e2e in machine mode, and `ssa-edits/*` is 10/10
inside the gate (75 s under load 23–35).

**The course on main's engine** (cb30e2e; accl, acclnode01, 16 CPUs
allocated, quiet; release binary built in the job; the course's edits in
their file order, before the warm pairs moved first). The job was
cancelled during rebuild 8, the footnote's revert, when the user asked
not to wait for the reflow edits. The numbers so far are in
`bench/results/baf5ad6-ssa-course-acclnode01.json`. The cluster image
has no `perf`, so there are no instruction counts.

| rebuild | ms (rebuild, link) | steps | commands | reads checked | readers marked | records made |
|---|---|---:|---:|---:|---:|---:|
| cold build | 120,009 (link 35) | | 66,063,925 | | | 1,609,529 |
| space | 51.1 (46.6, 4.5) | 2 | 1,903 | 3,019 | 0 | 15 |
| space, reverted | 31.1 (26.7, 4.4) | 2 | 1,903 | 3,019 | 0 | 10 |
| word | 116.0 (100.8, 15.2) | 6 | 4,450 | 6,599 | 32 | 49 |
| word, reverted | 75.8 (63.4, 12.4) | 6 | 4,450 | 6,594 | 32 | 12 |
| label | 9,675.6 (9,669.8, 5.8) | 244 | 3,821,794 | 719,419 | 884 | 2,073 |
| label, reverted | 10,496.0 (10,489.4, 6.6) | 244 | 3,822,938 | 721,792 | 887 | 1,331 |
| footnote | 584,869.3 (584,846.1, 23.2) | 851 | 23,604,036 | 1,902,564 | 7,734 | 291,109 |

- *Memory.* GNU time wrote nothing, since the job was cancelled.
  Slurm's sampled accounting of the batch step gives MaxRSS 18.4 GiB
  (19,253,632 KB) and MaxVMSize 18.3 GiB, over the cold build and
  rebuilds 1–8. That is under `scripts/heavy`'s 40 GB cap, and down from
  24 GB before 1ffe4d4.
- *The word edit* has LOG 2026-09-29's counts: 6 steps, 4,450 commands,
  6,599 reads checked, 32 readers marked. On a quiet node it takes
  116 ms; DESIGN 4.3's target is 16.7 ms.
- *The label* re-runs 244 steps and 3.8 M commands; DESIGN 4.3's target
  is 50 K. That is the step grain the `windows` agent replaces.
- *The footnote* re-runs 851 steps and 23.6 M commands (36% of the cold
  build's), ships 136 pages again, and makes 1.2 M `unsave` calls (914 K
  of them applied hits, 284 K records). It takes 585 s, almost five cold
  builds. That is 24.8 µs a command, against 2.5 µs in the label's
  rebuild and 1.8 µs in the cold build: something in the rebuild path
  costs more per command as more is re-run. Not profiled; for
  `rebuild-cost`.

## 2026-10-02 — The link costs the changed chunks: an offset tree, files written from the first changed byte (agent link)

DESIGN 4.3 item 4. The SSA build's link (`effects::link_cached`, LOG
2026-09-29) laid every file out from every live chunk at each rebuild:
the course's word edit linked in 12–14 ms, 6.3 ms of it its own work
(the layout 4.7 ms, the cross-reference section 3.2 ms of that, the
copy into the files' buffers 1.4 ms) and 5–9 ms writing 3.3 MB (the PDF
and the log, whole). Two more walks were outside its clock:
`ssa::step_effects` gathered the 3,427 chunks from the 58,710 steps,
and each file was hashed whole to see whether it changed.

**The form** (DESIGN 3.8, "The link costs the changed chunks", written
before the code):
- The runtime logs the steps whose chunks changed since the link last
  took them: a run closed (`step_closed`) or the step left the fold.
  `ssa::take_step_changes` hands them over with their keys and chunks.
  That is two pushes in `ssa/rebuild.rs`, `Steps::fx_changed`, and no
  walk of the steps.
- `effects::Splice` keeps the last layout:
  - the chunks in program order, each with its pieces of each file and
    its objects' marks at their offsets in the chunk;
  - a Fenwick tree per file over the chunks' lengths;
  - each object stream as rendered, with its objects;
  - the cross-reference sections and byte counts.

  A step with as many chunks as before has them replaced in place
  (`O(log n)` a file). Chunks inserted or removed make the order and the
  trees again in `O(n)`, without reading any other chunk's effects.
  Then the object streams a change reached are rendered again: those a
  new chunk closes, and the first one closed after each chunk put in or
  taken out. The cross-reference section is rendered from the trees
  when its file changed at or before it, and the byte counts with the
  files' new lengths.
- Each file comes out as its pieces, with its length and its first
  changed byte: the least prefix sum over the chunks whose bytes or
  marks in it changed. A chunk put in with the same bytes and marks does
  not count, so a step that changed only the log leaves the PDF.
- The CLI writes a file from that byte on (seek, write, `set_len`) when
  the file is as the link last wrote it: the same length and
  modification time, and no step opened it since (an open empties it).
  Any other file is written whole. Nothing is hashed.
- Deflate stays memoized by content, now keeping what the last eight
  links used: an edit undone finds its streams.
- The diagnostics, pages and closes are walked when a host wants them,
  not copied each link.
- "The edited page ready" is when the changed chunks are placed: their
  bytes and offsets are final, so the edited page's content stream is
  there. That is before the object streams, the cross-reference section
  and the byte counts. The rebuild line now reads `X ms (the rebuild R
  ms, the link L ms), the edited page ready P ms after the rebuild,
  files written W ms`. L is the link without the disk, and X = R + L +
  W.
- `PARTEX_LINK_SPLICE=0` links in full and writes every file, as
  before. A debug build, or `PARTEX_LINK_CHECK=1`, checks every link
  against a full one: files, terminal text, opens, closes, diagnostics,
  pages, and the chunks in order against `step_effects`. Virtual object
  numbers (machine mode) fall back to the full link. Machine mode keeps
  `link_cached`.

**Tests.**
- Unit tests: the Fenwick tree; a page edited (one chunk, the PDF
  changed from the page on); pages added and removed; a chunk that
  changed only the log, which leaves the PDF unchanged. A randomized
  test makes 300 random edits of a document (pages changed, added and
  removed, the object streams' sizes changed) and checks every link
  against a full link.
- `scripts/ssa-edits`: 10/10 cases, 61 stages, byte-identical to plain
  partex, in release and in debug (where every link is also checked
  against a full one).
- The course on accl (below): every stage byte-identical, PDF and
  `.aux` by `cmp`, log with the statistics masked.

**The course** (`bench/link-course.sh`, new: the word edit and its
revert alternated, 10 rebuilds in one SSA process, each stage compared
with plain partex on the same source; base c22e989 and this branch
built back to back on acclnode01, 16 CPUs allocated, load 3.5; release
binaries):

| per rebuild | base (c22e989) | now |
|---|---|---|
| the link's own work, rebuild 1 | 6.3 ms (layout 4.8: xref 3.2, object streams 0.6; copy 1.3) | 3.39 ms (deflate 3.34: the xref and one object stream) |
| the link's own work, rebuilds 2–10 | 6.1–6.8 ms | 0.18–0.19 ms |
| files written | 2 whole, 3,312,090 bytes, 5.1–9.1 ms | 2 from a byte on, 1,598,028 bytes, 0.60–0.76 ms |
| the link in the rebuild line (all of it, writes included) | 12.3–16.3 ms | 0.8–4.2 ms |
| the edited page ready | (not measured) | 0.02 ms after the rebuild |
| the rebuild itself | 115.9, then 63.9–67.6 ms | 115.6, then 61.7–66.2 ms |
| cold build (wall, max RSS) | 124.6 s, 15.0 GB | 124.8 s, 15.0 GB |
| cold link (own work; files written) | 23.9 ms; 10.8 ms | 29.0 ms (deflate 21.8: 44 object streams and the xref); 5.8 ms |

The first rebuild's link is the deflates. The word edit moves every
object after page 156, so the cross-reference stream (5,104 entries,
25,520 bytes, zlib level 9) is new. Of the two object streams rendered
again (40,758 bytes), the one holding the page's annotations changed
too. The memo has neither yet. From the second rebuild on, the edit and the
revert alternate, so every stream was compressed two links before and
the memo returns it: 0.19 ms is the link without any deflate. A new
edit pays the deflates every time. That is the zlib port's work, a
separate delivery: zlib's deflate in Rust, byte-identical, its matches
computed in parallel and kept before the first changed byte. The disk
write is from page 156 on, half the PDF: 1.6 MB in 0.6–0.8 ms.
## 2026-10-02 — The build printed as a program (agent view)

DESIGN 4.3 item 7: `PARTEX_SSA_VIEW=FILE` writes the build as a
`phitex-ir` program after the cold build, and to `FILE.N` after rebuild
`N`; `PARTEX_SSA_VIEW_STEP=ID` adds step `ID`'s per-call trace
(`FILE.stepID`). Commits on `np/view`: f7c9201 (phitex-ir), ecd7cd8,
29958fb, 025a77b, 2ac1f1e and this entry's.

**What was built.**
- `phitex-ir` is `no_std` + `alloc` now, so `partex-core` builds the
  `Program` itself (`ssa/view.rs`) and the CLI only writes its text; the
  other way, the core handing raw rows to the CLI to render, would have
  put the naming (which needs the engine's hash, fonts and eqtb layout)
  outside the core. Two forms were added, keeping the round trip: an
  operation that defines names, `op(operands; names)` (`Def::Op`), and
  an operand that imports a name from a value, `name=%n`
  (`Operand::Named`). A name is written bare unless a character in it
  would end it, then quoted as a literal. `check` now also verifies that
  each name imported from an operation is one it defines. Tests: the
  `PhiTeX` forms still round-trip, operations with odd names round-trip,
  `check` rejects an import a window does not define.
- The view (`ssa::view`): `%0 = format` is the job's start (the step
  that loaded the format: its record writes every eqtb slot, 632 K
  definitions on plain, which as a window took 8.2 MB of a 8.5 MB view
  of `incr.tex`; as a constant it is the format's definitions, and also
  stands for what no step defined); a `file NAME` constant per file;
  then `window(imports; exports)` per live step in fold order. Imports
  are the step's reads from outside it, each named, from the window
  `Fold::reaching` gives (or `%0`); the source is imported by runs of
  lines from the steps' line reads (`Steps::line_runs`, what a rebuild
  seeds from) and from where the previous step left the input
  (`Steps::ended_at`). Exports are the records' writes. The comment
  carries the step id, `(run N)` once a rebuild ran it again, its
  spans, the pages it shipped (`[3]`, from the log bytes of its
  `ship_out` records) and a text excerpt (the shipped page's nodes it
  read, else the page nodes it wrote, else what it added to its list,
  else its source).
- Names: a parameter is written without its escape (`hsize`), because
  `\hsize` is also the control sequence, whose meaning is another
  address: with both written `\hsize` a window imported `\hsize` twice.
  Frozen control sequences, font identifiers and pdfTeX's primitive
  copies take a prefix for the same reason.
- `Runtime::trace_of(recs, name)` (partex-ssa) prints one step's
  records with the trace's `to_text`, addresses named as in the view.
- A golden test (`crates/partex-core/tests/view.rs`): a small INITEX
  document built in SSA mode, its view's window-to-window edges, one
  step's named trace, then a word edited and rebuilt: what changed. It
  is an integration test, a process of its own: an SSA build turns on
  the boxes' versions for the whole process (`node::VERSIONS`), and as
  a unit test it raced machine mode's
  `the_page_leaves_rest_as_a_cell_of_its_own`, which hashes the page
  twice and saw the switch flip in between (the first `xtask check`
  failed on it).

**Measured** (release, sandboxed, this machine under load 15–25, so
instructions rather than ms):
- `incr.tex` (plain, e2e): 92 values, 225 KB; the cold SSA build is
  2.432 G instructions without the view and 2.471 G with it (+1.6%).
- `scripts/ssa-edits --env PARTEX_SSA_VIEW=../view.txt --env
  PARTEX_SSA_VIEW_STEP=3`: 10 of 10 cases identical, 61 stages, every
  view checked. `edits.tex` (pdflatex, `machine_edits`): 2,840 values,
  8.3 MB a view.
- After `edits.tex`'s first edit (`Para5x2 ` typed into), the views
  differ in exactly three windows, the three steps the rebuild ran:
  `step 2248 (run 2): edits:80-81 "Para5x2 TYPED sigma delta lambda
  macro"`, the fire `step 2255 (run 2): ships [9]`, and the job's end.

**Found.** The view shows where a rebuilt graph differs from a cold
build's. Fonts and hyphenation are not placed (DESIGN 3.15), so a
paragraph run again reads the interword glue and the packed patterns
the cold build computed later, writes them no more, and the next
reader's import moves to an earlier window (`font:cmr10.glue=%12`
becomes `=%5`, `hyph.patterns=%12` becomes `=%0`, the golden test pins
it). Harmless for the output (both are functions of what they were
made from), but a cold build and a rebuild do not have the same graph
there. And the pool's string numbers differ after a rebuild (allocated
numbers are not positioned, 3.2): the golden test writes them
`string:N`.

**Limits.** The per-call trace is the step's records as kept (each call
marked `new`); once records are lean (item 2) it needs the window run
again with the full recorder, not built. Value numbers are positions: a
step inserted by a rebuild shifts every later `%n`. A `Line` read (a
line number, relative to the step) is shown by its raw slot
(`line:1+5`).

## 2026-10-02 — A group's end is not a call; records for steps and typesetting calls only (agent records)

Commits 3b410c5 (item 1) and the one after it (item 2), DESIGN 4.3 item 2.

**What changed.**
- `unsave` (§281) is not a call: its restores are writes of the step
  that runs it. `Func::Unsave`/`UnsaveAfter` go with their records,
  frames, probes, applied hits and chunk cuts; `\aftergroup`'s tokens
  come from the step's own run, so no hit can lose them.
- Outside check mode only the steps and the pure typesetting calls
  (`line_break`, `hpack`, `vpack`, page steps, `ship_out`, the fonts)
  get frames and records (`Func::recorded`); `tokenize`, `write_out`
  and the output routine run in the frame around them. A step's frame
  (`Runtime::begin_lean`) keeps no own reads and no `Item::Wrote`: a read
  in it is only the open step's read from outside it (the fold's
  readers, as before), and no version is made for it. The tokenizer's
  code reads stay the step's reads (a lean frame or the root notes them
  as the step's). `PARTEX_SSA_LEAN=0` records as before.

**Tests.** scripts/ssa-edits: 10/10 cases, 61 stages identical after
each commit, plain and `--check` (the check reports' uncovered parts as
on main; fewer hits checked), and `PARTEX_SSA_APPLY=0`; the per-stage
steps and commands are main's.

**Measured** (a 3-chapter copy of the course: ch00, ch01, ch15; 8.8 M
commands, 49 K steps; `perf stat -e instructions:u`, `/usr/bin/time`,
local machine under load 12-20, so wall times are only indicative):

| build | instructions:u | / plain | peak RSS | minor faults | records |
|---|---|---|---|---|---|
| plain (main) | 42.09 G | 1 | 157 MB | 23.6 K | — |
| SSA, main (cb30e2e) | 141.24 G | 3.36 | 3.06 GB | 576 K | 223,943 (157.6 K unsave) |
| SSA, item 1 (3b410c5) | 120.19 G | 2.86 | 1.57 GB | 270 K | 66,307 |

Item 2 was not measured yet (the user's tokens ran out). What remains
for the 1.5x / 4 GB targets, from perf on main's binary (self time):
`Fold::close` 5-7% (the readers' and definitions' sorted inserts do a
binary search per entry; a cold build only appends), the tracker's read
path ~13% (`read_content`, `table_at` three times per read, `stamp`),
token-list versions ~4% (`TokenList::remake`), page faults ~3-5%, and
hashing at every eqtb write (`wrote_eqtb`), which a lazy version made
at a step's end or a recorded read would cut. Tokenize's record was
dropped with item 2; its rebuild numbers against the old record are
still to be compared on the course's word edit.

## 2026-10-02 — The job's end runs only if a font's union of glyphs changed (coordinator)

**Why.** A profile of the course's warm word edit (TODO 3: 80 edit and
revert rebuilds in one process, a frame-pointer build, `perf record
--call-graph fp` attached after the cold build) put 22% of the rebuild
in the job's end: the object streams, the cross-reference stream, the
name tree and the outlines, made again on every rebuild. The rebuild
trace says why: the page's ship writes its `glyphs:N` row, and the job's
end reads every ship's row to make each font's union. "expensive" →
"dear" changes the page's glyphs, not a font's union (the font files
were all hits), so the end ran for nothing.

**What.** An early cutoff on the union (DESIGN 4.3, "The job's end").
`glyphs_union` tells the recorder the version of the union it made
(`Tracker::glyphs_united`, kept in `Steps::glyph_union`); `pdf::ship::
glyph_union` makes the union for both. When a step closes with a ship's
glyph row changed, the rebuild makes the union again from the ships'
latest rows (`glyph_union_now`) and marks the end only if its version
differs. `scripts/ssa-edits`' `machine_edits` gains letters no page had
("QJZ"), then their removal: the end must run for both, and the font
subsets grow and shrink as plain partex's do.

**Tests.** `cargo xtask check` passes: ssa-edits 10/10 cases, 63 stages
identical (`--fixpoint`, `PARTEX_SSA_TRIPS=1`, and `--fixpoint
--check`), e2e 34/34 plain and machine mode, trip and etrip. In
`machine_edits` the digits edit now runs 2 steps (586 commands) where
main ran 3 (593); the "QJZ" edit and its removal run 3 on both.
`tests/view.rs` failed on main since the records merge (its step trace
prints a step's own reads, which lean records do not keep): it builds
with the full recorder now (`set_lean(false)`), as TODO 8 said.

**Measured** (the course on accl, acclnode01, job 6318 against 6317 on
main; final outputs equal to the cold build's):

| | main (6317) | this (6318) |
|---|---|---|
| word, median of 3 warm | 49.3 ms (rebuild 48.5) | 37.9 ms (rebuild 37.2) |
| word revert, median | 49.8 ms | 34.8 ms |
| word: steps, commands | 6, 4,450 | 5, 4,443 |
| label | 6.90 s | 6.62 s |

## 2026-10-02 — Typing on the course: 41.3 → 30.6 ms; TikZ edits measured (coordinator)

**The bench measures typing.** `bench/ssa-course.sh`'s warm word edits
put a different word in each time: the page and what the link
compresses were never made before. Alternating one edit with its revert
let the link find every stream it compresses in its cache (0.1 ms); a
new word costs it 3.1 ms. Three edits in ch15's pgfplots figure join
`bench/edits/course.txt` (`tikz-node`, `tikz-coord`, `tikz-plot`).

**Changes** (each measured on accl, acclnode01, the warm word edit's
median of 10, final outputs equal to the cold build's):

| change | job | word edit (rebuild) |
|---|---|---:|
| main (edd563d), typing | 6319 | 41.3 ms (37.5) |
| a data an edit replaced, no line of it read, not diffed again; inotify | 6320 | 37.6 ms (34.4) |
| a rerun step that read what its last run read keeps its reader entries | 6321 | 36.0 ms (32.7) |
| a read finds its table index once; `table_at`, `stamp`, `read_slot` inline | 6322 | 33.4 ms (29.8) |
| the watcher precise per name (`PARTEX_INOTIFY=0` on the same binary: 33.4) | 6324 | 30.6 ms (28.5) |

- The diffs: every data a file was ever loaded with was compared with
  its new text at each rebuild, so a session's rebuilds grew with its
  edits. A data an edit replaced has its readers moved to the new one
  (`seed`), and an input state that held it is mapped through that edit
  (`InputState::mapped` applies the edits in order), so once no line of
  it is read it is skipped (`Data::superseded`).
- The watcher (`crates/partex-cli/src/inotify.rs`): the loads'
  directories are watched; a load none of whose names (its lookup's
  candidates, its file) had an event since its last check needs no
  `stat`. Per name, because every lookup tries `.` first and an edit in
  `.` otherwise sent all 847 loads to their stamps. A first version took
  events per directory for all loads at once and lost them for loads a
  later trip did not ask about (the `incremental` case's `.toc`, under
  load): each load now keeps the count it was checked at.
- The fold: reads have a run counter of their own (`Step::rrun`), and a
  rerun whose reads are the last run's, in order, keeps their entries.
- LTO (`CARGO_PROFILE_RELEASE_LTO=fat`, one codegen unit; job 6323, on
  6322's tree): the word edit 33.4 → 30.9 ms, the cold build 73.9 →
  64.6 s. Not made the default: release builds get slower; the user's
  call.

**TikZ** (job 6319): an edit inside ch15's figure (a float with one
tikzpicture of two pgfplots axes) takes 1.18 s: the figure is one step of
995 K commands (steps are cut only outside boxes), all run again at about
the cold build's speed. pgfplots reads its 11 CSV files, parses each
number with pgfmath, and draws every plot at `\end{axis}`. Windows inside
boxes (DESIGN 4.3 item 1) would spare what comes before the edit and,
perhaps, the other axis.

**Profile after these** (local, a frame-pointer build of 210c870, 80
rebuilds of new words): TeX itself 30%, the tracker 21%, the rebuild's
other bookkeeping 19% (`run_step` 4%, positioning and restoring slots,
`Fold::latest`), the page's deflate 12%, the input check 7% (before the
per-name watcher), `Fold::close` 5.5%, the link 5%.

## 2026-10-02 — How much of a build could run at once: the step graph measured (agent parallel)

**Why.** DESIGN 3.10 runs the evaluation on many threads, and DESIGN 6
("Serial fraction") asks for the parallelism to be measured per document
before any claim. Nothing measured it: the view (4.3 item 7) prints each
window's imports, but not what each step costs or when in it a read is
made.

**What.** `PARTEX_SSA_DAG=FILE` (DESIGN 3.14) writes the live steps after
each build as a graph (`ssa::dag`, beside the view): per step its last
run's commands (`RecState::step_commands`, set where the step's call
closes); per read from outside it, the step whose definition reached it
(`Fold::reaching`) and how many commands into the step it was made; per
definition, its version and how many commands into the step it was last
written. The times come from the runtime's clock (`Runtime::set_timing`,
`set_clock`, `open::StepTimes`), which `SsaTracker::command` sets at each
command only when the dump is asked for: without it nothing is timed. A
first version kept the write times in one table cleared at each step;
the format's step grew it to 2 M slots, and clearing and walking it at
each of the 58 K steps took the course build past 13 minutes: the table
is made afresh per step. `scripts/ssa-parallel.py` reads the dumps:
`cold DAG` for one build, `rebuild DAG.K DAG.N` for what a rebuild ran
(the steps whose run changed). Two schedules, each with a worker per
step and the step boundaries taken as known (found by running, they
chain every step to the one before it): *whole steps* (a step begins
when every step it read from has ended) and *pipelined* (a step may
begin earlier, each read waiting for its definition's last write). Four
models of the reads: as recorded; *soft definitions* (a definition equal
to the one before it is passed through to that one); *soft reads* (the
step's own read of such an address dropped too: the save and restore of
TODO 2); *blind writes* (a step's read of any address it defines dropped:
an upper bound for 3.2's scoped definitions, which read no old value).
For speculation, a re-run step *validates* if every address it read
finds the same version at its place in the graph before the rebuild (a
definition that vanished because a re-run does not allocate again, a
font loaded or a name entered, counts as the same).

Outputs are unchanged: `scripts/ssa-edits --brief --fixpoint`, 10/10
cases and 63 stages identical, with the dump off and with it on
(`--env PARTEX_SSA_DAG=...`).

**Measured** (the course, local release build of np/parallel on edd563d,
one SSA process: the cold build, then `word`, its revert, `tikz-node` (a
node of ch15's pgfplots figure, from the coordinator's course edits),
its revert, `label`, its revert; counts, not times):

| cold build | |
|---|---|
| steps, commands | 58,710, 66,063,925 |
| setup (before the first ship) | 51,911 steps, 4,271,144 commands (6.5%) |
| body | 6,799 steps, 61,792,781 commands |
| reads from outside a step, definitions | 9.30 M, 2.87 M |
| definitions equal to the one before them | 1.07 M of 2.23 M (macros 660 K, save stack 172 K) |
| costliest steps | 5.31 M (ch24's figure), 4.27 M (ch07's), 3.87 M (ch04's), 3.57 M (the cover) |
| no edges at all | bound 12.44x (the costliest step) |

| critical path, bound | whole steps | pipelined |
|---|---|---|
| all reads | 66.04 M, 1.00x | 65.93 M, 1.00x |
| soft definitions | 66.01 M, 1.00x | 65.83 M, 1.00x |
| soft reads | 66.01 M, 1.00x | 65.79 M, 1.00x |
| blind writes | 62.39 M, 1.06x | 57.68 M, 1.15x |

- With all reads, 39,515 of the 58,710 steps are on the critical path.
  The mean number of steps running is 1.0 in every tenth of it; the widest
  depth is 2,343 steps, all cheap; list schedules on 2, 8 and 64 workers
  give 1.00x. The body alone is 1.00x in every model but blind writes
  (1.06x whole, 1.15x pipelined).
- What carries it (all reads): `align_state` is on 37,981 of the path's
  39,514 edges, `cond` 18,096, `str_ptr` 13,532, `hash_high` 13,241,
  `selector` 8,159, `\reserved@a`–`d` and `\@let@token` (written by every
  `\@ifnextchar`, read to be saved), the save stack (`save.ptr`, `level`,
  `group`, `boundary`), the nest (`list.*`). Before the expensive steps
  (by their commands): `align_state`, `cond`, the save stack and the nest.
  No family alone: keeping only the page builder's edges still gives
  1.01x, and ignoring classes one after another, the one that helps most
  first, reaches 1.01x after eight (engine scalars, conditionals,
  allocators, macros, log, hash, save stack, nest).
- Pipelined, soft reads: the binding reads before 47.6 M commands are of
  `\reserved@a`, then `list.list`, `save.ptr`, `cond`. The figures' steps
  read `\reserved@a` and `\@currenvir` at their first command, `\begin`'s
  local `\def`s, from the step before, which wrote them at its end. The
  body runs inside the `document` environment's group (`\document` does
  not close the group `\begin` opened, and `\end{document}` ends the job
  inside it), so every local assignment in the body saves the value
  before it, a read, and that saved value is never restored. Blind
  writes drop those: then `\if@nobreak` (42.5 M), `pdf.stacks`, `\count0`,
  `clubpenalty`, `page.goal` bind.
- With blind writes the page builder stops binding once pipelined: its
  edges alone give 12.44x (whole steps 10.36x), and with the nest's and
  the counters' 10.22x. Ignoring classes one after another (pipelined,
  blind writes): the macros 1.96x, then the PDF writer's tables 2.84x,
  the registers 3.84x, the parameters 4.86x, the hash 8.30x, and only
  then the page builder 12.43x.

| rebuild | steps, commands | dependency chain | bound | speculated: validate | bound | cold build from the last records: validate | bound |
|---|---|---|---|---|---|---|---|
| word | 5, 4,443 | all 5 | 1.00x | 2 of 5 (1,531 commands) | 1.15x | 99.995% | 12.44x |
| tikz-node | 4, 995,102 | all 4 (the figure step 991,637) | 1.00x | 2 of 4 | 1.00x | 99.997% | 12.44x |
| label | 247, 3,812,865 | 232 | 1.00x | 4 of 247 | 1.00x | 99.586% | 8.43x |
| label, soft reads | | 220 | 1.01x | 146 of 247 (3.03 M commands) | 3.63x | 99.828% | 11.99x |
| label, blind writes | | 137 | 1.55x | 181 of 247 | 3.63x | 99.888% | 12.44x |

- `word`: the `\input{ch15}` step and the edited paragraph validate; the
  steps after them read the paragraph's page nodes (`page[44]`, the line
  that changed) and fail, rightly. `tikz-node` is one step: the figure
  float is a single step of 991,637 commands.
- `label`: the first read that differs is `\@savsf` (`\count146`) for 196
  of the 247 steps: every output routine saves and restores it, and its
  definition is a new one each time (TODO 2). With soft reads the rest are
  `pdf.fonts` (31; the PDF writer's fonts and their object numbers,
  numbered by count), `font:…expand` (44; pdfTeX's expanded instances of a
  font, made on demand by the paragraphs that use them, so every such
  paragraph reads and rewrites the font's set), the 12 new steps (the
  `.aux`'s new lines) and `str_ptr`.

**What it says.**
- A schedule that waits for dependencies gains nothing on the course: at
  the step grain the build is one chain (1.00x), it stays one when steps
  overlap at their reads and when the save-and-restore reads are dropped,
  and dropping every read of what a step itself defines gives 1.15x. The
  chains are not the page builder's (pipelined, it allows 12x) but the
  state every LaTeX step touches near its start: the `\@ifnextchar`
  scratch macros, `\@currenvir`, `\if@nobreak`, `clubpenalty`, the
  conditionals and `align_state`, the save stack, the allocators.
- Speculation is what parallelizes, as 3.10 says: run from the last
  build's entry states, 99.6–99.995% of the steps validate after these
  edits, and the bound is the costliest step's, 12.4x. The four largest
  steps (pgfplots figures and the cover, 3.6–5.3 M commands) are 26% of
  the build; past 12x they must be cut into windows (4.3 item 1), each
  speculated too.
- For validation to hold, a version must be the content: `\@savsf`'s and
  the save stack's restores (soft reads, TODO 2; 4 → 146 of 247 steps on
  `label`), `font:…expand` and `pdf.fonts` (a cache and an object
  numbering read and written as values), and definitions that a re-run
  does not make again (fonts, names) are each a validation failure or a
  false wake-up.
- A save in a group that the job ends inside (the body's `document`
  group) is never restored, so it need not be a read at all; with 3.2's
  scoped definitions no save is one.
- A first cold build has no records to speculate from: its step
  boundaries and entry states have to be predicted (a session's last
  build, or the syntax tree's paragraphs). The setup's 51,910 steps
  before the cover's average 14 commands: they are better run as one span
  than spread over workers.

## 2026-10-02 — A step run again keeps its files' handles; an empty line's protrusion; trips that never settled; LTO the default (coordinator)

**A file opened again** (the Overleaf extension's feedback, item 2). An
edit in the first paragraph, which shares a step with `\begin{document}`'s
`\openout` of the `.aux` and whose page opens the PDF, re-ran every page
and every `\write` after it. Four causes, each fixed:
- A step that runs again opened its files on new host handles; the PDF
  writer's state (`pdf:4`) and the streams' versions hold the handle, so
  every later page and `\write` woke. The engine now asks the host to
  open the file on the handle the step's last run had
  (`Tracker::reopen`: the step's old chunks' opens, in order;
  `Host::open_write_again`, whose default opens anew; `NativeHost` gives
  the handle back if it gave it for that file). A first try, reusing a
  closed file's handle in the host, was wrong: the link keeps bytes per
  handle, so a file closed and written again in one build (beamer's
  `.vrb`) would get both contents.
- A rebuild restored the PDF writer before the first page from a fresh
  format engine (`initial`), made without `set_effects(true)`: its object
  streams were not symbolic, so its state hashed differently. The format
  value now gets the build's setting.
- A dropped run's opened files were never closed: the retry got new
  handles. Its outputs are dropped with their files closed
  (`drop_outputs`).
- `\jobname`, the log's and the output file's names are string numbers;
  the string pool is not put back, so a re-run made them again under new
  numbers and woke their readers (every DVI `ship_out` reads the output
  file's name). They are versioned by their characters.

`scripts/ssa-edits` has `aux_reopen` (`tests/e2e/reopen.tex`): the
first-paragraph edit went from 16 steps (4,165 commands) to 9 (1,374),
its revert from 16 (4,568) to 9 (1,777).

Left, measured on a small article in DVI mode (`\pdfoutput=0
\input{main}`): an edit that changes a page's DVI length re-runs every
later page's shipout. That is the format: each `bop` points back at the
previous one by its offset, and TeX's movement reuse (§611) depends on
the buffer's positions. PDF mode has no such chain (3 steps). And
LaTeX's `\input{main}` first opens `main` only to test that it exists
(`\IfFileExists`); that load counts as read whole, so any edit to the
file re-runs its step (the course's `\input{ch15}` step, 564 commands).

**The margin panic** (a crash the extension reported). With protrusion
on, a paragraph that ends with a forced break has an empty last line
(its `\parfillskip` pruned, §879); `post_line_break` took `line.len() -
1` on it. pdfTeX's `prev_rightmost` finds nothing there and puts no
kern in, and partex now does the same. The e2e `microtype` case has the
paragraph (identical to pdfTeX). The extension's acmart paper reached it
only in trip 2, with the bibliography.

**Trips that never settled.** The same paper's SSA build stopped after
5 trips, unsettled (6.0, 13.1, 0.5, 6.3, 6.9 s); plain runs settle two
runs after the bibliography. A `\write` line was stored to the file its
stream number last opened in any run (`out_addr`, the recorder's, never
put back), not to the file the stream stores to where the step runs: a
re-run step's `.aux` lines (biblatex's) went astray, every other trip.
Lines now go to the name the engine's stream holds (`streams.out_name`,
restored with the stream): 3 trips (5.4, 12.0, 0.5 s), as plain TeX
settles. The harness has `streams` (`tests/e2e/streams.tex`: one stream,
two files), which the binary before the fix fails. The trace now says
where a changed store first differs.

**LTO is the default** (fat, one codegen unit; the user's call). Release
builds take longer.

**np/parallel merged.** Its timing (`PARTEX_SSA_DAG`) costs nothing when
off. Job 6330, acclnode01, alternating, the course's word edit (median
of 10, rebuild in parentheses) and the cold build:

| run | cold build | word edit | revert |
|---|---:|---:|---:|
| without, 1 | 63.8 s | 31.9 ms (29.4) | 30.1 ms (29.4) |
| with, 1 | 63.9 s | 32.7 ms (28.9) | 28.3 ms (27.6) |
| without, 2 | 64.5 s | 32.9 ms (28.9) | 28.9 ms (28.2) |
| with, 2 | 63.3 s | 31.3 ms (27.9) | 30.0 ms (29.3) |

(The node was running the gate at the same time: compare the rows, not
the numbers with the last entry's.)

Also: with a host that cannot deflate, a stream the link compresses is
counted as zlib's stored blocks (what the link writes then), so "Output
written" gives the file's size.

The harness: 12 cases, 71 stages, identical with `--fixpoint`, with
`PARTEX_SSA_TRIPS=1` and in check mode.

## 2026-10-02 — A run that read a later definition stops past its budget (coordinator)

**The settle hang** (the Overleaf extension's: a one-page article in PDF
mode with no font map, an SSA build that never returned; plain runs end
with partex's fatal "PK fonts are not implemented"). The fatal error
comes in the last page's output routine, at `\end{document}`: the job's
last step, a fire, leaves `output_active` true in the arrays. Trip 2
reads the `.aux` trip 1 wrote, and the steps that read it are new (trip
1 found no file): predicted by a step whose reads did not include
`output_active`, the run read the arrays' value, the later step's, and
`\end` fired again and again. The miss check that drops such a run
(DESIGN 7.17.3, "A read resolves by prediction and validation") runs
only at the step's end, which never came.

A run is now watched once it passes its budget, twice its last run's
commands and 10,000: past it, main control asks the tracker at each
command (`Tracker::stop_due`) whether a read of the run so far found a
slot a later definition holds, not placed, and if so stops there, a
checkpoint. The rebuild ends the calls still open and drops the run as
before. Stopping at the first such read, without a budget, was
rejected: a dropped run finds all its misses in one run now (2 to 11
slots in the extension's article log), and stopping early would make
one retry per miss. Below the budget the check costs a compare per
command.

The extension's repro: 2 trips, settled, the fatal error as plain partex
gives it (the terminal output identical to a plain second pass; the
log's string counts differ, as after any dropped run, and are masked).
Steps 677 to 679 of trip 2 each stopped after about 10,800 commands,
then ran with the missed slots placed.

The harness has `fatal_end` (`tests/e2e/fatal-end.tex`): `\read16` in
nonstop mode at the shipout, a fatal error real TeX makes too, not
seeded, so that the `.aux` the first build wrote is read by its second
trip (or, one trip a build, by the next stage). The binary before the
fix times out in both modes. 13 cases, 74 stages, identical with
`--fixpoint` and with `PARTEX_SSA_TRIPS=1`.

Left: a loop that never reaches main control (pure expansion, `\def\a{\a}`
read from a stale meaning) is not stopped.

## 2026-10-02 — The page builder reads the nest; the nest placed whole (coordinator)

**The PGF manual's panic** (the Overleaf extension's: `pop_nest`,
"semantic nest", in the first rebuild after a cold build of one trip).
Reproduced on accl with `scripts/accl/tasks/pgfman.sh` (the manual from
`upstream/pgf`, the tutorial's "Karl is a math" made "maths"; job 6344:
cold build 419 s, 388 M commands, 7.1 GB), then locally on three of its
chapters (`\includeonly`: cold build 32 s).

The cold build of one trip finds no `.toc`; the first rebuild reads the
one it wrote, and the steps that set the table of contents are new,
each predicted by the old step after them, the `\tableofcontents` step
(DESIGN 7.17.3, "a new step's reads are predicted by the old step whose
text it runs"). That step had read `cur_list`'s twelve fields and not
the nest: `build_page` asks `nest_at(0)`, which with the nest empty read
the current level's fields and decided from the nest's length without
noting a read of it, and `contrib()` did the same. A new step beginning
at the fire that `new_graf`'s `build_page` deferred (§1091: the manual's
`\l@section` starts its line with `\leavevmode` after the section
penalty, where the page breaks) was placed in the paragraph's mode over
the arrays' nest, the job's end's, empty. Its run took the paragraph's
list for the contributions and, in the output routine, `end_graf`
popped a nest that was not there, before the miss check could drop the
run (738 of its 2,033 reads were of later definitions).

Two changes, each enough for the repro:
- `nest_at`, `nest_at_mut` and `contrib` read the nest whenever they
  decide from its length. `crates/partex-core/tests/nest.rs`: every step
  that runs the page builder reads the nest (the binary before fails it:
  a `\penalty` in vertical mode, a step of its own).
- The semantic nest is placed whole for every step run, as the save
  stack is: `cur_list`'s fields, the nest and the alignment state (20
  slots), where a later definition holds the arrays. A field placed over
  a nest that was not is a state no run makes; placing a slot at its
  reaching value is always right and costs a lookup.

The three chapters' rebuild now completes: 57 s against the cold
build's 32 s, 1,513 steps run, 814 runs dropped. Against plain partex
(one pass, the edit, a second pass) the PDF differs only in font
resource names: the rebuild keeps the cold build's font numbers, and the
table of contents now uses some fonts first (TODO 9). The slowness is
the prediction of new steps: a new table-of-contents line, predicted by
the `\tableofcontents` step, misses most of its reads (156 of 216) and
runs three times.

DESIGN 7.17.3's rebuild steps 2 and 3 have the nest and the budget stop
(the last entry's). The harness: 13 cases, 74 stages, identical with
`--fixpoint` and with `PARTEX_SSA_TRIPS=1`.

## 2026-10-02 — Loads by lines: a file only tested for, a file's end, a file read both ways (coordinator)

TODO 3 planned this to stop the course's `\input{ch15}` step re-running
(564 commands, the file "loaded whole" by LaTeX's `\IfFileExists`). It
does not do that: the kernel's `\IfFileExists` asks `\file_full_name:n`,
which reads the file's size with `\pdffilesize`, a read of the file
whole, and the step reads the size's digits (only whether they are
blank matters). What it does instead, each case in the harness's new
`inputs` case (`tests/e2e/inputs.tex`), which the binary before fails
at two stages:

- **A load says how it is read.** `\input` and `\openin` load by lines
  (`Tracker::load_lines`); `\pdffilesize`, `\pdfmdfivesum`, images and
  font files load whole (`Tracker::load`).
- **A file only tested for.** A file loaded by lines that no step read a
  line of, nor met the end of, and that nothing read whole: only its
  being there was read (`\openin` and `\ifeof`). An edit to it runs
  nothing again, and its contents are kept for the next check. The
  harness's probe edit: 1 step before, 0 after (0.3 ms).
- **A file's end is a read.** `input_ln` that finds no line records a
  read of no line at the end (`Tracker::eof_read`), which an edit that
  inserts there touches (`Edit::touches`). Before, a line appended after
  the last line of a file read by the `\input` primitive touched no
  recorded line: the rebuild ran nothing and the page missed the line
  (stage 2 differed, 0 steps run). LaTeX's `\input` hid this, as its
  `\pdffilesize` re-ran the step anyway.
- **A file read both ways.** A file read by lines whose contents changed
  also wakes its whole readers. Before, only the steps that read changed
  lines ran: a `\pdfmdfivesum` of an `\input` file printed the old sum
  (stage 4 differed).

The harness: 14 cases, 79 stages, identical with `--fixpoint`, with
`PARTEX_SSA_TRIPS=1` and in check mode.

## 2026-10-02 — A new step is predicted by the step just run too (coordinator)

A new step's reads were predicted by the old step after it, "whose text
it runs" (a paragraph split by Enter runs the old step's text). Where
the run reads text the old run never read, that step is unrelated: the
first rebuild after a cold build of one trip sets the table of contents
for the first time, and each new line, predicted by the
`\tableofcontents` step, missed most of its reads (156 of 216) and ran
three times. The step just run reads what the next line will, so it
predicts too: both steps' reads are placed (a slot placed at its
reaching value is always right; it costs a lookup), and the budget is
the larger of their last runs'.

Measured locally, the binary before and after, one-trip cold build then
one word edit:

| document | rebuild before | after | runs dropped |
|---|---:|---:|---:|
| article, 120 sections and their table of contents | 836 ms | 467 ms | 455 → 99 |
| PGF manual, three chapters (`\includeonly`) | 57.1 s | 34.6 s | 814 → 383 |

The same PDF before and after in both. The manual's rebuild still
exceeds its cold build (32.7 s): every page after the table of contents
moves, and a re-run costs more per command than a cold run (TODO 3).

The harness: 14 cases, 79 stages, identical with `--fixpoint`, with
`PARTEX_SSA_TRIPS=1` and in check mode.

## 2026-10-02 — A keystroke on a one-page document: the font map read once (coordinator)

The Overleaf extension measured about 170 ms a keystroke in wasm on a
one-page article with a TikZ picture (its `mock/tikz/main.tex`: 3 steps,
728 commands, 1 trip), and about 90 ms natively on its one-page article:
a cost per rebuild, not per command. Natively here (the quick build, no
LTO; a settled cold build, then a character typed and taken away, 100
times in one process): 2 steps, the paragraph (41 commands) and the
fire at `\end{document}` that ships the page (515), median 29.6 ms.

The profile (perf, frame pointers, 200 rebuilds): two thirds of it was
the font map. The only page is the first shipout, which reads the
default map (`pdftex.map`, 5 MB), so the step that ships it reads the
map again at every keystroke: the host read the file again, the tracker
hashed it for the load's version, `read_map_file` hashed it twice more
(the table's digest, the key of its warnings in the host's cache), and
the first lookup indexed its 42 000 lines again (`first_field` 22.8%,
`Version::of` 17.8%, the lookup's loop 10.8%, `StableHasher::of` 10.5%,
the index's inserts 4.2%). The course does not show it: only its first
page reads the map.

What the contents give is kept by their identity:
- the native host hands out the same `Arc` again for a file whose stamp
  is the one taken when it was last read (the stamps `unchanged` keeps);
- the tracker keeps the versions of the last 32 large contents loaded,
  by identity, the `Arc`s held so an address names one contents;
- the engine keeps the map's hash and its index (`fontmap::MapCache`, a
  cache like `cs_cache`, not state): one hash serves the digest and the
  warnings' key (`fontmap-warnings/2`).

Median 29.6 → 7.6 ms a keystroke (min 22.7 → 3.7 ms), the same PDF. A
host in wasm gets this if it hands out the same `Arc<[u8]>` for a file
that did not change. The harness: 14 cases, 79 stages, identical with
`--fixpoint`, with `PARTEX_SSA_TRIPS=1` and in check mode.

The extension's own host handed out the same `Arc` already; what it
still paid for was `Host::cache_get`, which it did not have: without
the warnings a full reading gave, `read_map_file` read all 42 000 lines
into the table again at each rebuild (its profile: `map_line`,
`read_map_file` and `scan_line`, 30%). With an in-memory cache it went
from 90 to 9 ms a keystroke natively and from 150 to 31 ms in wasm. The
engine now keeps those warnings in `MapCache` too, so a host with no
cache reads the map in full once per engine. Natively with the cache
turned off (`PARTEX_CACHE=0`): 76.6 → 6.2 ms a keystroke, the same PDF.
`Host::cache_get` says what it saves.

Also tried, and dropped: keeping the runtime's dense stamp tables from
one trip to the next (the serial going on, so every kept stamp is older
than the trip's frames, as zeros are), as `open::dense_at` was 11% of
the profile after the map. No change in the time a keystroke takes (600
rebuilds each, alternating: median 7.6 ms both, p10 3.9 ms both): the
cost is the accesses, not the allocation.

## 2026-10-02 — The recorder's stamp tables kept from one trip to the next (coordinator)

Each trip, so each rebuild, began with a new `open::Open`, whose dense
stamp tables (`dense` and `dpos`: 32 bytes of stamps and an 8-byte
position per slot, a vector per family, grown to the next power of two
past the highest slot touched) were grown and filled with zeros again.
One write of the newest string (the PDF file's name, which the step
that ships the only page makes again) grows the pool's table to its
highest string; a write of the eqtb entry of a name in the hash's extra
room grows the eqtb's past `eqtb_size`. On the TikZ article's keystroke
(`mock/tikz`), `perf annotate` put about 90% of `note_write_by`'s own
time in that fill loop. `Open::renew` keeps both tables and the serial:
a stamp left by an earlier trip is older than every frame and step of
the new one, as a zero is, and a write's entry is live only if its
frame, all of the new trip, holds that serial, so no test tells them
apart.

The keystroke (LTO, 300 rebuilds each, 3 × 100 alternating): median
4.5 → 3.2 ms, p10 3.6 → 2.9 ms, p90 8.1 → 6.6 ms, the same PDF. The
harness: 14 cases, 79 stages, identical with `--fixpoint`, with
`PARTEX_SSA_TRIPS=1` and in check mode.

The experiment the last entry dropped kept the tables in `Open::reset`
only, the path of a recorded trip (`trip_recorded`); a build's trips
start in `Runtime::open_trip`, which made a new `Open`, so it measured
an unchanged binary. Both go through `renew` now. Ruled out on the way:
page faults. The process takes none during a keystroke (its own minor
faults read from `/proc` around each rebuild: 0); `/usr/bin/time`'s
count includes the toggle script's Python processes.

Before this, 89289fe: wasm32 has no 64 × 64 → 128-bit multiply, and
`hash::fold`'s product called the compiler's `__multi3`, 4% of a
keystroke in the browser. It is now made from 32-bit halves there
(`mul_wide_limbs`, tested against `u128`'s natively). The Overleaf
extension measured the TikZ article's keystroke in Chrome at 31 → 20 ms
median (16.4–23.4 ms over 8 keystrokes).

## 2026-10-02 — The tracker's read tests inline (coordinator)

After the stamp tables, the keystroke's profile had the tracker's read
hooks as calls that did nothing: `Tracker::read` of an eqtb cell (its
version comes with `read_content`) pushed five registers to return at
once (1.4%), and `read_content`, `row_read`, `value_read` and
`value_wrote` paid a whole call to compare a slot's stamp with the call
generation, which is all a repeated read does (`read_content` alone
4.7% self). The test is inline now where the engine reads, and a read
or write it lets through is noted out of line (`read_slot_noted`,
`read_revision`, `value_read_noted`, `value_wrote_noted`).

Timings were too noisy to judge on the machine that day (the same
binary measured median 3.2 ms and then 4.7 ms an hour later). Counted
instead, over 200 keystrokes of the TikZ article in the process alone
(`perf stat -i`, so not the toggle script's children), twice: 2.141 G
→ 1.967 G instructions (−8.1%), cycles −3 to −3.5%, the same PDF. The
harness: 14 cases, 79 stages, identical in all three modes.

## 2026-10-02 — No `__multi3` per token in wasm (coordinator)

89289fe took the 128-bit product out of `hash::fold` on wasm32; two more
were left. `tok_poly_step` (a token list's polynomial, modulo 2⁶¹ − 1,
for every token of every list made: each macro argument is hashed as it
is made, `TokenList::remake`) and `pvec`'s `mulmod` multiplied two
64-bit numbers into a `u128`, which wasm32 has no instruction for. The
wasm32 assembly called `__multi3` once per token in `tok_version`'s
loop, in `TokenList::new`, `remake` and `shared`, and 20 times in
`pvec::Node<VNode>::fix` and `fix_last`. If V8 inlines that call, its
time shows up in the caller, which fits `macro_call`'s 12% self time in
the extension's wasm profile against 3.3% natively. Both now use
`hash::mulmod61`, which is the `u128` product natively and the 32-bit
halves of `mul_wide` on wasm32. Its test checks both products against
the `u128` residue: values do not change, so neither do versions. After
the change, partex-engine and partex-core have no `__multi3` call left
on a keystroke's path (two remain in the core's `strtol` and Type 1
parsing, for overflow checks). Natively the code is the same.

## 2026-10-02 — Sealed lines and a step before `\shipout`: the output routine stays put (coordinator)

A keystroke in the TikZ article ran two steps again: its paragraph (41
commands) and the fire that ships the only page (515), which is LaTeX's
output routine (`\@makecol`, `\@outputpage`, ltshipout's `\shipout`
with its hooks), about 5,700 lines of pdflatex's `\tracingcommands=3`
log. The page changed, so `\box255` read a new version, and so did every
command that moved it. The course's word edit is the same: its
shipout step was 2,844 of its 4,443 commands. The extension's wasm
profile put 89% of a keystroke in `run_step`, `ship_out` 3%.

The machine of §7.0 (`DESIGN_ARCHIVE.md`) had the answer: sealed lines
(`seal.rs`) and a boundary before `\shipout`. They are SSA mode's now:
- the line breaker seals each line: the box keeps its dimensions and a
  key, and its glue setting and list go to a table, each entry a slot
  (`Fam::Sealed`, `Row::Sealed`, versioned by the contents, placed as
  values: `SValue::Sealed`). The key is the step's id
  (`Tracker::step_salt`), the paragraph's count in the step (from 0 at
  each step's start, `seal_restart`) and the line's index, so each run of
  the step seals under the same keys, with no search for a free one;
- only what looks inside a line reads it (shipping out, `\unhbox`,
  `\showbox`, a display's width, `\leftmarginkern`);
- main control stops before a `\shipout`, and the next step begins with
  it (`CleanPoint::Ship`; the input state's `ship`; the step's name has
  a mark of its own, as a fire's has).

A word that leaves its line's dimensions changes the page box nowhere:
its lines are the same stubs. The output routine's step before the
`\shipout` reads what it read before and does not run again. The ship
step reads the changed lines and does, opening them from the table into
the box the routine left, which is placed as it was.

Check mode failed at first, in 7 of 14 cases: the pages after the edit
were not shipped. In check mode every routine has a frame, the output
routine's too, which now spans the ship boundary. The output routine has
no frame now in any mode (in lean mode it had none already). The view
names a step by the text it set, so it opens sealed lines from the last
definition of their slots; `tests/view.rs`'s cold build exports and
imports the sealed slots.

The TikZ article's keystroke (LTO, alternating 3 × 100): 2 steps and 75
commands (41 + 34) where there were 556, median 3.3 → 1.6 ms, p10 2.8 →
1.4 ms, p90 6.6 → 3.0 ms; over 200 keystrokes, 1.967 G → 0.723 G
instructions (−63%), cycles −55%; the same PDF. The harness: 14 cases,
79 stages, identical with `--fixpoint`, with `PARTEX_SSA_TRIPS=1` and in
check mode; the workspace's tests pass.

## 2026-10-02 — A step boundary after a file's size is read (coordinator)

The extension opened its document with `\input{main}` on the first
line, LaTeX's `\input`, and every keystroke ran again the job's first
step, 184 of its 259 commands: `\IfFileExists` asks
`\file_full_name:n`, which reads the file's size (`\pdffilesize`, a
read of the file whole) and only tests the digits for being blank; a
typed character changes the size. `pdflatex main.tex` opens the file
with the primitive, so the extension does that now
(`\csname @@input\endcsname{main}`): 2 steps and 75 commands a
keystroke, about 6.5 ms median in Chrome (5.9–9.1 ms) on its TikZ
article, under the 16.7 ms target.

A document's own `\input{chapter}` has the same lookup (the course's
`\input{ch15}` step, 564 commands). Main control now stops at the
command after one that read a file whole by name (`read_source` with
`lines` false: `\pdffilesize`, `\pdfmdfivesum file`, `\pdffiledump`,
`\pdfobj file`, an image), and the next step begins there
(`CleanPoint::Load`, like `Ship`: the input state's `load`, a mark in
the step's name). The step that read the size runs again; the rest of
the lookup, which sees only the name found, does not.

The extension's old first line natively (LTO, 200 keystrokes each, the
same PDF): the lookup runs 14 + 5 + 46 commands in three steps (`main`,
`main.tex` and once more) where it ran 184, 259 → 140 commands a
keystroke; 4.48 G → 2.00 G instructions (−55%), cycles −42%. Left: each
read of the edited file is data of its own, and a rebuild diffs five
of them. The harness: 14 cases, 79 stages, identical in all three modes;
the workspace's tests pass.

Also tried, and dropped: the check after a step's run asked
`Fold::latest` again of the reads its prediction had asked about (the
same slots, mostly in the same order, the fold unchanged in between);
reusing the first answers by position saved 0.9% of a TikZ keystroke's
instructions and no measurable cycles.

## 2026-10-02 — Pooled token lists carry no version (coordinator)

A pooled list (a macro's argument, tokens backed up or inserted:
`pooled_list`, `str_toks`) is an input level only. Every maker of one
puts it on the input stack, none is stored as a value, and the input
state is kept as shared chains, never hashed. Yet `TokenList::remake`
hashed its tokens for a version (2.4% self in a profile of a keystroke
that runs the output routine again, and dearer in wasm, whose 128-bit
product is made from halves). It makes none now. A list's `Hash` still
hashes the tokens when it has no version. Instructions, 200 keystrokes
of the TikZ article: −0.2% for a word, −2.3% for one that runs the
output routine again (a `(` changing its line's height); the same PDFs.
The harness: 14 cases, 79 stages, identical in all three modes.

The course A/B of the step boundary after a file read whole (433451e
against eb0085a, job 6383): no change, word edit 21.1 ms both (7
steps, 1,560 commands).

## 2026-10-03 — Keys that leave room, and a removed step's entries removed (coordinator)

The extension profiled edits to a section title (each runs the output
routine again and a second trip for the `.aux`): 57 ms a keystroke in
wasm, 41% of it in `rebuild` itself. Natively, `Fold::insert_after` was
54% of such a keystroke, all of it `Fold::renumber`. At the job's end
the `.aux` is read again, and its new `\@writefile{toc}` line makes the
step that reads it end elsewhere; its new steps went after it, before
the ones the last keystroke had put there (passed over after the run),
each a sixty-fourth of the gap from it. So the gap shrank sixty-four
times a keystroke, and every third one numbered the fold again: every
index entry rewritten (p90 59 ms, max 91 ms natively).

- The first new step after a step run again goes a sixty-fourth of the
  gap back from the next step; the later new steps of the run, a
  sixty-fourth on from the step before them, as before. The gap after
  the step run again now shrinks by a sixty-fourth a keystroke.
- Cold keys are 2^32 apart, not 2^20.
- Renumbering also dropped the dead entries of removed steps, which
  stayed in the lists until then. Once it was rare, the lists grew at
  every keystroke (four steps passed over each time), and keystroke
  100 took 19 ms where keystroke 10 took 11 ms. `Fold::remove` now
  drops a removed step's entries: its reads, and its writes, which the
  rebuild passes in.

The heading keystroke natively (LTO, 100 each, the same PDF): median
9.7 → 8.3 ms, p90 59.3 → 9.4 ms, max 91.1 → 18.8 ms; −42% instructions,
−62% cycles; the 10th, 50th and 99th rebuilds 7.4, 8.0 and 8.6 ms. The
harness: 14 cases, 79 stages, identical in all three modes; the
workspace's tests pass.

## 2026-10-03 — The page builder apart from a paragraph's lines, and readers kept (coordinator)

When an edit changes a paragraph's line count, the page totals change
for everything after it on the page. Every later paragraph's step read
them: `new_graf`'s page builder at its start (the `\parskip` glue) and
§1094's after its `\par`. So the paragraphs after an edited one ran
again down to the page's end, and the output routine with them.

- The page builder after a paragraph's end is deferred to the next
  command, a step boundary before it (`CleanPoint::Page`); the step that
  begins there runs it and ends at the next clean point.
- A paragraph's start is a step boundary (`CleanPoint::Graf`): the
  machine's `ParStart`, the first candidate after `new_graf`, so after
  LaTeX's restart of the paragraph from its `\everypar` (since 2021 the
  kernel ends the paragraph `new_graf` began and begins it again with
  `\noindent`: two `new_graf`s, two page builders, at every paragraph)
  and its first word. Only on the line the paragraph's level began on:
  the step that begins there takes the level's `mode_line` as its own,
  the line it begins on, since the step before kept a number an edit
  above it can move (the harness caught the overfull box's `lines
  155--160` where the line was 159).
- `new_graf`'s page builder is deferred too in a step that ran 64
  commands before the paragraph began: a picture built in vertical mode
  before its `\leavevmode`.
- The steps now end inside lines, and an edit in such a line made new
  steps where the old ones should have run again in place (new keys,
  so new sealed-line keys, the output routine run again, poor
  predictions and dropped runs). A step's end in a line is now placed
  by its offset from the line's start or from its end; where the line
  ends (`limit`, `first`, `last`) is the input's, compared by
  `same_input`.
- A step run again in its place can end elsewhere, and a definition it
  made moves to the next (new) step. Its readers were marked dirty when
  the first step stopped making it, and ran although the next one made
  it again with the same value (the heading keystroke's second trip:
  97 → 935 commands). A reader a changed definition marks now keeps the
  slot and the version it read, and is passed over when its turn comes
  if each such slot reaches it at that version again (`Dirty`).

The harness: 14 cases, 79 stages, identical in all three modes; the
rebuilds' commands 158,623 → 116,129 (−27%), fewer in every case
(incremental −40%, streams −61%, fatal_end 1,906 → 56, machine_edits
−24%), more steps run (746 → 1,057 in machine_edits). TikZ mock, 100
keystrokes each, instructions in the process: a word 3.62 → 3.62 M,
a `(` that changes its line's height 12.04 → 12.25 M, the heading
25.55 → 23.58 M (−7.7%), the long sentence that wraps 41.05 → 40.20 M;
the same PDFs. That last one still runs the TikZ picture's step again:
the picture's `\hbox`, built in vertical mode, pushes the outer level
onto the nest, and `push_nest` reads all its fields, `\prevgraf` (the
line count before) among them. That is the next change: the nest's
levels as slots of their own, by depth.

## 2026-10-03 — The nest's levels as slots of their own, by depth (coordinator)

A box built in vertical mode (the TikZ picture's `\hbox`, before its
`\leavevmode`) pushes the outer level onto the nest, and `push_nest`
read every field of it, `\prevgraf` (the line count of the paragraph
before) among them: `cur_list`'s fields were twelve slots shared by
every depth, so entering a level read them all and leaving one wrote
them all. A word that made the paragraph before wrap to one more line
ran the picture's step again (3,082 commands), and the page with it.

- Each level's fields are slots of their own: level `d`'s field `f` is
  `List(d * 32 + f)` (`track::list::slot`), level 0's where they were,
  so the nest, the alignment's fields and the view's names
  (`list.pg`, `list1.mode`, …) stay put. The nest's version is its
  depth.
- `push_nest` reads only what the new level inherits (the mode and
  `aux`: `prev_depth`, `space_factor`, `clang`, §216) and writes the
  new level's fields; `pop_nest` writes the nest alone (the level left
  to is in its slots as it was). A read or write of a field of the
  current level reads the nest's depth: which slot it is depends on it.
- The searches of the nest (`\prevgraf` read and set, e-TeX's
  `\showgroups`, a display alignment's `prev_depth` and modes in
  `init_align`) and the contribution list (level 0's list) read the
  field of the level they look at, not the whole level.
- A rebuild puts the nest back in three places (a dropped run's
  writes, a step's end, a step removed). The levels' fields go with
  it, down to the depth it is put back at, the nest placed first: the
  first version restored only the slots the run wrote, so a run that
  pushed a paragraph's level and was dropped left `cur_list` as the
  paragraph's while the nest said depth 0 (the harness's
  `semantic nest` panic in `pop_nest`). Bounded by that depth, not by
  `max_nest_stack`, which made every step place a dozen levels and
  cost 8–12% on the TikZ keystrokes.
- The view's text of a step is what it added to the list of the level
  it left, by the nest's depth there (the line break's step now shows
  the lines it set, `Hello World. One line.`, not the part of the
  paragraph's list after its first step's).

The harness: 14 cases, 79 stages, identical in all three modes; the
rebuilds' commands 116,129 → 112,369 (−3.2%), all of it in two of
machine_edits' stages (−1,880 commands each), every other stage the
same. The TikZ mock (LTO, 100 keystrokes each, instructions in the
process per keystroke): a word 3.62 → 3.64 M, a `(` that changes its
line's height 12.25 → 12.50 M (+2.0%), the heading 23.58 → 23.88 M
(+1.3%), the long sentence that wraps 40.20 → 19.25 M (−52%): its
rebuild runs 9 steps and 1,108 commands, the picture's step not among
them; the same PDFs. What the wrap still runs is mostly the output
routine (956 commands adding the sentence, 481 removing it): the page's
text changed.

## 2026-10-03 — The fold's index entries name steps, not keys (coordinator)

The course on accl, two rounds each, alternating: 93f8836 against
2b742d1 (job 6403; gate 6404 passed): the warm word edit 21.1/22.4 →
21.5/21.8 ms, its revert 19.7/19.5 → 18.4/18.3 ms, the cold build +1.5%,
records +4.6%, peak RSS 2.19 → 2.50 GB. bef4f62 against 93f8836 (job
6411; gate 6410 passed): the word edit 20.6/21.1 → 20.6/20.8 ms, the
cold build and RSS the same.

The RSS: `PARTEX_SSA_MEM=1` now prints what the recorder
holds after the cold build, by part (the values' own contents not
counted). The course (65,441 steps; 59,406 before 93f8836) held:

| part | before | after |
|---|---|---|
| the readers' lists: 11.6 M entries of 24 bytes, capacity 16.7 M | 383 MB | 63 MB |
| the definitions' lists: 3.7 M entries of 24 bytes, capacity 7.4 M | 168 MB | 84 MB |
| the records' writes: 4.1 M of 64 bytes, each with its value | 253 MB | 253 MB |
| the steps' reads: 11.6 M slots of 16 bytes | 177 MB | 177 MB |
| the records' reads, items, headers | 136 MB | 136 MB |

The page builder's steps (6,035 more) made the growth: each part grows
with the steps' reads and writes, about 53 KB a step, with nothing
wrong in any one of them. Most of a step's reads are slots every step
reads (135 slots are read by more than 8,000 steps each, 3.1 M entries;
1,776 more by 1,000 to 8,000).

- A reader's entry was the step, its run and its key, and two fields
  (`rec`, `ix`) only a definition uses: 24 bytes. It is the step's id
  alone now, the lists in the order of the steps' keys (a search reads
  each step's key); a definition's is the step, the record and the
  write's index, 12 bytes.
- No entry names a run: a run's close already put its own entries over
  the last run's and removed the rest, and a removed step's entries go
  with it, so every entry is a live step's last run's. A step's reader
  entries change only where its reads did (the slots it reads that its
  last run did not, and those it no longer reads), where every read was
  searched for before.
- No entry names a key, so making the keys again (`Fold::renumber`)
  rewrites the steps' keys and nothing in the lists. The searches read
  the keys from an array of their own, by step id (8 bytes a step, the
  course's half a megabyte), not from the steps (72 bytes each): read
  from the steps, `Fold::reaching` went from 2 to 32 of 2,200 samples on
  the wrapping keystroke.
- A step's reads that changed are found with a small hash set of each
  run's reads, not by sorting both (which cost as much again).

The course's cold build: peak RSS 2.577 → 2.384 GB (−193 MB, −7.5%),
wall 1:35 → 1:32. The TikZ mock (LTO, 100 keystrokes, instructions per
keystroke): a word 3.64 → 3.64 M, a `(` 12.50 → 12.48 M, the heading
23.88 → 23.70 M, the sentence that wraps 19.25 → 19.10 M; the same
PDFs. The harness: 14 cases identical in all three modes, the same
commands.

## 2026-10-03 — Deflate goes on from where it was (coordinator)

A profile of the course's word edit (an LTO build with frame pointers,
perf attached during the rebuilds only, 10 edits and their reverts):
zlib's `deflate` was 28% of the process. A word edit compresses two
streams again:
- the page it changed, 72 KB at level 9, about 4 ms, in the ship: the
  course has no virtual object numbers, so the ship writes its streams
  compressed (the link compresses only what `Effect::Deflate` defers,
  with virtual numbers);
- the cross-reference stream, 25 KB, about 4.5 ms, in the link: every
  object after the page moved.

A revert compresses the page as it was before. The edited page and its
last version share their first bytes up to the edit's place (6 KB of
72 for the course's sentence near the top of its page), and the
cross-reference stream shares its entries up to the first object that
moved (16 of 25 KB for ch15's page).

zlib's output is the same however its input is split. A new test checks
this at levels 1–9, with pieces from 1 byte to 64 KiB, on page-like
text, xref-like entries, random bytes and long runs. So a compression
can go on from a state taken partway through another (`deflateCopy`)
and give the bytes a compression from the start gives, provided the two
inputs agree up to that point:
- After the cold build, `zlib::deflate_stream` keeps its last 6 streams.
  For each it keeps the input, the output, and the compressor's state
  after each 4 KiB of input but the last (about 260 KB each at level 9,
  the output buffer's pending bytes with it).
- A stream identical to a kept one is that one's output again (a
  revert's page). One sharing its first bytes with a kept stream goes
  on from the last state within them, its output so far copied.
- `PARTEX_DEFLATE_RESUME=0`: from the start, as before. The cold build
  keeps nothing.

A test checks resumed outputs against compressions from the start
(levels 1, 6 and 9; edits early, mid-piece, at a piece's end and late;
shorter, longer and exact-piece-length streams).

Measured locally, the course's word edits, the same binary with and
without (the machine loaded, so times are rough): the link of an edit
4.4–6.5 → 3.0–5.2 ms (the cross-reference stream resumed 64% in), its
page resumed only from 4 KiB, and a revert's page compressed before
(about −4 ms). The PDFs were the same at all 13 checkpoints. The TikZ
mock (LTO, instructions per keystroke): a word 3.64 → 3.51 M (−3.4%),
a `(` 12.48 → 12.04 M, the heading 23.70 → 23.27 M, the sentence that
wraps 19.10 → 18.64 M; the same PDFs. The harness: 14 cases identical
in all three modes. The course on accl follows.

The course on accl, 636cdc4 against 895a16f (job 6414, two rounds,
alternating; gates 6412 and 6413 passed): the warm word edit
20.2/21.6 → 20.7/20.1 ms (its link 3.2/3.1 → 2.3/2.4 ms, the rest
noise), the revert 18.5/17.8 → 13.8/14.4 ms (−23%: its page compressed
before), the cold build and peak RSS the same (2.29–2.32 GB; 2.48–2.50
before the index entries went to 4 and 12 bytes).

## 2026-10-03 — A rebuild's fixed costs: the glyph union counted, the φ's versions kept, the format decoded before the first keystroke (coordinator)

The word edit's profile without the deflate showed costs that every
rebuild pays, whatever it runs:
- `glyph_union_now` (2.6% of the samples): whether the job's end must
  run again after a ship's glyphs changed. It made the union of every
  ship's glyph row, 295 rows, each font's bits merged into a map. Now
  `GlyphCount` keeps the rows it counted, and for each font how many
  rows have it and each of its glyphs. A row whose value changed (a
  different `Arc`) is counted out and the new one in, and the union's
  version is made again only then. Debug builds check it against the
  union made whole.
- `Steps::phi_seeds` (2.6%): the loads of the φ whose value is not
  what they read. It hashed every stored name's bytes (`.aux`, `.toc`,
  `.out`) at each rebuild. A φ kept from the last trip is the same
  bytes, shared, so its version is kept with it (`phi_vers`, by the
  `Arc`).
- `initial` (5.3% over 20 rebuilds, all in the first): the format's
  definitions, decoded the first time a slot no step defines is placed,
  about 40 ms on the course's first keystroke. `ssa::prepare_rebuilds`
  decodes them, and the command line calls it after the cold build's
  link when rebuilds are to come.

The same profile again (an LTO build with frame pointers, the course's
10 word edits and reverts): the first rebuild 65 → 35 ms; `initial`
5.3 → 0.1% of the samples, `phi_seeds` 2.6 → 1.5%, `glyph_union_now`
2.6 → 1.7% (the walk over the 296 rows' latest definitions, mostly);
`run_step` outside the commands 18.1 → 13.6%. What is left is mostly the
work itself: the commands 61% (the ship 33% of it: deflate 20%, the
page's operators 13%, whatsits 9% of those), the link 8%. The harness:
14 cases identical in all three modes; the workspace's tests pass.

## 2026-10-03 — A token list shown into a string: its characters straight to the pool (coordinator)

A ship makes each `\pdfliteral`'s text with `tokens_string` (§465's
`show_token_list` into a new string), and the course's pages have many:
`tokens_string` was 5.0% of the samples of its word edit and revert.
For each character token `show_tokens` called `print`, which read the
selector (tracked), and `print_char`, which read `\newlinechar` (tracked)
to see whether the character is the new-line one, only to append it to
the pool. With the selector `new_string` and no special or message
printing, §59's `print` of a character is its `print_char`, and §58's
`print_char` appends it and counts it whatever `\newlinechar` is
(`new_string` > `pseudo`). So `show_tokens` appends such a character
itself, its `tally` counted; control sequences and the rest are printed
as before.

The profile again (an LTO build with frame pointers, the course's 10
word edits and reverts): `tokens_string` 5.0 → 2.6% of the samples,
`pdf_list_out` 13.2 → 11.0%. The TikZ mock (LTO, instructions per
keystroke): a word 3.51 → 3.49 M, a `(` 12.04 → 12.01 M, the heading
23.27 → 23.20 M, the sentence that wraps 18.64 → 18.59 M; the same PDFs.
The harness: 14 cases identical in all three modes; the workspace's
tests pass.

## 2026-10-03 — A page's stream compressed on a thread of its own: slower on a desktop, not kept (coordinator)

A word edit compresses its page again (the course's: 72 KB at level 9,
about 4 ms, in `pdf_end_stream` after `pdf_list_out` made the page's
operators, 2–2.5 ms). Since zlib's output does not depend on how its
input is split, the stream's pieces could be compressed as `pdf_flush`
hands them on (16 KiB), on another thread, the stream's end waiting for
the rest only. Tried in full: a host feed for a stream's bytes (`PdfOut`
holding it, not state), a deflate thread that compares the pieces with
the kept streams while they are those streams' first bytes, then goes on
from the last state within them in pieces of 4 KiB, and at the stream's
end compresses the whole itself if what it was given is not it. Its
answers were zlib's output in every case tested (pieces of 100 B to
100 KB, gaps, other bytes at the end, streams left before their end,
edits early, mid-piece, at a piece's end and late; levels 1, 6, 9); the
harness was identical in its three modes; the course's PDFs were the
same at all 13 checkpoints.

It was slower. On this machine (a 12-core Haswell Xeon at 1.2–3.1 GHz,
the `schedutil` governor, load about 2.5), the course's rebuilds took
7–15 ms more with the thread than without, the same binary
(`PARTEX_DEFLATE_THREAD=0/1`, two rounds alternating): edits 24–37 →
30–50 ms, reverts 17–25 → 23–34 ms. A log of the waits: the stream's end
waited 16–17.6 ms for the thread, whose own end took 0.9 ms, so the
thread spent about 16 ms compressing what takes about 4 ms on the main
thread. A thread busy a few milliseconds per keystroke on an otherwise
idle core runs at that core's low clock, while the main thread, busy all
along, runs at its high one. Each hand-over also costs 60–300 µs of
wake-up: the TikZ mock's small streams were answered in 5–180 µs and
waited for 63–313 µs. Latency is what users feel on the machines they
have, so a thread for a few milliseconds of work per keystroke is not
kept (not committed). On a governor that keeps the clock up (servers, a
`performance` setting) it would save the overlap, about 2 ms of the
course's word edit.

## 2026-10-03 — A rebuild's walks over every file, stored name and ship, cut to what changed (coordinator)

DESIGN 4.3 item 3: nothing in a rebuild walks every step, record, file,
chunk or slot. The course's word edit's profile (an LTO build with
frame pointers, its 10 word edits and reverts) still had four such
walks:
- The load check (`NativeHost::unchanged`, 5.2% of the samples): for
  every file the build loaded, its lookup found by `(name, kind)` (the
  name copied, hashed with SipHash), then `inotify::Watcher::quiet` for
  each directory its lookup tried, each found by its path (SipHash), and
  in a directory that ever had an event (the document's own) each
  candidate's name split off and looked up. Now the host numbers the
  directories its lookups tried as it records them, and the watcher keeps
  each directory's state by that number: when it was watched from, its
  last event, and each name's last event. A directory with no event since
  the check is quiet whatever names are asked about, without looking at
  them. The lookups are found by name with no copy (a name's kinds kept
  together), in maps with phitex-doc's word-at-a-time hasher. And a
  directory that is not there cannot be watched: with
  `-output-directory`, every `\input` tries the output directory first,
  so the course's 218 `figs/data/*.csv` files each tried `out/figs/data`,
  and their lookups, never quiet, were checked by their stamps (a `stat`
  of each file found) at every rebuild: 227 lookups of the course's. A
  directory not there is now watched through the nearest one above it
  that is, for the name that would make it (`out`, for `figs`): 6
  lookups are checked by their stamps now (the three stored names, the
  edited file under its two names, and a font's VF).
- `Steps::trip_end` (1.8%): after each trip, each stored name's value
  (`.aux`, `.toc`, `.out`) made again from every step's stores, sorted,
  its lines joined, and compared with the last. Now each stored name
  counts its store changes (a step's run that stored to it otherwise than
  its last run did, a step that stored to it leaving the fold), and the
  value made is kept with that count: made again only when the count
  moved, and kept as the bytes the last trip read when equal, so the φ's
  bytes stay shared (and `phi_seeds` finds its version by identity).
- The tools between trips (about 1%): the command line copied each
  `.aux` and `.idx` stream to hand it to BibTeX and makeindex, and BibTeX's
  check copied each again, scanned it for `\bibdata`, and made its digest.
  Now they are handed on shared (`Arc`), and the scan and the digest are
  kept with the contents they were made of.
- `glyph_union_now` (1.7%): when a ship's glyphs changed, every ship's row
  (296) looked up again. Now the rows whose definitions a step's run or
  removal changed are noted, and only those are counted again; rows that
  appear or go (a row past the last counted, a row no longer defined)
  count them all again, as does a row whose value is not kept. And the
  union's version, made again whenever a row changed, made each font's
  set from its 256 counts (1% of the samples once the rows were counted
  by change): each font's set is kept as its counts change. Debug builds
  check the count against the union made whole.

The course (LTO, 6 word edits each reverted, `perf stat` over the
rebuilds, two rounds alternating with 8832cc6): instructions per rebuild
124.66/124.65 → 122.01/122.02 M (−2.1%), cycles 101.2/101.6 →
95.4/95.2 M (−6.2%: the walks were cache misses and `stat`s more than
instructions); the revert's median 24.6/24.5 → 22.3/22.9 ms, the edit's
36.9/34.8 → 34.5/34.8 ms (this machine, loaded). A profile before the
last two (the directories not there, the fonts' sets): the load check
5.2 → 3.8% of the samples, SipHash 1.8 → 0, `trip_end` 1.8 → 0, the
trips' end 3.5 → 1.2%, the glyph union 1.7 → 1.2%. The harness: 14
cases identical in all three modes; the workspace's tests pass.

The course on accl, 57d5260 (with 8832cc6's `show_tokens`) against
202c918 (job 6419, two rounds alternating; gate 6418 passed): the warm
word edit 19.0/19.1 → 17.4/16.6 ms (its rebuild 16.8/16.9 → 15.7/15.0
ms), the revert 14.1/12.9 → 11.8/12.2 ms; the cold build and peak RSS the
same.

## 2026-10-03 — More of a rebuild's fixed costs: the loads looked at by reference, the φ's loaders kept, the drawn items' hash kept (coordinator)

The profile after 57d5260 (the course's word edits and reverts, LTO with
frame pointers) still had costs every rebuild pays:
- The loads looked at (1.1%): `rebuild` made a list of every load the
  build made, each with its name copied, its kept contents and three set
  lookups, to ask the host which are as they were, then kept only the
  others. Now the host is asked with the names and contents by
  reference, and only the loads it does not know are as they were are
  copied out (the course: 3 to 5 of over a thousand).
- `Steps::phi_seeds` (1.3%): the loads of a φ that found other than the
  φ, found by walking every step's loads. The steps with a load of a φ
  are kept as their runs close and leave (`phi_loaders`), and only their
  loads are walked.
- The ship's versions (2.1%): a recorded call begun while a page is
  shipped (each `\write` of the page) versions the ship state first,
  which hashed the page's display items recorded so far (`drawn`) each
  time. The items are kept with the polynomial of their words
  (`DrawnList`, `TokenList`'s polynomial: position, kind and fields, a
  literal's length and bytes), made as each is recorded, and the state's
  version hashes that and their count.
- `show_token_slice` cloned the token list's `Arc` to show it (an atomic
  increment and decrement for each `\pdfliteral` shown): it shows the
  list it is given.

The TikZ mock (LTO, 100 keystrokes, instructions per keystroke, against
8832cc6, so with 57d5260's cuts): a word 3.49 → 3.31 M (−5.1%; its
median 2.9 → 1.4 ms), a `(` 12.01 → 11.86 M, the heading 23.20 → 23.10
M, the sentence that wraps 18.59 → 18.45 M; the same PDFs. The harness:
14 cases identical in all three modes; the workspace's tests pass. The
course on accl follows.

## 2026-10-03 — The fold's lists searched where their keys put them, and the loads indexed by name (coordinator)

The profile after de451fa (the course's word edits and reverts, LTO
with frame pointers) had costs that grow with the document:
- The fold's searches (5.8% of the samples: `Fold::reaching` 2.3%,
  `Fold::close` 2.3%): each a binary search (`partition_point`) over a
  slot's list, in the order of its steps' keys, each probe a load of a
  step's key from the array of keys, by step. The lists of the slots
  every step reads are as long as the fold (the course: 135 slots read
  by more than 8,000 steps each), and their probes miss the cache. The
  keys are spread evenly, and `Fold::renumber` keeps them so, so
  `first_not_below` guesses where an even spread puts the key, gallops
  from the guess to bracket it, and halves within the bracket: its
  answer is `partition_point`'s, in a few probes (a list under 32 long:
  `partition_point`). Every search of the fold's lists uses it
  (`reaching`, `next_after`, `readers_between`, `position`, `close`'s
  definitions, `remove`, `put_reader`, `drop_reader`).
- `Steps::phi_seeds`: de451fa kept the steps with a load of a φ, but
  every load of a name not stored is one (a trip's first run reads the
  value from before the job), so it walked most steps' loads. The steps
  are kept by the names they loaded (`loaders`, by load id, as runs
  close and steps leave), and `phi_seeds` walks only the loaders of the
  names whose φ's version changed.
- `mark_store_readers` (a step that stored differently: the loads after
  it that read the store) walked every step's loads; it walks the
  loaders of the names stored.

The profile again (the course's 10 word edits and reverts, an LTO build
with frame pointers): 1,804 → 1,687 samples; the searches 4.4 → 3.3%
of them; `phi_seeds` 2.9 → 1.5% (what is left hashes a φ's bytes read
again, the next change's). Instructions per rebuild (the course's 6
word edits and reverts, perf stat, two rounds): de451fa 120.20 M, with
the index 119.75–119.77 M, with the searches too 120.06–120.09 M; cycles
96.6–97.6 M in all, within the runs' noise. The guess is a 128-bit
division, a call, so the searches cost a few more instructions than
they save: a float's division is next. The harness: 14 cases identical
in all three modes; the workspace's tests pass.

## 2026-10-03 — A glyph shipped without taking the font table to change, a φ read again not hashed again, a trail's names split once (coordinator)

The profile after da668f3 (the course's word edits and reverts, LTO with
frame pointers) still had costs per glyph shipped and per rebuild:
- The font table (3.7% of the samples): each glyph took the PDF state
  of its font as `&mut` several times (`draw`'s first-use test and
  resources, `pdf_font_type`, `pdf_begin_string`, `adv_char_width` and
  `fm_entry`, `mark_glyph`), each `VTab::get_mut`: two `Arc::make_mut`,
  each an atomic compare-and-swap and a store, and the font marked to be
  versioned again. `pdf_font_ref` reads it as `pdf_font` reads it (the
  same tracked read), the table not taken; `pdf_font_type` takes the
  writer's scope only for a font whose type is not known yet;
  `mark_glyph` takes the font only for a glyph not marked yet; and
  `pdf_set_font` no longer clones the page's font list to look in it.
- `phi_seeds` (1.5%): each rebuild read again the stored names the link
  had rewritten (`.aux`, `.toc`, `.out`: their stamps changed), into new
  buffers, so the versions kept by buffer missed and the names' bytes
  were hashed again, a byte at a time. A version kept for other bytes
  is checked against them (compared) before they are hashed.
- The loads' check (`unchanged`, 3.7%): for a directory with events
  since the last check (the output directory, the edited file's), each
  candidate a lookup tried there was split at its last `/` to find its
  name. A trail keeps where each candidate's name begins.

Tried and dropped: a file read again in a rebuild's check with the same
bytes kept its old buffer, so the φ's version would be found by buffer.
A buffer's identity means something there: `InputState::mapped` follows
the edits from an open file's buffer by identity, and an edit's new
buffer could then be one it came from. The harness's readback case,
in one-trip mode, looped there for good.

Instructions per rebuild (the course's 6 word edits and reverts, perf
stat): da668f3 120.06 M; with this and the next commit 114.62 M
(−4.5%), cycles 97.9 → 94.5 M (one round). With the dropped hunk it was
113.10 M: the tracker's versions of large loads are kept by buffer too
(`contents_version`), so a file the link rewrote is hashed again when it
is read again; comparing its bytes first would recover that. The
harness: 14 cases identical in all three modes; the workspace's tests
pass.

## 2026-10-03 — The fold's steps live by a bit, the search's guess by a float (coordinator)

- `Fold::live`, asked of each entry a search passes over or stops at
  (`reaching`, `latest`, `next_after`), read the step's `live` from the
  steps (72 bytes each, megabytes for the course: a cache line a step).
  A bit per step (`alive`, eight kilobytes for the course) holds it, as
  steps are made and removed.
- `first_not_below`'s guess divided in 128 bits, a call; a float's
  division makes it now (a guess: any index in range is right, and the
  search's answer is `partition_point`'s whatever it is).

Instructions per rebuild: −0.02 M (113.10 → 113.07–113.09, measured
with the dropped hunk above); cycles within the runs' noise. The
harness: 14 cases identical in all three modes; the workspace's tests
pass.

## 2026-10-03 — A file's size asked for again and again: its lookup answered once a rebuild, its bytes shared, hashed a word at a time (coordinator)

A profile of edits that change only `ch15.tex`'s size (a comment line
at its end, added and taken away: the steps that run again are the five
that read the size of the file `\include{ch15}` looks up, and the one
that reads its last lines) showed where those steps' 107 commands spend
about 3 ms locally: 18% of the samples in `\pdffilesize` (expl3's
`\file_full_name:n`, which only tests the digits for being blank), each
asking the host for the file again:
- its lookup again: the output directory first (`stat`s), kpathsea's
  search with its trail, a `stat` of each directory the trail tried;
- the file read again from disk: a file under 2 s old is not kept (its
  stamp could stay the same through another write in the same clock
  tick, git's racy entries), and the file just edited is always that;
- its bytes hashed again for the load's version: the versions of large
  loads are kept by buffer, each read made a new one, and the stable
  hasher took a byte at a time.

Now:
- The host answers a name looked up again as the same kind, since its
  last check of the loads, as it answered it (`Seen::again`), until it
  opens a file to write (a lookup could find that file then).
- A file under 2 s old is kept until the next check of the loads
  (`Seen::racy`), with its stamp: read again in the rebuild, the same
  buffer.
- A large load's version kept for other bytes is found by comparing
  them (`contents_version`): a stored name the link rewrote, read again
  into another buffer, is not hashed again (what 6e55be8's dropped hunk
  had saved, 1.5 M instructions a rebuild).
- The stable hasher takes eight bytes at a time while its buffer is
  empty: the same words, so the same versions (a test feeds bytes
  whole, in pieces and one at a time).

Measured (the course; perf stat, user space, so the lookups' `stat`s
and reads, kernel time, are not counted): edits of the size only (6
and their reverts), 281be74 73.41 M instructions a rebuild, now 71.59 M
(−2.5%), the median rebuild 9.9 → 9.2 ms locally; word edits, 114.62 →
112.16 M (−2.1%), cycles 96.5 → 93.0 M (one round). The harness: 14
cases identical in all three modes; the workspace's tests pass.

## 2026-10-03 — A step's predicted reads placed with one lookup each (coordinator)

A step run again is placed at its predicted reads: for each slot its
last run read, whether a definition at or after the step holds the
arrays (`later`: a lookup of the slot's definitions, its last live
entry), then the definition that reaches the step (`reaching`: the
lookup again, and a search). `Fold::reaching_if_later` does both with
one lookup, and the placement takes the definition it found
(`value_of`); the slots added for the save stack and the nest, and a
dropped run's misses, are placed as before.

Measured: word edits 112.16 → 112.10 M instructions a rebuild, edits of
the size only 71.59 → 71.58 M; cycles within the runs' noise (a step
places hundreds of slots). The harness: 14 cases identical in all three
modes; the workspace's tests pass.

## 2026-10-03 — The host's lookup memos only between load checks (coordinator)

Gate 6440 (3c83e5f) failed the e2e suite in plain and machine mode (17
of 34 cases): 6114868's memos in the native host — `again`, a name's
lookup answered as it was, and `racy`, the contents of a file too new
to keep by its stamp — were cleared only by a check of the loads
(`Host::unchanged`), which an SSA session makes at each rebuild and
trip, and a plain or machine run never makes. Such a run writes a file
and reads it back (the `.aux`, a `\write` stream's file) and was
answered with what it read before. The host now keeps both memos only
once a check was made (`Seen::checking`): a run that makes none reads a
file again whenever it asks for it, as before 6114868; an SSA session's
rebuilds are unchanged.

Checked: e2e 34/34 identical in plain and in machine mode; the harness
is unaffected (its sessions check at every rebuild).

## 2026-10-03 — An undefined control sequence's suggestions read nothing of the job's (coordinator)

The diagnostic of an undefined control sequence suggests similar names:
`similar_control_sequences` went through every entry of the hash, each
by `eq_type` and `text`, the tracked accessors. In an SSA step each is
a read the step records (and a macro memo's), some sixty thousand at
each such error: a rebuild whose steps met undefined control sequences
again and again (a `\documentclass` option changed, below) grew to 4.7
GB in nine minutes, a quarter of its time in the scan. The scan now
peeks (`peek_eqtb`, `peek_text`): the suggestions are the host's, TeX
prints none of them.

## 2026-10-03 — Windows: a step ends inside a group, a box or a macro too (coordinator)

DESIGN 4.3 item 1, ported from np/windows (40b33be) onto main and merged.
A figure's caption edited re-ran the whole figure: a step ended only at
the clean points, and a float's body, a box or a TikZ picture is one
step (the demo, short.tex: 872,156 commands, 1.07 s for a word of the
caption). Now the engine also ends a step at a main-control boundary
inside groups, boxes and macros (token lists on the input stack): at
the first after a paragraph broken into lines, a deferred fire, a file
opened by `\input` or ended, or `PARTEX_SSA_WINDOW` commands (4,096 by
default; 0: the clean points alone). The clean points still end a
step. A rebuild enters a window with the nest, the alignment, the
conditionals and `align_state` placed whole beside the save stack, and
`force_eof` is part of the input's value.

An edit that changes a loop's command count shifts every count-cut
window after it, and the rebuild makes new steps until one ends where
an old one did. Their runs were dropped for slots no prediction had
(\petals 5 -> 7 in the demo's plot: 286 new windows, 304 runs dropped,
3.6 s against main's 1.0 s). Now: the eqtb entries a run of the rebuild
was dropped for are placed in every later step's run (a branch taken by
value, which the window's last run did not take); and a new window is
predicted by the reads and the eqtb writes of the old windows at about
its place, one further per new window, put back where a dropped run
read (a control sequence the old run made per plot point, which the new
run finds made and reads). 5 runs dropped; each slot looked up once.

The demo, LTO, one process (rebuild ms; main -> windows):

| edit | main | windows |
|---|---|---|
| a word of a paragraph | 5.0 | 5.2 |
| a word of the figure's caption | 1,071 | 15 |
| `\colorlet{petal}{red}` -> violet | 1,016 | 141 |
| `\newcommand\petals{5}` -> 7 | 1,011 | 1,552 |

`\petals` is read by every point of the plot: the windows run its
895,739 commands (main 871,375) and pay their placements and records,
some 300 windows'. The course's word edit (perf stat, 12 rebuilds, two
rounds): 112.11 -> 110.71 M instructions a rebuild.

Checked: scripts/ssa-edits 15/15 (a windows case added: a pgfplots
figure, a minipage, an alignment, math, a footnote, a loop,
\scantokens) with the default window (fixpoint, one trip, check mode)
and with PARTEX_SSA_WINDOW=0; e2e 34/34 plain and machine; the
workspace's tests; clippy. Open: PARTEX_SSA_RERUN_CHECK=1 (every window
run again alone, last first) fails 11 of the 15 cases, on save stack
entries (`save:10`, `save:14`) at main's own clean points (a
paragraph's start, a fire): the check predates those points, and what
it flags is not yet known to be wrong.

## 2026-10-03: PDF inclusion (`\pdfximage` of a PDF page)

The Overleaf extension's last blocker for a real paper: `\includegraphics`
of a PDF figure stopped the job ("PNG, JBIG2 and PDF images are not
implemented"). pdfTeX's inclusion is two pieces, ported apart:

- `partex-engine`'s `pdfread`: the file read as TeX Live's xpdf reads it
  for pdftoepdf.cc (its lexer's numbers, `50-100` is 50 and a real is
  summed digit by digit; xref tables and streams, `/Prev`, `/XRefStm`,
  object streams, a damaged file's objects found by scanning; the page
  tree and `PageAttrs`), with `inflate` (zlib, PNG and TIFF predictors)
  and the other filters a content array needs decoded.
- `partex-core`'s `pdf/epdf.rs`: `read_pdf_info` (the page by number or
  named destination, its box, the version check) and `write_epdf` (the
  form XObject as pdftoepdf.cc prints it: `stripzeros` of `%.8f`,
  `convertNumToPDF`, `copyObject`'s strings, names and arrays,
  `pdf_newline` after `pdf_last_byte`, the page group as its own object
  for the first image on a page, then every object referred to, under
  new numbers). The open documents are a writer field of their own
  (`EPDF`): an image read and not yet written keeps its document's
  copied objects for the next image of it, as `PdfDocument` does.

`scan_image`'s keywords (`page`, `named`, the boxes, `\pdfpagebox`,
`\pdfforcepagebox` and the obsolete parameters' warnings), `scale_image`
for a page turned a quarter, `out_image`'s matrix and page group number,
and `\pdfximagebbox`.

Checked against pdfTeX 1.40.29, byte for byte: plain TeX including pages
of PDF files pdfTeX wrote (object streams, an xref stream, a page group,
fonts, `/Rotate 90` with a crop box, `\immediate`, two images of one
document on a page), a file made by hand (a classic xref table, a
contents array, attributes and resources inherited from the page tree,
`/Rotate -90`, every page box, a named destination, strings and names
with escapes), and a LaTeX document through graphicx (width, `angle`,
`scale`, `page=2`, `trim` with `clip`). The logs differ only in pdfTeX's
memory statistics (`words of extra memory for PDF output`), which e2e
masks. e2e case `images`.

Not yet: pdfTeX's replacement of an included Type 1 font its map has
(`copyFont`'s first branch, the default `\pdfinclusioncopyfonts=0`): the
file's fonts are copied as they are, a valid PDF that differs from
pdfTeX's for figures pdfTeX made. xpdf's resource merging when both a
Pages node and its page have `/Resources` (only dictionaries kept), its
duplicate keys (the first place, the last value) and a few damaged-file
paths are next, as is the SSA build of the LaTeX case, whose page loses
its own font (the plain build is right).

## 2026-10-03: PDF inclusion, second pass: xpdf's own rules; SSA fixes

The reader (`pdfread.rs`) checked against TeX Live's xpdf 4.05 source
(`libs/xpdf`), file by file, and rewritten to it where they differed:

- `Dict::add` keeps a key given twice once, in its first place, with its
  last value.
- `PageAttrs` merges a node's and its page's `/Resources` when both have
  them: only categories that are dictionaries survive (a `/ProcSet` is
  dropped), the page's entries replacing the node's of the same name.
  pdfTeX's output showed it first: a hand-made file's page with its own
  `/Font` came out with its parent's `/ExtGState` and `/XObject` too.
- Boxes clamped to ±10⁹; a page found by its tree's `/Count`s
  (`loadPage2`), the page count from the top's (counted when 0 or above
  50,000); `findDest`'s name tree by `/Limits`, `LinkDest`'s validity.
- The lexer: `{` `}` are errors, `--123` is 0, an integer's digits wrap,
  a hex string's non-hex byte is a 0 digit, `#` escapes as xpdf reads
  them (`#00` an error, a name a C string). The parser: a negative
  reference is an error, arrays keep keywords and errors (pdfTeX then
  fails copying them, as it does). `makeStream`: a repaired file's
  stream ends at its `endstream`'s line, a wrong `/Length` gets 5,000
  bytes more, no length searches `endstream`.
- The xref: tables parsed byte by byte as `readXRefTable` does (the
  first section's entry wins, IBM's off-by-one table), streams'
  subsections failing at the data's end, `constructXRef`'s scan (object
  headers, trailers, `endstream`s, object streams' entries), an object
  stream's object whatever its reference's generation.
- The filters `Stream.cc` makes: `/F` and `/DP` as fallbacks, a
  parameter array only with a filter array, `StreamPredictor` exactly
  (a last partial row, TIFF at 16 and fewer bits), ASCIIHex's last 0
  byte, ASCII85's arithmetic on any byte, RunLength past the end, LZW's
  table cleared at 4,097.

pdfTeX's first LaTeX run of the `images` case wrote 47,948 bytes,
partex 47,949: the page group's object took 11 where pdfTeX's took 10.
pdfTeX numbers a font at its first use in the page and an image's group
in `out_image`, both in the content's order; partex numbers fonts in
the walk, but encodes an image's drawing later, with the drawn list. The
group is now numbered in the walk too (`image_group`). (e2e compares a
case's last run only: the case now uses a bold font after the image in
its final run.)

SSA: a page that uses a form (`\pdfxform` without `\immediate`; every
graphicx `clip`) lost its fonts: the form is shipped inside the page's
ship, after its contents, and `pdf_ship_out` began by clearing the
glyphs marked so far, then took them as the form's. The page's glyphs
went with the form's ship, the page's own were empty, and the job's end
embedded nothing of fonts only the page used (`Unknown font tag F32`).
Old: the windows binary and the one before it show it. The outer
ship's marking is now kept across the inner ship.

A `\font` whose TFM file was missing records the miss as a load, as
VF, map, encoding and Type 1 reads did: the step wakes when the file
comes (the Overleaf host supplies files as the job asks for them).
Checked natively: `\font\x=mytfm` built by SSA without `mytfm.tfm`,
the file copied in as the rebuild's edit: the rebuild (7 steps, 39
commands, 2 ms) writes the DVI a plain run with the file there writes.

The PK error names its font: "PK fonts are not implemented in partex
yet (font X, expanded from Y)" for an instance `\pdffontexpand` made
(with "not autoexpand" for a non-auto one), "(font X: no map entry)"
otherwise. The Overleaf peer meets it on a page with math in an acmart
document (libertine, newtxmath, microtype, zi4) that every native build
here writes as pdfTeX does; the message will say which font it is.

Also: the PNG average predictor wraps as C's `Guchar` does (a debug
build panicked on a sum past 255).

Tests: e2e 35/35 (plain and `PARTEX_MACHINE=1`), ssa-edits 16/16
(`--brief --fixpoint`, as `cargo xtask check` runs it; without
`--fixpoint` the oracle runs once a stage, and the cases whose first
build reads a file it writes differ by design).


## 2026-10-03: PDF inclusion: font replacement (`copyFont`)

With `\pdfinclusioncopyfonts=0` (the default), pdfTeX does not copy a
Type 1 font of an included page: when the font has its file (`/FontFile`,
or `/FontFile3` of subtype `Type1C`) and the map has an entry for its
PostScript name, the font is replaced by pdfTeX's own embedding of that
entry's file. partex copied every font (as `\pdfinclusioncopyfonts=1`
does), so every LaTeX-made figure differed from pdfTeX's output. Now:

- `lookup_fontmap` (`fontmap.rs`): the name without a subset tag,
  `-Slant_<n>`/`-Extend_<n>` read off its end, found in `ps_tree`. The
  default map is read lazily (42,000 lines; a document looks up a few),
  so `ps_tree` is found the way registering every line would have built
  it: the first line naming the PostScript name (found by its bytes) that
  is the first valid line of its TFM name and names a Type 1 file to
  include. The font file must be there (a load).
- `epdf_create_fontdescriptor` and the rest (`writefont.rs`): the
  descriptor of the entry's file is shared with TeX's fonts of that file
  (its glyphs the union, one subset), numbered when the image is written,
  its `/StemV` the PDF's; the glyphs of the PDF's `/CharSet` marked, or
  the whole font embedded (`all_glyphs`) when there is none or the entry
  is not subsetted; the font name an object of its own (`fn_objnum`),
  written with the descriptor at the job's end.
- `epdf.rs`: the copied objects have kinds (`objFont`, `objFontDesc`,
  `objOther`); a replaced font's dictionary is copied without its
  `/FontDescriptor`, `/BaseFont` and `/Encoding` (any key starting so),
  which pdfTeX's objects replace; its encoding is written as
  `/Differences` after the contents (`writeEncodings`, the last font
  first), a CID font failing there.
- The encoding is xpdf's (`partex_engine::gfxfont`, `GfxFont.cc` of
  4.05): the font's type from its dictionary and its embedded file
  (`getFontType`, with `FoFiIdentifier`), then `Gfx8BitFont`'s base
  encoding (the dictionary's `/Encoding` or `/BaseEncoding`, else the
  file's own: `FoFiType1::parse` of the first 100 lines, or
  `FoFiType1C`'s charset and encoding; else Standard, or WinAnsi for
  TrueType, or a Base 14 font's), Type 1C gaps filled from Standard, and
  `/Differences` over it (`fofi.rs`, with xpdf's tables generated into
  `fofi_tables.rs`).

Checked against pdfTeX: a plain document using cmr10 that includes three
figures (pdfTeX's CM fonts with built-in encodings; T1-encoded Latin
Modern, an `/Encoding` with `/Differences`; the first again through
Ghostscript, its fonts Type 1C with WinAnsi), on two pages, plain and
`PARTEX_SSA=1`: the same bytes. The `images` e2e case no longer sets
`\pdfinclusioncopyfonts=1`: its figure's cmr10 and the document's share
one descriptor.

`tests/view.rs` expected the view of a small document before `1483121`
made a missing TFM a load: the view now has that load's node (`%2 =
file cmr10`), the rest renumbered (the crate's tests were not run before
that commit).

Tests: e2e 35/35, ssa-edits 16/16 (`--brief --fixpoint`), the
workspace's tests, clippy.

## 2026-10-03: A rebuild's deadline; a preamble edit costs 47 cold builds

The Overleaf extension's repro: a one-page article with a TikZ picture
and a `circuitikz` environment, then `\usepackage{circuitikz}` added to
the preamble as one edit. Natively (release build of `824a511`, one trip
a build): the cold build takes 1.18 s (85,620 commands); the rebuild
55.0 s, running 25,038 steps (25,037 new, 8,997 runs dropped) and
772,325 commands, with 77.2 M slots positioned and 77.3 M restored,
about 3,100 of each per step. The extension measured 51 s in wasm. The
cost is per step, not per command: the edit makes every step after it
new, and each is run as a rebuild runs a step (its predicted reads
placed, its writes restored), where a cold build runs straight through.

A budget of commands cannot bound this: the cost of a command varies
tenfold between rebuilds (the extension's ChillCGRA rebuilds run
150–250 K commands in 1.7–3 s; this one 0.3 ms a command).
`SsaTracker::deadline` (the CLI's `PARTEX_SSA_REBUILD_MS`) is a clock,
the host's since wasm has no `Instant`, and a time on it. It is checked
after each step a rebuild runs; past it the rebuild stops as one past
its budget of commands does, and a host builds cold instead. The repro
with 2,000 ms stops at 2,002 ms, after 4,862 steps. Off by default.

Also: `824a511` was not formatted; the generated `fofi_tables.rs` is
now skipped by rustfmt (`#[rustfmt::skip]` on its module) and keeps its
layout.

## 2026-10-03: PNG images (writepng.c, libpng 1.6.58)

`\pdfximage` of a PNG file failed ("PNG and JBIG2 images are not
implemented"); the Overleaf peer's ChillCGRA paper has one. pdfTeX reads
it with libpng (`read_png_info` at `\pdfximage`, `write_png` where the
image object is written), so the output depends on libpng's reading of
the file, which `partex_engine::png` ports from libpng 1.6.58 (the
system's, which this machine's pdfTeX links): the chunks up to the
first IDAT (`png_read_info`: libpng's table of positions, lengths and
duplicates, each handler's acceptance, CRC errors fatal for critical
chunks and dropping ancillary ones, benign errors as warnings), and the
rows after pdfTeX's transformations (`png_set_tRNS_to_alpha`,
`png_set_strip_alpha` before PDF 1.4, `png_set_strip_16` without
`\pdfimagehicolor`; the row filters, Adam7, `png_read_update_info`).

`writepng.rs` writes what writepng.c does: the IDAT data as they are
with a PNG predictor (the "PNG copy") for a non-interlaced gray or RGB
image with nothing to change (no tRNS, alpha or 16 bits to strip, gamma
1 or none, none of cHRM, iCCP, sBIT, sRGB, bKGD, hIST, sPLT); otherwise
the rows: a palette as `/Indexed` (its own object), an alpha channel as
an `/SMask` image (8 bits: a 16-bit alpha's high bytes), the color
bytes and alpha bytes split as `write_*_pixel_*` do. An alpha PNG in
PDF 1.4 or later asks for a transparency group: one object per job
(`transparent_page_group`), the page's group where the image is read if
the page has none, and the image's (`img_group_ref`) where it is placed;
the group object is written after the first such image.
`\pdfimageapplygamma` is refused ("not implemented").

The decoder (`png.rs`, 2,800 lines) has zlib 1.3.2's `inflate` and
`inflate_table` inside it as a resumable state machine, fed as libpng
feeds it (8,192 bytes a read, a row at a time): that boundary decides
whether a damaged stream's end is an error or a warning. `inflate.rs` is
untouched. Not ported, because pdfTeX never asks for them: gamma and the
other transforms, text chunks' contents (warnings only), unknown chunks
kept, `png_read_end`, the progressive reader; libpng's limits are ported
(8,000,000-byte chunks, 1,000,000 pixels a side).

Checked against the system's libpng (`scripts/png-check/compare.sh`:
`harness.c` drives libpng as writepng.c does, the `png_dump` example
prints the port's reading the same way): PngSuite and 282 edge cases
(`gen_edge.py`: bad CRCs on every chunk kind, tRNS of every length,
iCCP profiles of every kind and damage, IDATs split and damaged, zlib
header, block and adler32 errors at the 8,192-byte boundary, 40
randomly damaged streams, IHDR/PLTE errors, truncations, Adam7), 458
files and 471 readings (every transformation set pdfTeX can ask for),
126 of them errors: identical (every info field with the whole `valid`
word, the transformed format, a hash of the rows, the message).

Against pdfTeX: 22 images (gray 1–16 bits, palettes with and without
tRNS, RGB(A) 8 and 16 bits, gray-alpha, Adam7, gAMA 1, pHYs, sRGB,
bKGD) in six jobs (PDF 1.1, 1.3, 1.4 and 1.5; compress levels 0, 6 and
9; `\pdfimagehicolor` on and off; object streams): the same PDFs, when
the 1- to 4-bit palette images' rows fill their last byte. When they do
not, pdfTeX's own output is not reproducible: the bits past the row's
end come from writepng.c's `xtalloc`'d row buffer, never written
(libpng keeps a row's trailing bits), and three runs of pdfTeX gave
three different bytes for one image; partex writes zeros there. The new
e2e case `png` (seven images, PDF 1.5, through graphicx) is identical.

Tests: e2e 36/36, ssa-edits (`--brief --fixpoint`), the engine's and
core's tests (11 new in `png.rs`), clippy, rustfmt.

## 2026-10-03: `cargo xtask parallel`, the step graph measured in Rust

`scripts/ssa-parallel.py` took 456 s on the step graph of four of the
PGF manual's chapters (61,706 steps, 19.1 M reads, 7.7 M definitions);
the whole manual's is ten times that. `cargo xtask parallel`
(`xtask/src/parallel.rs`) is the same models and schedules in Rust:
20 s on those chapters (7.9 s of it reading the dump), with the same
numbers (every model's edges and critical paths, whole and pipelined,
the list schedules, the definitions passed through).

It adds speculation from a predicted entry state: each *segment* (the
steps between two loads of a file `--segments` matches, by default any
`.tex`: an `\include`d chapter) or each step, started from the state the
setup left (the steps before the first that ships), a read of a later
definition mispredicted unless its version is the setup's; which
classes of addresses block, and the units that validate as classes are
taken as predicted, the class that validates the most first.

## 2026-10-03: A cascade gone cold: the preamble edit at a cold build's cost

The Overleaf extension's preamble edit (`\usepackage{circuitikz}` added
to a one-page article with a TikZ picture and a `circuitikz`
environment) ran 25,038 new steps in 55 s natively, against 1.2 s for
the cold build. Each new step ran as a rebuild runs any step: the old
build's steps after it were still live, their definitions later than
it, so the reads predicted for it (those of the two steps predicting it,
about 3,100 slots) were placed at their reaching definitions before it
ran and put back after (77.2 M slots each way), and its reads were
checked for later definitions (8,997 runs dropped). A frame-pointer
profile put about 78% of the rebuild in that placing and putting back
(`run_step`'s candidates, `latest`, the B-tree of slots placed, the
values cloned and dropped).

A cascade (new steps until a run ends where an old step ended) now
weighs what it has cost (its commands, a slot placed or put back as
half of one) against the old steps after it (their last runs' commands,
looked at once the cost passes 50,000). Past them, the rest of the job
run cold costs less than going on: those steps are retired at once,
the last first (`go_cold`), and each new step after that is the fold's
last. No definition is later than it, so nothing is placed, checked or
put back, as in a cold build: but for the page's nodes, which the
arrays hold only up to the list's length. Those the step is predicted
to read are placed and put back, and a run that read one not placed is
dropped, as before. (A first version placed none of them: page 1's ship
read stand-ins and lost the empty `\write` LaTeX makes there, an empty
line of the log; PDF and `.aux` were the same.) The choice is ski
rental's: at most about twice what the better of going on and going cold
would have cost; an edit whose cascade meets an old step soon never
weighs anything.

The repro: 55.0 s → 1.9 s (LTO, 1.88 and 1.90 s; 25,037 new steps, 1
run dropped, 78 K slots placed, 129 K put back), the PDF identical to
plain runs of the two texts and the log too (its string statistics
aside, as before). `scripts/ssa-edits` has `preamble`
(`tests/e2e/preamble.tex`, the extension's document): the package
added, taken away, then a word.

Tests: ssa-edits 17/17 (99 stages, `--brief --fixpoint`), e2e 36/36,
clippy, rustfmt.

## 2026-10-03: The whole PGF manual: how much of a build could run at once

The step graph of the whole manual (the `.aux` files settled by two
plain runs, then one `PARTEX_SSA=1` process dumping each build's graph:
the cold build and edits). 236,040 steps, 389.7 M commands, 164.0 M
reads from outside a step, 64.5 M definitions. The setup (the steps
before the first that ships) is 0.1% of the commands. The body is 379
segments (the `\include`d chapters and the files they load); the
largest, `dv-stylesheets`, is 72.1 M commands (19%), so chapters at
once can give at most 5.38x.

| model | whole steps | pipelined |
|---|---|---|
| all reads | 1.00x | 1.00x |
| blind writes (a definition that does not read the one before it does not wait for it) | 1.55x | 2.89x |

Every step reads something the step before it wrote: TeX's cursors
(`align_state`, the conditional stack, the save stack's pointer and
level, `selector`, `str_ptr`). With blind writes, the classes ignored
one after another, pipelined: macros 6.38x, registers 10.25x,
parameters 18.69x, the save stack 106x, the hash table 330x, codes
2,156x, engine scalars 6,987x, counters 11,327x. The page builder's
edges alone (whole steps): 20.77x. The data flow is wide; the state
TeX keeps in one place is what makes it a chain.

Speculation from the setup's end (each unit from the state the setup
left; a segment runs its steps in order): 1 of 379 segments validates
(0.0% of the commands), 11,296 of 203,021 steps. What the segments
misread (blind writes): the hash table and macros (first uses:
`\T1/cmr/m/n/10` and the other fonts NFSS loads when first used,
xcolor's mixins, TikZ's animation attributes, `\hook_use:n`), counters,
registers (`\tikz@lastx`), the `\write` streams (the `.aux` at its
offset), codes.

Rounds (`xtask parallel`, new): every unit at once from the setup's
end; a unit that misread a definition of an earlier unit runs again in
the round after that unit's last; a round takes its costliest unit.
Segments: 287 rounds, 0.03x, 145x the work. Steps: 91,048 rounds,
1.04x. With classes predicted, the class that validates the most first
(steps, blind writes): macros 1.94x, the page builder 2.09x, parameters
12.4x, PDF writer and fonts 20.1x, counters 25.7x, codes 48.4x,
registers 428x (3.0x the work), the nest 999x (1.3x), the save stack
1,116x in 4 rounds (1.01x the work).

From the last build's records instead (each step of the word edit's
rebuild from its entry state in the cold build): 236,037 of 236,040
steps validate (the others misread `str_ptr` twice and a `\write`
sealed by the step before), a bound of 93,450x.

Edits, the whole manual in one process without the dump (a
frame-pointer profiling build of 824a511): a word, 116.5 ms (the first
rebuild after the cold build; 109.5 ms of it the rebuild), its revert
20.6 ms (14.6 ms); a TikZ coordinate, 153.8 ms and 58.4 ms; a
subsection's title, 182.1 ms and 210.2 ms. About 80 ms of the
coordinate's and the title's is the link deflating the new page, after
the page is ready (0.03 ms after the rebuild). The 2.1 to 11.9 s
measured with the dump on were the dump's 7.1 GB per build. Open: the
first rebuild's 95 ms more than its revert for the same 6 steps.

## 2026-10-03 — A terminal that moves: the live line, the watch's log and keys (agent cli-ux)

Branch `cli-ux`: `6532acc` (the progress board), `15d62d8` (the
terminal), `b80f57a` (rustfmt of seven files of main), `375b998` (this
entry), `85932d6` (the bar's width steady, a watch's line shorter).

**Why.** `partex watch` printed `Rebuilding (inputs changed)` and then
`Pass 1 |  0 ms`, which stayed as it was until the rebuild was over.
The cause was in two places. The live line was drawn only by the
build's own thread: at a pass's start, and at each page a session
shipped (`Live::Page`, sent by the session's host). Nothing drew it on
a timer, so while the engine ran between pages its time and spinner
stood still. And machine mode, the default of `partex watch` and
`partex build`, never connected that page sink at all: during a
machine rebuild nothing called the drawing code, whatever the engine
did.

**The progress board** (`partex_core::progress`). The engine posts to
a board of relaxed atomics: every 1024 commands the commands run and
the file and line being read (the name copied only when it is another
file, by an FNV hash of its bytes, under a sequence lock, so a reader
never sees half a name), at each page shipped the count and `\count0`,
and when it starts to finish the PDF file a phase. It is observability
only: no tracker sees it, the engine never reads it, a replay or a
rebuild leaves it as it is. Between posts a command pays a test of its
count's low bits (`post_progress` is out of line, `#[cold]`). The board
is one for the process; a reader takes differences.

**The live line** (`live.rs`). A thread of its own draws the area
below everything printed, every 80 ms while a task runs and whenever
the state changes, sampling the board: `Pass 1 ⠹ ━━━━━━━━━━━━╺━━━━━━━━━━━
54% page 19/74 · big.tex:11  1.1 s · 0.9 s left`. The build's thread
only says what runs (a pass, or a phase: loading the saved build,
linking, writing, saving) through `Progress`, which gained `Phase`. A
build from the start shows a bar and the time left from the last full
build's totals (commands on the board, pages, time), kept in the cache
by directory and engine command line; the time left leans on the last
build's time early and on this one's rate later. Printing goes above
the area in one write, in synchronized output (`ESC [?2026h`), and
the area is cut to the terminal's width so a redraw always finds it. A
frame composed before a change is not drawn (a generation count), and
a change made while a frame is drawn is drawn next (a first version
waited for the next change and lost it: the footer kept the time of
the build before). Nothing is drawn for a build quicker than 150 ms.

**The result and the watch** (`render.rs`, `modern.rs`). One line per
build: `Finished big.pdf · 74 pages · 244 KB · 1 pass · 1.47 s · 10
warnings` (the PDF an OSC 8 hyperlink where the terminal has them). A
watch logs one line per rebuild, `18:13:53 ↻ big.tex:10 ✓ big.pdf ·
1.72 s · 54% run again` (the first line each edit changed,
from the machine's old and new contents, `Watch::last_changes`; the
pages and warnings only when their counts changed), then
the errors that are new in full (one that stays is its headline, `as
before`), the warnings that are new, and how many went, under a footer
`Watching big.tex · ▁▃█▁ last 372 ms · r rebuild  o open  q quit  w
warnings  ? help`. On a terminal in the foreground the watch reads
keys: echo, line editing and the signal keys off (`stty`), so Ctrl-C is
a key: the watch stops after saving its build (as `q`; a second Ctrl-C
stops it at once, with 130), and during a rebuild it says it will stop
once the rebuild is over. Ctrl-Z restores the terminal and stops the
job's process group as the shell's Ctrl-Z would, and sets the modes
again when it goes on. A watchdog `sh` started with the modes waits on
a pipe: if partex dies without restoring them (killed, crashed), the
pipe closes and the `sh` shows the cursor and sets the modes back. The
panic hook restores them too. Errors mark what they are about: the
undefined control sequence where the line has it, else the call of the
macro it happened in (`\greet{world}`), else the last token, and the
column is the mark's start (`modern.tex:17:4`, was `:29`, after it).
Package warnings get their own headline (`warning: lipsum: Unknown
language`). Off a terminal the same lines come out plain, as they
happen; `--color`, `NO_COLOR`, `CLICOLOR_FORCE` and `CLICOLOR` choose
the colours.

**No dependency.** The terminal's size (`stty size`, asked at most
once a second while something moves) and modes (`stty -g`, `stty
-icanon -echo -isig`) are `stty`'s, run on the terminal's descriptor;
whether this process may set them is `/proc/self/stat`'s foreground
group. A crate (`rustix`, `libc`) would have saved a few `stty`
processes per session, against the rule of no dependencies and no
`unsafe`.

**Found on the way.**
- Every cold machine build printed `partex: machine: candidates by
  level …` (the census, a debugging report) into the modern terminal;
  it is now printed only with `PARTEX_WATCH_DEBUG` or the cut timing.
- Saving the store after `partex build` takes seconds (1.5 s for the
  74-page document, 4 s for a 12-page one on this loaded machine) and
  was invisible: the process seemed to hang after its result. It now
  has a live line (`Saving ⠹ the build for the next run  0.8 s`).
- `main` at `824a511` failed `cargo fmt --check` in seven files of the
  PDF inclusion (`fontmap.rs`, `epdf.rs`, `writefont.rs`, `fofi.rs`,
  `fofi_tables.rs`, `gfxfont.rs`, `pdfread.rs`); `b80f57a` formats them
  and nothing else, and can be dropped if their author formats them.

**Measured** (this machine, loaded, release builds with fat LTO, the
branch against `824a511`):
- a 74-page LaTeX document (lipsum, hyperref), one plain pass
  (`--compat=pdftex`), 20 runs of each alternating: instructions
  3.2135 G → 3.2145 G (+0.03%), cycles (median) 1.6846 G → 1.6850 G
  (+0.02%);
- the same document's `partex build` from the start on a terminal (the
  live line drawn; no store, no saved session), 12 runs of each
  alternating: wall time 2.52 s → 2.52 s (median), user 2.065 →
  2.075 s;
- the course (299 pages), one plain pass, 2 runs of each alternating
  (`scripts/heavy`): instructions 274.63 G → 274.93 G (+0.10%), cycles
  146.9 G → 147.0 G (+0.1%), wall 55.4 s → 55.2 s. The PDF is the same
  size (3,184,359 bytes); e2e compares the bytes.

**Tests.** e2e's `modern` now checks the last line's facts (`Failed
modern.tex`, `1 error`, `3 warnings`, `1 page`, `3 passes`) instead of
the old text, and the error's column 4 (the mark's start) instead of
29; `modern_watch` waits for `-v`'s `Machine rebuilt in` after each
edit instead of the old result line's `modern.tex (`. The renderer's,
the terminal's and the board's unit tests are new (`render.rs`,
`live.rs`, `term.rs`, `snippet.rs`, `progress.rs`). In a
pseudo-terminal (a Python driver with a small VT100 model, not kept in
the tree): a cold
build at 60, 80, 100 and 120 columns, a watch's rebuilds, an error and
its fix, the keys, Ctrl-C during a rebuild (once, twice), Ctrl-Z and
`fg` under an interactive bash, and partex killed (the watchdog showed
the cursor and bash had the modes back). `cargo xtask check` passes:
fmt (with `b80f57a`), clippy (both feature sets), the wasm build, the
workspace's tests, trip 7/7 and etrip 18/18 (plain and machine mode),
e2e 35/35 (plain and machine mode), ssa-edits 16/16 (`--brief
--fixpoint`) and 16/16 (`PARTEX_SSA_TRIPS=1`).

## 2026-10-03: Glyph origins: each glyph of the PDF and the source bytes it came from (branch `synctex`)

For an editor beside the PDF (its renderer draws one glyph per character
code a text-showing operator shows, so the origins are given in that
order): `Tex::set_origins(true)` before the cold build, then after each
build or rebuild `Tex::origins(page)`, one `GlyphOrigin { file, start,
end, synthesized }` per code the page's content stream shows, forms at
each `Do`, and `Tex::origin_files()`. `PARTEX_ORIGINS=1` writes them
beside the PDF (`<job>.origins.jsonl`). DESIGN 4.4 has the rules;
`partex_core::srcmap`'s documentation is the reference.

How, and what was rejected:

- **A side channel in the nodes.** A parallel structure (a map from
  nodes to origins, or per-list vectors) would have had to follow every
  list operation TeX does (copies, `\unhbox`, line breaking's splits,
  hyphenation's reconstitution, `\vsplit`, the page builder). A handle
  inside the node follows by construction. `Glyphs` gave up a byte of its
  inline characters (15, not 16) for a 32-bit handle, so a node stays 24
  bytes; a `Ligature` has a handle; a `TokenList` has one per list (its
  tokens' entries consecutive). Handles are outside `PartialEq` and
  `Hash` (a `Side` wrapper that is always equal): no version, record or
  SSA decision depends on them. The table (`OrgTable`) only grows, so a
  handle in an old step's record keeps its meaning.
- **Synthesized ranges.** A glyph made by a macro body or `\the` has no
  bytes of its own; it gets the range of the call in the innermost file:
  from the start of the last command taken from that file (expanded
  there, or executed by main control) to the read position. A first
  version took only main control's fetches and gave the section number
  of `\section{Intro}` the range from `\newsavebox` three lines up:
  `\begin{document}` ends in `\ignorespaces`, whose `get_x_token`
  expands the next line's `\section` without main control taking
  anything from the file.
- **Arguments keep their bytes** (the coordinator's change of the
  contract, approved): `macro_call` records each argument token's origin
  as it stores it, and the argument's list carries them, through
  parameter substitution (`expand::apply`), `back_input` and
  `\expandafter`. A list with origins is read a token at a time (the
  bulk readers skip it), only when origins are on.
- **Order by construction.** The encoder emits a stream's list of
  handles as it writes the stream: glyphs as `pdf_print_char` writes
  their codes, a form marker at `Do`, no-source runs for literal text
  (counted by `partex_engine::pdftext`, the same walk the checker uses),
  pdfTeX's fake and interword spaces, and an included page's codes
  (counted once per page). In SSA mode the list is the ship step's
  effect, so reused steps keep theirs; the edits since (old and new
  data, the changed lines) map each origin to the current text when it
  is read.

Tests: the `glyphs` e2e job (`tests/e2e/glyphs.tex`: text, ligatures, a
macro, `\thesection`, hyphenated breaks, a TikZ node, a reused
`\savebox`, a form drawn twice, a literal's text, an included PDF page,
two pages) runs pdfTeX and partex with origins on (every file the same
but the side file), checks every page's glyph count against the PDF's
content streams, each one-byte letter or digit glyph against its source
byte, and chosen runs (`Hello`, `fi`/`ffi` ligatures, `bar` from `\foo`
synthesized as `\foo`, `1` as `\thesection`, `Saved` twice with the same
bytes, the break hyphens as the character before them, the literal's
three no-source codes, the figure's 19); then SSA mode, edited twice
(a comment line inserted before a paragraph: the second page's ship step
kept, its origins moved; then a paragraph inserted), each rebuild's side
file and PDF equal to a cold build's of its text. `cargo xtask origins
DIR JOB [--dump]` checks any run.

Costs (the PGF subset, 115 pages, plain run, against 824a511 built the
same way, `perf stat -e instructions:u`): off, +1.27% instructions at
first (147.26 G against 145.42 G). A profile put it in the hooks that
were calls (`origin_fetch` alone 0.68 G: an `#[inline]` hint the
compiler did not take) and in the bulk readers' test of the list's
origins, which read the list with origins off. Each hook is now an
always-inlined test of `Tex::org` with its body out of line, and the
bulk test reads nothing when origins are off: +1.02% (146.90 G); the
wall time is within the machine's noise. On: +10.0% instructions
(159.90 G), peak RSS 132 MB to 187–205 MB for 237,566 glyphs, mostly
argument tokens' origins (left to make compact); the side file 4.7 MB.

Also on the branch: the rustfmt main gave the font replacement commit
(`89fd1db`), so that `cargo fmt --check` passes here too.


## 2026-10-03: The outline, in the source and on the PDF (branch `synctex`)

The editor's outline pane (Overleaf's file outline): `phitex_doc`'s
static layer already read every heading; `Project::outline()` now gives
each as an `Entry` with its bytes (the command and its title: the
paragraph's offset in its file added to the fact's, the title's start
kept by the scanner, none through a document macro), its line and
LaTeX's number, before any build, and `Outline::section_at(file,
offset)`. `Outline::place` puts each heading on the PDF from a build's
glyph origins: the first glyph whose origin lies inside the title's
bytes and is not synthesized. That rule finds the heading and not its
other appearances: the table of contents reads the title back from the
`.toc` (origins in that file), a running head from a mark (synthesized).

The point is the glyph's origin on the page, which needs the content
stream walked with positions: `partex_engine::pdftext` now keeps the
text matrix, the CTM, `Tf`'s size, `Tc`, `Tw`, `Tz`, `TL`, `Ts`, the
fonts' widths (`/Widths`, a Type 3 font's `/FontMatrix`, a CID font's
`/W` and `/DW`), `q`/`Q`, and a form's `/Matrix` at its `Do`, so each
code it shows has its place, in the same walk the glyph origins are
counted by. On `glyphs.tex`, `Intro`'s `I` is at (157.977, 657.235):
`1` at 133.768, its `/Widths` 562.5 thousandths of 14.3462 pt, then the
`\quad` kern of 1125; MuPDF puts it 0.007 pt further right (it takes
the advance from the font program, not `/Widths`).

`partex outline FILE.tex --json` places the entries from the side file
next to the PDF (`<job>.origins.jsonl`, `<job>.pdf`; `--origins PATH`
for another): reading a side file costs nothing and keeps the command
static; running a build from the outline command would not.

Tests: `phitex-doc`'s `outline` tests (entries, ranges and numbers of a
document with an `\input`; `section_at` in the main file and inside the
input before and after its heading; placement from glyph lists,
synthesized glyphs passed over), and the `glyphs` e2e job's check of
`partex outline glyphs.tex --json` against the heading's place in
pdfTeX's PDF.

## 2026-10-04 — SSA optimizations as each step closes (branch `ssa-opt`, agent ssa-opt)

The question: which classic SSA optimizations, applied as each step is
built and closed, make the recorded graph more precise without losing
soundness, so that an edit reruns fewer steps. The base is `cancel` at
904ea54 (soft reads and class reads on). Measured with fastdev builds,
counting steps and commands, not times. The rebuild trace now says why
each step runs: `step N runs for: SLOT...`, which lists the slots whose
reaching definition is not the version the step read, or `a mark` (a
seed, a store, or the input after a step that ended elsewhere).

**Where the steps go.** pt (one edit: 3 steps), acro2 (15), ac3 (14)
and the course's word edit (8, seven of them seeds) were already
minimal. On a 64-page thesis (cold: 63,816 steps in trip 1, 4 trips), a
word or a space in body text reruns 6 to 22 steps. Three edit kinds
cascaded:
- a section title (986 steps);
- an insertion long enough to reflow pages (588);
- a line break added mid-chapter (547).

**What was wrong: a soft read decided something.** A local assignment in
a group the step opened reads the old value only softly, so a rebuild
never places that slot, and the run finds whatever the arrays hold:
often a later step's definition. TeX decides two things from that value:
- whether to save it (§277: its level against `cur_level`);
- whether to assign it at all (e-TeX's `reassigning` when equal).

In the section edit, step 65990's cold run found `\=` at level 5 and
equal to the new value, so it skipped the assignment. The rebuild's run
found the level-1 value, assigned it and saved it. The step's
definitions changed (`\=`, `\-`, `\'`, `` \` ``, `\lineskip`,
`\vfuzz`), and the save stack below every following window of a long
figure shifted. That changed 111,738 save-entry definitions over 880
steps. The output was right either way: the cascade recomputed
everything. The cost was the cascade, and a run whose behaviour hung on
an unrecorded value.

**Built** (DESIGN 3.12, "As each step closes"):
- *A soft read decides nothing* (`PARTEX_SSA_SOFT_PLACE=0` turns it
  off). A slot that holds the step's entry value, assigned in a group
  the step opened (`Tracker::entry_value`), is always saved and
  assigned, as a consistent state would do. An entry value still saved
  at the step's end (`SsaTracker::entry_saved`) is checked in a rebuild
  like a read: at a later definition the run is dropped and the slot
  placed.
  - Rejected first: placing every soft-read slot like a read. It cut the
    section edit to 96, but the extra dropped runs tripped `go_cold` in
    the reflow edit's second trip (564 + 1,592 steps instead of 562 +
    26).
- *Dead save stack entries are not definitions*
  (`PARTEX_SSA_DEAD_SAVES=0` turns it off). An entry at or above the
  pointer at a step's end is never read before being written again. A
  rebuild therefore places the stack below a step's pointer whole. An
  entry made a plain value drops a stale object left there
  (`mark_save`), which its version carried.

**Measured** (steps / commands of the rebuild, all trips; base is this
binary with `PARTEX_SSA_SOFT_PLACE=0 PARTEX_SSA_DEAD_SAVES=0`, the
same as `cancel`'s numbers where both were run):

| edit | base | ssa-opt |
|---|---|---|
| pt, acro2, ac3 | 3, 15, 14 | 3, 15, 14 |
| thesis: word, space, 5 other one-letter edits, a figure's text, an acronym use | 6–22 | the same |
| thesis: a section title | 986 / 3,637,766 | 84 / 51,168 |
| thesis: a line break added mid-chapter | 547 / 2,019,908 | 169 / 447,021 |
| thesis: a page-reflowing insertion | 588 / 545,564 | 590 / 530,012 |

Every thesis rebuild's PDF is byte-identical to base's, and for the
section, reflow and line-break edits to a cold SSA build of the edited
source.

The rerun check (`PARTEX_SSA_TRIPS=1 PARTEX_SSA_RERUN_CHECK=1`), as
definitions changed / ended elsewhere / runs dropped:

| document | base | ssa-opt |
|---|---|---|
| pt | 14 / 4 / 9 | 10 / 4 / 8 |
| acro2 | 15 / 1 / 20 | 5 / 1 / 19 |
| ac3 | 18 / 1 / 20 | 7 / 1 / 19 |
| thesis | 3,001 / 23 / 795 | 680 / 23 / 779 |

On the thesis, effects changed stays at 5. The rest are mostly
`save[8]` in fire steps, and the windows before a fire that end
elsewhere with an error.

**What is left, and what blocks it** (DESIGN 3.12, "Analysed, not
built"):
- *The reflow edit's 300 steps are PDF object numbers.* `pdf.last[6]`
  went 0439→0437: two objects fewer, so every later number shifts. Each
  later step that makes a destination or a link reads `pdf.objs`,
  `pdf.obj_trees` and `pdf.dests` whole and defines them anew. The
  numbers are in the PDF's bytes, so only numbers resolved at the link
  cut this: virtual ids, as machine mode's `vnum.rs` has, with each
  writer scope reading the table's shape, the entries it uses and the
  numbers it asks for.
- *The line-break edit's 101 `cond` steps.* The conditional stack is one
  slot that holds each open conditional's absolute `if_line`. One slot
  per level for the lines, read only by the messages that print them,
  would cut it.
- *A relative save pointer* (a step that leaves `save_ptr` as it found
  it and never pops below it neither reads nor defines it). It is sound
  under that condition, but the data showed no chain through the
  pointer once the soft decisions were fixed.
- *Value numbering, copy propagation, redundant stores.* The memo, the
  copy model of the save stack and backdating already give them. A
  redundant store is a firewall here, not a false dependency. The
  rebuild passes over a marked reader whose read definition comes back
  at the same version.

Oracle for `45f91fe`, both on accl: `edits --brief --fixpoint` gave 17/17
cases identical over 99 stages (job 6573; the base 904ea54 gave the same,
job 6575), and `gate` exited 0 (job 6574: e2e 37/37, trip and etrip
identical, clippy clean).

## 2026-10-04 — A stopped rebuild keeps its work (branch `resume`, agent resume)

**Why.** The Overleaf extension rebuilds once per keystroke and wants
the next keystroke to cancel the rebuild under way without losing what
it did. A rebuild that stopped (past `SsaTracker::deadline`, past
`SsaTracker::budget`, or `SsaTracker::cancel`) used to report
`unsupported`. The steps it ran stayed in, but the dirty steps it had
not reached were dropped, so the program no longer matched the source,
and only a cold build was sound (the CLI exited 3).

**What changed** (DESIGN 3.7, "A rebuild stopped"):
- It stops only after a step's run is placed (the cases that end a
  cascade, or before it would insert a new step). The dirty set left
  (`Dirty`: each step's why, the missed slots) and the trip's φ go to
  `Steps::pending`. The report says `stopped` (why) and `pending` (how
  many steps), with `unsupported` left `None`.
- A stop inside a cascade that runs on (the step ended elsewhere) marks
  the old step after it (`mark_next`), as when an input changed. That
  step then runs from where the stopped one ended, and the cascade's
  other state (target, prediction, `ahead`) is not kept. A run can cost
  one extra step, but it stays sound: the old step's run meets its own
  old end or an old step's later on. When no old step follows, it does
  not stop.
- `rebuild` takes the pending dirty set as its seeds, beside the new
  edits' seeds, and the stopped trip's φ over the files' (the link
  waited, so the files are the last complete build's). A new edit is
  diffed against the data each step read: an edit before, inside or
  after the pending region marks the steps whose lines it changed, and
  each step's end is mapped through every edit since its run. These are
  the same mechanisms that already serve steps left unrun across many
  rebuilds. `rebuild_trips` and `more_trips` return at a stop, and no
  trip end is computed from a stopped trip. `settle`, with work pending,
  goes on as `rebuild_trips`.
- The link waits: nothing is linked while work is pending.
  `ssa::pending(tex)` gives the count.
- CLI: with `PARTEX_SSA_REBUILD`, `PARTEX_SSA_CANCEL_AFTER=N` now counts
  from each rebuild's start, and the cold build is not cancelled. A
  stopped rebuild prints `stopped (…), N steps pending` and links
  nothing. The next line's rebuild prints `continued …`. After the last
  line, the work left runs unstopped (no deadline), and then the link
  runs. Each rebuild also reports the entry check.

**The check** (`scripts/ssa-stop`): edits, each rebuild cancelled at
its N-th step boundary, then the continuation. The PDF is compared with
a cold SSA build of the final source (SOURCE_DATE_EPOCH=1758800000,
FORCE_SOURCE_DATE=1), and the entry check must report 0 bad reads.
fastdev binary, this machine:
- `pt.tex`, 3 edits (one before the pending steps, one inside them),
  N = 1..14: all identical.
- `acro2.tex`, 2 edits, N = 1..40: all identical (stops in trips 1 and
  2).
- `ac3.tex` (`\include`), 3 edits in two files, N = 1..40: all
  identical.
- The thesis (private copy), the Chapter 2 edit alone, N = 1: identical
  (rebuild 1 stopped after 1 step with 4 pending, and the continuation
  ran 5 steps in 50 ms).
- The thesis, 3 edits (Chapter 2, then Chapter 3, then Chapter 1, the
  last one before the pending steps), N = 1, 2, 3, 4, 6: all identical.
  The exit code is 1 both cold and rebuilt (the document's own
  errors), and the entry check finds 0 bad reads at every rebuild.

Gates on `52f8199` (accl): `gate` exited 0 (job 6578); `edits --brief
--fixpoint` gave 17/17 cases identical over 99 stages (job 6579).
## 2026-10-03: SyncTeX, byte for byte, in plain runs and SSA rebuilds (branch `synctex`; gate and cost not yet seen)

`-synctex=N` writes pdfTeX's `.synctex.gz` (DESIGN 4.5): `synctex.c`'s
controller ported, fed by events where pdfTeX's hooks are, nodes'
places as handles outside their values.

Checked against pdfTeX 1.40.29 run in the same directory (the `Input:`
lines are absolute): a plain TeX document (font kerns, ligatures, math,
rules, leaders of both kinds, `\copy`, an alignment, a footnote,
`\vadjust`, discretionaries, hyphenation, accents, forms, a display, two
pages, an `\input`), the same file, terminal and PDF for `-synctex=1`,
`-1`, `2`, `4`, `8`, `9`, `15`, `-12`, `0` and batch mode; the e2e
`glyphs.tex` (LaTeX, TikZ, graphicx, a savebox, a form, an included
PDF). In SSA mode the cold build's file is the same, and so is each
rebuild's against pdfTeX on the edited text: twelve edits of the plain
document (a comment line, a paragraph inserted with the next one's first
words, a word, blank lines, a line deleted, two lines joined, a line
split, an edit in the `\input` file, a line of an alignment split) and
six of `glyphs.tex` (the same kinds, and a TikZ node's text).

What it took, beyond the port: pdfTeX in e-TeX mode turns glue set with
its box into a kern while shipping it out ("Handle a glue node for mixed
direction typesetting"), so its record is `k` with the glue's width;
the glue at a line break is reused as `\rightskip`, keeping its line;
`synctexcurrent`'s `=` compares the context's `curv`, not the point it
prints. In SSA mode a rebuild keeps a step whose reads are the same,
and with it the nodes it made, though a step run again before it made
equal nodes on other lines (two lines joined: the paragraph's lines
kept, their nodes' places stale): places are hashed there, each a handle
of its own, so that a node made again is another version.

Then (65e8322): a document's own `\synctex` turns it on as pdfTeX's
controller does. Files are counted from the job's start whatever the
setting (the counter a scalar row), the first file's name is kept for
`Input:1`, and a page shipped while `\synctex` was 0 leaves it off with
pdfTeX's warning; the `.synctex` files of an earlier run are removed at
the end as pdfTeX removes them. In SSA mode the controller's warnings
are printed by the steps (its flags a scalar row). Checked against
pdfTeX, plain and SSA: `\synctex=1` on the first line, in a LaTeX
preamble, after the first page (the warning), and with `-synctex=0`
(the warnings). A limitation none of these meets: pdfTeX gives a node
its place when it makes it, whatever `\synctex` is; here a node made
before `\synctex` is set gets its place where it next enters a list,
box or register.

`cargo xtask e2e` has a job `synctex`: pdfTeX and partex in one
directory, `-synctex=1`, `-1`, `12`, a document's own `\synctex`, and
LaTeX's `glyphs.tex` run twice; the files compared byte for byte, then
`synctex view` for every line and `synctex edit` on a grid of points on
each page asked of both, the answers compared (without the `synctex`
program it says so and compares only the files). It passes here, with
the program. `scripts/ssa-edits` has a case `synctex` (its files
compared as text, the two run directories' names replaced).

Checked on accl at 65e8322: `accl run edits --brief --fixpoint` (job
6523): 18/18 cases identical, 106 stages, `synctex` among them (7
stages). Not seen when this was written: the gate (job 6522: fmt,
clippy, `cargo xtask check` with the e2e suite, whose other jobs are
SyncTeX off's byte identity), and the cost off and on (job 6525,
`scripts/accl/tasks/synctex-ab.sh e5354cb 5`: the PGF subset, the
parent commit against this one off and on, five rounds alternated, the
PDFs compared; results in `partex-phitex-runs/cmd-6525`).

Not done: the DVI mode, and persisted builds and sessions with SyncTeX
(DESIGN 4.5 says what each would take).

## 2026-10-03 — Display lists: each page's drawing without the PDF (agent display-list) — partial

For the PhiTeX Overleaf extension's preview, which today links the whole
PDF and re-parses it to draw one page (about 37 ms of a 65 ms
keystroke): a per-page display list from shipout, so the PDF is linked
only for the download. DESIGN 4.5 has the design;
`partex_core::displist`'s documentation is the reference.

What was built:

- **The walk** (`partex_engine::pdftext`): `Text` gained an operator
  hook (`op`, with the token's offset and operands), a glyph hook with
  the text matrix and state (`shown`) and `end`; `pdftext::list` makes
  a stream's items (glyphs, glyph matrices, pdfTeX's rules from `re f`
  and its stroked thin lines, literals by the spans the ship noted,
  `XObject`s with the CTM), `pdftext::glyphs` the codes in glyph
  origins' order, forms walked where drawn. `pdfread::number_value`
  reads a number as the PDF reader does, for widths and boxes.
- **The record** (`displist.rs`): at each content stream's end the ship
  keeps the stream's bytes (`PdfOut::stream_bytes`: `zip`'s, or the
  pending bytes since the stream's start at level 0), the literals'
  spans (`emit_literal`), the fonts (`FontRec`: TFM name, map entry,
  size, the TFM's widths as `/Widths` prints them), forms and images, and
  the box; a page's after its page object (`\pdfpageattr`). SSA:
  `Effect::Display`, collected from the steps' effects like origins.
- **The API**: `Tex::set_display_lists`, `display_pages`,
  `display_hashes`, `display_list`, `display_form`, `display_glyphs`,
  `display_font`, `display_image`, `display_stream`, `display_forms`.
  Font ids are interned per engine (stable across rebuilds); form and
  image ids are object numbers. `writet1::builtin_encoding` reads a Type
  1 file's own encoding; `epdf::included_form_box` an included page's
  `/BBox` and `/Matrix`.
- **The side file** `PARTEX_DISPLAY=1` (`partex-cli/src/display.rs`):
  `<job>.display.jsonl`, fonts numbered by first use so a rebuild's file
  and a cold build's of the same text are the same bytes.

Rejected: building the items in the encoder from pdfTeX's positions
(`pdf_h`, `delta_h`): exact in sp, but not the PDF's (its rounding of
`Td`, kerns and `/Widths`), and a second implementation of the viewer's
arithmetic; walking the linked PDF: the point is not to link.

Tests (all passed): `pdftext`'s unit tests of `list` and `glyphs` (text,
a rule, a thin rule, a literal with text and a `cm`, scaled text, a
form, an image, a turned rule); the effects' round trip with
`Effect::Display`; the `display` e2e job (`tests/e2e/display.tex`: fonts
and sizes, math, rules and a table, colors, `\rotatebox`,
`\scalebox`, TikZ with an opacity and a turned node, a form drawn twice,
an included PDF page, a PNG, literals in each mode, a page with
`/Rotate 90`) against pdfTeX (every file the same with display lists
on) and each page's list against `pdftext` over the PDF: 254 items, 152
glyphs placed bit for bit; `microtype.tex` (font expansion, its `Tm`:
glyph matrices 0.98 to 1.03): 3,741 items, 3,419 glyphs; SSA rebuilds
(a comment line, then a paragraph inserted) the same side file and PDF
as cold builds, each checked; the `glyphs` job with display lists on too
(plain and both SSA rebuilds, glyph counts equal to origins'). fmt,
clippy (both feature sets) and the wasm32 check pass.

Verified on accl at c5e027d: the full gate (job 6515: fmt, clippy,
`cargo xtask check`, the e2e jobs including `display` and `glyphs`,
38/38 cases) and the SSA edits (`edits --brief --fixpoint`, job 6521:
17/17 cases identical). A run of the edits without `--fixpoint` (job
6516) differs only in the aux round-trip cases (readback, incremental,
machine_edits, label, streams, fatal_end, windows), as main does
without it.

`PARTEX_DISPLAY=keep` keeps the lists and writes nothing (what keeping
them costs a build); with `PARTEX_DISPLAY=1` stderr reports how long
each page's list took to make and hash and to write.

Left (wrap-up at the user's request):
- **Cost not measured.** The harness (`bench/display-cost.sh`, accl cmd
  job 6517) built both binaries and their formats, but every run exited
  with status 2 in 0.00 s, so there are no numbers. The cause, not
  looked into, is in the harness's `run` (likely `/usr/bin/time` or the
  `env` call in the container), not in partex. Lists off still has to
  be compared with c651cf3, and lists on measured.
- **Resource resolution is unverified.** It is on branch
  `display-resources` (8c0e3f3, not for merge):
  - `PageList::resources` and `attrs` as `PdfValue`:
    `\pdfpageresources`, `\pdfxform resources` and `attr`, and
    `\pdfobj` objects (`Effect::DisplayObj`), with a form or image
    named by its id;
  - page hashes become a walk over every stream reached;
  - the checker compares resources and attributes with the PDF.

  It is clippy clean, but no e2e has run on it, and `display.tex` does
  not yet have shadings, patterns or fadings. Until it lands, a list
  carries the literal (`/pgf@CA0.5 gs`, `/Sh sh`, `/pgfpat3 scn`) but
  not the resource it names.
- **Ids.** Form and image ids are object numbers, which an edit that
  makes objects before them can change. Images are recognized by their
  bytes.
- **The contract.** DESIGN 4.5 is a short note. The extension's
  contract, including the `display_` names and the id-stability rules,
  is in the documentation of `pdftext` and `displist.rs`.

## 2026-10-04 — e-TeX's saved registers above 255, entry by entry (branch `xchain`)

**The false dependency.** `save.xchain` (e-TeX's chains of registers
above 255 saved locally) was one slot whose version hashed every saved
entry of every level. Each local assignment of a register above 255 in
a group read and wrote it whole, so a saved value that changed anywhere
in the chain made every later step touching such a register run again.
On a private two-edit document (a tikzpicture after the edited
paragraph): the paragraph's last line gained a descender
(`list.prev_depth`, a real change), the next window saved a register
holding it, `save.xchain` changed, and steps 19234–19255 ran for
`save:4=save.xchain` alone.

**The change.** The slot is now the chains' shape (the current chain's
level, each level's length), and each entry is its own slot
(`save::XENTRY + i`, the chains laid end to end, outer levels first;
`save.xchain[i]` in traces). Entries are immutable once pushed: a save
reads and writes the shape and writes its entry; a restore reads the
shape and each entry it restores and writes them (dropped, so a rebuild
placing an earlier shape finds every entry's later definition) and the
shape. Placement: the shape (`set_chain_shape` keeps the entries laid
end to end in place, drops those past it, stand-ins for missing ones)
and each entry within the reaching shape that a later definition holds
are placed with the save stack (`save_stack_whole`). The entries are not
dead-save candidates (`DEAD_SAVES` filters `ENTRY..XENTRY` only): the
chains' vectors hold nothing past their end.

**Measured** (fastdev, this machine, the document above, rebuild 2 of
"x" then "y"): 36 steps changed, 258 ms → 16 steps, 43 ms. Entry check
0 bad reads at every build; the rebuilt PDF equals a cold SSA build of
the final source. `pt`, `acro2`, `ac3` (one edit each): 0 bad reads,
PDFs identical to cold SSA builds.

The course's word edit (fastdev, this machine, entry check on): 0 bad
reads cold and rebuilt; rebuild 1 links 8,006 steps changed (8,053
before), 8.1 s. Its rebuilt PDF is byte-identical to the one the
previous binary rebuilds (both differ from a cold SSA build of the same
local copy, as before this change). Gates on `29390b4` (accl): `gate`
exited 0 (job 6584); `edits --brief --fixpoint` gave 17/17 cases
identical over 99 stages (job 6585).
## 2026-10-04 — The log's and the terminal's columns are the link's (agent offsets)

**Why.** `term_offset` and `file_offset` (`alloc:7`, `alloc:8`) were
scalar slots that every printing step read and wrote. TeX reads them
only to decide what to print: the wrap at `max_print_line` (§58),
`print_nl`'s new line (§62), and the space or new line before a page's
`[` (§638), a file's `(` (§537), a `\message` (§1280) and
`\scantokens`' `( `. One message a character longer changed the column
for every later step that printed, so each of those steps ran again,
and its own end column, a changed definition, woke the next one. On the
thesis (64 pages; the edit `big data applications.` → `big data
applications. x` in `Chapter_2.tex`), 261 of the changed definitions over
the cold build's trips and the rebuild were these two slots.

**What.** In an SSA build the columns are output position, like an
object's offset (DESIGN 3.8, "Columns are the link's"):
- The engine records what it prints to the terminal and the log as
  `Effect::Flow` ops (`effects/flow.rs`): runs of characters, `print_ln`,
  and each column decision as an op of its own (`NLC` for `print_nl`,
  `SEP` for the space-or-new-line rule, with its threshold), raw bytes
  (`wlog`, `wterm`) and a byte count's digits (`LEN` … `LEN_END`). The
  engine keeps its own columns as TeX does, untracked; they no longer
  are slots (`offsets_read` and `offsets_wrote` do nothing).
- The link renders each step's flow from the columns the step before it
  left (`ssa::resolve_flows`, at `take_step_changes`): the steps whose
  chunks changed, then each next step while its columns at the end come
  out other than they were. A flow chunk's version is its rendered
  bytes', so the splice sees a step whose text moved as a changed chunk,
  and nothing else. The splice and the full link are as they were.
- A flow reads `Out(LOG)`, the log's being open, when it begins: a step
  run again in a later trip found the arrays holding the log closed (the
  job's end's definition), recorded its text for no log, and lost it (the
  first cut of this lost `(./Acknowledgment.aux)` and 20 lines after it in
  the thesis's cold build). The read makes the rebuild place the slot,
  as the log's flush read it before.
- `PARTEX_SSA_FLOW=0` keeps the columns as slots.

**Measured** (thesis, fastdev, native, defaults): the "applications. x"
edit's changed definitions of `term_offset`/`file_offset` over the build
and rebuild went from 261 to 0. Its rebuild still runs 5,800 steps (5,523
new): windows of 4,096 commands in a long TikZ stretch re-cut after a
window ran a different number of commands, which the columns did not
cause (below). The rebuild's PDF and log are byte for byte the base's
(9f13c82) rebuild's; the cold build's are the base's cold build's. Both
rebuilds differ from the cold build of the edited source (a "Float too
large" warning and a page's fancyhdr warning; the PDF too): a defect
that was there before this change.

**The save stack chain, looked at.** On that edit `save.ptr` is not in
the rebuild's chain (no step runs for it). What chains is
`save.xchain` (93 changed definitions, 90 steps run for it) and
`save[k]` (617): e-TeX's chain of saved registers above 255 is one slot
holding every level's saved entries, read and written whole by each
`\dimen324`-style save or restore, so a window deep in a TikZ figure
reads the outer levels' saved values too.

## 2026-10-04 — Virtual PDF object numbers in SSA mode (agent pdfnum)

**What prompted it.** The extension team measured the PDF object table
as the largest chain of a reflowing edit: `pdf.objs`, `pdf.obj_trees`
and `pdf.dests` were each one slot read whole by every writer scope, so
one link annotation fewer renumbered every later object and every later
step that made a destination, a link or a page ran again (DESIGN 3.12,
"Analysed, not built", now built).

**What changed** (DESIGN 3.12, "Virtual PDF object numbers"; switch
`PARTEX_SSA_VOBJ=0`):
- SSA mode turns on machine mode's virtual object numbers
  (`pdf/vnum.rs`): numbers in the bytes are relocations, the link writes
  pdfTeX's numbers from the numbering events. An object TeX identifies is
  named by its identity; any other by its step's id and its count in it
  (`1 + (step << 12 | count)`, hashed past 4,096 objects in a step or
  2^18 steps), so a step run again names its objects as before; an
  applied call (the fonts') by its own name.
- Each object entry (`pdfobj:ID`), each lookup tree entry
  (`pdfname:KEY`) and each step's numbering events (`pdfnum:STEP`, an
  append) are slots of their own. `pdf.objs` keeps the lists' heads only;
  `pdf.obj_trees` and `pdf.dests` are no longer read or written (the
  destinations' names are the destination list's, at the job's end).
- A number TeX observes (`\pdflast…`, a number given back, the job's
  end) reads every earlier step's events (`Tracker::steps_before`).
- The link: with virtual numbers SSA mode links in full every build
  (6–9 ms on the thesis), deflate memoized by content as the spliced
  link's; the splice no longer builds its mark tables for virtual
  chunks (their ids reach 2^31: a 17 s link of a 5-page document).
- Placing a stage's slots from the format's state copies the writer's
  `virt` (as `symbolic`): else a rerun placed `pdf.out` without
  relocations and wrote raw ids.

**Measured** (64-page thesis, fastdev, local, same process: cold build
then five rebuilds; base 9f13c82 → this branch): "x" after "big data
applications." 6 → 6 steps; a space after it 5 → 5; the long reflowing
insertion in Chapter 2 5,712 steps / 10.97 M commands / 46.4 s → 586
steps / 0.29 M commands / 3.0 s (its new steps, cut by the window
count, 5,505 → 93); a section title in Chapter 3 351 → 350 steps
(its 1.1 M commands are windows that do not line up again, not PDF
numbers); "Like an ASICx," 5 → 5. pt 3 → 3, acro2 0 → 0, ac3 14 → 14,
each rebuilt PDF identical to the base's plain run.

**Exactness.** The cold build's trip 1 is byte-identical to the base's
(PDF, `.aux`, `.toc`, `.out`, log). Run to its fixed point, the base's
cold build of the thesis made 66 pages and this branch's 64; real
pdfTeX run with BibTeX to its fixed point makes 64. On the thesis the
rebuilt PDF after the five edits differs from a cold build of the edited
source here, as it does in the base (there the `.aux` agreed and the PDF
did not; here two `\ACRO{pages}` records differ by one page): not
resolved yet.

**Merge of main (eaaa2db), and what it needed** (af5eeb2 and after):
the gate's e2e `glyphs` and `display` cases (new on main) failed with
virtual numbers. Display lists named forms by their virtual ids: they
are named by pdfTeX's numbers now, from the steps' numbering events. And
the display document's PDF came out 100,421 bytes where the engine
guessed 98,187: a byte count of another number of digits stopped the
link (`LengthDigits`). With the columns the link's (`effects/flow.rs`),
the text that prints the length is rendered again with the length the
link found, and the files linked again.

**The long insertion's rebuild against a cold build** (thesis, run
alone): with `PARTEX_SSA_VOBJ=0` (the base's numbering, pre-merge
binary) the rebuilt and cold `.aux` and `.toc` are equal and the PDFs
differ (the cold build's fixed point leaves two citations of the edited
paragraph undefined; real pdfTeX with BibTeX resolves them, as the
rebuild does). With virtual numbers the same citations differ, and also
the rebuilt `Chapter_2.aux` records three acronym uses (`mrrg`, `ii`) a
page early and the `.toc` one page number (24 for 25), where cold and
real pdfTeX agree. The page text is otherwise the real one. The two
settings reach different fixed points (66 pages with VOBJ=0, 64 with it
on, as real pdfTeX), so the comparison does not say whether the page
records' staleness is the numbering's: open.

## 2026-10-04 — The saved registers' shape puts its entries back with it (branch `xchain-fix`)

**The bug.** After the entry-by-entry chains (`29390b4`), a thesis
rebuild panicked: `restore_ext` met an entry with no location
(`peek_xeq_level`: `xeq_level` index out of range). After a step's run
the arrays are put back to the latest definitions only for the slots
placed or written. A placement of an earlier, shorter shape cut the
chains; putting the latest shape back padded them with stand-ins, and
the entries in between, neither placed nor written, were never put
back: the next restore read a stand-in.

**The fix.** As the nest's levels go with the nest, `with_levels` gives
every entry within a shape it puts its own value (placement, the put
back after a run, a dropped run's undoing), and `save_stack_whole`
places every entry within a shape it places.

**Measured** (fastdev, this machine): the thesis's five edits as
rebuilds (`th.sh`): no panic, entry check 0 bad reads at all six
builds. proj2: rebuild 2 still 16 steps (46 ms), PDF equal to a cold
SSA build; `pt`, `acro2`, `ac3`: 0 bad reads, PDFs equal to cold.

**The pages 19–21 difference was not the numbering's** (after merging
main aecd07f, 20e5bf6). The rebuilt page 18 had Figure 2.x's
`tikzpicture` scaled by `\resizebox` 3.544 instead of 1.200: the
picture's bounding box (pgf's dimen registers above 255) came out small,
so three lines moved up a page and the acronym page records with them.
The steps that made it ran for page, save-stack and list slots, none for
a PDF slot, and the entry check found 0 bad reads. With main's fix of
e-TeX's saved registers (every entry goes where their shape is put) the
same long insertion, run alone with virtual numbers, rebuilds in 199 + 26
steps (VOBJ=0: 6,476 + 26) and its `.aux` files and `.toc` equal the
cold build's; the PDFs differ only in the known `[?, ?]` citations.


## 2026-10-04 — Memory: a cold SSA build's records, values and fold, smaller (branch `mem`)

**The problem.** The private thesis repro (cold SSA build, settled in 6
trips, one edit; `PARTEX_ORIGINS=1`) peaked at 3.76 GB resident on main
24f149a (fastdev, 164 s) and fails to allocate in wasm32. A massif
profile of the run (heap 2.5 GB at its peak) put it in: the values the
records' writes hold (6.75 M, one `Arc<SValue>` each, 128 bytes before
this change), the writes themselves (7.09 M, 64 bytes each), the fold
steps' reads (306 MB at the peak), the engine's token lists (macro
arguments 165 MB, definitions 66 MB), the origin table (100 MB, append
only), the fold's tables (144 MB) and reader lists (90 MB). Trips 3 and
4, each running about 8 K steps again (cascades gone cold), add about
1 GB: the records the removed steps leave stay reachable from the last
three trips' roots until the arena doubles.

**The changes** (no behaviour change: same trips and steps, PDF, `.aux`
and `.log` byte-identical):
- `Fold::close` built a step's reads by collecting its (hash, address)
  pairs in place: the addresses kept the pairs' room, grown by doubling,
  2.2 times theirs (26 M reads held 863 MB). Now exactly as long.
- A removed step's reads and records were kept for good (15 M of the
  fold's 28 M reads). The rebuild that passed over it still reads them
  (its new steps' predictions); after it, nothing does:
  `Fold::release_removed` lets them go at each rebuild's end.
- `SValue::Pos` (120 bytes) and `SValue::LrBox` (112) set every value's
  size: boxed, a value is 48 bytes.
- `Version` is eight-byte aligned (`repr(C, packed(8))`): a 128-bit hash
  needs no more, and at sixteen every write (`(Slot, Option<SVal>)`) was
  64 bytes, now 48; records' reads and the version tables shrink too.
  Its hash (the `u128`'s) and order are the same.
- `PARTEX_SSA_MEM` also prints the resident set at each trip's end, after
  settling, after the link and after the rebuilds, and how many values
  the writes hold.

**Measured** (fastdev, this machine, the thesis repro): peak 3,758,312 KB
→ 2,641,312 KB, 164 s → 160 s; trips 6 (63796, 415, 8131, 8373, 726, 163
steps), rebuild 1: 5 steps, as before.

**The collector and a step's records** (same branch, next commit). A
settling trip that runs most steps again (trips 3 and 4 of the thesis,
cascades gone cold) made new records for them while the last runs' stayed
reachable from the last three trips' roots, and the collector ran only
when the arena's record count doubled, which a few thousand large step
records never do. Now a lean record (a step's: never looked up, its frame
keeps no reads) is kept only if a live step of the fold holds it; a kept
build's roots keep its children (the routines' records a later call may
hit). The collector also runs when the writes the records hold grew by
half since the last collection, and when a cascade goes cold, right after
the old steps after it are retired (`Runtime::collect_retired`; their
reads stay until the rebuild ends, for its predictions). After a
collection the kept roots name only kept records. Thesis: peak 2,641,312
KB → 2,123,168 KB; trips, steps, routine hits (126 of 252 calls) and the
outputs the same; `pt`, `acro2`, `ac3`: outputs equal, entry check 0.

**A step's writes that are not its definitions keep only their
versions** (same branch). A lean record's writes are read as values
only where the fold's index names them a definition; the others (a
save-stack entry above the stack's end, a slot a group's end put back,
a slot a later call of the same step wrote again) were each an
`Arc<SValue>` held for good. `Runtime::bare_writes`, at a step's close,
lets their contents go (`Value::bare`: the version stays). A lean record
made again by a run (the record deduplicated) gets the contents back from
the new run's, and one made by another step's run keeps them for good.
Thesis: values held 3.70 M → 2.82 M; peak 2,123,168 KB → 2,045,336 KB;
outputs, trips and steps the same; `pt` with two edits (3 and 377 steps
run) and `acro2`, `ac3`: outputs equal to main's, the entry check as on
main.

**A retired step's end goes with it** (same branch). Each step keeps its
end (`Steps::inputs`, an `InputState`: the input stack's levels, the
macros' arguments open there, the buffer) for the next run to start
from; a retired step's was kept for good, and no run starts from it
again. `retire` drops it. Thesis: peak 2,045,336 KB → 1,940,368 KB
(trip 1's end 1.52 GB; the trips that ran most steps again add 0.38 GB);
outputs, trips and steps the same.

**The job's start's definitions apart** (same branch). The fold's first
step (the job's start: the format's state) defines 633 K of a thesis's
826 K slots, most of them never defined again; each had a list of its own
in the definitions' table and a place in it (150 MB). Now `Fold::base`
keeps the first step's definitions apart, a packed record and write index
per slot in an array per dense family (`Machine::dense`), by address for
the others; the table holds the other steps' (249 K slots). Every lookup
(`reaching`, `latest`, `reaching_if_later`, `next_after`, `defines`,
`entry_of`) answers as before, the base's being the first entry. A slot's
first definition in the table gets a list of room one. Thesis: peak
1,940,368 KB → 1,825,580 KB; rebuild 1 16.7 ms (the rebuild 7.5); outputs,
trips and steps the same; `pt` (two edits), `acro2`, `ac3` as on main.

**The eager collections undone: a speed regression** (same branch). The
collections 4f667ed added (when the records' writes grew by half, and when
a cascade goes cold) made each thesis edit slower: rebuild 1 13.6 → 17.6
ms, the link 5.0 → 8.5 ms (4 edits, same binaries; the cold build
unchanged). perf over 600 edits put the difference in glibc's
`_int_malloc`/`realloc` slow path: a collection in the middle of a build
frees the old runs' records among live data, the heap stays full of
holes, and every later allocation (the link's first: it resolves the
virtual numbers into a fresh copy of every effect at each edit) pays for
them; with both collections off the link was 5.2 ms again (glibc's
`hugetlb=1` recovered half). Both are gone: the collector runs when the
arena has doubled, as before; a lean record is still kept only while a
live step holds it. `step_effects_as` counts the chunks first (its vector
was made by doubling, a dozen large reallocations each edit). Thesis peak
1.83 GB → 2.18 GB. 20 edits, medians of two alternating runs each: base
(24f149a) rebuild 7.2/6.9 ms, link 5.3/5.1 ms; this rebuild 7.4/7.2 ms,
link 5.6/5.5 ms (the cold build equal: user 161–164 s against 157–185
s). The remaining 0.3 ms is the holes the other cuts leave (values let go
at a step's close, retired steps' inputs) and the base lookups' family
mapping; collecting by compaction (records' writes in an arena per
generation, copied out whole) would remove the holes.
