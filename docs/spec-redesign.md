# The redesign, whole (2026-10-16)

One shot: this document is the final shape of `crates/engines/tex` (and the
output crate's link) on the vendored engine at 7be1080 or later. It replaces
the hint-trusting, chain-shaped client. Nothing in it is a migration step;
old paths are deleted in the same commit. Reference implementations of parts
exist on branches (named per section) and are to be taken, not re-invented.
Bars are tests; a section's test red = not done.

## 0. Invariants (each is a test or a counter)

1. No rule in the client names a LaTeX/expl3 macro, a file extension or a
   command-name list to decide a node's text, claim, declarations or scope.
   `docs/inventory.md` class 2 of `docs/inventory-classes.md` is the deletion
   list; `grep` for the names is the test.
2. Every static declaration (claim, effect/kinds, defs, contributions) is a
   guess the engine verifies; the three counters `claims_refuted`,
   `effects_refuted`, `claim_siblings` are reported per build; a hint pass
   mistake is a counted re-run, never output.
3. No node value contains state another node does not read: no "page so
   far", no "writer state with bytes", no text in a node's identity.
4. Node identity is positional (piece index, page index, unit index, file
   name + unit index). An edit re-runs nodes; it never re-makes them.
5. Bytes identical to pdfTeX at every thread count on every fixture,
   amsldoc (md5 98c96e004c with SOURCE_DATE_EPOCH=1700000000) and para4000
   (md5 bf12c9ddee6407a005c5140fc8a4b5cd).
6. A one-word edit re-runs: its piece, the scan steps until a fire whose
   breaks equal the old, the finishers of moved pages, the link. The sweep
   test (branch edit-sweep, tests/sweep) is the gate with bounds exec ≤ 2 +
   moved pages, finish = moved pages, place = 0 (no place nodes exist),
   dvi/link = 1, files ≤ 1 only when a read-back file changed.
7. The realised critical path is the scan steps plus the link: para4000
   ≤ 1 500 nodes (then ≤ pages + 10 when the scan steps are per page).
8. The paragraph run on native state: µs per paragraph reported against
   pdfTeX's 88 (para4000, -j1, accl); allocations per paragraph reported.
9. Runs per node ≤ 1.1 cold at Speculation::All, 8 threads.

## 1. Node kinds and their fields (positional identity, nothing else)

| node | identity | reads | defines | claim / effect |
|---|---|---|---|---|
| exec(i) | piece index i (document or file unit `wunit:<file>:<i>` / `unit:<file>:<i>`) | entry state: records `cattab parpar pagepar sftab lctab`; any field on first touch (touched-loc list); `scan:state` only when it is a piece after a cut (see resume) | definition delta (locals at their frame via `define_in_scope_at(k)`, globals at own level); `lines:i` (the paragraph's lines, ONE value); `vprof:i` (vertical profile); `wstream:<name>` items; read-time PDF effects as values keyed by what they read (`pdf:objbase`, `pdf:names`), never bytes | claim = hint pass guess (Count/None), verified; effect = `Brackets{closes, opens}` + `set_kinds` for every own open group at run end (branch loopback-units `leave_frames`) |
| scan(i) | piece index i | `vprof:j` for j in [last fire piece, i] (held by the engine's non-running gate: `vprof` in `reads()`), `scan:state` of the last fire | `scan:state` and `breaks:p` ONLY when a page fires in i (equal value otherwise = nothing) | none |
| finish(p) | page index p | `breaks:p`; `lines:j` of its range; routine state after p−1 as guesses (count:0 and routine gdefs from the previous *different* page's finisher, FinDefs from two different pages only: branch profiles fix (a)); `pdf:objbase`/`pdf:names` guesses | page record p = {bytes with symbolic handles, own object count, own byte length, own font usage (chars of this page only: branch profiles 6cc977fa), objects introduced, writes}; routine gdefs; `pages:fire:p` | effect Pending→ as the routine leaves it |
| resume(c) | cut index c (the piece after the page fired inside piece i: `pages:resume` field, positional: branch profiles 4d1965e3) | finisher gdefs of its page as guesses; `scan:state` | what exec defines | as exec |
| files(round) | loop-back round | the job's `\openout` streams (`wstream:*`) | each written file as units by the same slicer: `wunit:<file>:<i>`; `.aux` definition lines as `wdef:<file>:<field>`; round-1 guesses from the previous build's values (no name patterns) | none |
| link | one | every page record p (range read), document-level fields (fonts in load order, `\pdfobj` at document level, outlines, names) | the file: object base per page = prefix sum of object counts; byte offset per page = prefix sum of lengths; xref from those; font subsets = union over pages' own usage; renders ONLY the pages whose record changed (cached chunk per page keyed by record value + its bases) | none |

Deleted node kinds: fold (list assembly by range happens in finish from
`lines:j`), place (prefix sums in link), end node's page replay. `ps`,
`vtail`, `pdf:state`, `pages:foldin`-with-text, `file:<name>` blobs do not
exist. Nothing reads "the page so far".

Read-time PDF commands in a piece (`\pdfstartlink`, `\pdfobj`,
`\pdfannot`, `\pdfdest`, `\pdfrefobj`, `\pdfsavepos` ...) record what they
need as effects in the piece's value with symbolic handles; the object
numbers they return to TeX are relocatable (`h + k`), resolved at link,
except where a number is read as text by the document (`\the\pdflastobj`
into a written line): then the piece reads `pdf:objbase` (a guess from the
previous build / previous page; refuted and re-run when wrong). Exactness of
pdfTeX's numbering is the prefix-sum order: ship order, with immediate
objects where they ran (their count is part of the piece's/record's object
count).

## 2. The run executor (branch node-speed / docs/spec-runstate.md)

Native state inside the run: `Tok(u32)`, `Rc<[Tok]>` lists, flat slot table
for meanings `(cmd,chr)`, registers, params, codes as arrays, fonts by index
with direct metrics; native `Vec<Node>` hlist; typeset `line_break` on native
nodes; `main_control`/expansion/scanners/conditionals/`macro_call` on that
table (§341–§389 shape). Engine `Values` only at: entry (records + first
touch, touched-loc list) and commit (definition delta, `lines:i`, `vprof:i`,
effects). Parking snapshots the native table (copy at park). The old
per-token paths (`mean_of`/`get_f`/`set_cur` hash routes, per-token
encode/decode, `own/defs/def_ix` maps on the hot path) are deleted.

## 3. Scopes (branch loopback-units, engine ≥ 0ea3724)

A run that ends with own groups open leaves every one of them as a nested
frame (`Brackets.opens = n`, `set_kinds` outermost first, locals at their
frame via `define_in_scope_at(k)`, the `\aftergroup` list per frame). A
continuation inside inherited frames closes/opens with `Brackets{closes,
opens}`. `can_leave_scope` has no text test and no node-kind test; the run's
own nest decides. Files reached inside frames are sliced like the document.

## 4. Hints = speed only

The hint pass produces guesses: slices (paragraph-end cuts, every file the
same), claim count, scope effect, may-define set, read hints for the gate.
Every one is declared to the engine as a guess. The pass costs ≤ 10% of
evaluation instructions and is linear in text. Package load records
(`pkgrec`) stay as guesses of definitions (predicted_defs), with the counters.

## 5. Deletions (same commit)

- All `docs/inventory-classes.md` class-2 rules (R6, R7, R16, R24, R26, R28,
  R52–R55, R63–R67, R84; R82/R83 already gone on loopback-units).
- `pages.rs` fold chain (`fold_items` in the exec, `PageState` as a value,
  `vtail`, `needs_own_fold`'s defines-side), place nodes, `pdf:state`,
  `pdfout` replay-on-state for exact mode (replaced by chunk + relocation),
  text in fold/resume args, `file:`/`wfile:` blob fields, static `\input`
  text matching (`inputs.rs` target/plain_input/document_file path test).
- The run's Value-per-token paths (§2).

## 6. Gate (one round, parallel on accl)

suites (pdftex, edits, pages, plain, etex, hints, relaxed, inputs,
defuse_gaps), the latex target (sharded), sweep tests (bounds of §0.6),
amsldoc exact j1/j8/j16 bytes + counters + cp, para4000 bytes + cp + wall
j1/j8/j16 + µs per paragraph table, cold runs per node. One report, failures
first, every number against its bar.
