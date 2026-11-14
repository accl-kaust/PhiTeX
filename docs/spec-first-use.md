# First-use reads that do not flip (design, not built)

Status: design (agent "resume", 2026-11-14), following
docs/spec-resume-moved-read.md §10. Agent "firstuse", 2026-11-14:
§2.1 built (§7); §2.2-§2.4 parked on the measurements of §4, with the
mechanism worked out in §5 and two corrections to §2.3 in §4.1. That section measured where a Grail
picture's runs at -j8 are refuted: early reads whose answer depends on
whether an *earlier* node has already used something.

Traced moves, from `PHITEX_CK_TRACE` and the watch log (Grail -j8, accl
28540, 28584; acclnode11 runs f19/f20):

| field | read at (of ~92k reads) | how often among picture moves |
|---|---|---|
| `cs:MT@font@list` | 4-21k | most often |
| `cs:pgf@@ar@<tip>@<params>` (pgf's arrow-tip cache) | 13-24k | about once a picture |
| `cs:TU/LinLibertine(0)/b/n/8` (a font control sequence) | 44k | once |
| `cs:pgf@sys@id@keylist@`, `cs:pgfpositionnodelaterpath` | 1.8-3.5k | about once |

## 1. The common shape: an idempotent block guarded by "done for K yet?"

Each case is TeX code of this form:

```
\ifx\csname X@K\endcsname\relax  (or: K is in \list)
  <block: global definitions that depend on K alone; add K to \list>
\fi
```

In TeX's true order, the first node that meets K runs the block, and
every later one skips it. The state after the guard is the same either
way: the block's global definitions are those the later node loads,
with the same values. What differs is only:

- **provenance:** who defines the values, and therefore who publishes
  them;
- **the block's side effects outside the state table:** log lines, a
  font loaded, and for pdfTeX the font's internal number.

So the reader's run splits at the guard, and every later read of the
block's products moves when the first user moves. That is the
refutation the traces show.

## 2. Per case

### 2.1 `pgf@sys@id@keylist@` (a dead read)

`\pgfsys@invalidate@currentid` (pgfsys.code.tex:727) is
`\expandafter\global\expandafter\let\csname pgf@sys@id@keylist@<ref>\endcsname\pgfutil@empty`.

- `\csname` of an undefined name makes it `\relax` (§372, a local
  `eq_define`). That is a read: is the name defined?
- `\global\let` then overwrites it at once.
- Whatever `\csname` found, the binding after `\global\let` is the same
  global one.
- The local `\relax` save entry is retained at the group's end, since
  §283 skips a level-one entry. It shows only in `\tracingrestores`.

**What the read becomes:** none. A `\csname` whose control sequence is
assigned globally by the very next command (`\let`, `\def`, `\gdef`,
`\xdef`, `\font`, `\chardef` and the like, with `\global` or
`\globaldefs>0`) records no read of it. The run defers the
`\csname`-made `\relax` and drops the deferral at the global assignment
(spec-deferred-reads.md's mechanism). Only when something else comes
next does it read and record as today.

**Exact because** the state after the assignment does not depend on
what was read. The one visible difference is `\tracingrestores>0`
("retaining" lines). The deferral keeps the read whenever
`\tracingrestores` or `\tracingassigns` is positive.

### 2.2 `MT@font@list` (microtype's set of fonts set up)

microtype's `\MT@set@all@codes` → `\MT@in@clist{\MT@font}\MT@font@list`
→ if absent: `\xdef\MT@font@list{\MT@font@list\MT@font,}` and the
font's setup (`\MT@setup@font`: global macros `\MT@<feature>@<font>`,
derived from the configuration and the font's name alone), plus
`Package microtype Info` log lines. The append is already a relation
(c76b9bcb0), and membership answers from it, but the answer still
decides the branch.

**What the read becomes:** a *first-use contribution*. A run that finds
K absent before itself (any node before it in the order may add it
later) runs the block as if first. It files the block's global
definitions and its log lines as a contribution keyed by
(`MT@font@list`, K), not as plain definitions. The engine folds these
contributions in text order:

- the first contribution for K is the definition;
- every later contributor's identical definitions are filed as reads of
  the first's (a cutoff by value);
- its log lines are dropped.

A contributor whose block made *different* definitions from the first's
is refuted and runs again on the real answer, today's path. The engine
compares them at commit, under the field's own lock.

**Exact because** the state each later reader sees is the first
contributor's (TeX's), and the log has the first's lines where TeX
wrote them. The equality check makes "the block depends on K alone" a
checked fact, not an assumption.

### 2.3 pgf's arrow-tip cache `pgf@@ar@<tip>@<params>`

pgf caches a computed tip: `\ifcsname pgf@@ar@...\endcsname` else
compute it and `\expandafter\xdef\csname pgf@@ar@...\endcsname{...}`.
This is the same shape as 2.2, keyed by the control sequence. The
mechanism is the same first-use contribution, for any `\ifcsname X`
followed by a `\gdef`/`\xdef` of X with nothing else global in the
block. The engine detects that shape generically (an existence read of
X, then a global definition of X, in one run with no global definition
in between), not by pgf's names.

### 2.4 The font control sequence and the font's identity

LaTeX's `\selectfont` → `\define@newfont` when `\csname <font>\endcsname`
is undefined: `\global\expandafter\font\csname <font>\endcsname=<name>
at <size>`, plus `\font@info`/`\@font@warning` log lines and the
shape's substitutions.

- **Where load order is visible in the output:**
  - pdfTeX writes a font's resource as `/F<n>`, where n is the internal
    font number, which is load order. pdfTeX also writes
    `\pdffontname` / `\fontname` / `\the\font`.
  - XeTeX's XDV numbers fonts by load order too. xdvipdfmx renames them
    in the order of first use in the XDV (page order), so Grail's PDF
    does not show load order. PhiTeX writes that PDF directly.
- **What the read becomes:**
  - The font control sequence's value must be the font's identity by
    its name and size (a value interned by (file, size, features)), not
    a load-order number.
  - The internal number pdfTeX would give is a *fold*: the font's rank
    among the distinct identities first loaded in text order. The
    engine already folds contributions in order, so this is a prefix
    count of first loads, computed where it is read: at a `/F<n>` name
    in the writer and at `\fontname`.
  - The guard (`\ifx\csname <font>\endcsname\relax`) then becomes 2.2's
    first-use contribution keyed by the identity.
- **To verify before building:** what a font control sequence's value
  holds today (`crate::fonts`). spec-picture-child.md §5 says "the font's
  number by load order", in which case the first step is the identity
  value. Another question is whether anything the TeX code can read
  (`\fontname`, `\the\font` in `\message`/`\write`) prints the number.
  Such a read reads the fold and depends on order, as it does in TeX.

**Exact because:**
- the identity is what the font *is*;
- the number is computed in TeX's order, never at the first-use read;
- the log lines are the first loader's, as in 2.2.

## 3. Order of building

1. The first-use contribution in the engine: a contribution kind with
   "first in order wins; later identical ones become reads of it". It
   covers 2.2 and 2.3, with generic detection in the interpreter.
   `MT@font@list` is the most frequent picture move in the traces.
2. 2.1, the dead `\csname` read: small, interpreter-only.
3. 2.4 needs the font-identity step first. It is the largest change,
   and touches the PDF writer's font numbering for pdfTeX.

Measure each as before: Grail -j8 wall, kept/wall, wasted seconds,
picture runs, md5 at -j1/-j8/-j16, and edit times.

## 4. What the traces of dev show (agent firstuse, 2026-11-14)

Dev 9150034b9, acclnode11, Shelf's files, a profile+observe release
build, Grail at -j8 with `PHITEX_RUN_LOG` (one build; the runs and the
field that woke each are from the run log, the chain from the
critical-path script):

- wall 15.5 s, busy 34.6 s, kept 7.4 s; picture children 25 runs of
  13 nodes, 70% of their time thrown away;
- reruns woken by a first-use field (font list, font control sequence,
  font codes, pgf's arrow cache):
  - before the citations are defined (the first pass, to about 11.5 s):
    about 1.3 s of runs; on the chain only picture 719 (484 ms, woken by
    `cs:pgf@arrow@id@count`) and picture 1326 (126 ms, `MT@font@list`);
  - after (the second pass): about 3.1 s of runs, on the chain pictures
    930 (921 ms, `MT@font@list`) and 983 (662 + 674 ms,
    `cs:TU/LinLibertine(0)/b/n/8`): about 2.3 s of the 15.5 s.

### 4.1 Two corrections

1. **pgf's arrow cache is no `\ifcsname X ... \xdef X`.**
   `\pgfarrows@getid` does `\global\let\pgf@arrow@id\csname<full name>\endcsname`,
   then `\ifx\pgf@arrow@id\relax \pgf@arrows@instantiate{#1}\fi`, so
   the guard reaches the test through a `\let`, with a global definition
   in between. `\pgf@arrows@instantiate` allocates the tip's id from the
   global counter `\pgf@arrow@id@count`, and every name it defines
   (`pgf@ar@saves@<id>`, `pgf@ar@hull@<id>`, `pgf@ar@ends@<id>`,
   `pgf@ar@visual@<id>`, `pgf@ar@inst@code@<id>`, `pgf@ar@cache@<id>`)
   carries that id; it also leaves `\pgfutil@tempcnta` set locally.
   Two pictures' blocks make equal definitions only when they
   instantiate their tips in the same order from the same count. And
   the read that moved in 719 is the block's read of the counter, not
   the guard: the block's own reads have to count only while the guard
   answers absent (§5).
2. **The second pass's flips go the other way (present to absent).** In
   the first pass an undefined `\cite` prints a bold `?` (LaTeX's
   `\@citex`), so node 250 (evaluation.tex's first `\cite`) is the first
   use of the bold Libertine and of its microtype setup. Once `grail.aux`
   is read back the citations are numbers, node 250 uses no bold, and the
   pictures after it become the first users: they skipped the block on
   their first run and must now run it. No first-use contribution removes
   that rerun; the run that skipped never ran the block. Going back to a
   checkpoint at the guard and cutting off once the state agrees again
   (§10 of spec-resume-moved-read.md, its stage 3) is the only way seen,
   and the checkpoints were measured there: no gain on Grail and 3-4x
   memory. Not pursued.

So §2.2-§2.4 together would take at most about 0.6 s off Grail's -j8
chain (719's and 1326's reruns). Most of the chain is placement:
the class (1.3 s), then the pictures 641, 719, 930, 983 and 1312 made
one after another as their figures' runs reach them, then three runs of
about 1.2 s each at the page layer's frontier (spec-picture-child §8.5).

## 5. The first-use read as worked out (parked)

**Engine.** One generic read-path step, `Step::FirstUse(spec)`, with
spec = [guard field X, own value v_X, the block's global definitions
[(Y, v_Y)], the block's reads [(R, path, seen)]]. Its answer where the
reader stands is true iff

    (X absent and every R still answers its seen) or (X = v_X and every Y = v_Y)

The run files one read through it for each field involved (X, every Y,
every R), seen true, in place of its plain reads of X and of the block's
reads. A walk or a stands check evaluates it where the reader stands,
reading those fields at the reader's anchor one segment lock at a time
after the commit's lock (a client path, as `Step::Derived` reads its
second field today). Nothing else is filed: a later contributor keeps
its block's definitions in its own segment under its own commit lock;
they are equal to the first's, so a reader after it sees the same value
whoever defined it, and a change of the first's makes some Y differ,
which refutes it as today. No global lock, no fold in document order.

**Log lines.** A block's log events are contributed as one guarded
event [X, anchor node, v_X, events] in the contributor's own place in
`log` (its segment; for a picture child, its output, which the parent
applies in place). The log is laid out once over the folded stream: for
each guarded event the layout node reads X at the guard's anchor
(`Ctx::read_at(X, At::Before(anchor))`) and prints the events where they
stand if X is absent there (they are the first contributor's, in
pdfTeX's place) or drops them if it is v_X. Those reads are its own, so
a first contributor that appears or goes asks it again.

**The block, in the interpreter, by no package's names.** The guard is a
read of X answered undefined (`\csname`, `\ifcsname`, `\ifdefined`,
`\ifx`, followed through one `\let`; microtype's membership through
`Run::clist_has`); the block is the branch of that conditional, to its
`\fi`. It is a first-use block only if all hold, else its reads stay as
today:

- the other branch is empty, and the `\fi` ends at the input level of
  the test;
- no list material, `\write`, `\special`, insert or mark, no font loaded
  (until §2.4's identity), no `\tracingassigns`/`\tracingrestores`;
- the bindings it leaves locally are dead: their locations are marked
  so that the run's next touch goes through `Run::load_slow`, where a
  read of one disqualifies the block;
- a read of the block touched again after it becomes a plain read
  there (the same marking);
- microtype's append becomes an append-if-absent relation: a second
  contributor's plain append would hold the font twice, unlike TeX.

## 6. Order now

1. §2.1, built (§7).
2. §2.2-§2.4 parked: about 0.6 s of Grail's -j8 chain at most (§4).
3. The pass-two flips (§4.1, 2): not pursued, see there.
4. Grail's chain: placing picture children early
   (spec-picture-child §8.5), the next target.

## 7. Built (agent firstuse, 2026-11-14)

**§2.1, the `\csname` bound at once** (`Run::csname_defer`,
`Run::csname_force`). `cs_name_end` puts its look-up off where the run
has not touched the name, holds no pending append on it, and neither
`\tracingassigns` nor `\tracingrestores` is positive. It is made, as
§372 makes it, at the first of: a touch of the location
(`Run::load_slow`, `Run::touch`, the name fetched as a command,
`set_cur_cs_rest`), a local binding of it, any command but an
assignment (`\global`, `\let`, `\def`, `\chardef`, `\font`, `\read`),
the end of the command it was put off before, or the run's exits
(`Run::run_all`). A global binding of the name drops it unread. One put
off inside a command (the main loop's look ahead past characters,
§1038) waits for the command that look ahead backed up. Test
`latex::a_csname_bound_globally_at_once_reads_nothing`.

**What Grail's chain held at -j8 instead** (found while measuring): three
resume runs of 1.1-1.3 s each, one after another, thrown away. Each
read a continuation's state that its placer replaced 9 ms later (acmart's
`\Sectionformat` let to `\ACM@NR@adjustedSectionformat` and not back),
so `\realSectionformat` called itself with an argument one token longer
a level, until "TeX capacity exceeded [main memory size=5000000]": 5
million tokens copied in about 10 000 commands and calls. A run's first
reads are filed nowhere a walk can re-ask, so only `Ctx::recheck` could
stop it, and the run's work ticks counted commands and calls, not
tokens. Now the tokens a macro call with arguments writes count too
(`Run::tok_work`, a tick every 8192), and the recheck stops such a run
once its read moved.

Measured on acclnode11 (Shelf's files, release, warm format, base dev
9150034b9 and new interleaved, median of 3): Grail -j1 12.24 / 12.47 s,
-j8 13.60 / 11.84 s, -j16 15.90 / 11.48 s; amsldoc -j1 1.78 / 1.76 s,
-j8 1.97 / 2.00 s (5 each), -j16 2.36 / 2.31 s. The PDF and log md5s
are the same base and new at each -j.
