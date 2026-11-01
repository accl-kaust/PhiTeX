# A sequential region: one run for a format or a preamble (2026-10-21)

Status: design only, nothing built. It answers the user's question: can a
format or a preamble run as one sequential machine that publishes only its
final changes? Measurements come from job 15680 (node harrier, profile
build of dev 5d89881e, 3 reps each) and from the clean-core amsldoc
recording (`docs/note-preamble.md`).

## 1. What the time is made of

### The format build (latex.ini; the cold first build of a one-page document)

| | PhiTeX | pdfTeX `-ini` |
|---|---|---|
| wall | 19.9 s | 6.06 s |
| peak resident memory | 990 MB | 63 MB |

The PhiTeX side breaks down as follows. All three reps agree within 1%;
-j 1 and -j 8 are the same.

| phase | time | what |
|---|---|---|
| the run | 7.85 s | **one** resume node runs the whole format text: 12.3 M commands, 0.64 µs a command |
| commit/apply | 1.75 s | the engine takes in the run's 21,943 field writes (`clean prof` apply 1.67 s, settle 1.28 s within it) |
| final bindings | **9.73 s** | the format's 181,911 final values read back one field at a time: `def_value` 9.52 s (52 µs a field), `reaching_def` 0.09 s, names 0.06 s |
| export, store | 0.04 s | 10.9 MB written |

- The format already runs as one sequential node: 3 nodes run, 3 runs, no
  speculation.
- Half the build is reading its result back. `def_value(d, f)` costs time
  in proportion to the node's definitions, so 181,911 calls on one node
  are quadratic.
- Beyond that, the run itself (7.85 s) is 1.3× pdfTeX's whole `-ini`, which
  includes building the hyphenation tries.

### The preamble (amsldoc with an empty body, format cached)

| | PhiTeX | pdflatex |
|---|---|---|
| -j 1 wall | 1.24 s | 0.22 s |
| -j 8 wall | 1.39 s | — |
| amsldoc whole, one pass | — | 0.52 s |
| amsldoc whole, -j 1 / -j 8, 3 loop rounds | 4.21 / 5.45 s | — |

- About 198 nodes run. The five loads take 817 ms (load 171 alone
  737-750 ms) and the body-start node 20-25 ms a run.
- Publishing is small: `clean prof` commit 33 ms, apply 39 ms. Each load
  already publishes only its final values: load 170 makes 6,531
  definitions of 6,531 distinct fields. Over the whole preamble, 10,378
  definitions cover 9,671 fields, and only 253 fields are defined by more
  than one node.
- **The cost is per command:** 352k commands in ~0.91 s of runs is
  2.6 µs a command, four times the format run's 0.64 µs.
  - The difference matches the loads' first touches of the format's
    values. About 20.5k engine reads (load 170: 10,018; node 180: 4,029)
    at ~33 µs each would be the extra 0.68 s.
  - Each is an engine read, then a decode of the value (a macro body, a
    record) into the run's native state.
  - Inside the format run every value is the run's own, so nothing is
    decoded.
  - This is an estimate. A callgrind of the empty-body file at -j 1 is
    the first thing to run before building.
- **Node 180 (the body's start) runs 4 times** because it reads back the
  job's written files (`.aux`, `.out`: `wtext:`/`wkeys:`/`wdef:` fields
  change each loop round). That is P5's business, not this note's.

## 2. The design

### (a) A declared sequential region

- **A region is one node.** It runs the text from its start to its end on
  the run's local state, with no speculation and no cuts inside, and
  publishes only its final changes.
  - It is a run like any piece's, only longer. The format already runs
    this way (one resume node), and so does each load.
  - What the region adds:
    - the preamble's ~175 slices and the body-start node run as one run,
      instead of 175 runs each re-reading the format's values;
    - its output is a snapshot (b), not ~10k separate definitions.
- **Where a region ends** (generic TeX, no names):
  - for a format: `\dump`, or the end of the format's text;
  - for a document: the first of
    - the first contribution to the main vertical list (§994's
      `build_page` with contributions: a box, rule, glue, kern, penalty,
      insert, mark, `\vadjust` material or a non-immediate `\write`'s
      whatsit put on the main list),
    - the first read of a field the job writes later (a loop-back read: a
      job-written file read back).
  - The second rule keeps the read-back out of the region. Otherwise the
    whole region would rerun every loop round, where today only node 180
    (25 ms) does.
  - In amsldoc the region would end inside the body-start node's run,
    where it reads `.aux` back. So the body start splits into the
    region's tail and a small read-back piece.
- **Inside the region** there are no candidates (no main-list
  contributions by definition), so no pages and no cuts.

### (b) A snapshot publish

- **The region's output is one value:** its final state as a persistent
  map from field to value, split into content-hashed chunks (for example
  by the field's hash, 256 or 4096 buckets). The engine holds one
  definition, the snapshot.
- **A reader after the region** asks for a field as today. The engine
  resolves the reaching definer, the region, and the field's value comes
  out of the snapshot: one map lookup, materialized only when asked.
- **The read is recorded as a read of that field's value**, not of the
  snapshot. So a reader is woken only when its field's value changes.
- **An edit inside the region:**
  - the region runs again, into a new snapshot;
  - the old and new are compared chunk by chunk (equal hash, equal
    chunk), then field by field inside the chunks that differ;
  - only the fields whose values changed wake their readers.
  - An edit that changes nothing a reader read runs nothing after the
    region. That is the same cutoff as today, without republishing 10k
    definitions.
- **The format's export becomes the snapshot.** The 9.73 s readback and
  the 1.75 s commit of 21,943 writes are replaced by building the map
  once from the run's final state (estimated well under 0.2 s for 182k
  entries).

### (c) Format and preamble, one mechanism

- The format is a region from INITEX's state to `\dump`. Its snapshot is
  what the cache stores (as `prelude_bytes` does today) and what a
  document's first region reads.
- A document's preamble is a region from the unit's start to the end
  rules in (a). It reads the format's snapshot natively:
  - the snapshot is decoded once per process into a shared, immutable
    native table the region starts from (copy on write);
  - this replaces ~20.5k per-field engine reads and decodes.
- The body's pieces read the preamble's snapshot through the engine, by
  field, as in (b). They stay parallel, and their def-use edges are as
  fine as today's.

### (d) Caching across builds (described; cold vs warm is the user's call)

- **A region's key:** the hash of its text, plus the values it read from
  outside itself (the format snapshot's hash, each file's content hash,
  the job-level settings it read). This is `prelude_key_of`'s scheme,
  extended from the format to the preamble.
- **On a matching key** the region does not run; its stored snapshot is
  the definition.
- A warm amsldoc would then start the body at ~0.1 s instead of ~1.1 s.
- The user decides whether a cold build may use what an earlier build
  stored.

## 3. Requests for the engine (IncEngine2), with expected gains

| # | request | gain (from section 1) |
|---|---|---|
| E1 | a node's final values in one pass (an index from field to definition, or `final_values(node)`) instead of `def_value` per field, which is O(defs) per call | format build 19.9 → ~10.4 s (−9.5 s). Small and independent of the rest; do first. |
| E2 | a bulk definition: a node defines one snapshot value with a lookup (field → value). Readers' reads are recorded per field; the commit takes one value, not 21,943 writes | format build ~10.4 → ~8.8 s (commit/apply −1.6 s). Preamble: ~70 ms of commit. |
| E3 | republish by diff: a node running again gives its new snapshot, and the engine wakes only the readers of fields whose values differ (chunk hashes first) | edits inside a region cost the region's run, plus the readers of changed fields only |
| E4 | a region declaration: a node that is never run speculatively and has no children before its end (the region's end rules decide where its text stops) | the preamble as one run: ~175 nodes → 1 |

On PhiTeX's side, with no engine change: the region starts from the
format's snapshot decoded natively once per process, instead of a
per-field read and decode.

- Expected (to confirm with callgrind first): the preamble's per-command
  cost from 2.6 to ~0.7 µs, i.e. amsldoc's preamble from 1.24 s to ~0.4 s
  at -j 1, against pdflatex's 0.22 s.
- The rest of the gap is the interpreter itself, the per-command line of
  work.

**Totals (estimates):**

- the first ever build of a one-page LaTeX document: 19.9 s → ~8 s with
  E1+E2, against pdfTeX's 6.1 s;
- amsldoc -j 8: ~5.4 s → ~4.6 s from the preamble region alone, more with
  (d) when warm.

## 4. Against the design anchors

- **The graph owns the values at the boundary.**
  - The region is a node, and its snapshot is a value the engine holds.
  - Every reader after it reads a field through the engine, and its
    read is recorded and rechecked per field.
  - Nothing outside the region reads the run's state.
- **Not a document-order fold, not eqtb as the design.**
  - A region is one run with local state: what any piece's run is, with
    no position-keyed lookups and no fold over the document.
  - The snapshot is a value (a map), not a table other nodes consult in
    place.
  - Seriality is confined to text that is serial anyway: the five loads
    form a chain (each reads 20-75 of its predecessors' values), and
    format text runs as one node today.
  - Everything after the region stays a graph of pieces.
- **No names.** The region's end is stated in TeX primitives:
  - a main-list contribution (§994);
  - a read of a job-written field (a loop-back);
  - `\dump`.
