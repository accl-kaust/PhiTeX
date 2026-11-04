# Plan: a page whose content did not change costs nearly nothing in the writers

Status: plan, 2026-11-04, not built. Builds on `docs/note-link-relocation.md`
(pdfTeX's link, parked 2026-10-21), and on the XeTeX writer's convergent
fold (`crates/engines/tex/src/xdvpdf.rs`, `Session::converged`, `Graft`).

## 1. The bar and where it stands

The user's bar: every edit under 100 ms at -j1 and -j8; pages whose layout
really changed are allowed about 0.15 ms each. The writers today:

| writer | what an edit costs | measured |
|---|---|---|
| XeTeX exact fold (`xdvpdf::fold`) | ~5 ms a page from the first page whose object numbers moved to the end | 16 pages: 77-90 ms of a 125-138 ms edit (chain3, accl) |
| XeTeX live link (`xdvpdf::live`) | ~12 ms fixed + ~3 ms a changed page | Grail \cite, 12 pages: 66-80 ms (chain2, accl 24023) |
| pdfTeX link (`pdfout`) | ~0.27 ms a record replayed, every record past the first renumbering | para4000 deleted paragraph: 583 records, 160 ms (matrix2) |

The exact file is what `--exact-pdf` writes and what a watch writes when
idle (`phitex_pdf::Layout::live`), so its cost reaches the user on every
edit that is followed by a pause, and on every edit of the bench.

## 2. The root cause, in both writers

Both writers number objects in page order (pdfTeX and xdvipdfmx do), so a
page that gains or loses one object (an annotation, a destination, a
resources dictionary, an image) renumbers every object after it. The
writers can only keep a later page when the whole writer state there
equals the base's (`Session::converged`, pdfTeX's `digest` resync every
8 records). With every later number shifted by `d`, the state is never
equal, so every page after the first change is interpreted, laid out and
compressed again, although its operations are the base's.

What a kept page's bytes hold that depends on the shift:
- `N 0 obj` and `N 0 R` the writer printed itself (page objects, the
  annotations' and resources' dictionaries, parents, `/Contents`);
- object streams (pdfTeX and xdvipdfmx both pack up to 200 objects):
  their index (`num off` pairs) and `/First`, and their deflated bytes;
- the cross-reference section (rebuilt anyway).

What it never holds: page content streams name fonts, forms and images by
resource names (`/F12`, `/Fm3`, `/Im2`), not numbers. They are kept as
deflated, so the dominant cost (interpreting and compressing the page)
goes away.

## 3. The design

### 3.1 Renumbering, a partial map

At an aligned page pair (new page `i`, base page `j`, same operations, the
same identity `ids[i] == base.ids[j]`), let `a` be the first object number
at which the two writers' object tables differ. Below `a` the objects are
equal. From `a` on, the objects that exist on both sides map by one
shift `d` (the objects the edit added or removed are on one side only). The
map is `ρ(n) = n` for `n < a`, `ρ(n) = n + d` for base objects past the
removed ones, undefined on objects only the base has.

### 3.2 State equal up to the map

`converged` becomes `converged_up_to(ρ)`: the new state equals `ρ(base)`.
The comparison is derived over every part of the state, as today's is;
each part that holds an object number is compared through `ρ`:
- xdvipdfmx: `obj` (the object table, `next_label`, the open object
  stream's members), `pdfdoc` (the page tree's entries, the current page's
  resources, annotations, beads, the names and dests trees, outlines),
  fonts (`resource` numbers given at first use), images and forms
  (`xobjects`), `pdfcolor`'s ICC and colour space objects, `epdf`'s
  imported objects;
- pdfTeX's `PdfState`: `objs`, `obj_ptr`, `head`, `index`, `aux`
  (marks, dests, outlines), `os`, `last_page`, the page's mark lists, the
  fonts' numbers, `dest_names` (note-link-relocation §2).
A part the map misses only makes the comparison fail: time lost, never
wrong bytes. This keeps the existing invariant: a graft happens only at
a state proven equal.

### 3.3 Relocation notes, not pattern matching

Each chunk the writer flushes carries, beside its bytes, the offsets at
which it printed an object number of its own and the number: every
`begin_obj`, every reference it formats (xdvipdfmx: `pdf_write_obj` of a
`PDF_INDIRECT`; pdfTeX: the ~35 print sites of note-link-relocation §3),
and for an object stream the offsets in its plain text (kept beside the
deflated bytes, an `Arc`). User text (literals, `\special` and
`\pdfliteral` bodies) is never scanned: a literal `12 0 R` the user typed
stays as typed, as pdfTeX writes it.

### 3.4 Relocating a kept page

For each chunk of a kept base page: copy the bytes, patch each noted
number through `ρ`; where a number gains or loses a digit, the bytes after
it move and the chunk's object offsets are recomputed. A content stream
has no notes and is copied whole (its deflated bytes). An object stream
whose members' text was patched is rebuilt: index and `/First` from the
patched text, deflated again at the document's level (pure zlib, the
same bytes as a fresh write), `/Length` written. The end (fonts, the
document's objects, xref) is written from `ρ(base end state)` plus the
graft's glyph additions, as `Graft` does today with byte offsets.

The cost of a kept page: copying and a handful of patches, plus a share
of re-deflating the object streams that held its objects (about 100 KB in
all on amsldoc, 1-3 ms, parallel). That meets about 0.15 ms a page on the
documents of the bench; the exact bound is measured per document (§5).

### 3.5 Where the two writers differ

- XeTeX: extend `Graft` with `ρ` (`a`, `d`, the removed and added
  numbers); `Session::converged` takes the candidate `ρ` from the first
  difference of the two object tables at the page where operations agree
  again. The base's page chunks are relocated instead of only shifted
  (`delta`).
- pdfTeX: the same in `Tail::link`'s resync (note-link-relocation §2-§4):
  the digest compared is `digest(ρ(old writer))`.

## 4. Order of work

1. Relocation notes on chunks (both writers), observed only: under
   `observe`, every link checks that patching the base's chunks with the
   identity map gives the same bytes (proves the notes are complete).
2. `converged_up_to(ρ)` with the state comparison through `ρ`; observe
   flag prints the first part that fails (as `PHITEX_LINK_WHY`).
3. Relocate kept pages and rebuild touched object streams; the end from
   `ρ(base)`.
4. The check (`PHITEX_LINK_CHECK`, `PHITEX_XDV_CONVERGE_CHECK`): every
   link also links from the start and asserts equal bytes; run with the
   edits, pages and latex suites and the corpus edit histories.

## 5. Bars and proof

- Bytes: every edited PDF identical to a cold build at -j1, -j8, -j32
  (amsldoc, Grail, para4000, book5, a XeTeX manual with hyperref); the
  checks of §4.4 silent on the suites.
- Time: the writer's share of an edit that adds or removes one
  annotation on page 1 (a `\ref` that stops resolving; a figure added):
  at most 2 ms plus 0.15 ms per page kept, at -j1 and -j8, measured
  against today's on the same node.
- Tests: a document whose first page gains an annotation keeps every
  later page relocated (count of pages interpreted = 1); a literal
  `N 0 R` in a `\special` is not shifted; an object stream split across
  the change point is rebuilt with the same bytes as a cold write.
