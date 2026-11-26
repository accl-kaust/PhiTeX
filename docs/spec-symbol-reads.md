# Symbols across pieces and per-line read-back (design, 2026-11-26)

Status: design for options 2 and 3 of the beamer frame-insert analysis
(an inserted frame in a 112-page XeLaTeX beamer deck: 7.2 s at -j1, the
PDF already byte-identical). Extends docs/spec-relative-counters.md and
docs/spec-readback-child.md. Hot path: the keystroke bar is tens of ms;
nothing below adds work to a run that meets no symbol.

## 1. Symbols across pieces (cause B)

A picture advances `\pgf@picture@serial@count` and names itself
`pgfid<the count>`: in a piece that is a symbol S = (count, piece, j)
(spec-relative-counters §2.1). Inserting a picture shifts every later
picture's number by one. Every later remember picture then ran again,
twice, though nothing it shows moved. Measured on the deck (edit's
round, `PHITEX_SYM_TRACE`): 430 forcings of the serial in picture
children, all in pgf's `\pgf@shape@interpictureshift`:

1. `\expandafter\ifx\csname pgf@sh@pi@#1\endcsname\pgfpictureid`: an
   `\ifx` of two macros, one body holding S (164, all another node's
   symbol: the picture began in the piece before the continuation whose
   child draws it).
2. `\pgfsys@getposition{\pgfpictureid}` =
   `\edef\pgf@marshal{\let\noexpand#2=\expandafter\noexpand\csname
   pgf@sys@pdf@mark@pos@pgfid<S>\endcsname}`: the name's symbol is
   another node's (189), or the piece's own and the derived name is
   then made concrete by `\noexpand` (77).

The rules (each exact: the answer is TeX's, computed without the digits
where the digits cannot change it, else forced as today):

- **B1. `\ifx` of macros holding symbols** compares token by token. Two
  equal tokens (the same symbol included: same digits) go on. At the
  first difference: neither a symbol, or a symbol against a token that
  is no character `-0123456789` of category 12 (the only tokens a
  symbol's digits are), or one list ended where the other holds a
  symbol (a symbol is at least one digit): false, no read. Two symbols
  of one register and piece with different steps: their digits differ,
  false. Anything else forces both lists, as today.
- **B2. A derived read of another node's symbol.** `\csname` of a name
  holding symbol S of node P (not the reader's own) is a derived read
  whose base is the register's value reaching P (`At::Before(P)`, what
  forcing S reads, `defer::resolve`), the named field read where the
  reader stands. Engine: the derived spec carries the base's node;
  `read::derived_field_at` reads the base there; the reader is filed
  on the base at `Before(P)` and on the named field at its own place,
  so a move of either re-asks it, and a move of the base that names a
  field holding the old answer leaves it standing.
- **B3. `\noexpand` of a derived name** keeps the name's symbol (it is
  not expanded, so its digits are not needed); stored by an `\edef` it
  stays one. `\let\x=<derived name>` gives `\x` the meaning read through
  the derived read (`defer::dname_meaning`). Elsewhere it is made
  concrete as today.
- **B4. Read-back keyed by symbol** (built 2026-11-26, below).

### B4: why not resolve the name where the file is read back

The first design defined `pos@pgfid<S resolved where the file is read
back>`: `\begin{document}`'s read-back (node R) resolving each marked
line's symbol S = (count, P, j) to the count reaching P now. P is a
picture's piece long after R, and the picture serial is a register the
page layer also steps (a picture in a headline or a footline steps it at
every shipout), so the count reaching P is known only once the pages
before P are made: R's read of it is held at the page frontier
(`Ctx::hold_at_frontier`) until the layer passes P. The layer cannot
pass P while R, at the document's start, has not committed: every piece
and every page after R reads what R defines. Tried on the text level
(the marked units resolved by `\begin{document}`'s run), node 90 waited
at the frontier on `count:316` before its P, and the pages waited on node
90: a livelock. It is no scheduling accident: the name such a read-back
defines depends on the layout after it, which depends on the read-back,
a cycle inside one round. The mergeless engine's rounds read only
backwards in the text (a later node's value reaches an earlier one only
across rounds, through the files), and nothing short of a serial
re-ordering of that round would break it. A definition of a derived
name made at R and resolved by the engine later (a definition whose
field moves with its base) meets the same cycle at any reader between R
and P that reads such a name with its digits: it would have to know the
count before P to know whether the definition is its field.

### B4 as built: keyed, read where the name is read

Nothing is resolved where the file is read. The written line's
definitions get a key each, a field named by the symbol, not its digits:

- A finisher's line to an open file (`\pgfsyspdfmark{pgfid<S>}{x}{y}`,
  written at shipout) and a piece's `\immediate\write` line keep each
  symbol with its digits where the line is written (the finisher forces
  S, as it did to print it). Each definition the line's read-back makes
  of a name holding those digits after the text before the mark
  (`pos@pgfid` + `17`) gets key `pos@pgfid<S>`, defined `[17,
  pos@pgfid17]`: the digits S had and the field the name was then
  (`defer::sym_keys`, `defer::sym_key`). The keys are read-back keys like
  the others (per field; a mixed fold's through `wmsym:`, defined by the
  read-back child).
- A name made with a symbol (`\csname pgf@sys@pdf@mark@pos@\pgfpictureid
  \endcsname`) is a keyed derived read (`Ctx::read_derived_keyed`): the
  base read before S's piece, the key and the named field where the
  reader stands, all through the derived path. Where the key says other
  digits than the base gives now, the field the key names is read for
  the name (`read::key_redirect`); else the name, as TeX reads it.
  Re-asked on a change of the base, the key or the field read, it stands
  where the answer is the same.

After a shift the base gives 18 for P's picture; its key still says
`[17, pos@pgfid17]`, so it reads `pos@pgfid17`, its own position from
the round before, and stands (unkeyed it read `pos@pgfid18`, the next
picture's, and ran again, and again in the round after). The round after
writes `[18, pos@pgfid18]` and `pos@pgfid18` its position: the key moved,
the answer did not, it stands again.

Exact at the fixed point: in the round that reads what it writes, every
symbol's line was written with the digits the base gives now, so every
keyed read is the name's, TeX's. A key that says other digits exists only
in a round whose files change, whose output is not the build's; its
readers are asked again when the key moves. A key matched to the wrong
definition (the text before the mark found elsewhere in the name) costs a
run, never a wrong answer at the fixed point. No read crosses forward:
the key is a field defined at the read-back point like any other.

The engine's part: `Step::Derived`'s spec may carry the key (a seventh
element); `read::derived_field_at`, `through_at` and the witnesses use the
field the key redirects to; the reader records its read of the key with
the derived path. A derived read across nodes that stood where its name
moved recorded the old name's field, and a change of the new name's
field leaves both recorded fields' change counts as they were: such a
read is now checked through its path, never by its fields' counts
(`RunRecord::read_holds`; before, it stood on 10 with the field at 30,
derived.rs).

Not keyed: a name met undefined (TeX makes it `\relax` where it is
looked up, a binding in the run): its symbol is forced, as before. A
picture that remembers nothing (no line of its own in the `.aux`) looks
its position up undefined in every round, binds the `\relax`, and reads
the picture serial: after a shift it runs again. On the deck that is
the most of what pictures still run again on (see the numbers).

Cost: B1 and B3 only where a symbol is met (none in a document without
relative counters). B2 one read more per cross-node name (it replaced a
forced read). B4 one key per marked definition, one read more per name made with a symbol.

## 2. Per-line read-back (cause C)

A written file whose lines all define is read back per field (the
child of spec-readback-child.md: a reader depends on the keys it uses).
One line that does more than define (babel's `\babel@aux`, which loads
a font as it selects the language) makes the whole file a text: the
`\begin{document}` and `\enddocument` runs read every line inline and
run again on any change of any line. The deck's `.aux` has 1535 lines,
1345 of them `\babel@aux`; each run spends ~350 ms in them.

Measured on the frame insert: between the rounds the lines that do more
than define (the skeleton) are the same sequence; only definition lines
moved (`\@writefile`, `\HyPL@Entry`, `\pgfsyspdfmark`).

The design: such a file is read back **mixed**. Its definition lines
are its key set and per-key definitions (`wkeys:`/`wdef:`, as a
per-field file), and the lines that do more are its skeleton
(`wskel:<file>`, their texts in order). At the file's first read-back
point the reader places the read-back child for the keys and reads the
skeleton inline. It depends on the skeleton and on the keys it uses.
Exact where the two commute, checked where the reader reads: no
skeleton line touches a key (the child's first-use check fires inside
the skeleton), and no definition line read what a skeleton line
defined (the union of the definition lines' reads against what the
skeleton defined). Where either fails the reader rolls the skeleton back
(as a written line's scratch interpretation does) and reads the text
inline, as today. Later read-backs (`\enddocument`'s, which writes the
`.toc`/`.nav` lines) read the text inline, as today.

Cost: each written line of such a file is interpreted where it is
written (before, the lines after the first that did more were not); a
skeleton line's interpretation stops at its first command that does more.

As built (2026-11-26):

- After a line of its run that did more, a run tells the later lines
  apart only where the file has a read-back point (`wpoint:`): a file no
  one reads back is never mixed (`\enddocument`'s 1345 `\babel@toc`
  lines to a `.toc` no one reads cost 1 s a round when told apart).
- A text any run found to do more is taken to do more by every run
  after, unread (`run::more_seen`; the same `\babel@aux` lines on every
  page and in every piece that switches the language, each interpreted
  with its font loading once per run before: 11 G instructions, 3.3 %,
  of the deck's clean build). Taking it so is safe, the skeleton then
  holding a line that might define. Its entry says only that it does
  more (no definitions made before it did), so a run's entries are the
  same whether it interpreted its lines or found them: a memo whose
  entries carried what the first run's interpretation made changed a
  finisher's output with no change of its page, and the deck's edits
  ran 2x the instructions. The skeleton is then the lines that do more
  and the definition lines' checks are all where the file is read
  (they were before for the deck: the closure added nothing). A line
  that defines is interpreted each time: its read-back depends on the
  state where it is written (a memo of those gave bibtopic's
  `\citation{*}` the read-back of its first writing, and wrong
  citations).
- `\inputlineno` read by a line's interpretation: into the log (which
  the interpretation drops) it reads nothing; into anything else the
  line does more (its number where the file is read back is that file's
  line, not known where it is written).
- A file with no defining line left outside its skeleton is not mixed.
- The skeleton and the defining lines' text are resolved
  (§2.7 marks) in the file's node, as the whole text is.
- Where the reader reads a key a skeleton line defines (`\@abspage@last`,
  `HyPL@Labels` at the beamer deck's `\begin{document}`), it runs again
  on that key, as it reads it: an inserted frame changes both.

Measured on the deck's frame insert (-j1, n11): `\begin{document}`
runs once in the edit's round before and after (it reads
`\@abspage@last`, which the insert changes); its rerun before was the
`.aux` text's, now the key's.

### What `\begin{document}` reads of the read-back keys

Traced on the deck (the run's first touch of each key, where the
read-back child's value is taken into the run, `Run::rb_check`):

- `\@abspage@last`: `\ifnum\@abspage@last<\maxdimen
  \xdef\PreviousTotalPages{\@abspage@last}\fi` (the LaTeX kernel's
  `\UseOneTimeHook` at `\begin{document}`). The value is used, not its
  presence: it becomes `\PreviousTotalPages`, which the run defines and
  later pieces read. An inserted frame makes it 113 for 112, so the run
  defines another `\PreviousTotalPages` and must run again; a finer read
  of the key (presence, or the `\ifnum`'s answer alone) leaves the copy,
  which needs the digits. What would spare that run is a copy that is no
  read: `\PreviousTotalPages` defined as the key's field where it is read
  back (an alias the engine follows, `read::alias_of`), with the
  `\ifnum` against `\maxdimen` a predicate read of the key. Neither
  exists for macro bodies taken by an `\xdef`; it is a change to how a
  run expands a macro into another's body, not to how it reads the key.
- `HyPL@Labels`: `\ifx\HyPL@Labels\@empty \Hy@WarningNoLine{…}`
  (hyperref). Only its equality with `\@empty` is used there. It is
  taken first after `\@abspage@last`, whose use already runs the node
  again on the insert, so a finer read of it saves nothing on this edit.

No change is made for either: on the frame insert the run's rerun is
TeX's own (`\PreviousTotalPages` moves).
