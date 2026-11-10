# Reads as per-node guard sets, no filed reader index (the design, 2026-11-10)

Status: design for part C of the TikZ manual's memory work (agent mem2,
branch `mem1-c`). Measurements are mem1's (`phitex-runs/mem1-notes.md`),
dev 16b0fab3f, the TikZ manual (pgfmanual) at -j1, 660 s into the build.

## 1. What it removes

At 660 s the live heap is 6.45 GB, and the reads and their readers hold
about 2.4 GB of it:

| what | bytes | per read |
|---|---|---|
| `ReadRec` storage, the records' reads (24 B each; `commit.rs:1111` 685 MB, the first allocation in `Ctx::record` 423 MB) | ~1.1 GB | 24 B + slack |
| segment reader lists (`Inner::readers` values, `Reader` 24 B each, 25 M filed, 23 M standing) | 738 MB | ~29 B |
| segment reader maps (`FxHashMap<Field, Vec<Reader>>` room, 3.9 M keys) | 347 MB | ~14 B |
| the readers' field index (`Engine::readers`, 3.9 M `(Field, Pos, SegId)`) | ~95 MB | ~4 B |
| `Node::anchored` (a set of fields per anchor: 144 000 at a picture's parent) | tens of MB | |
| the read-key table (`ReadKeys`, 2.2 M interned `(ReadKind, PathId)` with their hash shards) | ~200 MB | |

Each read is held three times: in its node's record (what it saw), in a
reader list of its anchor's segment (so that a walk finds it), and, for
a read not of a whole field at the reader, in the key table. The reader
lists also keep the dead records of earlier runs until a sweep.

## 2. The rule

A node's reads are kept once, in its record, as a **sorted, delta-coded
read set**. Nothing else lists them. A definition finds its readers by a
**forward scan of the nodes after it**, looking each one's read set up
for the field. A segment keeps only **which fields** the read sets of
its nodes guard (a set of field ids, no reader records), and the field
index keeps, per field, the segments whose set has it, as now: it is
what lets a walk skip the segments that read nothing of its field.

### 2.1 What a record keeps of a read

A read is validated by recomputing its answer and comparing it to what it
saw. Three things the 1-2 B guard of the brief does not hold stay, each
because something rests on it:

- **what it saw** (`seen`): a walk wakes a reader only where its answer
  through its path moved (`reask_reader`, `walk_here`): without it every
  reader between a changed definition and the next one would run again,
  path or no path (a font's member read by thousands of paragraphs, a
  list's element). `Ctx::last_run_stands`, `Ctx::last_reads_hold_from`
  and the commit's check of the reads a resumed run kept recompute the
  answer and compare it to `seen` too.
- **the field's change count before it** (`epoch`): the commit's and
  `last_run_stands`' fast path (a field no change has counted since it
  was read holds without a look). Without it a picture's 144 000 reads
  are recomputed one by one at every check (70-140 ms each).
- **its place in the run** (`ordinal`): `Ctx::goes_on_after_reads(k)`
  keeps the last run's first `k` reads, `Ctx::goes_on_as_last(m)` and
  `Ctx::last_reads_hold_from(m)` its reads from the `m`-th on; the
  counts are the client's (a checkpoint's reads so far).

So a read is one entry of the set, sorted by (field, kind, anchor or
bounds, path, seen, ordinal), coded as:

| part | coding | bytes |
|---|---|---|
| head | kind (13 codes), filed bit, `seen`'s tag (7 = none) | 1 |
| field | delta from the entry before (0 within a field) | ~1.5 |
| kind's operands | anchor node, unit, fold bounds, placement index: varint | 0 (most) |
| path | varint (`WHOLE` is 0) | 1 |
| seen | a sequence's hash: 8 raw bytes; a plain value: zigzag delta from the field's id; an integer: zigzag | ~3-5 |
| epoch | varint | ~2-3 |
| ordinal | varint | ~2 |

About 10-13 B a read against 24 B, and none of the 43 B of reader list
and map. Every 8th entry starts a block, whose first field is kept in
a table of the blocks' offsets and first fields (12 B per 8 entries,
1.5 B a read) lets a look-up by field binary-search the blocks as plain
words and decode a few entries (a block's first entry codes its field
as a delta from its head's, 0). A filter of the fields read (two bits a
field, about a byte each) answers most look-ups of a field the node does
not read with no search at all: a walk under scopes passes many nodes.

The kind and path are coded in the entry itself: no read is interned,
so the read-key table goes, and with it the lookups that cost para4000
+2.0% Ir at -j1 (e734bdf6b). While a run runs, its reads are a plain
vector of a 48-byte form with the kind inline (`ReadRec`); the commit
sorts and codes them into the record. The order-sensitive uses
(`goes_on_after_reads`, `goes_on_as_last`, `last_reads_hold_from`, the
commit's merge of kept reads) decode by ordinal; the others (checks,
walks, collection roots) decode in set order.

### 2.2 What a segment keeps

`Inner::readers: FxHashMap<Field, Vec<Reader>>` becomes
`Inner::guarded: FxHashSet<Field>`: the fields read at the positions of
its nodes (their own reads at themselves, reads of other nodes anchored
at them, witnesses), a superset of those that stand. A field enters it
when a commit files a read of it there (and then the field index, as a
first reader does now); it leaves at a recount: a split or a give
(rebalance) recounts both sides from their nodes' records, and the
quiet-point sweep (`Engine::sweep_readers`) recounts every segment when
replaced records outnumber standing ones, as it drops dead reader
records now. A field left in a set after its last reader went costs a
walk a scan of that segment, no wrong answer.

### 2.3 Reads anchored at another node, witnesses

A read at another node's position (`At::Before(n)`, `At::AfterOwn(n)`:
a picture's reads at its parent, 144 000 of them) is in its reader's
record; the anchor `n` keeps the readers that read at it,
`Node::foreign` (`readers: Vec<NodeId>`, a node each, not a field each),
filed under `n`'s segment's lock in the commit's step 2 as the reader
records are now. A derived read's witness (`Step::Derived`: the reader
filed again under the field its name gives now, `file_witness`) is not
in any record; it stays a `Reader` record (24 B) in the anchor's
`foreign.witness` list, as rare as derived reads are, dropped when dead
(its serial, as now).

## 3. The walks without the index

A walk asks the readers of `f` anchored in `(from, stop)` (the first
definition that hides the one walked from, or a hole), segment by
segment. "The readers of `f` at node `x`" are now:

1. the entries of field `f` in `x`'s record anchored at `x` (`Me`,
   `Before(x)`, `AfterOwn(x)`), a binary search over its blocks;
2. for each `r` in `x.foreign.readers`, the entries of `f` in `r`'s
   record anchored at `x` (`Before(x)`, `AfterOwn(x)`);
3. the live witnesses of `f` in `x.foreign.witness`.

Each is made into the `Reader` the walk asked before (node, its record's
serial, anchor, inclusive, path, seen) and asked as before
(`reask_reader`, or answered in place under the lock). A record is the
node's current one (taken under the segment's lock, as a commit installs
it under that lock), so every entry stands: the serial test of a filed
reader is now the record's being the node's.

| walk (commit.rs) | before | now | cost |
|---|---|---|---|
| `walk_here`: in the committing segment, under its lock | `inner.readers[f]`, every record of every run, each tested for standing and range | `f` in `inner.guarded`, then the nodes in `(from, stop)`, each looked up | nodes in range × one lookup (~0.1 µs); a chain of definitions (a counter per paragraph) is one node |
| `reask_from`: segment by segment after it | the same per segment, under its read lock | the same scan per segment | as above |
| `next_with` / `reask_later` | the field index of definitions and readers | unchanged (the index lists the segments whose set has `f`) | O(log) |
| `reask_after_effect_with`: a bracket's region | every reader record in every segment of the region, those of the fields a scope can move kept | every entry of every node's record in the region, decoded, the same filter | entries in region (was: records in region, dead ones too) |
| the raced and past-the-stop parts of it | the same, every segment to the end | the same | as above |
| `Movable::learn` | the segment's reader fields | the segment's `guarded` | same |
| `file_witness` | a reader filed under `g` in the anchor's segment | a witness in the anchor's list, `g` in its segment's set | same |
| `retract_products` (a node removed) | the readers anchored at it, by `Node::anchored`'s fields | its `foreign.readers`' entries anchored at it, and its witnesses | entries of those readers |
| `lift` / `land` / `lifted_walks` (relocate) | others' readers anchored at it taken out and filed again at the new place | nothing moves: the readers stay with the node; the new segment's set gets its fields | entries of the node and its foreign readers |
| `commit_stood` | witnesses only | same | same |
| ranges, `next` stops, finals, ends, environment | `Engine::ranges` | unchanged: not readers at positions | |

The scan costs nodes, not records: a segment holding one reader of `f`
among 64 nodes costs 64 lookups where it cost one record. It never scans
a segment whose set lacks `f`, and a walk's range is up to the next
definition of `f`: for the fields defined again and again (a counter, a
font, `\@currentlabel` per paragraph) that is the next node or two. A
walk that met a dead record cost as much as one that met a standing one
before; there are no dead records now.

## 4. Why it stays mergeless and correct

A commit installs its record (`Node::last`) under its segment's write
lock, then checks its reads; a definition's commit files the definition
under its segment's lock, then walks, taking each later segment's read
lock and the scanned nodes' `last` under it. Either the walk's lock of
the reader's segment comes after the reader's commit, and the scan sees
the new record, or before, and the reader's check sees the definition:
the pairing the reader lists gave, with the record in place of the
list. A read anchored at another node is found through that node's
`foreign.readers`, which the reader's commit files under the anchor's
segment's lock before its check, as the reader record is now. No lock
is added: a node's `last` and `foreign` are rows of the node's locks,
taken under a segment's as a commit takes `last`, never two at once.

A read a record keeps from the run before as filed (a parked loop's,
`ReadRec::filed`) is asked with the filed serial at the node's floor,
as its reader record was (`commit::FILED`): a commit that kept it
installs its record before its serial, and a walk in between must still
re-ask the node (`Node::asked`), which is what makes the commit check
the reads it kept. Its new reads the commit checks in any case.

A segment's set and the field index only grow between recounts, so a
walk never skips a segment holding a reader; a recount is under the
segment's write lock (split, give, sweep), with the index changed there,
as a split changes it now.

## 5. Steps

1. Reads coded in the record (`node::Reads`), in-run reads with the kind
   inline, the key table removed. Walks still use the reader lists.
   Gate: amsldoc, para4000, pgfmanual j1/j16 exact; para4000 j1 Ir
   against 5bca61ea7; the defuse and tex suites.
2. Reader lists, maps and `Node::anchored` replaced by the segment sets,
   `Node::foreign`, and the scans of section 3. Gate: the same, plus
   amsedit and para4000 word edits at j8/j16, and wasm t4 on the TikZ
   manual.
