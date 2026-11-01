# The evaluated graph kept between processes (design note, 2026-11-01)

Status: a design, not built. It is the root fix for the warm build's
cost. A warm build is a new process on a document that was built before.

## Why

A warm build of the user's 8-page acmart paper evaluates the whole
document again (mc 77677: 640 of 744 live nodes run at -j1, 3.6 s; it
does not scale: -j4 runs 2556 times, 449 refuted). The cache keeps the
format, the package and node records (`pkgrec`, `noderec`) and the file
analyses. It keeps nothing of the evaluation. `phitex watch` keeps the
graph in memory: a rebuild with nothing changed runs no node, and a
keystroke runs the few that read what it changed. The persisted graph
gives a warm build the watch's second build.

## What is stored

One file per document, `docs/ID/graph`, beside the records:

- **The values the records name.** The store's values reachable from
  the records below are interned once, as the prelude cache stores a
  format's bindings (`fmtenv.rs`: a table of distinct nodes, loaded
  without decoding until a run asks).
- **Per live node:**
  - its operation and arguments, its unit and its place in the line (the
    order of nodes, not labels: labels are made anew on load);
  - its last committed record: definitions, contributions, placements,
    output and effect, and its reads with the values seen (kind, path,
    seen);
  - its parked state.
- **The environment.** The `dline:` lines, the job's environment and the
  read-back files' versions the evaluation ended with.
- **The output**, the link's PDF layout, so an unchanged build writes
  the file it has.

Not stored: wakes, speculation state, timings, scheduler queues. A loaded
graph is quiet, as after an evaluation.

## Invalidation

- **On load:** the stored text of every document file is compared with
  the file on disk, as watch's `send_edit` does. Each changed file is an
  edit (`apply_text`: changed slices replaced, nodes re-asked by
  `Cause::Edited`). Files the job read (classes, packages, `.bib`,
  images) are checked by their content keys (`file_stat_key`, then the
  hash); a changed one is an environment change (`Cause::Environment`).
  The evaluation that follows is an edit's evaluation.
- **Format or engine changed:** the graph is dropped and the build is
  cold.
- **Options that change the output** (the date, `--exact-pdf`, the
  shell, the thread count) are in the key. The thread count matters only
  in that a graph built at -j1 must be valid at -jN. A committed record
  does not depend on the schedule, so -j is left out of the key, and the
  watch-equals-cold check covers it.

## Size

What the watch holds for the paper, encoded once: about 744 nodes'
records. The reads dominate. The cold build files about 1.4 M reads
(77677: exec 734 k, resume 592 k), most of them field reads that encode
in a few bytes against a value table. Estimate: 20–60 MB raw for the
paper, a few MB after deduplicating values and with zstd framing (lz4
for speed). A thesis is a few hundred MB. The cache's LRU limit (4 GB)
holds it like the formats. One file per document, replaced whole
(written aside, renamed).

## Versioning with the cache

- The file's header names the engine identity (`engine_id`, the source
  hash) and the format's key. Either differing means the graph is not
  read (no migration).
- The value encoding is the prelude cache's (`fmtenv`), versioned with
  it.
- A truncated or corrupt file (a crash mid-write cannot leave one, the
  write is whole) reads as none: the build is cold.

## Work, in order

1. Serialize and load the engine's records (defuse: `Engine::export` and
   `import`, the node table, records and segments' order without
   labels).
2. Rebuild the client's side: `Build`'s node list, `dlines`, the hint
   registry (from the kept analyses), the loop states (in the records).
3. Apply the files' differences as edits; evaluate.
4. Bars:
   - an unchanged warm build runs no node and writes the same bytes;
   - a warm build after an edit runs what the watch runs for that edit;
   - the PDFs are identical to a cold build at -j1 and -j4;
   - the watch equals cold, as now.
