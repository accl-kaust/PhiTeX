# A format's text in chunks: continuations that hold no text (design, 2026-11-14)

Status: the design, written before the build, as agreed with the
coordinator on 2026-11-14. Item (b) of the per-edit overhead work.
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

Boundaries then move only where an edit touches them. A chunk's
identity is the hash of its text, made unique by its occurrence among
equal chunks in order. Comments are already in their canonical form
(`%\0<id>`, `doc:mark:<id>`), so a comment's words are in no chunk's
text: a comment edit changes no chunk.

Each chunk is published as environment fields, pushed with the document:

| Field | Value | Read by |
|---|---|---|
| `jtext:<id>` | the chunk's lines, positions relative to the chunk | a run that reads a line of it |
| `jnext:<id>` | the next chunk's id (none at the end) | a run that reads past the chunk's last line |
| `jline:<id>` | the line number of the chunk's first line | `\inputlineno`, a diagnostic, `\errorcontextlines` |
| `jpos:<id>` | the byte offset of the chunk in the text | never a run: origins, diagnostics' places, SyncTeX |

An edit inside one chunk changes that chunk's `jtext` and its id, and
the `jnext` of the chunk before it. It changes `jline`/`jpos` of the
chunks after it only where lines or bytes were added or removed. Those
two are read only where a run needs a number, never to read text.

### 2.2 Frames

A new frame kind, `"j"`: `[tag, chunk id, line in the chunk, column,
state]`, in place of the `"s"` frames a continuation holds today.

The reader (`input.rs`) gets a `Seg::Chunk` whose lines are read from
`jtext:<id>` when the run first needs one. Each read is a field read,
recorded as any other. Past the chunk's last line it reads
`jnext:<id>` and goes on in that chunk.

`Run::leftover` and `cont_encode` write one `"j"` frame for the rest of
an -ini text: the chunk and line where it resumes. They write no text.

### 2.3 What a resume reads

A rest node reads:
- its continuation: the chunk and line where it resumes, and the run's
  state;
- the `jtext` of each chunk it reads lines of, and the `jnext` of each
  chunk it reads past.

An edit in a chunk it has not reached moves none of these, so it stands.
An edit in a chunk it read re-asks it at that read: its own lines, as
for a document node.

### 2.4 Line numbers and places

`\inputlineno` in a chunk reads `jline:<id>` plus the line in the chunk.

A diagnostic made in a run records (chunk id, line, column). The Build
places it through `jpos`/`jline` when it reports the diagnostic, not in
the run. Glyph origins do the same: a span is (chunk id, offset), and
`origins_input` resolves it with `jpos`.

### 2.5 The environment push

The document's push (`Graph::push_marks`, the text staging) recuts the
chunks from the edited lines to the first boundary that comes back, and
pushes only the fields that changed. A same-bytes save pushes nothing.

## 3. Steps

1. **Chunker.** `hints::chunks_of(text) -> Vec<Chunk>`, with unit tests:
   - boundaries stable under an edit away from them;
   - ids unique among equal chunks;
   - a comment edit changes no chunk.
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
- The engine (defuse) is untouched: chunks are fields, and their reads
  are field reads.
