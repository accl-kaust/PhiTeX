# The live page renderer: display lists, tiles, gates

Status: design, decided 2026-10-28; nothing built yet. Replaces MuPDF
on the live path (the editors' and the watch's viewer) in one switch,
once the whole corpus passes both gates (§5). There is no second path:
no page, figure or operator is ever drawn by MuPDF on the live path, and
nothing the renderer cannot draw is routed anywhere. An operator it does
not draw is a renderer bug, failed by the coverage gate (§5.2). MuPDF
stays only for whole-PDF jobs (compare, an uploaded PDF).

Why: a keystroke's update with MuPDF is 49-115 ms (one-page article,
two-column tikz paper; headless Chromium 153, 1.33-2.67 px/pt), almost
all of it a full software redraw of the page and the pixels pushed;
pdf.js is 215-281 ms (it reparses the PDF and reconverts its fonts every
build). MuPDF's wasm also traps on Electron 42 and 43 (VS Code's
preview). Target: build to pixels <= 10 ms for a one-word edit at
2.67 px/pt (§5.3).

## 1. One source: the content stream, as the writer writes it

The display list of a page is its content stream **as the PDF writer
writes it**, structured instead of serialized: the same operators with
the same numbers, the resources resolved to content hashes. It is made
by the same walk that writes the bytes (`crates/output/pdf/src/ship.rs`),
through a tap at every point the walk writes content (today's
`GlyphTap`, `crates/output/pdf/src/tap.rs`, extended), so the PDF bytes
path is unchanged and byte-identity is untouched.

Why the stream's own numbers and not TeX's positions (sp): the PDF's
pixels are those of its numbers (`print_bp`'s decimals, a `TJ`'s
thousandths of the font size, `Td`/`Tm`/`cm` relative moves). A list in
sp would differ from the PDF by the writer's roundings, and the exactness
gate (§5.1) would be noise. With the stream's numbers, a reader of the
PDF (§4) makes the same list by construction, and any difference is a
missing tap.

### 1.1 Items

```
Num      = (mantissa: i64, decimals: u8)        // a number as written: 72.123 = (72123, 3)
Op       = the content-stream operator with its operands, parsed:
           q Q cm w J j M d ri i gs            graphics state
           m l c v y h re S s f F f* B B* b b* n W W*   paths, clips
           CS cs SC SCN sc scn G g RG rg K k sh  colour, shadings
           BT ET Tc Tw Tz TL Tf Tr Ts Td TD Tm T* TJ Tj ' "   text
           Do  BI..EI (inline image)  BMC BDC EMC MP DP  d0 d1 (Type 3)
Group    = { ops: [Op], kind: Line | Box | Page }   // see 1.2
Page     = { media: [Num; 4], groups: [Group], resources: Resources, annots: [Annot] }
Resources= name -> Hash for /Font, /XObject, /ExtGState, /ColorSpace, /Pattern, /Shading, /Properties
```

A literal (`\pdfliteral`, `\special{pdf:...}`, a colour stack's
operator, `\pdfsetmatrix`) is tapped as the operators it consists of,
parsed (the writer already writes them verbatim; the tap parses the same
bytes). A literal that does not parse is written to the PDF as it is and
is the same unparsed bytes in the list: the renderer's interpreter
handles it exactly as a PDF reader would (operators up to the error,
the rest ignored), and the coverage gate names it.

Resources are content-addressed: a font by the hash of the embedded
font program, its encoding and its `/Widths` (what the writer embeds,
after subsetting; §3), a form or image by the hash of its object's
stream and dictionary (forms recursively: a Form XObject is itself a
list, §1.1 applied to its stream). The renderer keeps every resource
once per session, keyed by hash; a build sends only hashes it has not
seen.

### 1.2 Groups

The walk marks a group at each line: an `hlist_out` whose parent is a
`vlist_out` (a line of a paragraph, a display's row), and at each box
`\shipout` places directly; everything else on the page (rules between,
literals at page level) is in a `Page` group of its own run. Groups are
only cut points in one sequential op stream: the stream keeps its
relative moves (`Td`, `cm`), its `q`/`Q` and its text objects (`BT`
spans several lines in pdfTeX's output) across group boundaries, as
written.

### 1.3 Identity and damage (the renderer's, not the engine's)

The renderer interprets a page's whole op stream every build (a page is
thousands of operators; interpreting is microseconds per line, far below
rasterizing), resolving every group to absolute geometry: its entry
graphics state (CTM, colours, clip, text state) and its drawing in that
state. A group's **key** is the hash of its ops with its entry state
expressed relative to the group's own origin, and its **place** is that
origin on the page. Between two frames of a page:

* same key, same place: **kept** (no pixels);
* same key, other place: **moved** (its pixels translated; a line
  pushed down by an insert above it is a move, not a redraw);
* otherwise **changed** (its old and new bounds are damage).

Equal key means equal pixels relative to the place, so a move is exact
(for a translation by whole device pixels; a sub-pixel move is a
redraw). The engine invents no ids; nothing on the engine side tracks
identity. Pages the engine did not ship again (their content bytes the
same: exact mode's replay) are not sent at all.

## 2. Fonts and glyphs

Glyphs are drawn from **the font program the PDF embeds** (the subset
the writer made: Type 1 charstrings via `t1.rs`'s output, TrueType and
OpenType/CFF via `ttf.rs` and xdvipdfmx's), with its encoding and
`/Widths` as written: no second font lookup, no system fonts. Type 3
fonts (PK bitmaps, METAFONT) are lists (their `CharProcs`) drawn through
the interpreter. A glyph raster cache per (font hash, glyph, size in
device pixels, sub-pixel offset in quarter pixels), session-long.

## 3. The interpreter and the rasterizer

Rust, in the same wasm as the engine (`crates/output/render`, new):

* the interpreter of §1.1's ops, the full imaging model a TeX document
  carries: paths, every colour space (DeviceGray/RGB/CMYK, CalRGB, Lab,
  ICCBased, Indexed, Separation, DeviceN), shadings (types 1-7) and
  patterns (tiling and shading), transparency groups (isolated,
  knockout), soft masks (alpha and luminosity), blend modes, Type 3
  fonts, images with every filter (Flate, LZW, RunLength, ASCIIHex,
  ASCII85, DCT, JPX, JBIG2, CCITT) and their decode arrays, masks and
  `/SMask`s, Form XObjects, and included PDF pages (`pdftoepdf`'s
  forms: their own resources, their own fonts);
* a tile rasterizer (a tiny-skia-class crate; the user's approval of
  external crates was relayed by the frontends session 2026-10-28, to be
  confirmed directly before any is added) drawing only the tiles under damage (§1.3), at
  the viewer's scale; on a zoom, tiles redraw at the new scale while the
  old ones stay up stretched;
* opaque objects (a form, an image, a shading) rasterized once per
  (hash, scale, transform's non-translation part) and cached.

## 4. The reader (for the gate only)

`PDF -> [Page]`: reads a PDF the engine wrote back into §1.1's items
(content streams parsed, resources hashed as the writer hashes them),
using `pdftext::page_codes`' content-stream parser. It exists for the
exactness gate; the live path never reads a PDF.

## 5. Gates

### 5.1 Exactness

For every page of the corpus (the bars' documents, the reds, the
manuals, the XeTeX sweep) at -j1 and -j8: the list the walk tapped
equals the list the reader reads from the PDF, item for item (so the
pixels are identical), and the renderer's pixels of it equal its pixels
of the reader's list. A difference is a tap missing or wrong.

### 5.2 Coverage

Every operator, colour space, shading type, filter, blend mode and font
type the corpus's pages hold is drawn: the interpreter counts what it
did not handle, and any count above zero fails the gate, named (page,
operator). A coarse comparison with `mutool draw` of the same PDF
(per-page, tolerance on anti-aliasing only) catches the rasterizer's own
bugs; it is the only test with a tolerance.

### 5.3 Speed

In the frontends' headless Chromium harness, on their one-page article,
their two-column tikz paper and amsldoc: build to pixels on screen
<= 10 ms (median of 10 one-word edits, warm) at 2.67 px/pt; a cold
build's first page no slower than MuPDF's.

## 6. The host's side (phitex-frontends)

The renderer gives the host, per build: for each page changed, the
tiles redrawn (page, tile column and row, scale, RGBA pixels as a
transferable buffer), the tiles moved (from, to), and the page's text
layer and links from the list itself (text runs per line with their
boxes, from the fonts' `/ToUnicode` the writer made; links from the
page's annotations), updated per changed group. The frontends own the
compositor (`panel.ts`), the text layer, the wiring into Overleaf, VS
Code and the watch's viewer.

## 7. Order

1. The tap extended to every content write (ops, groups, resources) and
   the list types; the reader; the exactness gate (§5.1) on the corpus.
2. Fonts and glyphs (§2), then the interpreter's text and paths with
   the tile rasterizer, and the coverage gate running from the start
   (its counts are the remaining work).
3. Colour spaces, shadings, patterns, transparency, images and their
   filters, Type 3, forms and included pages, until coverage is zero.
4. The host's protocol (§6) and the frontends' compositor; the speed
   bar (§5.3).
5. The switch, once §5.1-§5.3 all pass on the whole corpus.
