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
- **B4. Read-back keyed by symbol.** A line a finisher writes with S in
  it keeps S's mark (spec-relative-counters §2.7, there for
  `\immediate\write` only), and the definitions its read-back makes are
  keyed by the mark: the field they define is named with S resolved
  where the file is read back now, not where the line was written. After
  a shift, P's line defines `pos@pgfid<new number>` with P's own old
  position from the first round, so P's picture reads its own position
  and stands; the round after writes the same line. Exact at the fixed
  point: the file is written with the digits TeX writes, and a round
  that writes what it read is the last, as before.

Cost: B1 and B3 only where a symbol is met (none in a document without
relative counters). B2 one read more per cross-node name (it replaced a
forced read). B4 a resolution per marked key per read-back.

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
`.aux` text's, now the key's. B4 (read-back keyed by symbol) is not
built: the remember pictures still run again on the positions their
shifted numbers name until the second round; making the derived
definition (`pos@pgfid<S>`) read back by symbol needs the engine to
define a derived name's field at a place, which it has not.
