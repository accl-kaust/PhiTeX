# The output tail of an edit (exact PDF)

Owner: edit-tail agent, for Phi. Branch `edit-tail`.

## 1. Measure (566fb842, before)

amsldoc, release CLI, `watch --exact-pdf`, a one-word edit after the cold
build (`target/tools/edit.sh`: `bench/watchedit.py` with `--exact-pdf`,
`PHITEX_TEX_TIME=1 PHITEX_RERUN_OPS=1`). Local box (24 cores, shared with
two agents, every run under `nice -n 19`), so walls are upper bounds;
per-op busy time from the `nodetime:` lines of the edit round.

Edits: *mid* = line 1654 `operator` -> `operators` (page 30 of 44);
*end* = line 2767 `discussion` -> `discussions` (the bibliography, before
the index).

| edit | wall ms | nodes | exec | place | resume | finish | fold | dvi |
|---|---|---|---|---|---|---|---|---|
| mid j1 | 4563 | 65 | 22 / 194 ms | 22 / 50 ms | 10 / 3596 ms | 5 / 78 ms | 5 / 1 ms | 1 / 36 ms |
| mid j8 | 4031 | 65 (85 runs) | 22 / 196 ms | 50 runs / 144 ms | 16 runs / 3053 ms | 5 / 66 ms | 5 / 1 ms | 2 runs / 115 ms |
| end j1 | 3128 | 29 | 4 / 18 ms | 6 / 10 ms | 7 / 2831 ms | 5 / 48 ms | 6 / 0.4 ms | 1 / 29 ms |
| end j8 | 3353 | 29 | 4 / 21 ms | 10 runs / 15 ms | 13 runs / 2886 ms | 6 / 86 ms | 6 / 1 ms | 1 / 58 ms |
| mid j8, relaxed PDF | 238 | 4 | 1 / 5 ms | - | - | 1 / 17 ms | 1 / 0.1 ms | 1 / 50 ms |

(`n / t`: runs and busy time.) The relaxed row is the same edit without
`--exact-pdf`: the brief's 22-23 nodes / 173-193 ms are not what exact
mode gives on this edit; the place chain is exact mode's.

**Why each re-ran** (mid j1; the engine's last cause per node):

| op | cause | field | nodes |
|---|---|---|---|
| place | Range | `pdf:xpages` | 18 |
| exec | Changed | `pdf:state` | 17 |
| exec | Text | (the edit) | 1 |
| exec | Range | `lines` | 1 |
| fold | Changed | `ps` | 1 |
| resume | Changed / Range | `pdf:state` / `streams` | 1 / 1 |
| dvi | Range | `pdf:xpages` | 1 |

(+ 24 nodes re-made by their parents: finishers, places and resumes after
the edited page.) At j8 the places' causes are `pdf:state` Refuted 11 and
Changed 5: the same chain, run speculatively.

- **The place chain**: every place reads the `pdf:xpages` range before it
  (for the fonts' names, `font_tags`, over all pages so far); the edited
  page's piece key is new, so every later place re-runs. 22 places,
  ~2 ms each: 50 ms at j1, 144 ms busy at j8 (refuted speculation).
- **The writer-state key** (the dominant cost): `pdf:state` is
  `hash(writer digest, hash of every byte written so far, forms)`. The
  edited page's content stream has other bytes, so every later step has a
  new key: 17-18 pieces with read-time PDF commands (hyperref links,
  `\url`, `\cite`) re-run, and the resumes that read `pdf:state` - the
  index's multicol output routine and the end of the document - re-run
  for 2.8-3.6 s. The object counter and tables they read are equal.
- **The end node** (`dvi`): 36 ms at j1, of which 32 ms `finish_exact`:
  it clones the final state and bytes, copies all chunks and writes the
  whole tail again (fonts embedded, pages tree, catalog, xref). It does
  not replay pages (the places did), but it writes the tail anew on every
  edit even when no font, page count or table changed.
- Place share: 22 of 65 nodes; with the exec/resume reruns they cause,
  the writer-state chain is 50 of the 65 nodes and > 95% of the time.

## 2. Design

A step of the exact writer (a read-time command, a page's placement) is a
pure function `(state before, its input) -> (state after, chunk)`.

1. **No byte position in the state.** An object's offset is the chunk's,
   not the state's: a chunk is `(bytes, [(object, offset in bytes)])`; the
   state keeps only "object written" (offset 0) and the count of chunks.
   The state's key (`pdf:state`) hashes the writer digest, the forms and
   the fonts' names; not the bytes. A page re-rendered with other bytes
   leaves the state after its placement equal, and every reader after it
   cuts off. (The writer's `/Length` and `save_offset` positions are the
   step's own and are reset at its end; the last page's shown content is
   the step's output, not state.)
2. **The chunks are an accumulator** (`pdf:chunks`, one entry per step
   that wrote bytes, keyed by content in a memo). The end node copies
   them in document order; an object's file offset is the prefix sum of
   the chunk lengths before its chunk plus its offset in the chunk; the
   xref is made from those. An unchanged page's chunk is the one its step
   wrote before: copied, never rendered again.
3. **The characters used are an accumulator** (`pdf:chars`): a step takes
   the marks it made out of the state and contributes them; the end node
   unions them into the fonts before embedding. A page that uses one more
   character changes only the union, not the chain (the subset is a union
   over chunks at link; a record keeps only its page's own characters).
4. **The fonts' names** (`/F<n>`, pdfTeX's order of first load) are a
   fold carried with the writer state (`FontTags`, page by page), not a
   read of every page before: a place reads only the state before it.
5. **The tail** (fonts, pages tree, outlines, names, catalog, `/Info`,
   the last object stream) is a pure function of the final state: kept by
   the state's digest (`phitex_pdf::Tail`), relinked with the chunks'
   prefix sums (xref stream or table, trailer, `startxref`).

So an edit on page k runs: the edited exec node, its page's finisher (and
fold/scan if a break moved), page k's place (its record changed: it
renumbers nothing new, writes its chunk and cuts off), and the end node
(copy chunks, link). Nodes are identified by position as before (the
place by its finisher's page).

pdfTeX byte identity: objects are numbered by the places in line order on
the state (unchanged: interleaved `\immediate\pdfobj` and other read-time
objects are steps in that chain and take their numbers there); a chunk's
object offsets are relative, so the xref, `/Length`s and `startxref` come
out as pdfTeX's after the prefix sums; fonts are written at the end in
load order from the same state, their subsets from the union of all
steps' characters (pdfTeX's `char_used` is that union).

Why place nodes stay (not replaced by a pure prefix sum over object
counts): object numbers are values TeX reads back (`\pdflastobj`,
`\pdflastannot`, link and destination numbers), so a read-time command
after a page needs the numbering state after it. The place is that
serial numbering step; it costs ~2 ms and now runs only for a page whose
record changed. Symbolic object numbers relocated at link are relaxed
mode's; exact mode keeps the place chain but cuts it off.

## 3. Results

See the commit messages and the final report (numbers before/after, the
suites, the LaTeX suite on accl).
