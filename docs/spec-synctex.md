# SyncTeX: design (no code yet)

Status: design only. Nothing here is built until stable node ids land
(r3-cuts), because the anchors below are those ids. Bar: recording
positions costs nothing measurable on amsldoc, cold or edit (callgrind
Ir and peak RSS within noise with SyncTeX off, and within 2% with it
on).

## What it is for

- `JOB.synctex.gz` as pdfTeX writes it (`-synctex=1`), for external
  editors and viewers.
- In `phitex watch`: double-click on a page opens the source line in the
  editor (inverse search), and the editor's cursor shows its place on
  the page (forward search). The viewer's find maps to source places too.

The file need not be byte-identical to pdfTeX's (it is not an output
the oracle compares, like the `.log`). It must give the same answers:
the same file and line for a point on a page, and the same box for a
line.

## What pdfTeX records

synctex.c gives every box, glue, kern and math node a *tag* (the input
file) and a *line*, taken from the input when the node is made. At each
`\shipout` it writes one sheet: for each box its kind (`h`/`v`), tag,
line, position and size, and inside it records for glue, kern and math
nodes (tag, line, position). Characters carry no record: a line of a
paragraph is found through the glue between its words, each made where
its word was read.

## Where PhiTeX has positions

- In a run, the input segment being read has `origin` (the file's name
  value and the byte of the segment's start) and `lineno`
  (`input.rs`, `Seg`). `Run::diag_kind` already turns this into a
  diagnostic's place, the same thing SyncTeX needs.
- A document piece's byte is relative to its node's text (`Src::Doc` in
  `api.rs`), and the file's line is found from the document's line
  starts when a diagnostic is placed (`Build::diagnostics`).

## The record: an anchor, not a line

A record stores `(anchor, offset)`, not `(file, line)`:

- `anchor`: the stable id of the document node whose text was being read
  (a paragraph, display, alignment or picture node), or the name value
  of an `\input` file read whole.
- `offset`: the byte in that node's text or that file.

The client turns `(anchor, offset)` into a file and line when it writes
the file or answers a click, using the text as it is now. That is the
point of the design. Inserting a line at the top of a document changes
the line number of every later box. With lines in the records, every
later page step's output would change, and every page would run again
after a one-line edit. With anchors, a page step's records change only
when its own material changes, so SyncTeX costs edits nothing.

## Carrying it (only when asked)

`Options::synctex: bool`, off by default (the CLI turns it on with
`--synctex`, and `watch` turns it on when a viewer is open).

- Off: no record is made. The only cost is one branch on a flag
  that a run reads once, where a box, glue, kern or math node is
  appended. Measured on amsldoc before merging.
- On: the anchor and offset (a `u32` each) go with the node. Where they
  live is the one choice still open, to be made by measurement:
  - (a) a `src: Option<NonZeroU32>` index into a per-run table, in
    `BoxNode` and the glue/kern/math variants. Its size must stay inside
    the existing padding, or `Node` grows for every document.
  - (b) a parallel `Vec<u32>` beside each `List`, kept only when SyncTeX
    is on, and moved with its items by the operations that rearrange a
    list (line breaking, `\unhbox`, page building). This doesn't touch
    `phitex-typeset`'s node size, but every list operation has to keep
    the vector aligned.

  (a) is simpler and stays right by construction. Take it if the size
  check passes.

## The API

```rust
/// One SyncTeX record of a shipped page, in scaled points from the
/// page's top left corner (pdfTeX's sheet coordinates).
pub struct SyncRecord {
    pub kind: SyncKind,       // HBox, VBox, VoidHBox, VoidVBox, Glue, Kern, Math
    pub anchor: Anchor,       // a node id, or an \input file's name
    pub offset: u32,          // byte in the anchor's text
    pub h: i32, pub v: i32,   // position
    pub width: i32, pub height: i32, pub depth: i32, // boxes only
    pub depth_level: u16,     // nesting, for the sheet's open/close
}

impl Build {
    /// Page k's records, made by its page step alongside its PDF piece
    /// (None: SyncTeX off, or no page k).
    pub fn synctex_page(&self, k: usize) -> Option<Arc<[SyncRecord]>>;
    /// The file and line of a record's anchor and offset, in the text as
    /// it is now.
    pub fn place(&self, anchor: Anchor, offset: u32) -> Option<(String, u32)>;
}
```

The page step makes its records when it ships the page, as pdfTeX does
at `\shipout`, and keeps them in its result next to the PDF piece. They
are part of the step's output, so page steps stay independent, as the
no-sequential-page-steps rule requires. An edit that leaves a page alone
leaves its records alone.

## The CLI's side

- `phitex build --synctex` writes `JOB.synctex.gz`: the preamble with
  the inputs' tags, then one sheet per page, each record's place resolved
  through `Build::place`, gzipped with the workspace's pure deflate.
- `phitex watch` keeps an index from page to records, rebuilt for the
  pages that changed. A double-click on page k at (x, y) picks the
  innermost box there (then its nearest glue or kern record), resolves
  it, and sends `file:line` to the editor (`editor.rs`). Forward search
  takes the cursor's `file:line`, finds the records whose resolved line
  is nearest it, and scrolls the viewer to them.

## Open questions for the engine side

1. Are node ids stable across edits for paragraphs whose text didn't
   change (r3-cuts)? This design assumes they are.
2. Which runs make the page's material for a line from an `\input` file
   read in pieces (`Run::doc_file_regions`)? Is the anchor that file's
   name plus a byte, or the piece's node?
3. Choice (a) or (b) above, once `Node`'s size is checked.
