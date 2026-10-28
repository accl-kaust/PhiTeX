# Glyph origins and SyncTeX: design (phase 1, no code)

Status: built (phase 2), as reviewed by main and the frontends
session; their choices are in §3 and §4. It replaces
the record layout of docs/spec-synctex.md (an anchor and offset in
`BoxNode` and the glue, kern and math nodes): the node format and the
typesetter's char path stay as they are (agent "xetex" is changing
them), and origins are made beside the glyph path, by the PDF page
writer and a pure matcher.

## 1. What is asked

From the brief:

- (a) a switch on `phitex_tex::Build` that records origins, off by
  default;
- (b) per output page, each glyph's source `(file, byte)`, in the order
  MuPDF and `phitex_codecs::pdftext::page_codes` count the page's
  characters (every code a `Tj`/`TJ`/`'`/`"` shows, a form's at each
  `Do`);
- (c) after an edit, pages whose nodes did not run again keep their
  origins (not made again);
- the watch's viewer answers `origins` (crates/cli/src/view.rs, empty
  today), and `-synctex` / `synctex = true` writes `<job>.synctex.gz`
  from the same data.

From the frontends session (requirements):

- (F1) the per-page answer is cheap to fetch from the wasm build:
  parallel `u32` arrays per page (file index, byte offset, ...) and a
  file table, never JSON per glyph;
- (F2) after an edit the build says which pages' origins changed, and
  the viewer fetches only those again;
- (F3) a byte offset is in the file version the Build holds (the text
  staged by `set_file`/`edit`, the editor's buffer), never the file on
  disk.

SyncTeX bars (user, through main), in place of parity with pdfTeX's
file:

- `synctex view` (forward: `file:line` to page and position) and
  `synctex edit` (inverse: page and position to `file:line`) answer
  correctly, word-accurate where the origins allow, checked on a plain
  and a LaTeX fixture on accl;
- writing it costs a small fraction of a build, measured on amsldoc
  cold and after an edit;
- after an edit only the changed pages' records are written again,
  where the format allows.

## 2. Where a glyph's origin comes from

A shipped page is a `BoxNode` tree of `Node::Char { font, ch }` and
`Node::Lig` (with `original`, the characters it replaces). No node
carries where it was read, and none may (the constraint above). What
the build does know:

- every text node's source: `Build::node_spans` gives each document
  node and each `\input` file's slice node its text, its file and its
  byte range in the Build's current version of that file (edits'
  shifts applied, `DocFile::shifts`); the progress offsets
  (`Build::progress_offsets`) are the same map for document nodes only;
- every shipped glyph, in the content stream's order, with its
  position: the PDF page writer (`crates/output/pdf/src/ship.rs`,
  `char_out`, `char_run`, `output_one_char`) emits each code.

Which node made a glyph is not recoverable from the page box itself:
sealed lines name their piece (`nodes::Sealed::piece`), but a line the
routine opened, gave back (`Src::Rtn` spans: LaTeX's float and
`\@specialoutput` give-backs) or moved into an insertion does not, and
a line box value is interned (two pieces making the same line share
one value). So origins are made the way SyncTeX's readers use them: a
glyph's origin is the place in the sources whose text it shows. Two
parts:

1. **The writer's glyph list** (per page, recorded by the PDF page
   writer while it writes the page, only when the switch is on): one
   entry per code the page's own content stream shows.
2. **The matcher** (pure, on demand, per page): the page's glyph text
   matched against the text of the source nodes it comes from, word by
   word; each glyph gets the bytes of the source characters it was
   matched to.

### 2.1 The writer's glyph list (the tap)

`crates/output/pdf` gets a trait the page writer calls when it is set
(`PdfState::tap: Option<Tap>`, a wrapper that clones to `None` and is
outside the state's digest, like `Memo`):

```rust
pub trait GlyphTap {
    /// A code shown for the character node being written: font,
    /// character, the characters a ligature replaces, position (sp,
    /// the writer's `cur_h`, `cur_v`) and width.
    fn code(&mut self, f: FontId, ch: u16, lig: &[u16], h: Scaled, v: Scaled, w: Scaled);
    /// Codes the page shows that no character node made: pdfTeX's
    /// interword and fake spaces (one each), a literal's (`\pdfliteral`,
    /// `\special{pdf:...}`, a virtual font's special: its bytes, counted
    /// by the tap with `pdftext::walk`).
    fn none(&mut self, n: usize);
    fn literal(&mut self, bytes: &[u8]);
    /// A `Do` of a form or an included page: its codes come from its
    /// own stream, counted where the page is asked for (2.4).
    fn xobject(&mut self, name: &[u8]);
}
```

Where the writer calls it:

- `hlist_nodes`: the current character node (`Char` or `Lig`) noted
  before `char_out`, so that every code written for it, a virtual
  font's packet codes included, is that node's;
- `print_char_bytes` (every code of `char_out` and `output_one_char`
  passes there): `tap.code(...)`;
- `char_run` (the steady-string fast path that writes bytes itself):
  returns at once when a tap is set, so its characters go through
  `char_out` one by one. The bytes are the same (`char_run` is
  `char_out` repeated; a test checks the PDF is the same with the
  switch on and off);
- `insert_interword_space`, `insert_fake_space`: `tap.none(1)`;
- `literal`: `tap.literal(body)`;
- `out_form`, `out_image`: `tap.xobject(name)`.

The engine's tap (`crates/engines/tex/src/origins.rs`) keeps a
`PageGlyphs`: per code its font, character (or ligature originals),
`h`, `v`, `w`, and the non-glyph entries; about 24 bytes a glyph.

`PdfPage` (the page's final, parallel write, `pdfout.rs`) writes the
page with a tap when the switch is on and keeps the list with its
record (`Made`), by the page's identity in the link
(`Linked::shown`'s word: its key and its sealed lines' hash). The link
writing a page again (`replay_page`'s refusals) writes the same codes.

### 2.2 The matcher: a glyph to its byte

Input: the page's glyph list; the text nodes (`node_spans`). Output:
per glyph `(node, offset in the node's text, length, synthesized)`, or
none.

1. **Glyph text.** A code's character is its code point (TeX text
   fonts: ASCII letters, digits and most punctuation at their codes;
   T1's upper half is Latin-1); a ligature is its `original`
   characters (`fi` is `f i`, `--` is `- -`, ` `` ` is `` ` ` ``).
   Words split where the next glyph is on another baseline (`v`) or
   starts more than a fifth of an em after the last one ends (`h + w`).
   A word ending in `-` at a line end is joined to the next line's
   first word (a hyphenation; the `-` is synthesized, as the old
   core's break hyphen was).
2. **Source text.** A node's text scanned as TeX reads a document:
   control words and symbols (`\emph`, `\'`), comments (and the
   canonical text's comment marks), braces, `$`, `&`, `#`, `^`, `_`
   are not candidates; `~` is a space; every other character is a
   candidate with its byte range (UTF-8 decoded, `é` matching T1's
   `é`; a folded comparison lets `\'e`'s `e` match `é`).
3. **Which nodes.** The page's words looked up in an index of the text
   nodes' 4-word shingles (hashes, weighted by how rare they are; built
   when origins are first asked for, per text version, nodes an edit
   made indexed again): the page's nodes are the run of nodes in
   document order holding most of its weight; a tie between equal texts
   goes to the nodes of the page's sealed lines' pieces
   (`PagePiece::pieces`, kept already), then to the nodes after page
   k-1's. (para4000's text is a 50-word vocabulary: single words are
   not rare there, 4-word shingles are.)
4. **Alignment.** Patience diff of the page's words against the
   candidates' words (the heaviest cluster of voting nodes, widened by
   up to three short nodes on each side: a heading, an environment's
   line). Anchors are the words unique in both, in order, recursively,
   or where there are none, three words in a row unique in both. A
   range with no anchor is aligned by a dynamic program that keeps the
   page's text together in the sources: a skip of source words between
   two matches costs half a match, so of two copies of a sentence the
   one next to the rest wins. The page's start and end are not
   anchored (no common prefix or suffix taken, a skip there free): the
   page before may hold the first copy of a repeated sentence. Then,
   character by character (LCS), the gaps between matched words and
   the characters left between two matched ones of one node
   (`footnote*` against `footnote\footnote*`). Last, the page's
   unmatched runs of three words or more aligned again against every
   cluster: floats, footnotes, captions.
5. **What the unmatched get.** A glyph between two matched glyphs of
   the same node on its baseline is *synthesized*: its range is the
   source between them, narrowed to the first control sequence there
   (`\LaTeX`, `\ref{x}`, `\thesection`), as the old core gave a macro's
   glyphs the call's range. Any other (a running head, a page number,
   `\pdfliteral` text, a form's) has none.

Every step is a function of the glyph list and the node texts:
deterministic, and the same at every thread count.

### 2.3 Offsets in the Build's version of the file (F3)

A result is kept as `(node, offset in its text)`. When asked, it is
placed in the file the Build holds now: the node's start in its file
(`node_spans`: the document's text as last given to `edit`, a file's
as staged by `edit_file`/`stage_file`), plus the offset; for the
document's own text, which the nodes hold as its canonical text
(comments replaced by marks), the line and column taken over to the
raw text (`Build::raw`; a line's bytes before its `%` are the same in
both). Never the file on disk.

### 2.4 The order of `page_codes` (b)

The glyph list is in the order of the page's own content stream, the
order `pdftext` walks it. Its `xobject` entries are replaced by as many
"none" entries as that form or included page shows codes, counted by
`pdftext` on the linked PDF (the Build's output; walked only for pages
that have such an entry, each form counted once). A test checks every
page's length against `page_codes`.

## 3. After an edit (c, F2)

- A page whose `PdfPage` does not run again keeps its record and its
  glyph list (the record memo is keyed by the page's key and its lines'
  hash, unchanged).
- A page's match is kept while its glyph list is the same and every
  node it matched to is live with the same text (an edit keeps a node's
  id where its text is unchanged): only its offsets are placed again
  (2.3). Otherwise it is matched again, alone.
- `Build::origins_changed() -> (Vec<usize>, Vec<(u32, u32, i64)>)`
  (the frontends' choice): the pages whose answer changed, and the
  edits since the last ask as `(file, at, delta)` in `origin_files`
  indices: a byte at or past `at` of the file as it was is now `delta`
  further (one entry a file, from the common prefix and suffix of its
  bytes then and now; none where the length is the same). A page is
  in the list when its answer is not the last one given with those
  deltas applied (offsets, files, positions, widths compared): an
  inserted line moves the later pages' offsets but lists none of them;
  the viewer shifts its cached offsets itself. The deltas come from
  the Build (in `phitex watch` and the planned `phitex lsp` the viewer
  never sees the edit). The first ask lists every page.

## 4. The API

```rust
pub struct Options { /* ... */ pub origins: bool }   // off by default

pub struct PageOrigins {
    /// Per code, in page_codes' order (all the same length):
    pub file: Vec<u32>,    // index into Build::origin_files(); u32::MAX: none
    pub start: Vec<u32>,   // byte in that file (the Build's version)
    pub end: Vec<u32>,     // end byte (a ligature's two, a call's range)
    pub synth: Vec<u32>,   // bit set, one bit a code: synthesized
    pub x: Vec<f32>, pub y: Vec<f32>, // the glyph's origin, bp from the page's top left
    pub w: Vec<f32>, pub size: Vec<f32>, // its width and its font's size (bp)
}

impl Build {
    pub fn page_origins(&self, k: usize) -> Option<Arc<PageOrigins>>;
    pub fn all_page_origins(&self) -> Vec<Option<Arc<PageOrigins>>>;
    pub fn origin_files(&self) -> Vec<String>;   // append-only: indices stay
    pub fn origin_texts(&self) -> Vec<Option<Arc<[u8]>>>; // the bytes the offsets are in
    pub fn origins_changed(&self) -> (Vec<usize>, Vec<(u32, u32, i64)>);
    pub fn nearest(&self, k: usize, x: f32, y: f32) -> Option<(String, u32, u32, bool)>;
    pub fn glyphs_of(&self, file: &str, lo: u32, hi: u32) -> Vec<(usize, usize)>;
}
```

All take `&self` and work from any thread while the Build is idle:
the memos they use are the Build's own (`origins::State`, each behind
its own mutex, held only to look up or insert; the matching runs
outside them) and the writer's glyph lists (a mutex the page writers
hold only to insert, during an evaluation). Nothing they lock is held
by an evaluation for longer than an insert, and an evaluation needs
`&mut Build`, so none runs while they do. The answers depend only on
the glyph lists and the texts, so they are the same at every thread
count (the tests compare -j1 with -j8). `nearest` and `glyphs_of` are
the viewer's `nearest` and `from` (sync.ts) on the arrays, for the LSP
and SyncTeX sides.

A file's bytes: the Build holds a file as one character a byte
(Latin-1, `latin1`), so an offset counts characters; a text given with
characters past U+00FF counts its UTF-8 bytes.

Arrays, not records (F1): the wasm bindings hand them over as typed
arrays without copying per glyph. The watch's `origins` op (view.rs)
answers `{files, files_from, n, file, start, end, synth, x, y}` (the
frontends' choice), each array base64 of its little-endian words,
`files` the names from the index `files_from` the client sends on (the
table only grows). The watch hands the viewer every page's answer
after each build (`Built::origins`); the wasm core takes the arrays as
typed arrays.
`x`, `y` are the writer's position converted to PDF points from the
page's top left (sync.ts's coordinates), so the viewer needs no second
walk of the page.

## 5. SyncTeX

What it needs beyond the origins:

- **Lines**: a byte to its line by the line starts of the file's
  text as the Build holds it (`origin_texts`).
- **Positions and sizes**: the tap's `h`, `v`, `w` and font size
  (`PageOrigins::x`, `y`, `w`, `size`).
- **Records**: a sheet per page, `{k` ... `}k`; per run of glyphs on
  one baseline from one file an hbox record `(tag,line:h,v:W,H,D` ...
  `)`, inside it one `x` record per word at the word's position with
  its own line (a paragraph's lines map to the source lines their
  words came from, which pdfTeX's box-level records do not give);
  synthesized glyphs give their call's line, glyphs with none are left
  out. Tags: the preamble's `Input:` lines, every file the origins
  name, numbered in the order of `origin_files`.
- **Units**: pdfTeX's preamble (`Unit:1`, `Magnification:1000`,
  `X Offset:0`, `Y Offset:0`), positions in sp from the page's top
  left, as the writer's `cur_h`, `cur_v` are: the same numbers as
  `pdflatex -synctex=1`'s for the same boxes (a line's box at
  h 8799518, v 10300473 in both).
- **Records as built**: per page `{k`, one page box `[...]` around
  everything (the parser's search starts from it), then per run of
  glyphs on one baseline from one source line of one file an hbox
  `(tag,line:h,v:W,H,D` (H and D 0.7 and 0.2 of the font size) with an
  `x` record per word, `)`. A line of the page made from two source
  lines is two boxes.
- **Writing**: each page's sheet compressed as its own gzip member
  (the workspace's deflate), the file the preamble's member, the pages'
  and the postamble's concatenated; `synctex` reads it as one stream
  (checked). No `!` byte counts: the parser reads the file without
  them (checked). A page's member is kept while its answer is the same
  (the same `Arc`, which the Build keeps while it is) and so are the
  line starts of its files up to its last byte: after a word's edit of
  amsldoc 1 or 2 of 44 pages are compressed again.
- **Where**: `crates/cli/src/synctex.rs`, called where the job's output
  is written (`session.rs`, the `waiting_notes` note dropped); the
  switch on whenever `-synctex` or `synctex = true` asks, and in
  `watch` with its viewer (`--view`).

## 6. Costs

- **Off: zero.** No field, key or node changes; the page key, the
  records and the PDF are byte for byte as before. What the off path
  executes: the writer's `Option` test at each character node's
  `char_run` call and in `print_char_bytes` (the slow path), and at a
  literal, a space-font code or a `Do`. Checked by gate-fast: amsldoc
  and para4000 md5 and walls against the control.
- **On, measured** (amsldoc, 44 pages, -j8, accl, `fastdev` build,
  `synctex = true`): the PDF the same (98c96e00); the cold build's
  SyncTeX 42 ms (24 ms of it matching the 44 pages) of a 3.1 to 4.6 s
  build (the node's noise is larger than the difference); after a
  one-word edit (10 edits, gate-edits.py) the edit's wall 7.0 ms off,
  13.3 ms on (median): the origins 2.5 to 4 ms (the inputs 0.8, the
  sources 1.1, the pages 0.6 to 1.7: the changed page matched, the
  others kept), the SyncTeX file 3.2 to 5.2 ms in all with them, 1 or
  2 pages compressed; the rest the tap in the changed pages' writes.
  `PHITEX_ORIGINS_TIME=1` prints these; `PHITEX_ORIGINS_DEBUG=1` the
  matcher's clusters and unmatched words.

## 7. What it does not cover

- The DVI writer (a DVI build's viewer PDF is dvipdfmx's) and XDV
  native words: the tap is a trait of the writer; XDV's is on agent
  "xetex"'s list.
- Glyphs whose text is nowhere in the sources (a counter, `\today`, a
  running head made from marks, a form's text): none, or synthesized
  with the call's range (2.2.5).
- Repeated identical text: the alignment keeps a page's text together
  (§2.2.4); a page made only of a sentence repeated elsewhere, with
  nothing around it to place it, takes the first copy.
- `phitex --compat=pdflatex` on a document whose `.aux` pdflatex wrote
  grows past 10 GB in seconds, with or without origins, on origin/dev
  10fc78fd too (found by the SyncTeX check; reported, not this work's).

## 8. Files touched

- `crates/output/pdf/src/ship.rs` (the calls of 2.1), `lib.rs` (the
  `tap` field, outside the digest and clone), a new `tap.rs` (the
  trait);
- `crates/engines/tex/src/pdfout.rs`: `PdfPage` writes with a tap when
  the switch is on (`PdfPage`'s flag, `Ops::origins`, set from
  `Options::origins`: no field read, no key changed), the glyph list
  kept by the page's identity in the link;
- `crates/engines/tex/src/origins.rs` (new): the tap, the shingle
  index, the matcher, the placing of offsets, `PageOrigins`;
- `crates/engines/tex/src/api.rs`: `Options::origins`, the inputs
  (`origins_input`: the pages' identities, `spans_named`, which
  `node_spans` now maps), the Build's memos; `run.rs`: `Ops::origins`;
  `lib.rs`: the module; the Build's methods are in `origins.rs`;
- `crates/cli/src/synctex.rs` (new), `session.rs` (`Job::origins`,
  the SyncTeX file written with the outputs, the viewer's origins),
  `view.rs` (`origins`), `watch.rs` (the origins handed to the viewer
  after each build), `modern.rs`/`compat.rs`/`config.rs` (the switch;
  "no SyncTeX yet" removed), `main.rs`;
- tests: `crates/engines/tex/tests/pdftex.rs` (`glyph_origins_plain`,
  in the gate's suites shard), `latex.rs` (`latex_glyph_origins`, a
  latex shard), the checks shared in `tests/common/mod.rs`.

Not touched: `crates/core/typeset`, `crates/engines/tex/src/nodes.rs`,
the char nodes, `hpack`, the interpreter's char path, the page layer.

## 9. Tests and checks

- `glyph_origins_plain` (pdftex.rs: a plain pdfTeX document of five
  pages, a footnote, a logo, ligatures) and `latex_glyph_origins`
  (latex.rs: `pdfdocs/envs.tex`, four pages, its sentences repeated):
  every word of the text has each of its bytes shown by a glyph that
  came from it, every glyph placed on one byte shows that character,
  every page's count is `page_codes`'; -j8's answers are -j1's; after
  an edit the answers are a cold build's of the new text; plain: the
  PDF the same with origins off, a same-length word's edit gives no
  delta and some pages, a line inserted before gives one delta and no
  page; latex: the PDF still pdfTeX's.
- view.rs: the `origins` answer's arrays and files (`origins_answer`,
  `base64_as_rfc_4648`).
- SyncTeX on accl (a two-section article with a footnote, against
  `pdflatex -synctex=1` with the synctex tool 1.21): `synctex view`
  for lines 4 to 11 gives pdflatex's page and line positions (y equal,
  h equal; x the run's start where pdflatex gives its first `x`); for
  line 11 also the footnote's text at the page's foot, as pdflatex.
  `synctex edit` at six points gives pdflatex's line at five; at the
  sixth (the footnote's first word, `The`, which is on line 10) ours
  says 10, pdflatex 11 (its records are a paragraph's, ours a word's).
