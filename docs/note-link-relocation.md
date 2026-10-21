# Note: the link past a renumbering (relocation)

Status: design, 2026-10-21; parked (about 12 ms of a 227 ms edit). The parts marked
"done" are in `crates/engines/tex/src/pdfout.rs`.

## Where the link stands

An edit's link copies the last link's records from the start up to the
first changed record. After that it replays records until the writer
matches the last link's writer at a snapshot (every 8 records), and from
there it copies the rest (spec-final §6.2). Done:

- The two record lines are aligned from their ends. Record `i` of the
  new line is record `i + (old length - new length)` of the old one, so
  an edit that adds or removes records can still resync.
- A writer that equals the old one except for the bytes of the object
  stream being filled is compared again once that stream is written.
  Only the step that writes the stream reads those bytes. This is the
  heading edit's second round: an outline title differs inside the open
  object stream.
- `PHITEX_LINK_CHECK` (observe): every link also links from the start
  and asserts that the bytes are equal. `PHITEX_LINK_WHY` (observe)
  prints, at each failed resync try, the writer fields that differ.

What is left is an edit that changes how many objects the writer
numbers before the resync point, for example a reference that no longer
resolves and so drops its link annotation. Every object after that point
gets another number. From then on the old and new records give the same
bytes up to that renumbering, but the writer never matches, so the link
replays to the end. On amsldoc's label edit this is 14.4 ms of a 26 ms
link and a 227 ms edit.

## What relocation needs

1. **The renumbering.** At aligned records `i` (new) and `j` (old), let
   `a` be the first object number at which the two writers' object
   tables differ. Objects below `a` are the same. Some objects from `a`
   on exist on one side only (here the dropped annotations). The rest
   map by a constant shift `d`. The map `ρ` is partial: it is undefined
   on the old-only objects, and the new-only objects have no image
   under it.

2. **The state.** The new writer must equal `ρ(old writer)`, with the
   old-only objects removed and the new-only ones added. A renumbering
   of `PdfState` has to cover every field that holds an object number.
   On the label edit these are `objs` (the entries and their type-list
   links), `obj_ptr`, `head`, `index`, `aux` (keys and the numbers in
   marks, dests and outlines), `os` (its stream number and object list),
   `last_page`, the page's mark lists, the fonts' object numbers and
   `dest_names`. The resync test is
   `digest(renumbered old) == digest(new)`. A field the renumbering
   misses only makes the test fail, which costs time and never gives
   wrong bytes. The end writer for the tail is the old end writer
   renumbered, with the same removals and additions.

3. **The bytes: relocation records, not pattern matching.** The writer
   notes, per chunk, every place where it printed an object number of
   its own:
   - `N 0 obj` (`begin_obj`);
   - `N 0 R` (about 35 print sites in marks, ship, font, image, finish
     and lib);
   - the object stream index (`num off` pairs).

   Each note records the offset in the chunk's plain bytes and the
   number. For an object stream the notes point into the stream's
   uncompressed text, which the chunk keeps beside the compressed bytes
   (an `Arc`; amsldoc has about 13 streams of 6 to 10 KB each).

   Nothing else is scanned. A user's text (object bodies, annotation and
   link attributes, literals) is copied as written, so a literal
   `12 0 R` the user typed is never shifted. Page content streams name
   no object numbers: `/F` takes the font number and `/Fm` and `/Im`
   take the form and image counters, so content streams are copied
   unchanged.

4. **Relocating a chunk.**
   - Patch each noted number. When the number of digits changes, the
     later bytes move: the chunk's direct-object offsets (`Chunk::objs`)
     are recomputed.
   - For an object stream: rebuild the index and `/First` from the
     patched text, recompress it at the document's level, and write the
     length into the padded `/Length` field (11 characters, so no bytes
     move outside the stream).
   - Recompression cost: each relocated stream is deflated again. That
     is about 100 KB in all for amsldoc, run in parallel on the
     executor, an estimated 1 to 3 ms. Direct chunks are a copy plus a
     few patches.
   - The cross-reference section is rebuilt from the relocated chunks.

5. **Check.** `PHITEX_LINK_CHECK` also covers relocation: under observe
   every link compares its bytes with a link from the start. The edits,
   pages and latex suites, amsldoc and the corpus edit histories are
   run with it on.

## Expected gain

On amsldoc's label edit the link drops from 26 ms to about 14 ms: the
records go from 14.4 ms to 2 or 3, and the tail (5 ms, a new end state)
stays. The rest of that edit (scan, resume, finish: about 200 ms) is
outside the link.
