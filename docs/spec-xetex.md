# XeTeX in the interpreter: design report (2026-10-21)

Status: proposal, not built. The user and the coordinator decide when it
lands relative to the page-chain redesign (`docs/spec-page-chain.md`).
Nothing here may change pdfTeX's output or speed: amsldoc
98c96e004c1c82db1e3d562cf0181a3e and para4000
4146793290dc0ea887d79a1d2d1b4c89 at every thread count, para4000's walls
unchanged.

## 0. Where we stand (measured, branch r7-xetex)

| Part | State | Evidence |
|---|---|---|
| `phitex-xdvipdfmx` (XDV → PDF) | 131/131 corpus XDVs byte-identical to TeX Live 2026's xdvipdfmx 20260113 | `scripts/accl/xetex-corpus`, job 15499 |
| `phitex-otf` font lookup | 2035/2035 traced requests resolve to XeTeX's file and face; all 370 native fonts of the XDVs matched | same job, `otf-xdv` |
| `phitex-otf` shaping | 32851/32852 native words identical (width, every glyph id, x, y) | same job |
| Shaping gap | Graphite (`/GR`) on a font with Graphite tables: shaped with OpenType here | one word, `graphite-awami` |
| Interpreter | 8-bit input (Latin-1 decode), no native fonts, no XDV writer | §2–§5 |

Oracle image: TeX Live 2026 (Arch), XeTeX 0.999998, xdvipdfmx 20260113,
HarfBuzz 14.5.0, FreeType 2.14.3, zlib 1.3.2.

## 1. Bars

1. pdfTeX unchanged (above), checked on every push that touches the
   interpreter.
2. XeTeX: our XDV byte-identical to `xetex -no-pdf`'s (oracle.sh
   `--no-pdf`) on plain and LaTeX documents, then our PDF to xelatex's.
   The PDF follows from the XDV: the converter is already exact (§0).
3. The XDV bar is per page record: a page's bytes equal the oracle's
   page (bop..eop) once the link has placed the font definitions.

## 2. Unicode character codes everywhere

What is already Unicode-ready:

- **Tokens.** `Tok` (`val.rs`) is `cat << 21 | char`: 21-bit
  characters. Nothing to change.
- **Line buffers.** `Line::buf` (`input.rs`) is `Vec<u32>`.
- **Category codes past 255.** `Run::cat` (`run.rs`) takes `u32`; codes
  0–255 come from the dense slots `CAT0` through `cat_cache`, others
  from the field `cat:<c>` (`fields.rs` `Fields::cat`).

What changes:

1. **Input decoding.** Files are decoded one byte per character
   (`latin1`, `api.rs` ×5 and `assemble.rs`). XeTeX decodes UTF-8 by
   default, with `\XeTeXinputencoding`, `\XeTeXdefaultencoding`, a BOM
   and the "bytes" encoding.
   - The decode moves to one function chosen by the engine. It is per
     file, never per character.
   - Positions stay byte offsets: edits, slices and the hint pass speak
     bytes. A line keeps its byte start; a character's column is only
     needed for error context.
   - `\endlinechar` and `\newlinechar` take any code up to `max_char`
     (`input.rs` `0..256` → `0..=max_char`).
2. **The source pass** (`crates/core/source`, `Regime::catcode(b: u8)`)
   reads bytes and calls every byte above 127 category 12.
   - Under UTF-8 it decodes a sequence to its character and looks the
     character up: the dense 0–255 table, then a sparse map for the
     rest.
   - This matters because a Unicode format makes non-ASCII characters
     active (category 13) or letters, and the pass must see the same
     categories as the run.
3. **Code tables.** `lc`, `uc`, `sf`, `mc`, `dc` have dense slots for
   0–255 only (`CODE0`). They get `lc:<c>`-style fields past 255, as
   `cat:` already has.
   - The records `cattab`, `sftab`, `lctab` (read whole at a piece's
     entry) become a dense 0–255 part plus pages of 256 codes, kept
     copy-on-write and keyed by page.
   - A Unicode format sets these codes for tens of thousands of
     characters. One page record per 256 codes keeps a piece's entry
     read at a handful of pages, not 0x110000 cells.
   - `records_of_prefix` (`fields.rs`) tests `c < 256`; that becomes
     `c <= max_char`.
4. **Scanners.**
   - `scan_char_num` (`scan.rs`, 0..=255) becomes `0..=max_char`, as
     XeTeX's `scan_usv_num`.
   - The alphabetic constant (`scan.rs`, "Improper alphabetic
     constant" above 255) takes any character.
   - `^^^^xxxx` and `^^^^^^xxxxxx` join `^^` in `getnext.rs`.
   - `max_char` is a run constant: 255 for pdfTeX, 0x10FFFF for XeTeX.
     It costs one compare, the same as today's constant.
5. **Printing and writing.**
   - The log, the terminal and `\write` streams print characters past
     255 as UTF-8, as XeTeX does. Control characters still print in
     `^^` form.
   - `tools.rs` writes Latin-1 today and gains the UTF-8 form.
6. **Fast paths.**
   - `fast_char` (`getnext.rs`) stays dense on 0–255; above that it
     falls back to the general path, as today.
   - For XeTeX text, `cat_cache` can grow by 256-code blocks (one
     block per script in use) with the same generation check.
   - pdfTeX never decodes a character past 255, so none of its branches
     change.

## 3. The `\XeTeX…` and `\U…` primitives

Grouped by where they land:

- **Font queries.** `\XeTeXfonttype`, `\XeTeXcountglyphs`,
  `\XeTeXglyph`, `\XeTeXglyphindex`, `\XeTeXcharglyph`,
  `\XeTeXglyphname`, `\XeTeXglyphbounds`, the OpenType and AAT
  script, language, feature and selector queries, `\XeTeXfirstfontchar`
  and `\XeTeXlastfontchar`.
  - All exist as functions of `phitex_otf::xetex::XeTeXFont`. `otf-check`
    already compares them with XeTeX.
  - The primitives are thin: `prims.rs` entries reading a font's field.
- **Font loading.** `\font` with a native name goes through
  `find_native_font` over the persisted font index. It is a read-time
  definition like a TFM load, with the font's fields defined once.
- **Text and modes.** These are parameters, read where XeTeX reads
  them:
  - `\XeTeXinputencoding`, `\XeTeXdefaultencoding` (§2.1);
  - `\XeTeXinputnormalization`, which needs NFC/NFD tables;
  - `\XeTeXinterchartokenstate`, `\XeTeXinterchartoks`,
    `\XeTeXcharclass` (§4.2);
  - `\XeTeXlinebreaklocale`, `\XeTeXlinebreakskip`,
    `\XeTeXlinebreakpenalty` (§4.3);
  - `\XeTeXdashbreakstate`, `\XeTeXuseglyphmetrics`,
    `\XeTeXinterwordspaceshaping`, `\XeTeXupwardsmode`,
    `\XeTeXgenerateactualtext`, `\XeTeXhyphenatablelength`,
    `\XeTeXtracingfonts`.
- **Pictures.** `\XeTeXpicfile`, `\XeTeXpdffile`, `\XeTeXpdfpagecount`.
  - A picture's size is read at the command, so it is a read-time
    effect.
  - The image readers exist in the converter (`pdfximage.rs`,
    `pngimage.rs`, `jpegimage.rs`, `epdf.rs`). The interpreter needs
    only the bounding box. Expose that as a function of the converter
    crate, not a second parser.
- **Unicode math.**
  - The codes: `\Umathcode`, `\Umathcodenum`, `\Umathchar`,
    `\Umathchardef`, `\Umathcharnum`, `\Umathcharnumdef`, `\Udelcode`,
    `\Udelcodenum`, `\Udelimiter`, `\Uradical`, `\Umathaccent`.
  - These are 21-bit fields beside today's `mc`/`dc`.
  - Math layout uses the font's MATH table (`phitex_otf::math`) where
    XeTeX does. That goes in `math.rs` and the typeset crate's math.
- **Characters.** `\Uchar` and `\Ucharcat` make a character token of
  any code (and any category for `\Ucharcat`). This is expansion
  (`expand.rs`).

## 4. Native words in the typesetter

### 4.1 The node

- **The node:** `phitex_typeset::node::Node::NativeWord(Box<NativeWord>)`.
  - Boxed, so `Node`'s size stays the same and pdfTeX's lists do not
    grow.
  - It holds the font, the text (UTF-16, as XeTeX keeps it), the width,
    height and depth, and the laid-out glyphs (id, x, y) as a shared
    `Arc<[…]>`.
- **Making one.** XeTeX's main loop collects a run of characters in a
  native font into one node. Here:
  - The switch is made when the current font changes: a native font
    selects the collecting path. The per-character loop of a TFM font is
    untouched.
  - Measuring is `XeTeXFont::layout_word`: pure in (font, text, letter
    space, last script), memoized by the shaper. Measured at 1.7 µs per
    word.
- **Last script.** XeTeX keeps a per-font `last_script` across words:
  the default direction of a word with no strong script. It is state
  that passes in document order.
  - Make it a field per font (`font:<id>:lastscript`), written only when
    a piece's last native word in that font changes it.
  - Guess it from the previous build. Nearly every document keeps one
    value, so this is a rare chain.
  - `otf-xdv` counts words that match only from a fresh state: 0 of
    32852 in the corpus.

### 4.2 Interchar tokens

`\XeTeXinterchartoks` inserts token lists between character classes in
the main loop. It changes the token stream and belongs in the run's
character path. The path checks it only when
`\XeTeXinterchartokenstate > 0`; with the state 0 (pdfTeX, and most
documents) nothing is inserted.

### 4.3 Line breaking

- **Breaking.** `linebreak.rs` treats a native word as a box with its
  width: it has no break points of its own except those §4.3's locale
  and the hyphenation pass give it.
- **Hyphenation** (`hyph.rs`, `hyphenate_all`).
  - XeTeX hyphenates the text of a native word. The pass reads the
    word's characters and `lc` codes from the node's text, not from char
    nodes.
  - A chosen break splits the node, and each part is laid out again (as
    XeTeX re-measures the pieces).
  - That needs the shaper inside line breaking. It is pure and
    memoized, so it costs no state.
- **`\XeTeXlinebreaklocale`.**
  - Break opportunities inside a run come from ICU's line breaker, with
    `\XeTeXlinebreakskip` and `\XeTeXlinebreakpenalty` inserted.
  - This needs UAX #14 exactly as ICU 78 tailors it. That is the
    largest parity risk of the plan: port the rules and test them
    against xetex on CJK text.
- **Glyph metrics.** `\XeTeXuseglyphmetrics` height and depth come from
  the glyphs (`layout_height_depth`), already ported.

### 4.4 What pages.rs sees

A native word is a node with width, height and depth. Vertical profiles,
the page builder and the finishers need no change.

## 5. The XDV writer

- **Where it goes.** `crates/output/dvi` (`DviWriter`) is §583–§642.
  Its module doc already names XeTeX's XDV variant; the variant is not
  written.
- **What it adds:**
  - the XDV preamble id;
  - the native font definition (op 252: path, face index, size, color,
    extend, slant, embolden; as `otf::xetex` `makefontdef` builds it);
  - glyph runs: op 253, or op 254 with the text under
    `\XeTeXgenerateactualtext`;
  - pictures, as XeTeX ships them.
- **Order.** The walk (`hlist_out`, `vlist_out`) is the same; a native
  word is one more node kind.
- **DVI mode is the template.** `assemble.rs`'s `dvi_file` runs the end
  node over the pages accumulator. It resumes a `DviWriter` from a
  memoized snapshot at the longest unchanged prefix. The XDV writer is
  that writer with the new ops.

## 6. Fitting the writer interface (spec-page-chain §6)

§6 asks for:

- a self-contained page record from each finish node;
- a link that numbers objects and offsets by prefix sums in fire order
  and renders only changed records;
- writers that name no output format.

**The page record (XDV):**

- the page's bytes, bop..eop;
- the native fonts it uses, by font id;
- its glyph usage, for the viewer.

**Where XDV breaks self-containment, and the fix:**

- **Font definitions.** A font definition is written at a font's first
  use in the file (`font_used`). That depends on earlier pages.
- The record therefore carries its used fonts and no definitions. The
  link inserts a definition before the first page that uses a font,
  just as pdfTeX's link unions font usage.

**Why the PDF link is a fold, not a prefix sum:**

- xdvipdfmx numbers a font's objects at its first use.
- It writes resources and annotations into object streams of 200
  objects as the streams fill, during any later page.
- It writes the page dictionaries and the page tree at the end.

So object numbers depend on the earlier pages' state, not only on their
counts.

**What the link does for XDV:**

1. Run `Session::page` over the records in fire order.
2. Keep a snapshot of the session before each page. `Session::snapshot`
   exists, and `xdv2pdf --check-resume` already proves that resuming
   from any page's snapshot gives the cold bytes.
3. On an edit, resume from the first changed record's snapshot. Stop
   re-running once the session state after a page equals the old state
   there, using a digest of the counters, font usage and object-stream
   fill; splice the old bytes after that.
4. Then run `finish`: font subsets, xref, trailer.

This is the DVI mode's memoized fold (§5), one layer down. It needs no
change to §4: finish nodes give records and only the link is
writer-specific. The interface should say "the link renders changed
records" and leave "by prefix sums" to the pdfTeX writer.

Costs, amsldoc set with fontspec (42 pages), instructions:

| Step | Instructions | Note |
|---|---|---|
| `Session::new` | 688 M | 608 M of it parses the font map files. Snapshot after `new`, keyed by the map files' bytes and the options, and reuse it across builds. |
| One page | about 10 M | |
| `finish` | 183 M | `pdf_close_fonts` 144 M, deflate 140 M of the whole run |

An edit therefore costs one or a few pages (a few ms) plus `finish`.

`finish` needs a cache of each font's subset and its compressed bytes,
keyed by the font and its glyph set:

- usually the glyph set is unchanged, so the subset is not rebuilt;
- without the cache, `finish` (≈40 ms) misses the 16 ms edit bar.

## 7. Performance and parallelism of the XeTeX libraries (measured)

Measured on amsldoc set with fontspec: 42 pages, 843 KB XDV.

**`xdv2pdf`:**

- 0.21 s against xdvipdfmx's 0.25 s.
- 1.34 G instructions against C's 1.06 G (it was 1.53 G before this
  branch's sbuf0 fix).
- Before this branch the tool took 10.7 s: one `kpsewhich` process per
  file lookup. It now uses `phitex-kpse` in process, as the CLI's viewer
  does.

**Where the converter's time goes, and what to do:**

- **Font map parsing**, 45% of a cold conversion.
  - Done once per session in C; in a watch it is repeated every build.
  - Cache the session after `Session::new`.
  - The parse itself allocates per line: `read_c_litstrc`,
    `parse_string_value`, `mfgets`, `calloc`.
  - `phitex_kpse::Kpse::cnf_get` costs 82 M for its lookups. Cache
    variable expansions per format.
- **Pages**, 33%.
  - `do_glyphs` and `pdf_dev_set_string` (97 M after the fix).
  - `record_glyph_run` (120 M) feeds only the viewer. Make it optional
    in an exact build.
- **Fonts and compression**, 14%.
  - Each font's subset and its streams' deflate are independent of the
    others. Run them in parallel in `finish`, using the codecs crate's
    pure deflate, which must stay zlib-identical.
- **Pages in parallel.** Splitting `page` into two steps would make page
  conversion parallel:
  - a pure step that renders the content stream with symbolic font and
    resource handles and records glyph usage;
  - a cheap sequential step that numbers objects and fills object
    streams.
  - That restructures a 60k-line port. It is worth it only if the fold
    of §6 shows on the critical path; it should not at about 3 ms a page
    beside typesetting.

**`phitex-otf`:**

- **Shaping** is 1.7 µs a word with the memo on (2.1–2.7 µs off), over
  33567 layouts in the corpus.
  - Inside one shaping call, harfrust's `ShaperFont::new` and
    `LookupInfo::new` are rebuilt every call (≈15% of shaping). Keep
    them per face in the `Shaper`.
  - `Buffer::enlarge` (≈17%) can reuse one buffer.
  - Shaping is pure per word, so it runs wherever the paragraph runs,
    in parallel across pieces.
- **Font loading:** 2035 loads in 1.3 s with in-process kpathsea.
  - The first name lookup computes fontconfig-style names for every face
    of TeX Live (`fc_names`, 133 M instructions per process).
  - Store those names in the persisted font index (`otf-index`) instead.

## 8. Where it lands, what it shares with the redesign, pdfTeX unchanged

| Area | Files | Shared with the redesign? |
|---|---|---|
| Input decoding, `max_char`, `^^^^` | `input.rs`, `api.rs`, `assemble.rs` (decode), `getnext.rs`, `scan.rs`, `expand.rs` | no |
| Unicode code tables and records | `fields.rs`, `runstate.rs`, `val.rs`, `crates/core/source` | records are read by pieces (§4.1 of the redesign): same mechanism, more pages |
| Primitives | `prims.rs`, `etex.rs`, `math.rs` | no |
| Native fonts | `fonts.rs`, `crates/fonts/otf` | no |
| Native words, interchar tokens | `run.rs` main loop (character path), `crates/core/typeset` (`node.rs`, `linebreak.rs`, `hyph.rs`, `pack.rs`) | `run.rs` (pieces): only the character path, not the piece boundaries |
| XDV writer | `crates/output/dvi`, `assemble.rs` | `pages.rs` `ship_out` chooses the writer: one call site |
| PDF from XDV | `crates/output/xdvipdfmx`, a link beside `assemble.rs`'s | the §6 interface |
| Engine | `crates/engines/xetex` (today a stub) | no |

Order that avoids the redesign's files until it lands:

1. **Libraries** (§7): otf caches, the font map session cache,
   `finish` in parallel. Gate: `scripts/accl/xetex-corpus`.
2. **Unicode characters with TFM fonts only:** `xetex` on plain
   documents with cm fonts. Gate: XDV bytes equal to `xetex -no-pdf`.
   This touches no redesign file except the input path.
3. **Native fonts, native words, the XDV writer.** These touch
   `run.rs`'s character path and `ship_out`'s writer choice. Land them
   after the redesign's P2 (or with it, coordinated).
4. **The Unicode LaTeX format, the link of §6, edits.**
5. **Locale line breaking, Graphite.** Last, each with its own parity
   test.

**Keeping pdfTeX's output and speed:**

- The engine (pdfTeX or XeTeX) is fixed per build, with the format.
  XeTeX's branches sit at boundaries:
  - file decoding, once per file;
  - font selection: a native font switches the character path;
  - `max_char` compares that already exist;
  - the writer choice at `ship_out`;
  - the interchar check, behind its state parameter.
- No per-character test is added to the TFM path. `Node` keeps its size.
- Every push that touches the interpreter reports amsldoc j1 (and j8)
  md5 and para4000's walls against dev on one node.

## 9. Risks and open questions

| # | Question |
|---|---|
| R1 | ICU 78's line breaking for `\XeTeXlinebreaklocale`: a port of UAX #14 with ICU's tailoring, or a smaller exact subset. Needs a CJK parity corpus (the image has no Noto CJK; TeX Live's CJK fonts only). |
| R2 | Graphite: a graphite2 port, for fonts that ask for it. One word of the corpus today. |
| R3 | `\XeTeXinputnormalization`: Unicode normalization tables matching ICU 78. |
| R4 | The per-font `last_script` field (§4.1): confirm on real documents that it is constant. |
| R5 | The §6 state digest: which session fields decide equality. `--check-resume` gives the test. |
