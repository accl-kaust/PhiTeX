# Relaxed PDF mode

Exact mode (the default, `Options::exact_objects = true`) is the
`pdf:state` fold: one writer state threaded through every PDF command
and shipout, byte-identical to pdfTeX. Relaxed mode drops one promise:
object numbers and resource names may differ from pdfTeX's. Each page's
content stream, every font program and subset, every image and form
stays identical. In return, every page is written independently of
every other.

## Shape

- **One writer node per page.** Each node is a pure function of the
  page's box, its whatsits' data, the parameters `\shipout` reads, and
  the objects it references (fonts, forms, images, raw objects), which
  it names by handle. A node starts from a fresh `PdfState` in relaxed
  mode, ships the page with the shared writer methods (the same
  `ship`, `whatsit`, `out_form` and font code as exact mode), and
  produces:
  - a chunk of bytes with symbolic references;
  - its local object count;
  - its used glyphs per font (a set);
  - its marks: annotation and link rectangles, dests, beads, the names
    tree entries.

  Nothing is read from another page's node. There is no `pdf:state`
  and no snapshot.
- **One node per read-time object.** `\pdfobj`, `\pdfxform`,
  `\pdfximage`, `\pdfoutline`, `\pdfannot reserveobjnum` and the
  `\pdfinfo`/`\pdfcatalog`/`\pdfnames`/`\pdftrailer` texts are each a
  node: a pure function of its own scanned input, giving a handle and a
  chunk. A form's box is shipped by its own node, as a form (same
  `ship(.., false, ..)`). An image is read and written by its own node.
- **`finish` at the root** reduces the per-node results with
  associative merges, then rewrites the chunks in one pass:
  1. **Object numbers:** a prefix sum of the per-node object counts, in
     a fixed order (pages in page order, then read-time objects in
     document order). That gives every handle its number.
  2. **Fonts:** the union of used glyphs per font, a commutative merge.
     Each font's objects (dictionary, descriptor, subset program,
     widths, encoding, ToUnicode) are then written once, with the
     shared font code, after the pages.
  3. **Outlines and the dests name tree:** sorted merges, keyed by
     document order and by name. Then the threads and the page tree.
  4. **Rewrite and xref:** one pass replaces handles by numbers and
     fills each stream's padded `/Length` field. The xref offsets are a
     prefix sum of the rewritten chunk lengths. In relaxed mode, object
     streams (`\pdfobjcompresslevel`) are packed here from the
     rewritten objects.

## Handles

A handle is an integer that TeX can hold (`\pdflastobj`,
`\pdflastxform`, `\pdflastximage`, `\pdflastannot`, `\pdflastlink`,
`\pdfpageref`, `\pdffontobjnum`). It can therefore reach the document's
text: hyperref writes `\the\pdflastobj\space 0 R` into literals and
annotations. So:

- A handle lives in a reserved range, [2^30, 2^31). pdfTeX's own
  object numbers stay far below it (its object table caps at 8M).
- In written bytes, a handle shows up only as an indirect reference
  `H 0 R` or an object header `H 0 obj`. `finish` rewrites exactly
  those two forms, whether the writer or the user's text produced them.
  In relaxed mode, content and object streams are compressed in
  `finish`, after the rewrite, so no handle is ever inside deflated
  data.
- **Page-owned objects** (the page object, its contents, its
  resources, and the links, annotations and beads shipped on page n)
  get dense handles: `2^30 + n·2^12 + i`. That allows 4096 objects per
  page and 2^18 pages; the writer fails loudly past either limit.
- **Read-time objects** (`\pdfobj`, `\pdfxform`, `\pdfximage`,
  `reserveobjnum`, outlines) are hashed into their own half:
  `2^30 + 2^29 + (hash(node id, occurrence) mod 2^29)`. The engine's
  node ids are content-hash ids, stable under unrelated edits, so the
  owner is the node id plus the command's occurrence within the node.
- **Collisions** (two owners, one handle): `finish` detects them. The
  build then falls back to exact mode for that document (the fold path,
  sequential), with a warning in the log that names both owners. Relaxed
  mode is an optimisation with a sound fallback, never a failure mode.
  A test forces a collision.
- Resource names are derived from handles, e.g. `/F<font id>`, and
  `/Fm<h>` and `/Im<h>` with the handle's low bits. They are stable per
  object and independent of order.

## What each command yields in relaxed mode

| command | relaxed result |
|---|---|
| `\pdfpageref n` | `Page(n)` handle, a pure function of n |
| `\pdflastobj` / `\pdfrefobj` | `Obj` handle of the `\pdfobj` node |
| `\pdflastxform` / `\pdfrefxform` | `Form` handle |
| `\pdflastximage` / `\pdfrefximage` | `Image` handle; size from the image node (an input of whoever reads it) |
| `\pdflastannot`, `\pdflastlink` | `Obj` handle, owner = the page for links and annotations shipped there, the command for `reserveobjnum` |
| `\pdffontobjnum`, `\pdffontname` | `Font(f)` handle; the name is `/F<f>` |
| `\pdflastxpos`, `\pdflastypos` | unchanged (positions do not depend on numbers) |
| `\pdfcolorstackinit` | unchanged: the stack's number is TeX-side |

Colour stacks persist across pages. Each page's writer node therefore
takes the stacks' state at its start as an input: a small value that
the page builder's fold carries, folded from the previous pages' stack
commands. Given that value, the page writer nodes are independent. This
is the one cross-page dependency, and it is TeX-level (literals), not
writer state: documents that use xcolor or hyperref pay exactly this
and nothing else.

## Fallbacks

A document the relaxed mode does not cover is built in exact mode
(`Build::pdf_exact_reason` says why). The triggers:

| trigger | why relaxed cannot do it | in LaTeX documents | status |
|---|---|---|---|
| a form with `\pdfcolorstack` | the stacks a form changes depend on the page that ships it | rare (xcolor inside `\pdfxform` boxes) | permanent |
| threads and beads (`\pdfthread`, `\pdfstartthread`) | a thread's beads chain across pages | rare | permanent |
| thread actions in links | the action names a thread object made by the pages | rare | permanent |
| `\pdfpageref` | its number is text: a handle typeset differs from pdfTeX's page object number | rare (hyperref does not use it) | permanent |
| a form without `\immediate` when `\pdfpageheight` is 0 | pdfTeX writes it under the height of the first page that references it (its origin); the page is not known when the form is made | rare (LaTeX sets `\pdfpageheight`) | permanent |
| `\pdfobj useobjnum` with a literal number | the number was not given by the mode, so it is not a handle | rare | permanent |
| fonts of included PDF files replaced (`\pdfinclusioncopyfonts` 0) | the replacements are the document's fonts, merged across pieces | uncommon | permanent for now |
| a transparency group of an included PDF page | the group is numbered by the page that first shows the image | uncommon | permanent for now |
| `\pdffontattr`, `\pdfincludechars`, `\pdfnobuiltintounicode`, `\pdffontobjnum`, `\pdffontname` | they act on the document's font objects, which the finish writes | common (cmap, hyperref drivers, font packages) | to be brought in: replayed into the finish from the fields TeX keeps, like the catalog texts |

Brought in: PNG transparency groups (the document's one group object
has its own handle; the finish writes it once when a piece refers to
it).

Not detected, documented: a handle printed as text (`\the\pdflastobj`
typeset) differs from pdfTeX's number.

## Options

`Options::exact_objects` defaults to true until the LaTeX exact gates
pass; then the default flips. The CLI exposes it as `--exact-pdf`.

## Gates

1. Every exact-mode gate is unchanged (exact stays the default).
2. Relaxed equivalence, on every PDF fixture and the 18 LaTeX oracles:
   - parse both files;
   - map objects by content, not number: a graph isomorphism from the
     page tree down;
   - map resource names by what they refer to;
   - with pdfTeX's file as the oracle, require equal content streams,
     font programs, images, forms and annotations.
3. Edits: an edit on page k re-runs page k's writer node only (engine
   counters), and the result equals the relaxed cold build byte for
   byte.
4. Identical output at 1, 4 and 16 threads.
