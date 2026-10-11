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
#   for each document (latex-*, pdf-*, plain-*, pgfmanual):
#     phitex-trace record DOC.tex --engine pdflatex|pdftex --out target/corpus/traces/NAME.jsonl
#         (a cold run in a fresh scratch directory; skipped when the trace exists,
#          PHITEX_RERECORD=1 records again)
#     phitex-trace replay target/corpus/traces/NAME.jsonl --out-dir target/corpus/replay/NAME \
#         --doc NAME --dirs DOCDIR:SCRATCH --commit SHA
#     phitex-trace validate target/corpus/replay/NAME/trace.jsonl target/corpus/replay/NAME/edits.jsonl
#   phitex-trace index target/corpus/replay > target/corpus/replay/INDEX.md
```

Outputs (under `target/`, never committed): per document
`trace.jsonl` (one `{"meta": ...}` line, then one step per line),
`edits.jsonl` and `meta.json` (the meta object again, for the index).
`replay` validates both files before it returns; `validate` checks any
file again.

## The step format

```
{ "id": u64,                       // dense, 0.., the trace's order (the order pdfTeX acted)
  "text_order": u64,               // file_index * 2^32 + byte offset
  "nesting_depth": u32,            // group level at the step (\tracinggroups)
  "reads": ["field", ...],
  "writes": [{"name": "field", "global": bool}, ...],
  "consumes": {"Known": n} | "Pending" | null,
  "may_define": "None" | {"Names": [..]} | {"Prefixes": [..]} | "Any",
  "unfold": null | {"produced": [step ids]},
  "hints": { "claim": {"Known": n} | {"Predicted": [n, "confirm_field"]} | "Pending",
             "scope": null | {"Known": "Open"|"Close"} | {"Predicted": ["Open"|"Close", "confirm_field"]},
             "cause": null | "string tag" },
  "source": {"file": "...", "line": u32, "position": "exact"|"call-site"|"line"|"inherited"} }
```

Fields are the recorder's (`count:12`, `cs:\foo`, `param:\tolerance`,
`catcode:64`, `font:current`, ...; `src/fields.rs`). `meta` carries the
document's name, the main file, the engine, the command the recorder ran
(with the tracing settings), the engine's banner (`This is pdfTeX,
Version 3.141592653-2.6-1.40.29 ...`), the trace tool's commit, the
conventions below (as strings) and the counts the index shows.

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

- **`id`**: the trace's id, dense from 0 in the order pdfTeX acted.
- **`text_order`**: `file_index * 2^32 + byte_offset`, files numbered in
  the order they first appear in the trace. A step joined to a file has
  its atom's offset (`position: exact`); a step joined to a macro body has
  its call's position (`call-site`; recursively, the call's own
  position); a step not joined has, if a body is open, the innermost
  call's position (`call-site`), else the furthest position joined in its
  file or the start of the line the log reported, whichever is later
  (`line`), else the step before's (`inherited`). `source.line` is the
  joined atom's line, or the log's.
- **`nesting_depth`**: the group level `\tracinggroups` reports at the
  step's start (`entering ... group (level N)`).
- **`reads`/`writes`**: the recorder's; reads keep `"?"` (a read the log
  does not show); writes drop the recorder's value hash.
- **`unfold.produced`**: for a macro call (and a token-list parameter TeX
  inserts, `\output->`), the steps after it up to the next call at the
  same or a shallower *expansion level*, where a step's expansion level
  is the depth on the modelled input stack of the text it joined (0 for
  a file; an unjoined step keeps the current depth). Produced lists nest:
  a step is in every open call's list.
- **`consumes`**:
  - a macro call: `Known(n)`, n the steps up to the next macro call (the
    recorder's call extent), or `Pending` where the pass says the call's
    extent is pending or the call did not join;
  - a primitive or conditional joined to an atom the pass gives an
    extent: `Known(n)`, n the steps up to the first step joined at or
    past the end of that extent in the same text (or to one in another
    file, or the text finished), `Pending` where the pass says pending;
  - `null` for a step that consumes nothing (characters, groups, fonts,
    an atom the pass reads as another command's operand, a primitive the
    pass gives no extent); `Pending` for an unjoined primitive the
    recorder's table says consumes.
- **`hints.claim`**: the pass's status for the joined atom: `Known(n)`,
  `Predicted(n, "cs:\name")` (a definition visible in the text, right
  unless `\name` is redefined: the field to confirm), `Pending`; n as in
  `consumes` (0 when no claim was open). `Pending` where a control
  sequence step did not join; `Known(0)` for characters and groups.
- **`hints.scope`**: the direction from the step: group characters,
  `\begingroup`/`\endgroup`, `\begin`/`\end` (the macros); `Known` when
  the pass pairs that atom with a status Known, otherwise `Predicted`
  with the field that decides it (`catcode:123`/`catcode:125` for braces,
  `cs:\begingroup`, `cs:\begin`...). The pass's Pending pairs are
  reported as Predicted: the trace saw the group open or close.
- **`hints.cause`**: the pass's cause kind for a non-Known item
  (`macro defined elsewhere`, `\expandafter`, `visible definition`,
  ...), `predicted` for a Predicted item with no cause, `join-failed`
  where the step did not join.
- **`may_define`** (field names, `cs:\x`):
  - the pass's definition harvest at the joined atom (`\def`,
    `\newcommand`, `\let`, `\newif` (three names), `\newcount`,
    `\newcounter`, ...): `Names`;
  - a primitive definer (`\def \gdef \edef \xdef \let \futurelet \chardef
    \mathchardef \countdef \dimendef \skipdef \muskipdef \toksdef \font
    \read \readline`) the harvest did not read: its literal target
    (`Names`), a `\csname` target with a literal prefix lit
    (`Prefixes(["cs:\lit"])`, checked against the step's written names
    in the trace and made `Any` when they disagree), else `Any` (a
    parameter, `\expandafter`, an empty prefix);
  - an unjoined primitive definer: `Any`;
  - a macro call: what its own body defines directly (the harvest and
    the rules above over the body), `Any` if any definition in it has no
    literal target; not through the macros it calls, whose steps carry
    their own;
  - `None` otherwise.
  Mixed names and prefixes are reported as `Prefixes` (a name is a
  prefix of itself).
- **edits** (`edits.jsonl`): the edit-impact analysis's sample (the main
  file's lines that write something, shuffled with seed 1, the first 200:
  `Graph::edit_samples`, the same lines `analyze` reports on), visited in
  that order round after round while fewer than 200 edits are drawn; for
  each, a form (`change_writes`, `insert_after`, `delete`) and one of the
  line's writing steps drawn with seed 2, no (form, step) twice; fewer
  than 200 when the lines' writers run out. Each record carries `line`,
  `expected_impact` (the analysis's steps reached from all the line's
  writes, its own steps excluded) and `expected_impact_step` (the same
  from the chosen step's writes alone). `insert_after`'s step is the
  target's record with a fresh id (steps + edit index), no reads, the
  target's writes (it shadows them), `consumes: null`, `unfold: null`,
  `may_define` its written `cs:` names, cause `inserted`.

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
  the join's errors; an unjoined call stays open until a call at its
  level or shallower.
- **`consumes` counts steps**, not tokens: a `\def`'s body is no step
  (0); an `\hbox{...}`'s content is the steps until the join passes the
  closing brace.
- **`may_define` of a call** is direct only, and the harvest keeps one
  site for a repeated identical definition in a file.
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

## Counts (`meta.counts`, and `INDEX.md`)

`steps`, `fields` (distinct names read or written), `consumes_*`,
`claim_*`, `may_*` (one per class), `unfolded` (steps inside at least
one produced list), `macro_calls`, `joined_exact`, `joined_body`,
`join_failures` (steps with a name or character that joined no text),
`unkeyed` (font selections, assignments outside any command, `\output`
and other token lists TeX inserts: nothing in a text to join),
`reshown`, `aliases`, `bodies`, `bodies_borrowed`, `bodies_hidden`,
`may_any_join`, `consumes_pending_join`, `prefix_mismatch`, `files`,
`files_read`, `edits`.
