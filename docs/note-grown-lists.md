# Lists grown by appends: read only what is used of them

Status: built (deep2, 2026-11-06, branch deep2-app), on edit11's append
relation (`Rel::Append`, `Run::append_def`).

## 1. The shape

Several fields are lists that nodes grow one element at a time, each
element added by the first node that needs it, and that later nodes read
to ask about *their own* elements:

| field | grown by | read by | what a reader uses |
|---|---|---|---|
| `cs:MT@font@list` (microtype) | `\xdef\MT@font@list{\MT@font@list\MT@font,}` at a font's first use | `\MT@in@clist{font}\MT@font@list` at every font selection | whether its font is in the list |
| `pdf:elink:<f>` (pdfTeX `pdf_font_elink`) | a paragraph's first ask of font `f` for expansion ratio `e` | every paragraph set in `f` (its line breaking) | whether each ratio it asks was asked before |
| `fontinfo:<k>` of an expanded font `k` | the first asker binds `k` to itself | every paragraph set in the asking font | the binding of the ratios it asks |
| `pdf:rtfonts`, `pdf:rtdests`, `pdf:rtforms`, `pdf:rtpages` | a read-time command naming a font, destination, form or page first | every page's record (its object count) | the count of new names before the page |

Read whole, each list moves for every reader after the node whose first
use moved: delete the paragraph that used a font first and every later
font selection runs again; type a letter that makes a paragraph ask a
new expansion ratio and every later paragraph in that font runs again
(tikz-cd: 80 paragraphs, 0.66 s); add a section whose outline names a
destination first and every later page's finisher runs again (amsldoc:
43 finishers). TeX's output depends on none of that.

## 2. The mechanisms

1. **Append without reading** (edit11): the appender publishes
   `Rel::Append(items)`, recomputed in place by the engine on the value
   reaching it; it reads no base. A membership test answers from the
   run's own pending items, else through `Step::Has(item)` on the list's
   projection.
2. **Check the names, not which names** (`Step::Within`, defuse): an
   `\xdef` appending to a macro must know that every name already in it
   does not expand. The appender looks at the list (`Ctx::peek_path`, no
   read), checks each name's class, and records a read of
   `[Proj(append), Index(1), Within(names checked)]`: it stands while the
   list loses names or keeps them, and runs again only for a name it
   never checked. A name the run holds itself (a font identifier it
   looked up) is checked from its own binding (`Run::does_not_expand`).
3. **Pending appends are answered, not made whole**: a lookup that steps
   over the name (`get_next`'s `\outer` check), or asks whether it is
   defined or `\outer`, answers for an `\xdef`'s macro without reading the
   base; a global or base-level redefinition drops the pending appends
   unread. A document node whose run is recorded (`pkgrec`) appends too:
   its record lists the list as a write, its value not predicted.
4. **Look, then record what was used** (`Ctx::peek_field`,
   `Ctx::note_path`): the paragraph builder looks at a font's ratio list
   and at its expanded fonts' info without reading them, typesets, and
   records, for each ratio the paragraph asked
   (`View::asked`), only whether it was in the list and that font's info
   (`Run::note_elink_asks`). The record's seen value is the looked-at
   one, so a change between the look and the record re-asks the run at
   its commit.
5. **Names counted where they are named**: a read-time command naming a
   font, destination, form or page first contributes an entry to the
   pages' allocations (`pdf:alloc`, `rt_named`) at its own place; the
   fold of the allocations (`fold_allocs`) counts each name once, where
   it first comes. No page reads a list of them.

## 3. Exactness

Each reader depends on exactly what it uses of the list: its own
member's presence (1, 4), the absence of an unchecked name (2), its own
place among the allocations (5). The relation is recomputed from the
value reaching the appender, as `\xdef` computes it. A change between a
look and its record is caught by the commit's check of the record.

## 4. Measured

tikz-cd, one thread (mc 80902, 80917): typing a letter mid-document
834 ms -> 43 ms; a citation 1.69 s -> 1.16 s; a section at the start
2.39 s -> 1.51 s. All edits identical to a full rebuild.
