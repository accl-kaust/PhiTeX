# The page chain, P0: what the recordings say (2026-10-21)

Status: measurements for [spec-page-chain.md](spec-page-chain.md) §8 P0 and
§9 Q4. Nothing here is built.

Sources:

- Round-core recordings of dev b1d65067 at -j 8 (job 15461). The last
  finished run of each node counts. The counts come from
  `bench/r6/fin-reads.py` (job 15485).
- The candidate trace of the same tree at -j 1 (`PHITEX_CAND_TRACE`,
  `bench/r6/cand-classify.py`, job 15489).
- Pieces are `exec` and `resume` nodes; finishers are `finish` nodes.
- A read is linked to a writer when exactly one node wrote the value the
  read got to that field. A value written by several nodes is counted as
  ambiguous. So the chains below are lower bounds for fields whose values
  repeat, such as flags.

## 1. Finisher → piece: pieces refuted if output were invisible

These are pieces that read a value only a finisher wrote.

| document | pieces | finishers | refuted | fields (pieces refuted) |
|---|---|---|---|---|
| para4000 | 8809 | 692 | **0** | none |
| amsldoc | 1259 | 115 | **28** | `pagepar` 11 (the page parameter record: `\vsize`, `\maxdepth`, `\topskip`, `\holdinginserts`), `fontinfo:64` 7 (a font the routine loads first), `count:0` 5 (the page counter, read by 13 pieces), `deadcycles` 3, three float-list macros 2 each, `box:56`/`box:253` 2 each, nine column-mark macros 1 each, `count:189` 1 |

No single global read by every piece dominates. The widest is the page
counter: 13 readers, of which 5 got a finisher-only value. Fields that
finishers write and many pieces read, but never with a finisher-only value
(`aftertoken` 530, a list flag 188), are not refuted.

## 2. Piece → piece chains

**para4000:** the longest chain is 4. Its only multi-link field is
`pages:resume` (3), the cut's own field, which the spec removes. There is
no running global. `\prevgraf` (`pg:0`) is written twice in every
paragraph, `set 0` and then `add n`; its 4000 reads are of the value 0,
which many pieces write.

**amsldoc:** the longest chain is 165 links (159 without `pages:resume`).

The longest chain through each field alone:

| field | writers | linked readers | longest chain | writes |
|---|---|---|---|---|
| the global current-anchor macro | 90 | 87 | 87 | read-modify-write |
| the current anchor macro | 91 | 98 | 57 | rmw |
| `count:187` | 58 | 56 | 56 | add 2 (55), set 119 (1) |
| the section-number macro of a heading | 55 | 54 | 54 | rmw |
| a bookmark parent macro | 45 | 44 | 44 | rmw |
| scratch macro c | 265 | 47 (335 ambiguous) | 38 | rmw |
| a second scratch macro | 57 | 65 | 36 | rmw |
| `count:319` | 67 | 55 | 25 | add 1 (56), add 0 (9), set 57 |
| scratch macro a | 403 | 43 (154 ambiguous) | 25 | set 234, rmw 191 |
| `count:165` | 23 | 12 | 12 | add 1 (13), add 0 (9), set 0 |
| `count:153` | 30 | 11 | 8 | add 1 (12), set 0 (11), add 3/9/2 |
| `vlist:last` (the main list's tail) | 586 | 85 | 2 | |

- The 165-link chain is made of: the heading macro 31, scratch macro a 29,
  the global anchor macro 22, `vlist:last` 16, `pages:resume` 14, the
  anchor macro 12, `box:35` 10 and counters 11.
- So "typeset every piece first" is itself a chain on amsldoc. It runs
  through the cross-reference anchor names and the heading macros, about
  160 pieces long. With today's piece run of ~0.5 ms, that is about 80 ms
  plus the steps: not the bound while the preamble takes 1 s.
- Open lead, not verified: many macro links are reads made just before a
  definition. These may be the old value a local definition saves, not a
  use of it. If so, the chains are an artifact of the save stack, not of
  TeX.

## 3. Counter writes

Every integer register write on the chains above has one of two forms:

- `add c`, with c a constant per site (+1, +2, +3, +9, and +0, which
  means re-written unchanged);
- `set c`, with c a constant (0 when a parent counter resets it, 57, 119).

So each counter's value is a prefix of affine maps (§5.1 applies). The
same holds for `fontcount` (add n).

## 4. Page-state readers

- **para4000:** none.
- **amsldoc:**
  - 3 pieces read the scan state (`\pagetotal` and its kin,
    `Run::page_dimen`);
  - 3 read `\deadcycles`;
  - 244-246 read the main list's tail fields (`vlist:pd`, `vlist:since`,
    `vlist:last`: `\prevdepth`, `\lastskip` and its kin).
  - The tail is piece → piece today. After a fire, it is what the routine
    leaves.

## 5. What para4000's resumes hold

- **4002 of 4004 resumes are empty rests at a slice end.**
  - The candidate trace puts their only candidate at "no input left".
  - Their last run contributes no items unit.
  - They make 20-29 engine calls each.
  - The 4061 resume runs cost 953 ms at -j 8 (847 ms at -j 1), about
    0.2 ms each, all for nothing.
- amsldoc: of 419 resumes, 90 contribute no items, 40 contribute 1-2 items
  and 289 contribute 3 or more.

## 6. Q4: candidates inside a piece's own groups, lists or conditionals

From the candidate trace, the last line per (node, candidate):

| document | candidates | ended there (cut) | inside own group/list/conditional | nothing contributed since the one before | slice end |
|---|---|---|---|---|---|
| para4000 | 8863 | 4004 | **0** | 457 | 4402 |
| amsldoc | 7327 | 426 | **4048** (in 422 pieces) | 2641 | 212 |

- Pieces with a candidate inside, by count: 262 have 1-4, 94 have 5-9,
  17 have 10-14, 9 have 15-19, 13 have 20-24, 21 have 25-49 and 11 have
  50 or more. One piece has **1083**.
- The nesting at those candidates (levels, open conditionals): mostly 1-5
  levels, 0-6 conditionals.
- If every candidate ends a piece (§4.1), the 1083-candidate piece becomes
  a chain of 1083 resumes. Each reads the one before (`pages:resume`), and
  all sit inside one slice. That is the long chain Q4 warns of.

## 7. The engine reads the design needs (against the clean core)

**What works today:**

- A scan step can read a piece before it in two ways:
  - `use_field_at(f, Some(piece))`;
  - one folded range, `acc_range_to(VPROF, from, to)`.
- It can place a finisher right after a piece before it with
  `follow_after(piece)`.

**What does not:**

- **A read cannot reach past its reader.**
  - `use_field_at`/`use_field_before` give `BadPosition` when the node is
    not before the reader (`clean/ctx.rs`, `use_field_pos`).
  - `acc_range_to`'s window is capped at the reader's own position
    (`World::window`: `hi` is never past the reader).
  - `follow_after(at)` falls back to `follow` when `at` is not before the
    placer.
  - So scan p must sit after d_p. The spec's "places … scan p+1" cannot
    put scan p+1 at d_p, because it reads the pieces after d_p. The scans
    have to sit after the pieces, for example as a chain at the unit's
    end: scan 1 placed last, and each scan placing the next with `follow`.
- **Pattern (a) needs the pieces' node ids in order before it can read
  them.**
  - `acc_range_to(f, from, to)` takes `to` as a node id. Scan p learns
    the pieces after d_{p−1} only from the units of a range it has
    already read (each unit carries its piece's id).
  - A first read of `(d_{p−1}, me]` waits for every piece to the end. It
    also records the whole range, so any edit after d_{p−1} rechecks scan
    p: every scan before an edited page reruns. That breaks invariant 4.
  - Wanted, one of:
    - `acc_range_until(f, from, stop)`: fold the units after `from` in
      order. Wait only on an in-work contributor inside the prefix, and
      stop where `stop(node, unit)` says. The read is `(from, stop node]`,
      asked again when a unit in it changes or a contributor is placed
      inside it.
    - A structural read of the next contributor of `f` after a node,
      without its value.
  - Pieces come up front from the source, so the slices' ids could be
    known. But resumes are placed by runs, so their ids are not.
- **Pattern (b)** (a window of W pieces) has the same id problem for the
  window's end, and adds the accepted extra run.

**The lane: what scan p+1 reads of finish p** (so the two do not run in
either order):

- `rtn:<p>`: the routine's give-back, its list and markers. The held-over
  insertions go in the material.
- The page-parameter record `pagepar`, where the routine sets
  `\vsize`/`\maxdepth`/`\topskip`/`\holdinginserts` (11 amsldoc pieces
  read such a value).
- The insertion classes' registers `box:<n>`, `count:<n>`, `dimen:<n>` and
  `skip:<n>`. The scan reads them at the pieces (§1008); the routine
  empties `\box n`.
- `\deadcycles`, read at `\end`.
- The material the next page begins with, `scan:mat:<p+1>`, which today
  the continuation defines; the spec merges it into scan p+1.
- When the routine gives nothing back and sets none of these, scan p+1
  reads nothing of finish p.
