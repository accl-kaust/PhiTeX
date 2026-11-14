# A format's text in chunks: continuations that hold no text (design, 2026-11-14)

Status: built (steps 1-3, 2026-11-14), as agreed with the coordinator
on 2026-11-14, with the change §2.6 gives: chunks are named by their
place, not their text, and a run reads each line as an element of its
chunk's field. Item (b) of the per-edit overhead work.
The gate is the byte-identity of the -ini tests (`crates/engines/tex/tests/pdftex.rs`,
the plain and initex suites). The test is the cli `big` fixture: a comment
edit runs only the edited node, and the PDF equals a clean build's.

## 1. The defect

A text written for INITEX's catcodes (`hints::looks_initex`: plain.tex,
latex.ltx, a document run without a format) is not sliced: it is one
node. Its runs cut where a page may fire (docs/spec-continuations.md), and
each rest-of-piece node resumes from a continuation, `cont:<piece>:<k>`
(`Run::cont_encode`). That value holds the run's whole input left. For a
document node that is a few frames. For an -ini text it is every line not
yet read, as `"s"` frames with their absolute positions
(`Run::leftover`, `line_frame`).

So an edit anywhere in the text changes the continuation of every cut
before it:
- its text, where a line's bytes changed;
- its positions, where the edit moved the bytes after it.

Each of those rest nodes is then asked again and runs, although none has
read the edited line yet. On the cli `big` fixture a comment edit ran 20
resumes. The user's rule: when nothing a run read changed, nothing runs.

The rule for continuations follows from it. **A continuation's key is
what it has read so far plus where it resumes.** It is never the text it
has not read.

## 2. The model

### 2.1 Chunks

The text is cut into chunks at line ends, by content:
- A line ends a chunk where a hash of its own bytes is 0 mod 16 (from
  the 8th line of the chunk on).
- No chunk is longer than 256 lines.

Boundaries then move only where an edit touches them. A chunk's name
is a number that stays while its place does (§2.6): the chunks an edit
cuts again take the names of the old ones they replace, in order, and
any more get new ones; the first chunk is always 1. An -ini text is not
in canonical form (its comment character is the text's to set), so a
comment edit changes its chunk's line like any other edit.

Each chunk is published as environment fields, pushed with the document:

| Field | Value | Read by |
|---|---|---|
| `jtext:<id>` | the chunk's lines, a list of strings | a run, one element per line it reads |
| `jnext:<id>` | the next chunk's id (none at the end) | a run that reads past the chunk's last line |
| `jline:<id>` | the line number of the chunk's first line | `\inputlineno`, a diagnostic, `\errorcontextlines` |
| `jpos:<id>` | the byte offset of the chunk in the text | never a run: origins, diagnostics' places, SyncTeX |

An edit inside one chunk changes that chunk's `jtext`, in the elements
of the lines it changed (and the ones after them where it added or
removed lines). It changes `jnext` only where it moved a boundary, and
`jline`/`jpos` of the chunks after it only where lines or bytes were
added or removed. Those two are read only where a run needs a number,
never to read text. A chunk an edit removed keeps its fields: a run that
resumes in it before the run before it is asked again reads the text it
was cut from, not an end of text.

### 2.2 Frames

A new frame kind, `"j"`: `[tag, chunk name, line in the chunk]`, in
place of the `"s"` frames of the lines a continuation has not reached.
The line a run is cut inside stays an `"s"` frame of its rest (one line,
which the next run reads).

The reader (`input.rs`, `chunks.rs`) gets a `Seg::Chunk` (chunk, line):
the text from that line on, the line there when the segment was made.
Where the run reaches it, it reads that line, element `line` of
`jtext:<id>` (an environment read through a path, `Ctx::env_path`), and
whether the chunk has a line after it (`[line + 1, present]`); past the
chunk's last line, `jnext:<id>`. The segment becomes the line and the
segment of the line after it, so whether input is left is known without
a read.

`Run::leftover` and `cont_encode` write one `"j"` frame for the rest of
an -ini text: the chunk and line where it resumes. They write no text
and read nothing.

### 2.3 What a resume reads

A rest node reads:
- its continuation: the chunk and line where it resumes, and the run's
  state;
- each line it reaches, and whether the line after it is there; the
  `jnext` of each chunk it reads past.

An edit of a line it did not reach moves none of these, so it stands.
An edit of a line it read re-asks it at that read: its own lines, as for
a document node. An edit that adds or removes lines re-asks the runs
that read the lines after it in the same chunk (their elements moved);
the chunks after it keep their names and lines.

### 2.4 Line numbers and places

`\inputlineno` in a chunk reads `jline:<id>` plus the line in the chunk.

A diagnostic made in a run records (chunk id, line, column). The Build
places it through `jpos`/`jline` when it reports the diagnostic, not in
the run. Glyph origins do the same: a span is (chunk id, offset), and
`origins_input` resolves it with `jpos`.

### 2.5 The environment push

The text's push (`Graph::push_text` at the build, `Absorb::absorb` at
an edit) cuts the chunks again from the edited lines to the first
boundary that comes back (`chunks::rechunk`), and pushes only the fields
that changed (`chunks::changes`). A same-bytes save pushes nothing. The
text's node holds `["j", 1, 0]` whatever the text: an edit never
replaces it.

### 2.6 Why chunks are named by place

The design first named a chunk by its text's hash. Then an edit renamed
its chunk, and every continuation that named a line of that chunk
changed: each resume cut inside the edited chunk ran again, though it
read nothing the edit changed (`edits::a_gobbled_call_is_read_for_its_shape_only`:
a `\gdef` edited on the text's third line re-ran 5 nodes, not 1). A
name that stays, and a line read as an element of the chunk's field,
make a continuation name only where it resumes and a run depend only on
the lines it read.

## 3. Steps

1. **Chunker.** `chunks::chunks_of(text)`, `chunks::rechunk`, with unit
   tests: boundaries stable under an edit away from them; a recut equal
   to a cut of the whole text; names kept, each given once.
2. **Fields and push.** The four fields per chunk, pushed by diff
   against the last push, for texts that `looks_initex`.
3. **Reader.** `Seg::Chunk` and the `"j"` frame. `leftover`/`cont_encode`
   write `"j"` frames for an -ini text. Gate: the -ini tests byte-identical
   at -j1 and -j8.
4. **Numbers.** `\inputlineno`, diagnostics and origins through
   `jline`/`jpos`.
5. **Test.** The cli `big` fixture: a comment edit runs only the edited
   node, a word edit runs that node and the resumes that read past it,
   and the PDF equals a clean build's.

## 4. What it does not change

- Document slicing (`incr`, hints) is untouched; a LaTeX or plain
  document with a format is already sliced into nodes.
- The continuation's state (nest, groups, conditionals, counters) is as
  in docs/spec-continuations.md.
- The engine (defuse) gains one call, `Ctx::env_path`: an environment
  read through a path, which its environment readers already held
  (`EnvReader::path`).
