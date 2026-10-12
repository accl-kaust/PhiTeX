# Replay traces: recorded pdfTeX workloads for a TeX-free engine

`phitex-trace replay` turns a recorded trace (`phitex-trace record`, see
`src/lib.rs` and `REPORT.md`) and the document's source files into steps
a TeX-free engine replays to measure itself, and the edits it measures
edit cost with. Nothing here runs TeX: the steps are what pdfTeX's e-TeX
tracing log showed, joined to what the syntactic pass
(`phitex_source::pass`) reads from the text.

```sh
# every cargo command and every pdftex run through the sandbox
PHITEX_TRACE_COMMIT=$(git rev-parse HEAD) scripts/sandbox crates/tools/trace/replay.sh [DOC...]
#   for each document (latex-*, pdf-*, plain-*; not the pgf manual):
#     phitex-trace record DOC.tex --engine pdflatex|pdftex --out target/corpus/traces/NAME.jsonl
#         (a cold run in a fresh scratch directory; skipped when the trace exists,
#          PHITEX_RERECORD=1 records again)
#     phitex-trace replay target/corpus/traces/NAME.jsonl --out-dir target/corpus/replay/NAME \
#         --doc NAME --dirs DOCDIR:SCRATCH --commit SHA
#     phitex-trace validate target/corpus/replay/NAME/trace.jsonl target/corpus/replay/NAME/edits.jsonl
#   phitex-trace index target/corpus/replay > target/corpus/replay/INDEX.md
```

Outputs (under `target/`, never committed): per document `trace.jsonl`
and `edits.jsonl` in **format v1**, the replay client's
(`crates/vendor/defuse/examples/replay.rs`, loader in
`examples/common/trace.rs`), `edits.meta.jsonl` (each edit's provenance)
and `meta.json` (the provenance and counts, for the index: format v1's
meta line admits no other keys). `replay` checks both files against v1's
rules (`src/v1.rs`, a copy of the loader's) before it returns; `validate`
checks any file again. Replay a document with

```sh
scripts/sandbox cargo run --release -p defuse --example replay -- \
    target/corpus/replay/NAME/trace.jsonl target/corpus/replay/NAME/edits.jsonl \
    --threads 1 --check-every 50
```

## Format v1, as written

Line 1: `{"meta": {"version": 1, "document": NAME, "steps": TOP-LEVEL STEPS,
"sequences": ["vlist", "hlist"], "env": {"files": 1, "time": 1}}}`; then
one top-level step per line, the steps a call unfolds nested in its
`children`. Per step:

- **`id`**: `s` and the recorded trace's id (dense from 0 in the order
  pdfTeX acted, nested ones included); an inserted step is `e` and the
  edit's index.
- **`class`**: the primitive's or macro's name (`\hbox`, `\section`), a
  conditional's (`\ifx`), `char` (the first character of a run), `space`,
  `{` and `}` (group characters), `font` (a font selection), `page` (the
  output routine TeX inserts: the page builder).
- **`pos`**: the text order (`file_index * 2^32 + byte_offset`, see the
  join below); at the top level made strictly increasing (a step whose
  text order is not after the step before's gets that one's plus 1:
  `counts.pos_clamped`), as the loader requires; nested steps keep theirs.
- **`depth`**: not written. TeX's group level does not match the scopes
  the engine counts (the paragraphs it opens implicitly, `\begin` and the
  `\begingroup` its body runs).
- **`reads`**: the recorder's, without `"?"` (a read the log does not
  show) and without the clock's parameters (`env_reads`).
- **`writes`**: the recorder's, `locality` `global` for a `\global`
  assignment (or a `retaining` at a group end). A step with `children`
  and no writes of its own is a pending definition set: `writes: null`
  and **`may_define`**, the pass's bound (`Hints::may_define`, with its
  localities) if every write of the step's subtree in the trace is within
  it, else `any` (`counts.may_unsound`); an unjoined call, `any`. A step
  with children and writes of its own keeps them (the loader takes no
  bound beside writes: the engine assumes `any`, `counts.may_own_writes`).
- **`scope`**: the pass's `ScopeHint` for the joined atom (a prediction
  as its effect: the trace saw it); unjoined, what the trace shows (group
  characters, `\begingroup`/`\endgroup`). **`scope_kind`** on an opener:
  `V` (`\vbox`, `\vtop`, `\vcenter`), `H` (`\hbox`), `M` (`$`), `D`
  (`$$`); none for a simple, semi-simple or environment group. An item
  that starts or ends a paragraph by itself (`Implicit`) is
  `scope: "pending"` with **`starts_implicit: "H"`** or
  **`closes_implicit: "H"`**, unless it has a scope effect of its own,
  which wins (`counts.implicit_dropped`). A top-level close with no
  top-level open before it is written `pending` (`counts.scope_unmatched`).
- **`contributes`** (`hlist`/`vlist`): characters, spaces in a paragraph
  and horizontal material (`\hskip`, `\unhbox`, `\vrule`, ...) to `hlist`;
  vertical material (`\vskip`, `\hrule`, `\unvbox`, ...) to `vlist`;
  material either list takes (`\kern`, `\penalty`, `\hbox`, `\special`,
  ...) to the current one. A step that opens a scope does not contribute
  itself (the engine takes no contribution from an opener): its
  contribution is a step of its own just before it, id `s<id>c`, class
  `contribution`, no reads or writes (`counts.opener_contributions`). The current list is a guess: the trace does not show the mode;
  a paragraph is taken to start at horizontal material or an implicit
  starter and to end at vertical material, an implicit closer or `\par`.
- **`reads_prev_contribution`**: the catalogue's F2 readers (`\lastbox`,
  `\unskip`, `\lastpenalty`, `\prevdepth`, `\spacefactor`, `\/`, `\over`
  ..., `\par`), of the current list.
- **`fold`**: `{"sequence": "vlist", "step_class": "page"}` on the `page`
  steps.
- **`claim`** (the pass's `Hints::claim` for the joined atom). The
  trace's steps are what pdfTeX did, and the text a claim takes (a
  macro's arguments, a `\def`'s body) is no step of it: a count is always
  0 steps. `Known(n > 0 tokens)`: `{"known": 0}`. `Predicted`:
  `{"predicted": {"n": 0, "until": {"class": C, "inert": false,
  "confirm": [F]}}}`, C the class of the step after the claimer's subtree
  (its delimiter), F the field of the assumption (`cs:\name`,
  `catcode:N`); F is dropped (`confirm: []`, `counts.confirm_dropped`)
  where no step before the delimiter writes it (a primitive's or the
  format's meaning: an undefined field would never end the claim) or the
  claim may define it. `Pending` with a cause that is a delimiter
  (`delimiter ...`, `verbatim ...`, `parameter text ...`): `{"until":
  {"class": C, "inert": false, "confirm": []}}`. Otherwise, and where the
  delimiter is not a sibling leaf, reads what the claimer's subtree
  writes or its bound covers, or reads the current list (each would be a
  cycle: the claimer waits on the delimiter's output),
  `"claim": null, "value_dep": true` (`counts.claim_fallback`). No field
  gives a count, so no `count_from`; no `limit`.
- **`env_reads`**: `time` for `\time`, `\day`, `\month`, `\year`,
  `\pdfcreationdate` and reads of those parameters; `files` for
  `\input`, `\openin`, `\read`, `\readline` and pdfTeX's file queries.
- **`children`**: the steps a macro call (or a token list TeX inserts)
  unfolds, nested; see `unfold` below.
- **`cause`**: the pass's cause kind for a non-Known item, `predicted`
  for a prediction with none, `join-failed` where the step did not join,
  `inserted` on an edit's new step. **`prior`** is not written (cold
  traces). **`unfold`**, **`limit`**, **`count_from`** are not written.

**Edits** (`edits.jsonl`, v1 ops). `expect.steps_reached` is the
edit-impact analysis's steps reached: an upper bound over the recorded
def-use edges, with no cutoff (a step reached is counted whether or not
its value would change), excluding the edited step's own steps (its
line's, or the deleted call's subtree). A run that executes no more than
the bound is consistent with it; the ratio of nodes run to the bound
measures the engine's cutoff. (The engine side would rather call it
`reached_bound`; the client's loader accepts only `steps_reached`, so the
name stays until the loader changes.) the analysis's sample as before (the main
file's lines that write, seed 1, 200 lines; a form and one of the line's
writing steps, seed 2). v1 edits name top-level steps, so a sampled step
inside a call is reached through its top-level ancestor: `change_writes`
of a top-level step rewrites its writes (new text, new values); a nested
step's `change_writes` and every `insert_after` insert, after the
ancestor, a step `e<k>` writing the sampled step's fields (it shadows them
for what follows); `delete` deletes the ancestor (the whole call;
expected: the steps reached from its subtree). An ancestor an earlier
edit deleted moves to the top-level step before it. Expectations are the
analysis's for the sampled step's writes otherwise. `edits.meta.jsonl`
records per edit the op, form, target, anchor, line and both impacts.

## How the steps are joined to the text

The log names every primitive, character, macro call (with its body)
and group, but not where in the text each came from: it reports input
lines only at group and conditional boundaries. The converter recovers
positions by modelling TeX's input stack:

- the bottom of the stack is the file the log's parentheses say is being
  read; each macro call pushes its body as the log prints it
  (`\foo #1->body`);
- a step matches the next unconsumed atom of the pass's reading of a text
  (a control sequence by name, a character by its character, a group
  character by `{`/`}` or `\bgroup`/`\egroup`, a blank space by a space),
  in the innermost text that has one: up to 8 atoms back (an atom
  `\expandafter` skipped over), then 4096 forward in a body, 512 forward
  in a file (and again from the start of the line the log last reported);
- a match in a lower text finishes the texts above it, except a body
  that waits at a parameter with more after it (the argument comes from
  below); a body read to its end is finished;
- a token shown twice across a mode change (TeX backs it up to start a
  paragraph, `tex.web` §1090) is one token: the second step takes the
  first one's position and its claim.

Every file the log opens is read (absolute paths as printed; `./x`
relative to the document's directory and the scratch directory), its
definitions harvested into the document's definitions (in the order the
log opens the files) and the pass run over it with them in scope. Bodies
are analyzed with the document's definitions as harvested so far.

## Conventions

- **text order** (`pos`): `file_index * 2^32 + byte_offset`, files numbered in
  the order they first appear in the trace. A step joined to a file has
  its atom's offset (`position: exact`); a step joined to a macro body has
  its call's position (`call-site`; recursively, the call's own
  position); a step not joined has, if a body is open, the innermost
  call's position (`call-site`), else the furthest position joined in its
  file or the start of the line the log reported, whichever is later
  (`line`), else the step before's (`inherited`).
- **unfold** (`children`): a call's steps are those after it until a
  step read from the call's own text or below (the text after the call,
  or an argument written there: its expansion level, the depth on the
  modelled input stack of the text it joined, is at most the call's),
  nested: a call inside is a child with its own children. A subtree ends
  at its parent's end at the latest.

## What is approximate, and why

- **Positions** come from a heuristic join, not from TeX: the log shows
  neither the token a step read nor its place. A join can land on the
  wrong atom of the same name (a repeated `\fi`, a letter that occurs
  again further on), and the windows bound how far it looks.
- **Primitives read through aliases**: `\tracingcommands` shows a
  primitive's own name (`\expandafter`), while the text says
  `\exp_after:wN`, which the format `\let` to it, untraced. An unmatched
  primitive takes the innermost text's next control sequence as an alias
  when that name was never seen as a macro or another primitive, and the
  alias is remembered (`counts.aliases`).
- **Bodies printed under another `\escapechar`** (LaTeX sets it to -1
  while it handles file names): their control sequences cannot be told
  from text, so the recorder keeps no body (`body_hidden`), and the
  converter uses the same macro's body printed with `\` elsewhere in the
  trace (`counts.bodies_borrowed`) or an empty one (`bodies_hidden`).
- **Characters**: `\tracingcommands` shows only the first character of a
  run (TeX's main loop takes the rest silently), so a word is one step;
  the step joins to its first character.
- **Macro arguments** are not in any body: their steps join to the
  caller's text when the argument was written there, otherwise not.
- **Unfold extents** use the modelled expansion level, so they inherit
  the join's errors; an argument's steps end the call (they join the
  caller's text), and the rest of the call's body then follows at the
  caller's level.
- **The current list** (`contributes`, `reads_prev_contribution`) is a
  guess from the material's kind: the log shows the mode only when it
  changes, and the converter does not follow boxes.
- **Plain documents**: plain.tex is loaded untraced, like a format, and
  its macros' bodies come only from the log; the recorder's wrapper
  (tracing settings, `\input`) is a file of the trace too.
- **Edits** are chosen on the analysis's line attribution (the log's
  coarse lines), so a sampled line's steps may come from neighbouring
  lines; the expected impact is the analysis's, under its def-use model
  (reads the log does not show are not edges).
- **Recording** is one cold run (no `.aux` from an earlier run).
- **Files** are read as UTF-8 with replacement: a byte that is not UTF-8
  shifts the offsets after it on that line (the pass works on text).

## Counts (`meta.json`'s `counts`, and `INDEX.md`)

The join's (`steps`, `fields`, `macro_calls`, `joined_exact`,
`joined_body`, `join_failures`, `unkeyed`, `reshown`, `aliases`,
`bodies*`, `files`, `files_read`) and format v1's: `top_level`,
`claim1_known`/`_predicted`/`_until`/`_null`, `claim_fallback`,
`confirm_dropped`, `may1_none`/`_names`/`_prefixes`/`_any` (pending
sets), `may_unsound`, `may_own_writes`, `scope_unmatched`,
`implicit_dropped`, `pos_clamped`, `contributes`, `reads_prev`, `folds`,
`env_steps`, `opener_contributions`, `edits`. (`consumes_*`, `claim_*`, `may_*` are the older
format's, kept for comparison.)
