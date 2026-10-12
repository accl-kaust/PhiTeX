# Gate 2: the replay client on the format v1 corpus (PARTIAL)

Status: **partial, stopped on request.** The engine-side runs below were
made with an earlier conversion (before the opener-contribution step,
a7f885c1) and the engine before its cold-cost fix (dev a047cc99). They
indicate behaviour; they are not the gate's table.

## Commands

```sh
# convert (one pdftex at a time; recordings under target/corpus/traces are reused)
PHITEX_TRACE_COMMIT=$(git rev-parse HEAD) CARGO_BUILD_JOBS=8 PHITEX_MEMORY_MAX=12G \
  nice -n 19 ionice -c 3 scripts/sandbox crates/tools/trace/replay.sh
# gate (threads 1: cold, then 200 edits with --check-every 50 at --spec off and predicted)
CARGO_BUILD_JOBS=8 PHITEX_MEMORY_MAX=12G nice -n 19 ionice -c 3 scripts/sandbox crates/tools/trace/gate2.sh
```

Engine: the top-level commit of this branch (e0a7f86c for the runs below;
the vendored defuse subtree has no commit of its own).

## Conversion

At the stop, 20 of the 33 documents were converted with the current
converter (a7f885c1), and the v1 rules (`src/v1.rs`) accepted all 20.
- LaTeX docs/: all 20 except latex-math, which was not reached in this pass.
- pdfdocs/ and plain/: not reached in this pass.
The earlier pass converted latex-math and all five plain/ documents
(accepted).

## Indicative runs (earlier conversion, engine before a047cc99, threads 1)

| document | steps (top-level) | runs cold | runs / node | critical path | cold wall (loaded machine) | edits, check-every 50 | nodes run vs bound, p50 |
|---|---|---|---|---|---|---|---|
| plain-story | 527 (226) | 563 | 1.07 | 14 | 19 ms | 183 edits, checks pass | 1 vs 0 |
| plain-table | 909 (154) | 912 | 1.00 | 39 | 43 ms | 200 edits, checks pass | 3–4 vs 0 |
| plain-display | 563 (310) | — | — | — | — | 135 edits, checks pass | 2 vs 0–3 |
| plain-hyphens | 483 (23) | 501 | 1.04 | 10 | 14 ms | 67 edits, checks pass | 1 vs 0 |
| plain-pages | 2141 (316) | 2156 | 1.01 | 27 | 48 ms | 108 edits, checks pass | 1 vs 0 |
| latex-math | 91919 (952) | 18 683 322 | 203 | 249 | 129 s | incremental differs from cold at the check after edit 50 | — |

Critical path is the client's `eng.critical_path()`. The edit-impact
analysis gave latex-math a critical path of 0.04–0.3% of steps
(REPORT.md); the engine's 249 on 91 919 steps (0.27%) is consistent with
it.

## Engine findings (forwarded)

1. Cold evaluation hangs at 100% CPU for a step with scope "open" and
   "contributes", followed by a reader (reads_prev_contribution) of the
   same sequence. Repro is 2 steps. The converter now emits an opener's
   contribution as its own step before the opener.
2. Cold evaluation re-ran each node about 203 times on latex-math. Fixed
   on dev in a047cc99.
3. On a valid trace, the incremental result differs from a cold replay
   (latex-math, at the check after edit 50).
4. Stream claims easily form cycles; the converter avoids each case (see
   REPLAY.md, claim):
   - an `until` fires only on a defined confirm field;
   - the delimiter's reads and the confirm read bind through the
     claimer's pending set;
   - a reader of the list reads the claimer's contribution.

## Format v1 fields that fit poorly

- **Claim counts** (`known`, `predicted.n`) are steps, but the text a claim
  takes is no step of pdfTeX's trace, so the count is always 0.
- **`confirm`** needs a field the trace defines; a primitive's or the
  format's meaning has none.
- **`may_define`** goes only with `writes: null`, so a step with
  children and writes of its own gets `any`.
- **Edits** reach only top-level steps, so a nested target becomes a
  shadowing `insert_after` or `delete` of its ancestor.
- **`meta`** admits no extra keys, so provenance goes to a sidecar file.
- **`pos`** must strictly increase at the top level, but execution order
  is not text order, so it is clamped.
- **`depth`** is not written: TeX's group level does not match the
  engine's scopes.
- **`expect.steps_reached`**: the engine side wants it named
  `reached_bound`, but the loader rejects that name.
- **The mode** is not in the trace, so `hlist` vs `vlist` is a guess.

## Remaining

1. Merge dev (a047cc99 or later) into replay-traces.
2. Run both commands above for all 33 documents, without the pgf manual.
3. Fill the table, before and after the fix, from
   `target/corpus/gate2/*.log`, recording for each document where the
   check-50 mismatch lands.
