# PhiTeX — design

The design is `AGENTS_BRIEF.md` (the parallel pure-SSA design: values,
names, nodes, evaluation, the cross-run fixpoint) and the engine's own
record, `crates/phitex-flow/CORE.md` (every primitive on the graph, the
formats, LaTeX, pdfTeX's PDF mode, the conflicts and their resolutions).
This file keeps what is not there: the crates, and the φ core's design
(chapter 7, cited as `DESIGN 7.N`).

The machine-mode engine (partex-core, partex-engine, partex-incr,
partex-ssa, partex-trace, phitex-ir and the old CLI) and its design
(chapters 1–6, appendix A: the dynamic SSA over an imperative engine,
windows, checkpoints, the machine runtime) were removed when the CLI
moved onto phitex-flow; they are in the history up to commit cc8ffa53349c.
Comments in the remaining crates that cite `DESIGN 3.x`–`6.x` refer to
that version.

## The crates

- `phitex-flow`: TeX as a suspending dataflow graph over content-versioned
  values (AGENTS_BRIEF.md, CORE.md). The only engine.
- `phitex-cli`: the `phitex` binary on phitex-flow: `build` (the format
  made on the graph and cached, the document, the cross-run fixpoint with
  BibTeX and makeindex), `watch` (the same Build kept, edits as updates,
  the live viewer), the engines' command lines by program name
  (`pdflatex`, `latex`, `pdftex`, `etex`, `tex`), `outline`, `sync`,
  `clean`.
- `phi`: the φ core (chapter 7).
- `phitex-syntax`, `phitex-doc`, `phitex-diff`, `phitex-git`: the CST, the
  document's structure without TeX, latexdiff natively, git.
- `phitex-draw`: the viewer's pages from the PDF's bytes.
- `phitex-codecs`: inflate, MD5, PNG, a PDF reader and its page text.
- `phitex-kpse`, `phitex-bibtex`, `phitex-makeindex`, `phitex-otf`,
  `phitex-xdvipdfmx`: kpathsea, BibTeX, makeindex, OpenType shaping,
  xdvipdfmx, ported as pure libraries.

---

## 7. The φ core (2026-10-08, design for review)

`crates/phi` (package `phi`): a dataflow core that knows nothing of TeX.
The engine will be ported onto it (branch `pure-ssa` designs the TeX
side: ops, value kinds, node counts). It exists because chapter 3, as
built, records reads and writes around opaque chunks of an imperative
engine, and every attempt to make it finer drifted back there, because
the engine could reach around the model. In φ, **an op receives its
operand values and returns its results; it has no `&mut` to anything**,
so an impure op cannot be written against the API (7.9 says exactly
what the type system does and does not exclude).

The core is chapter 3's invariant made structural:

> A node runs again if and only if a version it read changed; the change
> stops at a node whose read results come out equal. The graph is built
> by evaluating it, and an edit rebuilds only what it reaches.

It depends on no crate of the workspace (a test fails if it does,
7.12), builds for `wasm32-unknown-unknown` with one thread, and has no
`unsafe`.

### 7.1 Overview

```text
 Seq (persistent, Poly-versioned) ──► input nodes ─┐
                                                   ▼
 Graph<L>: one arena of nodes, struct of arrays    scan / unfold nodes
   leaf  = op(operands) → value                    (steps emit children,
   step  = op(state, operands, names read) →        read and define names)
           (children, defs, next state)                  │
   effect / barrier / publish / cross                    ▼
 the name store (reaching definitions, scopes)    chains (ordered effects)
 the worklist (push, by position) ◄── edits        cross-run slots (μ)
 Executor: sequential | threads (rounds, validate, commit in order)
 Memo: content-addressed, Codec + Evict hooks      passes: DVE, CSE, fold
```

A client implements `Lang` (its value enum and op enum). The core
evaluates; the client never holds the graph during an op.

### 7.2 Values and fields (primitive 1)

```rust
#[repr(C, packed(8))] pub struct Ver(pub u128);    // partex-ssa's, copied
pub trait Value: Clone + Send + Sync + 'static {
    fn ver(&self) -> Ver;                           // O(1): cached in composites
    fn field(&self, f: u32) -> Option<Self> { None }
    fn field_ver(&self, f: u32) -> Ver;             // default: field(f)'s ver
}
pub enum Sel { Whole, Field(u32), Name(NameId) }    // what an operand reads
```

- A version is the content's: scalars by value, composites as a Merkle
  hash of their parts' versions made when the value is made (the stable
  128-bit hasher of `partex-ssa::hash`, copied, not depended on).
- A derived value's *memo key* is `Ver::node(op, operand versions)`
  (7.8). Its *version*, which cutoff compares, is its content's: a node
  whose operands changed but whose result is equal stops propagation.
- An operand reads `(node, Sel)`. When a node's value changes from `old`
  to `new`, each reader is woken only if `old.field_ver(f) !=
  new.field_ver(f)` for the field it reads (inside a region by the
  in-order sweep, across regions by reader lists, 7.11). The old value
  is held only during that comparison.
- Forces it: the page builder reads a box's height and depth, not its
  list; a glyph's metrics, not its identity; a step reads `\foo`, not
  the whole environment.

### 7.3 Nodes and purity classes (primitive 2)

```rust
pub trait Lang: Sized + 'static {
    type Val: Value + Default + Debug;
    type Op: Copy + Eq + Hash + Debug + Default + Send + Sync + 'static;
    fn eval(op: Self::Op, args: &Args<'_, Self>) -> Self::Val;                // leaf
    fn step(op: Self::Op, st: &Self::Val, args: &Args<'_, Self>,
            cx: &mut StepCx<'_, Self>) -> Step<Self::Val>;                     // 7.4
    fn scan(op: Self::Op, st: &Self::Val, x: &Self::Val,
            args: &Args<'_, Self>) -> (Self::Val, Self::Val);                 // 7.6
    fn scan_result(op: Self::Op, st: &Self::Val, outs: &Seq<Self::Val>, args: &Args<'_, Self>) -> Self::Val;
    fn as_seq(v: &Self::Val) -> Option<&Seq<Self::Val>>;                     // an input
    fn chain_val(items: Seq<Self::Val>) -> Self::Val;                         // a chain or family
    fn entries(op: Self::Op, input: &Self::Val, args: &Args<'_, Self>) -> Vec<Entry<Self::Val>>; // 7.4
    fn memo(op: Self::Op) -> bool { false }                                   // 7.8
    fn op_tag(op: Self::Op) -> u64;                                           // memo keys
    fn fmt_op / parse_op / fmt_val / parse_val                                // 7.14
}
pub enum Class { Pure, Effect(Chain), Barrier, Publish(Slot), Entry(Fam, Slot) }
pub struct Args<'a, L> { /* the node's operands, read through the core */ }
impl<'a, L: Lang> Args<'a, L> { pub fn get(&self, i: usize) -> Proj<'a, L::Val>; pub fn len(&self) -> usize; }
```

- A node is an op, a class, and operands. Kinds: *leaf* (`eval`),
  *input* (a value set by the client between runs), *scan* (7.6),
  *unfold* (a scan whose steps are nodes, 7.4), *step* (one step of an
  unfold).
- Classes:
  - `Pure`.
  - `Effect(c)`: the value is a payload on chain `c`, ordered within the
    chain by position (7.7). Evaluation is still pure; only the commit
    is ordered.
  - `Barrier`: orders every chain's effects around it.
  - `Publish(s)` and `Entry(f, s)`, with the `Cross` and `Family`
    kinds that read them: the cross-run slots (7.7).
- Dispatch is `L::eval`, a `match` over the client's op enum, so the
  engine is monomorphised over `L` and the hot path has no `dyn`.
- Forces it: `\write` and the PDF's bytes are effects on chains; a
  `\ref` reads what `\label` wrote last run; `hpack` and `line_break`
  are pure.

### 7.4 Dynamic construction and stable keys (primitive 3)

The graph is built by evaluating it. Only steps create nodes.

```rust
pub enum Step<V> { Next { st: V, key: u64 }, Done(V) }
pub struct StepCx<'s, L> { /* the step's emission buffer; brand 's */ }
impl<'s, L: Lang> StepCx<'s, L> {
    pub fn next(&mut self) -> Option<&L::Val>;          // consume an input element
    pub fn peek(&self, k: usize) -> Option<&L::Val>;    // consume() must follow a peek it depends on
    pub fn read(&mut self, n: NameId) -> Option<&L::Val>;   // a name at this position (7.5)
    pub fn emit(&mut self, op: L::Op, class: Class, args: &[Arg<'s>]) -> Local<'s>;
    pub fn emit_unfold(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) -> Local<'s>;
    pub fn emit_scan(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) -> Local<'s>;
    pub fn key(&mut self, k: u64);                      // the next emit's key (default: its ordinal)
    pub fn define(&mut self, n: NameId, v: Arg<'s>, global: bool);
    pub fn open_group(&mut self);  pub fn close_group(&mut self);
    pub fn name(&mut self, spelling: &[u8]) -> NameId;  // content-addressed (7.5)
    pub fn cancelled(&self) -> bool;
}
pub enum Arg<'s> { Local(Local<'s>), Up(u16), Name(NameId), Field(Local<'s>, u32), Lit(/* constant node */) }
```

- **The unfold.** An unfold node has an input (a `Seq`), an initial
  state and operands. Its first step runs on the initial state; each
  step consumes input elements (`next`), reads names, emits children,
  defines names, and returns the next state and the next step's key,
  or `Done(result)`. The steps are nodes, in a linked list under the
  unfold; a step's children are its own region. A macro call is a step
  that emits nothing: it rewrites the input, which is the state (the
  pending tokens) plus the core's cursor in the `Seq`.
- **Regions and scoping.** A step can only refer to what it holds: its
  unfold's operands (`Up(i)`), names (resolved by the core), and the
  `Local`s it emitted in this call. `Local<'s>` carries an invariant
  brand lifetime, so a handle cannot escape the call (it cannot be put
  in a `'static` value nor passed to another step). A node in a region
  is therefore read from outside only through names or through its
  step's state, so replacing a region never leaves a dangling edge.
- **Keys.** A node's identity is `(parent, key)`. A step's key is the
  client's (TeX: the stable id of the element where the step starts,
  with the expansion it came from); a child's is its ordinal unless the
  step gives one. When a step runs again, its new emissions are matched
  to its old children by key: a match keeps the old node's id, value
  and readers; it is re-evaluated only if its op or operands differ or
  an operand's version changed. Unmatched old children are removed.
- **Resync and convergence.** When a step runs again and returns
  `Next { st, key }`, the core compares with the old successor: if its
  key is the same, its input cursor maps to the same old element and
  `st` has the same version, the old successor and everything after it
  are kept (convergence: the rest of the region is not visited). If not,
  the core finds the old step with that key (a map per unfold), removes
  the old steps in between with their regions, and runs the new step.
  An unmatched key inserts a new step. So a flipped conditional, a
  macro that now takes another argument, or a paragraph merged with the
  next replaces exactly the steps between the edit and the point where
  the state comes out equal.
- **Input edits.** When an unfold's input `Seq` changes, the core diffs
  old and new (7.6: equal subtrees skipped by version), finds the step
  that consumed the first changed element (each step keeps the count it
  consumed and the core keeps a map from a step's first element to the
  step), and runs it again. Steps before it read nothing that changed
  and are not visited.
- **Keys decide reuse, never results.** A wrongly matched node is
  re-evaluated because its op or operands differ, and its readers are
  woken by version as always. So a key collision (keys are hashed to 32
  bits in the node table) costs work, not correctness, and the random
  edit tests check that.
- **Why this keying, against the alternatives.**
  - *Adapton* names thunks explicitly and globally, and dirties
    demand-driven: an edit marks every transitive dependent dirty
    before re-evaluating, with no cutoff during the marking. A preamble
    `\def` would mark the document. φ propagates by push in position
    order with cutoff at each node (self-adjusting computation's change
    propagation), and names are scoped to a parent, so uniqueness is
    local and an ordinal is a safe default.
  - *Salsa* keys queries by interned arguments and verifies by pulling
    from what is demanded: each revision walks the dependencies of the
    outputs (the PDF), O(document) per keystroke. Durability only
    coarsens that. It has no ordered program, no scoped names and no
    sequence reuse. φ takes from it backdating (early cutoff),
    interning (names), and the iterated cycle (7.7).
  - *Self-adjusting computation* (Acar) re-executes a changed
    function's interval and matches memoized calls inside it in order.
    φ's resync is that matching, with keys instead of call equality,
    bounded by convergence of the state instead of by the end of the
    interval, which is what TeX's sequential main loop needs.
- Forces it: macro expansion rewrites the input; `\if` reads one arm;
  `\csname` creates a name; an edit that flips a conditional replaces a
  subgraph and the rest of the document must be found again, not
  rebuilt.

**The state stays small.** Anything addressable by name is a name,
never part of a step's state. The state holds only what has no name:
TeX's mode, the pending input, the conditional and group depth. A
small state is what makes convergence come early and speculative entry
(below) predict exactly. `Config::debug` reports each unfold's state
size per step (`Value::bytes`, mean and max), so the client can be
held to the rule.

**Transient interiors** (checkpoint 3a, as built). A step's pure
leaves and constants are not nodes. The step runs into an emission
buffer, and only *big* emissions become its members:
- the sources of its definitions;
- whatever is not pure (effects, barriers, publishers, entries);
- creators (nested unfolds and scans), and the members they read;
- cross, chain and family reads.

The rest is evaluated in the buffer (most of it eagerly, as it is
emitted) and dropped. The step's own operands are its state before it,
the names it read, and every read its interior made from outside it,
each with the field it read. Any change to them runs the step again,
whole.

- *The rule on `Lang::step`:* a step's interior is bounded by
  construction. Anything that can grow with the input (a paragraph's
  words, a box's contents, a list) is a nested unfold, a scan or a
  sequence value, each a node with its own incremental behaviour.
  `Config::debug` reports each op's interiors (`Graph::interior_sizes`:
  steps, max, p99), so a client is held to it. The toy's and the bench
  client's interiors do not grow with paragraph length (test
  `interiors_are_reported_and_bounded`: 10, 100 and 1,000 words a
  paragraph, same max).
- *Nothing is lost at a step's edges.* Definitions' sources are members
  with values, so field-level versions hold across a step's boundary.
  A reader that wants one field of a name reads it with
  `Arg::NameField(n, f)`. Its operand's selector has two levels (the
  definition's own field, then the reader's), and re-resolution keeps
  the reader's level. The test `an_equal_width_page_change_stops_at_the_width`:
  a `\pageref`-like slot changes a digit at equal width. The step that
  shows the page runs again; a step two definitions on that reads only
  the width does not, with or without sealing.
- *Readers of what a step defines* are resolved again after its sweep,
  when a new source has its value. Before that, a new source read as
  the default value and woke its readers.
- *Check mode with interiors kept.* `Config::keep_interior` makes every
  emission a member (the text form then shows them). Check mode runs
  every live step again, dry, from the state before it. It compares the
  output state, the members in order (kind, op and value) and the
  definitions, so with interiors kept every leaf is compared (test
  `kept_interiors_check_every_leaf`, 12 random-edit seeds).
- *A step woken through its state* compares the new state with the one
  it ran from (`in_ver`), not with the old value of the node before it.
  So a step inserted before it that ends in the same state changes
  nothing for it.

**Speculative entry** (the unfold's parallelism). One chain is
sequential: on a cold build its critical path would be the whole
document. So the chain is also started at later points from predicted
states, and the real chain accepts each speculated suffix when it
arrives with the same state. That is the convergence check above,
reused.

```rust
pub struct Entry<V> { pub at: usize, pub key: u64, pub guess: Option<V> }
// in Lang:
fn entries(op: Self::Op, input: &Self::Val, args: Args<'_, Self>) -> Vec<Entry<Self::Val>>;
```

- *Proposals.* For an unfold built cold, or whose input changed past
  its old steps, the core asks `L::entries`: element indices where a
  step may start (TeX: paragraph or block boundaries of the source),
  each with the key the step there would have.
- *Predictions,* in order:
  - the state of the old step with that key (in a session the old
    steps are there; across sessions the memo store keeps
    `(unfold key, step key) → state`, 7.8);
  - else the client's `guess`;
  - else none, and the entry is not used.

  With names out of the state, the state at a paragraph boundary is
  small and often exactly the guess.
- *Segments.* Each used entry starts a *segment*: a chain from its
  predicted state that runs until its cursor reaches the next used
  entry's element. Workers run segments in parallel, each into a
  private buffer of steps and children (the graph is only read). A
  segment ends with the state it reached at the next entry, a better
  prediction for that entry than the guess.
- *Names read in a segment* resolve first in the segment's own
  definitions, else against the index as committed when the segment
  started (the last build's, or the segments committed so far). Each
  read records the definition it reached and its version (or
  "undefined").
- *Arrival.* Segments are committed in order. When the real chain (the
  committed segments before) reaches an entry:
  - same cursor and same state version: the segment is accepted whole.
    Its recorded name reads are checked again at their positions in
    the committed index. A read that now reaches a definition with
    another version wakes the step that made it, as a rebuild would.
  - otherwise the prediction was wrong: the segment's steps become the
    "old" steps of a rebuild. The first one runs again from the arrival
    state; children are matched by key, and the chain stops at the
    first step whose state converges with the segment's. A guess wrong
    only in what a paragraph's end resets costs one or two steps.
- *Rounds.* After a round, every segment whose arrival state is now
  known and differs from its prediction is run again, in parallel. The
  steps that the real chain reached decide the result, so the result
  is the sequential one (7.12). The test: a cold build of many
  independent paragraphs at W workers is faster than at one, and equal.

### 7.5 Names: reaching definitions with scopes (part of primitives 3 and 5)

A step's state is not an environment threaded through every step:
that is the cursor chain V8 left the sea of nodes for (3.9). Names are
an index of definitions by position, chapter 3.2's definition index made
generic.

```rust
pub struct NameId(u32);         // interned from a 64-bit content hash of the spelling
```

- `define(n, v, global)` at a step is a definition of `n` at that
  position: the value of node `v`. `open_group`/`close_group` are scope
  events at positions. A definition is *alive* at position `t` if it is
  global or its group has not closed before `t`.
- A read of `n` at `t` resolves to the latest definition before `t`
  that is alive at `t`: TeX's save stack, including `\global` inside
  groups (checked against §279–§283 in the tests). A group's end makes
  no node and no definition; it only ends lives.
- A read is an edge to the defining node with `Sel::Name(n)`, so it is
  woken when that value changes (field-level, 7.2). Inserting or
  removing a definition, or moving a group's close, re-resolves the
  readers of `n` in the affected range: those whose old definition's
  range it cuts.
- **The cost of a new definition** (a `\def` added early, shadowing
  readers of `\foo` across the document).
  - A definition's readers are kept as a run sorted by position, and a
    reader reaches its definition through the run.
  - Inserting a definition of `n` at `p` splits the runs of the
    definitions alive at `p`: at most one per open group level, so in
    practice one. The part after `p`, up to the next definition of `n`
    alive there, moves to the new definition. That is O(log R) for R
    readers, not O(R).
  - Readers are woken only if the new value's version differs from
    the old one's. A definition with an equal value costs the split
    and nothing else.
  - A different value wakes exactly the readers after `p` that it
    shadows. Each now reads something else, which is the edit's
    inherent reach, not overhead.
  - Removing a definition merges its run back into the one before.
    Moving a group's close splits or merges at the close.
  - *As built (checkpoint 2):* a name's readers are a vector sorted by
    their *anchor*, the top-level step or root node they are under,
    whose order never changes. A change at `p` binary-searches to the
    anchor of `p` and visits the readers after it, each re-pointed one
    by one: O(log R + readers after `p`). Splitting runs, so that
    readers beyond the next definition are not visited either, comes
    with checkpoint 3's memory work. Before this, every new definition
    visited every reader of its name: a cold build of 800 paragraphs
    took 5.6 s, and 0.25 s after.
- Name ids are interned from a stable 64-bit hash of the spelling. The
  table is the core's, never seen by an op, so the numbering does not
  affect results.
- Children read names statically: `Arg::Name(n)` resolves at the
  child's position. So `\the\count1` makes a node that depends on
  `\count1`, while the step that made it does not.
- Memory: the index holds every live definition. Folding steps (7.10)
  keeps only a run's net definitions (3.2's measurement: 95% are
  overwritten within 1,024 commands).
- Forces it: `\def`, `\let`, registers, catcodes as names; groups that
  open in one paragraph and close in another; `\global`.

**The name index, as built (checkpoint 3b).**
- *One record per definition.* `NameTab::recs` holds 32-byte records:
  name, step, ordinal in the step, source, selector, group and global.
  A step's definitions are a range of it, and each name's list holds
  4-byte indices into it, in position order. A step whose definitions
  change gets a new range; the old one stays until compaction rebuilds
  the records and remaps the lists. An import (a segment's name read
  from before it) is a record at the root that no step holds.
- *Lists in runs.* Each name's definition and reader lists are blocks
  of at most 256 entries with their start indices (`graph/runs.rs`).
  Finding a place is a binary search over the blocks and then inside
  one. Inserting or removing moves at most one block's entries plus
  the block starts after it. So a name read or defined a million times
  costs O(log R + 256) per change, not O(R).
- *Bounded re-resolution.* After a definition of `m` changes at `p`, the
  only readers that can resolve differently are those before the next
  definition of `m`, in a later anchor, that is alive to the end
  (global, or in no group): every reader past it reaches it or a later
  one. Only that run of readers is visited. A name with no reader after
  `p` costs one comparison.
- *Reader entries leave where they are.* When a node's operands change
  or it is removed, its entries are found by anchor and removed, rather
  than the whole list being pruned at the run's end.

### 7.6 Persistent sequences and scans (primitives 4 and 5)

```rust
pub struct ElemId(pub u64);
pub trait Measure<T>: Clone + Send + Sync { type S: Copy + Send + Sync; fn of(x: &T) -> Self::S; fn op(a: Self::S, b: Self::S) -> Self::S; const ID: Self::S; }
pub struct Seq<T, M = ()> { /* Arc'd B-tree, B = 32 */ }
impl<T: Value, M: Measure<T>> Seq<T, M> {
    pub fn splice(&self, at: usize, del: usize, ins: impl IntoIterator<Item = (ElemId, T)>) -> Self;
    pub fn get(&self, i: usize) -> Option<(ElemId, &T)>;
    pub fn ver(&self) -> Ver;                       // Poly: independent of shape
    pub fn measure(&self) -> M::S;  pub fn prefix(&self, i: usize) -> M::S;  // O(log n)
    pub fn diff(&self, old: &Self) -> Hunk;         // common prefix/suffix, equal subtrees skipped
    pub fn from_par<E: Executor>(items: Vec<(ElemId, T)>, e: &E) -> Self;
}
```

- The tree is `partex-ssa::pvec`'s (32-way, `Arc` nodes, the two-lane
  polynomial hash `Poly`, so a sequence's version is its content's
  whatever its edit history). Elements carry an `ElemId` the client
  assigns (deterministically), which is the identity steps and scans
  key by. The version does not include ids.
- Each tree node caches its measure, a monoid. Prefix sums are
  O(log n), splices update O(log n) summaries, and building the tree
  bottom-up over chunks is the parallel prefix (each subtree on a
  worker).
- **Scan** (`emit_scan`): `fold(L::scan, init, input)` with a state at
  every element. The scan node keeps checkpoints (the state after each
  element, every `k`th with a configurable stride) and its outputs, a
  `Seq` keyed by the input's element ids. After its input changes it
  diffs old and new, resumes at the checkpoint before the first changed
  element, and after the last changed element stops at the first
  checkpoint whose state has the old run's version at the same element;
  the rest of the outputs is the old run's. Its value is
  `(final state, outputs)`. The count of elements stepped is reported
  (the tests assert it).
- An unfold (7.4) is the same algorithm whose steps are nodes, consume
  any number of elements and emit children.
- Forces it: line breaking is a DP over a paragraph whose state (the
  active breakpoints) comes out equal a few lines after an edit; the
  page builder is a scan over the vertical list; `\count0`, footnote and
  section numbers are prefix sums; the main loop over the source is an
  unfold.

### 7.7 Effects, barriers and the cross-run fixed point (primitive 6)

- **Chains.** An `Effect(c)` node's value is a payload. The chain is
  the payloads of its live effect nodes in position order, kept as a
  sorted index per chain. Chains are unordered with respect to each
  other except at barriers: a `Barrier` node splits every chain into
  segments, and a sink receives segment by segment. Evaluation is never
  ordered by effects; only commit is (`Graph::chain(c)`,
  `Graph::segments()`).
- **Cross-run slots are per entry.** One slot per label, citation key,
  or TOC, LOF or index entry, keyed by the client (`Slot(u64)`, a hash
  of what it names), holding a value with fields (number, page, …). A
  book has 10⁴–10⁵ of them.
  - `Publish(s)` nodes give slot `s` its value for the next run: the
    last in position order, or absent.
  - A `Cross(s)` node reads slot `s`. Its readers read it by field
    (`Arg::Field`), so a changed page number wakes only the readers of
    the page.
  - `Entry(f, s)` nodes publish slot `s` and list it in *family* `f`.
    A `Family(f)` node reads the family as a `Seq` in the last run's
    order, each element keyed by its slot, and a scan over it resumes
    at the changed entry. So renaming one TOC entry runs one TOC line
    again; the test asserts exactly one.
  - A run checks only the slots and families whose publishers changed
    in it (`dirty_slots`, `dirty_fams`): an edit that touches no
    `\label` costs the cross-run loop nothing, however many slots
    there are.
  - Files like `.aux` are a write chain, committed at the end. They
    are not slots.

  A run:
  1. predicts each slot from the last run (in the session, or the memo
     store's persisted copy, 7.8);
  2. propagates to quiescence;
  3. compares each read slot's prediction with what was published;
  4. for each mismatch, sets the `Cross` nodes' value and wakes their
     readers: only the slice that reads the slot runs again;
  5. repeats until no slot changes, at most `max_iters` (default 5).
     If a slot still changes after that, the run ends with a
     diagnostic, `Report::oscillating`: each slot with the versions it
     took, in order, and the nodes that published them. A run that did
     not converge says so, and the result is never silently
     truncated.

  A correct prediction costs no extra iteration, and the tests count
  them.
- Forces it:
  - `\write` to `.aux`, `.toc` and `.idx`, and the log, are chains.
  - `\openout` truncation and `\shipout`'s order across streams are
    barriers.
  - Each `\ref`, `\pageref` and `\cite` target is a slot.
  - The `.toc` is a family of slots, one per entry.
- **Append lists, as built (3b).** LaTeX's hooks (`\AddToHook`) and
  expl3's `\tl_put_right` / `\clist_put_right` append to a list
  without reading it. They are effects on a chain whose identity is the
  list: the append's operand is the payload alone, not the list's
  version, so an append wakes nothing before it and reads nothing. A use
  reads the chain with `StepCx::chain_before(c)`: the payloads before
  the reader's position, in position order. (A `ChainRead` with the
  `BEFORE` bit reads up to its position; a plain one reads the whole
  chain.)
  - Chains are kept in position order (`chains: Map<u32, Runs<u32>>`,
    the same blocked runs as the name index, so an insert or a removal
    is O(log n)).
  - A changed payload wakes only the bounded readers after it
    (`wake_chain_at`). So editing one append re-runs that append's step
    and the uses after it, never another append and never a use before
    it.
  - A segment's chain reads (7.15) are marked dirty at the graft, since
    the segment saw only its own payloads.
  - Test: `an_append_list_reruns_only_what_follows_the_edit`. Forty
    appends with a use in the middle and one at the end; an edit to the
    30th append runs one hook evaluation and at most two steps, and
    equals a fresh build. The toy's `\addto{h}{w}` and `\usehook{h}`
    are in the random-edit test's snippets.
- No relocatable values. The core has no offset or relocation
  primitive. Positions are node identities (7.4), and what a client
  would relocate it recomputes from the identities.

### 7.8 The memo store (primitive 8)

```rust
pub trait Codec<V> { fn encode(&self, v: &V, out: &mut Vec<u8>); fn decode(&self, b: &mut &[u8]) -> Option<V>; }
pub trait Evict { fn touch(&mut self, k: Ver, bytes: usize); fn victims(&mut self, budget: usize) -> Vec<Ver>; }
pub struct Memo<L: Lang, E: Evict = Lru> { /* Ver -> (L::Val, bytes) */ }
impl<L: Lang, E: Evict> Memo<L, E> {
    pub fn get(&mut self, k: Ver) -> Option<&L::Val>;  pub fn put(&mut self, k: Ver, v: L::Val, bytes: usize);
    pub fn save<C: Codec<L::Val>>(&self, c: &C, out: &mut Vec<u8>);
    pub fn load<C: Codec<L::Val>>(&mut self, c: &C, b: &[u8]) -> Result<(), Error>;
}
```

- Keys are content: `Ver::node(L::ver_op(op), operand versions)` for a
  leaf, `(op, init version, element version, state version)` for a scan
  checkpoint. So entries are valid across builds and across documents
  (a paragraph broken once is broken for every document with the same
  paragraph and parameters).
- Probed only for ops that opt in (`L::memo`): a probe costs a hash and
  a lookup, more than a small op. Node identity (7.4) is what reuses
  work inside a session; the memo is for expensive ops and for cold
  starts.
- Eviction is a trait: LRU by bytes by default.
- Persistence is a byte image through the client's `Codec`, with a
  format version; a decode error drops the image, never a build.
- Forces it: fonts, a TikZ picture, the line breaking of an unchanged
  paragraph, a cold start after a restart.

As built (3b, `crates/phi/src/memo.rs`):

- `Memo<V, E: Evict = Lru>` over the value type; the graph holds one
  behind a mutex, shared with the segments of a parallel cold build and
  read by dry steps (7.12), so a worker's miss fills it for the others.
  Off (budget 0) by default: `Graph::set_memo_budget`, `memo_stats`
  (entries, bytes, probes, hits), `save_memo` / `load_memo` (a bad
  image leaves the store empty, `MemoError`).
- Probed at every place a leaf is evaluated: emitted with ready
  operands, evaluated in its step, re-evaluated in a sweep or on its
  own. A leaf's key is `Ver::node(op_tag, operand versions)`, made
  without allocating for up to 8 operands.
- Scans: per element, two entries (the state after, the output), keyed
  by `(op_tag, state, element, arguments)`. The scan's own convergence
  (7.6) still decides where it stops; the memo only saves the element's
  op.
- `Lru`: a use stamps its entry; victims are sorted only when the store
  is over budget, and it then frees an eighth of the budget beyond the
  excess, so eviction is amortized.
- The check (7.13) evaluates raw, never through the store, so a wrong
  entry would be caught.
- Cost: a probe is about 90 ns (lock, key hash, two lookups; `prof`
  with every Add memoized: 123.7 -> 201.3 ns/node over 2M probes,
  99.95% hits). Worth it for ops above about 0.2 us, which is what
  `Lang::memo` is for.
- Tests: `a_memo_image_spares_a_fresh_build_its_memo_ops` (a fresh
  graph with another's image evaluates no hook use and no line breaker
  element, and equals a build without the store; an edit then steps
  only the edited paragraph's words; a truncated image errs and leaves
  the store empty; a 256-byte budget holds), and the parallel document
  of the random-edit test runs with a 4 KB store (hits, misses and
  evictions, compared with the sequential build after every edit).

### 7.9 Purity, by type

- An op's whole input is `Args<'_, L>` (shared references to operand
  values) and, for a step, `&mut StepCx`, which only buffers emissions
  and resolves reads through the core. No op signature mentions the
  graph, the store, the worklist or another node's state; there is no
  way to name them.
- `L::Op: Copy` excludes owned interior mutability (`Cell`, `RefCell`
  are not `Copy`). `L::Val: Send + Sync` excludes `Cell`/`RefCell` in
  values.
- `Local<'s>` is branded: emitted handles cannot escape their step (no
  edges to nodes out of scope).
- What the types do not exclude: a `static` with a `Mutex` or an
  atomic, or a value holding an `Arc<Mutex<_>>`. Those are reviewed
  away, and *check mode* (`Config::check`) evaluates every node that
  propagation would reuse and asserts its value is equal, naming the op
  that is not a function of its operands. The property tests run with
  it on.

### 7.10 Passes (primitive 9)

Each pass preserves the graph's results; each fold says why it adds no
dependency the unfolded graph lacks.

- **Dead values.** A node with no reader, no definition, no chain or
  slot and not its step's result keeps its structure (for matching) and
  drops its value. No result reads it, so none changes.
- **CSE.** Within one region, pure nodes with the same op and the same
  operands (node and `Sel`) are merged; the second's readers read the
  first. By purity their values are equal now and after any edit (they
  read the same operands), so every reader sees the same versions.
  Across regions the memo shares results instead, without merging
  identities.
- **Fold.** A *single-entry, single-exit* set of pure nodes in one
  region (no member read from outside but the exit, no definitions,
  effects or children) becomes one node holding its members' ops and
  local wiring (about 16 bytes a member, against 7.11's ~70). Its
  operands are the union of the members' outside operands.
  *Argument:* the folded node reads exactly what its members read, so
  every change that wakes it woke a member before; a member's value
  was read only inside the set, so no reader loses an edge; the fold's
  value is the composition of pure functions, equal to the exit's.
  Cost: an operand change re-runs the whole set where one member might
  have sufficed (interior cutoff is lost), never a wrong value.
- **Step fold.** A run of consecutive steps whose intermediate states
  are read only by the next step becomes one step: consumed counts
  added, reads unioned, children concatenated, and only the run's *net*
  definitions kept. *Argument:* a definition overwritten (or ended by a
  group's close) inside the run reaches no position after the run, so
  only members read it; the run's reads from outside are the union of
  the members'. Cost: a change re-runs the run from its start.
- A fold is undone (unfolded) when its node runs again more than
  `refold_after` times in a session: granularity follows edits.

As built (3b). Three of the four passes are not separate passes: the
structure that 3a built does each one where it costs nothing, with the
same argument. CSE is the one pass left, and it is built.

- **Dead values: 7.4's transient interiors.** An emission that no one
  outside its step reads (no definition, effect, slot, creator or
  outside reader) never becomes a node. Its value lives in the sweep's
  buffer and is dropped after the sweep. The emissions that remain nodes
  are the ones something outside reads, or may read: a definition's
  value is kept even when no reader reaches it yet, because a reader
  that appears later, and the cutoff, compare against its version.
  Nothing a result reads is dropped, so no result changes.
- **Fold: the step's interior.** A step's interior is single entry
  (its state and the reads it records) and single exit (its
  definitions and big emissions), and it is pure where it is
  transient. A woken step runs its interior again whole: that is the
  fold, and its cost (no interior cutoff) is the cost the design
  accepts. `keep_interior` is the unfolded form. Both build the same
  results; the property test checks both.
- **Constant folding: at emission.** A leaf whose operands are all
  known when it is emitted is evaluated then (`Spec::done`), so no node
  or sweep work is left for it.
- **Step fold: 7.11's sealed regions.** Net definitions only, reads
  unioned, consumed counts added. The refold rule is the unseal: a woken
  sealed run unseals, and is sealed again only once it has been quiet
  for `seal_quiet` runs.
- **CSE: built, per step, opt-in per op (`Lang::cse`).** When a step
  emits a pure leaf equal to one it emitted before (same op, aux and
  operands), the earlier emission is returned and no new one is made. An
  operand that reads a name counts as equal only if the step made no
  definition and no group event between the two reads, so both resolve
  to the same definition. *Argument:* the two leaves read the same
  operands, so by purity their values are equal now and after any edit;
  the readers of the second read the first, and no edge is lost. The
  merge is a function of the step's emissions only, so a fresh build
  and an edited one merge alike (the random-edit test compares them
  with the `\twice` command in its snippets). Hash collisions are
  checked against the operands, never trusted. `Report::merged` counts
  merges. Cost: about 20 ns a probe (`prof`, every Add probed, no
  merges: 116 -> 133 ns/node over 10M probes). That is why CSE is opt-in
  per op: a client turns it on for ops it emits twice in a step.
  Across steps and regions the memo store (7.8) shares results instead.
- The property test found a core bug here: in a segment (7.15), a leaf
  that read a name after its own step defined it, while that definition
  was still pending, was left for the sweep. The sweep resolved it at
  the step's position and recorded a read of the step's own definition
  as an outside operand. The values were right, but the operands were
  not the sequential build's. The sweep now resolves a name against the
  step's own definitions first, at the emission's place
  (`Emit::own_def_at`), as `eval_now` does.

### 7.11 Memory layout and cost

As built (checkpoint 2), the graph is one arena indexed by `NodeId(u32)`:
a header per node, its value in a column beside it, and two shared
arenas.

| | bytes | |
|---|---|---|
| header `Hdr` | 64 with an 8-byte op | `ord: u64` (label among siblings), `aux: u64` (table index, chain or slot), `op`, `parent`, `next`, `first`, `key` (hashed, for matching only), `a0` (first operand), `rd` (reader list head), `era`, `an: u16`, `depth: u16`, kind, class tag, flags |
| `val: L::Val` | client's (the bench's: 32) | |
| operand `(src, sel, name)` | 12 each | `name` set when it was resolved from a name |
| reader entry `(node, era, next)` | 12 per edge | singly linked from the source; stale entries (an older `era`) unlinked when met |

- The header was columns first: 16 vectors, about 20 cache lines
  touched per node made. One 64-byte header is one line, and a cold
  build of 1 M leaves went from 122 ms to 93 ms.
- **Every edge has a reverse entry**, which is not what 7.11 first
  said. The design had a region swept in order instead, but a client's
  root region can hold millions of nodes, and the sweep would make a
  one-leaf edit O(region). With reverse entries everywhere, a one-leaf
  edit of 1 M is 93–114 ns.
- Positions are Dewey paths (`parent`, `ord`), compared in O(depth).
  Steps' labels have gaps of 2³², and an insertion where there is no
  room relabels that unfold's steps.
- No allocation per node on the hot path: the header and value columns
  grow by doubling; emissions, operands and ids go through scratch
  buffers that are reused; the core's maps hash with a multiply-rotate
  hasher, not SipHash. A step that defines names allocates its
  definition list (a `Vec`), the remaining per-step allocation.
- Measured (7.16): 144 B per root leaf with two operands (64 header +
  32 value + 24 operands + 24 reverse entries) and 206 B per node of
  an unfold with ten leaves a step. That is above "tens of bytes".
  The retained tiers below are the answer.
- Targets: tens of ns per node evaluated (leaf, cold, sequential), tens
  of bytes per unfolded node, 10⁸ nodes at checkpoint 3.

**Retained memory** (designed before checkpoint 3). At 64 B a node, 10⁸
nodes are 6.4 GB: too much for a laptop or wasm. Once evaluated, most of
the graph is kept only for rebuilds, and a rebuild needs much less than
the evaluation made. Three tiers:
- *Live:* every column. The regions an edit touched lately, and
  anything with a cross-region reader still being propagated.
- *Version-only leaves.* A pure, cheap leaf (its op says so) drops its
  value and keeps its op, its operands and an 8-byte version. When it is
  read, it is recomputed from its operands, recursively through other
  version-only leaves, so a fold bounds the depth. When it is woken,
  the new value is compared with the kept version. 64 bits suffice for
  this comparison: a false "equal" needs a 2⁻⁶⁴ collision between a
  node's own successive values. The memo's keys stay 128-bit. About
  24 B a leaf.
- *Sealed regions.* A step whose region is done (its steps folded,
  7.10) keeps only:
  - its entry state (for re-entry);
  - its net definitions;
  - its outside reads (name, the definition reached, version) for
    waking;
  - its effects' payloads and its output state.

  The interior is dropped. When the region is woken it runs again from
  its entry state, keys matching nothing (there is nothing to match),
  and it can be sealed again. A paragraph of 1,000 command nodes keeps a
  few hundred bytes, under 1 B per original node.
- *Target:* at most 16 B per original node retained on average, live
  tier included, and below 2 GB for the course. This is measured
  against the TeX layer's node counts when they come; the core reports
  bytes by tier (`Graph::mem`).

**Sealed regions, as built (checkpoint 3a).** `Config::seal = F`
turns sealing on. A sealed region is the first step of a run of a root
unfold's steps, flagged and holding:
- its entry: the first step's key, cursor, group and the version of the
  state it ran from;
- its output state (as its value);
- its net definitions, with their sources kept as its members;
- its reads from outside (deduplicated, each with its field);
- its input length and its steps' and emissions' counts.

Everything else of the run is dropped, and compaction frees it.

- *Which runs.* A run holds only steps whose members are pure leaves
  and constants. An effect, a publisher, a creator or a cross read
  ends it, so a paragraph's end with its line breaker stays live.
- *Cuts.* A run is cut only where the group in force is the one it
  started in. Then no group the run opened is still open after it, and
  no group open before it closes inside it. Groups opened and closed
  inside are dropped with it. Among those balanced points a cut is made
  where the step's identity (key, cursor) hashes to one in F
  (content-defined, so a fresh build and an edited one cut alike).
- *The bound* is in emissions, not steps: `Config::seal_max = 4096`.
  Rationale: a first edit in a sealed region re-runs it whole. At the
  measured 60–80 ns per emission that is at most about 0.3 ms, under 2%
  of a 16.7 ms frame, leaving the frame to the edit itself and to
  layout. Measured on the bench client (F = 16, 100 k steps): runs of
  16 steps on average, p99 65, max 117; 178 emissions on average, max
  1,287.
- *A woken region unseals.* It runs again as its first step (`run_step`
  clears the flag), and the steps after it are made again until one
  meets the step that followed the region. Those steps are live and
  stay live: a step that ran after the first build is sealed again only
  after `Config::seal_quiet = 16` runs without running. A queue of such
  steps, by the run they become due, drives a local pass: back to the
  stretch's start (through each step's state operand) and forward to
  the next sealed region. So only the first edit in a region pays for
  it. The next edit there re-runs exactly the steps it would with
  sealing off (test `a_second_edit_in_a_sealed_region_is_step_precise`:
  equal step and evaluation counts).
- *Fields at the edge.* A region's net definitions keep their sources'
  values, so a reader of one field of them is woken only when that
  field changes (the equal-width `\pageref` test, sealed).
- *The name index.* A whole pass rebuilds each touched name's
  definition and reader lists in one merge. A local pass splices the
  run's range (from its first step to the node after it) in place.
  Readers that land out of order during a run wait in a per-name buffer
  and are merged once per run, not shifted in one by one.
- *Compaction* (`Graph::compact`, run after sealing when at least half
  the node table is dead): live nodes are moved into holes from the
  top, the tables truncated, and the operand, reverse-edge and
  definition arenas rebuilt from the live nodes. Root-level nodes never
  move (the client holds their ids). Group ids embed their opening
  step's id: they are remapped, and a dead step's slot that still names
  a referenced group is not reused.
- Equality: with sealing on (F = 1–4) and compaction after every run,
  40 random-edit seeds give the same observable results as fresh
  unsealed builds checked in full (`sealed_random_edits_equal_fresh_builds`).
  The text form can differ: where a fresh build cuts and where an
  edited one re-sealed are both valid.

### 7.12 The scheduler (primitive 7)

- **Sequential** (the default, and wasm's): a cold build evaluates in
  creation order, which is topological, with no queue. A rebuild pops
  the worklist (a binary heap by position): a region sweep from its
  first dirty child, a step to run, a scan to resume. In the acyclic
  part each node runs at most once per change.
- **Parallel** (`Executor` with width > 1, std threads, never on wasm):
  rounds, as Block-STM runs a block.
  1. Take the earliest dirty items up to a work budget.
  2. Workers evaluate them against the graph as it is (shared, read
     only during the round), each item locally in order, recording every
     outside version it read (operands, names with the definition they
     resolved to). Items further on are *speculative*: they use values
     an earlier item may change.
  3. The coordinator validates outcomes in position order as they
     arrive, against the changes accepted earlier in the round: an
     outcome whose read versions still hold is accepted, else its item is
     requeued. Accepted outcomes are applied in position order at the
     round's end (values, matched or new nodes, definitions, wakes).
  4. *Cancellation:* when an accepted outcome changes a value that an
     in-flight item reads statically, that item's flag is set; long ops
     (scans, unfolds) poll `cancelled()` between steps. A new edit bumps
     an epoch that cancels every item.
- **Determinism.** Every op is a function of its operands, every
  accepted outcome read the versions current at its position, and
  every later change re-wakes its readers; in the acyclic part the
  fixed point is unique, so any schedule ends in the sequential result.
  The order only changes the wasted work. Tests compare the two runs'
  values, chains and slots.
- **Cold builds** are wide through speculative entry (7.4): round one
  runs a segment per entry on the workers, and the commit walks them in
  order, accepting the segments whose arrival state matches.
- Forces it: a preamble `\def` wakes thousands of paragraphs at once;
  a cold build is wide only if later chapters run from predicted states
  (the last build's), validated as the chain reaches them; a keystroke
  cancels the background convergence.

As built (3b, `graph/par.rs`). The rounds are simpler than the plan
above: workers never apply anything. The coordinator stays the
sequential scheduler, and an outcome only replaces running an op.

1. When the heap's top is a dirty step, a round takes it and the dirty
   steps after it along its unfold's step list (a walk, not heap
   pops), up to the round size.
2. Workers (`std::thread::scope`) run each step's op *dry*: against the
   graph as it is, read only, into an emission buffer from a pool. Each
   records what it read from outside, with versions: its state, the
   unfold's operands, every name it read with that name's definition
   generation (`NameTab::gens`, bumped by any insert or remove of a
   definition of that name), every node operand, the group table's
   generation, the name count, the input's version and the step's
   position.
3. The sequential run goes on as before. When it reaches a step with an
   outcome, the outcome is used if all of these still hold (`holds`);
   otherwise the op runs again. Either way the step is then matched,
   swept and woken as before.

- *Correctness* is the sequential run's, by construction. A used
  outcome is the op's result on exactly the inputs the sequential run
  would give it, because an op is a function of what it reads, and
  every read is recorded and checked.
- *Round size* adapts. It starts at 64 steps a worker and doubles, up to
  4096 a worker, while at least 90% of a round's outcomes are used;
  otherwise it halves.
- *Gating:* one step in 32 is timed, and rounds run only when the mean
  op cost is at least `Config::round_min_ns` (default 10 us). A cheap
  op costs less to run in turn than its outcome costs to hand over:
  thread start, allocation, cold cache.
- *Measured* (`prof` with `PRE` and `HEAVY=20000`, about 75 us an op; a
  preamble edit re-runs 20k steps): 1512 ms sequential, 483 ms at W=4,
  263 ms at W=8.
- *Tests:* the random-edit test keeps a third document with
  `workers = 2`, `round_min_ns = 0` and a 4 KB memo store. It is
  compared with the sequential one after every edit, and asserts that
  outcomes were used.

Cancellation, as built: `Graph::cancel_token()` gives an
`Arc<AtomicBool>` that another thread may set. The run polls it between
work items, workers poll it between dry steps, and a long op may poll
it through `StepCx::cancelled()`. A cancelled run returns at once with
`Report::cancelled`. Its remaining work stays queued (dirty flags and
the heap are untouched), the flag is cleared, and the next `run`
finishes the job. The result equals an uncancelled run's
(`a_cancelled_run_leaves_its_work_for_the_next`).

### 7.13 The acceptance client and the tests

A toy TeX-like language in `crates/phi/tests/toy/` (not TeX):
- tokens are words and `\commands` in a `Seq`; the document is an
  unfold over it;
- `\def\x{body}` and macros that rewrite the input; `\csname`;
- `\if<a><b> … \else … \fi` with only the taken arm read, as a nested
  unfold, and a φ node at the join for each name the arm defined;
- `{`/`}` groups with local definitions and `\global`;
- a running counter (`\step`), and the same as a prefix sum;
- words become a paragraph `Seq`, broken by a scan (a minimum-raggedness
  DP), whose lines feed a page scan;
- `\write{out}`/`\message` effects on two chains, a barrier;
- `\label{k}` publishes, `\ref{k}` reads the slot.

Tests (seeded PRNG, as the repo's other tests; no `proptest`):
- after random edit sequences (words, macros, conditionals flipping,
  names appearing and disappearing, groups moving), every node value,
  chain and slot equals a fresh build's;
- parallel equals sequential, at several widths and seeds;
- a field nobody reads changes: zero downstream evaluations, asserted
  exactly;
- a scan resumes at the edit and stops at convergence: the elements
  stepped, asserted exactly;
- a correctly predicted slot costs zero extra iterations;
- `deps`: `crates/phi/Cargo.toml` has no path or workspace dependency
  (no TeX or PDF crate can creep in).

Benches (criterion, `crates/phi/benches/phi.rs`): build+evaluate N
leaves; rebuild after one leaf edit; scan resume; parallel scaling;
memory per node (counted, not timed).

### 7.14 The text form

Every IR has a canonical text form that round-trips (5.3), and so does
the graph. `Graph::to_text` prints one node per line in position order,
indented by region:

```text
%12 = step #a41f(%3, @foo:%7) ; st=…        key, operands, names read and where they resolved
  %13 = leaf add(%12.1, 4) = 7               a child: op, operands (.f a field), value
  def \foo = %13 [g2]                        a definition and its group
```

Ops and values print and parse through `Lang` hooks (`fmt_op`,
`parse_op`, `fmt_val`, `parse_val`). `Dump::parse(to_text(g))` gives
the same text and values again, which a test checks after random edits.
It is the debugging view, and it comes with checkpoint 2.

### 7.15 Speculative entry, as built

- A segment is a private `Graph` run on a worker. Its root holds the
  unfold's input, a guessed initial state and the arguments, and its
  unfold starts at the entry's element and key and stops at the next
  entry's element.
- Names the segment does not define are *imports*. Their values come
  from a snapshot of every name as the main graph has it where the
  segments start (`ext`, shared by all workers). An import is an input
  defined at the segment's very start, so a local definition shadows
  it.
- Grafting copies the segment's steps and their subtrees into the main
  unfold after its last step, remapping:
  - node ids, group ids (which embed the opening step's id), name ids
    (by spelling), and the operands that pointed at the segment's root
    inputs (to the unfold's own operands);
  - each operand that read an import or an undefined name, resolved
    again at its place here. If the version differs, its reader is
    woken: that is the validation of reads made during speculation.
- Segments are grafted in order as they finish, while workers run the
  rest. Each boundary is then checked by the ordinary successor check.
  A segment whose chain ended (`Done`) ends the unfold there.
- Segments keep the main graph's slot and family predictions and never
  run the cross-run loop themselves.
- *Bulk-append grafting* (checkpoint 3a). The worker that ran a segment
  also packs it (`Graph::pack`). The nodes under its unfold are
  renumbered from 0 in their order, every reference inside is made
  relative, and values and the kinds' tables are moved out. What
  crosses the segment's edge is listed apart:
  - its operands that are the unfold's;
  - its reads by name from before it, with the version each read;
  - its readers and definitions by name.

  The main thread appends: headers with ids shifted, values extended,
  each name's readers and definitions appended in one go when they come
  after the list's last. Then it fixes the listed edge operands, the
  first step's state operand and the top steps' labels. The first
  segment's sizes reserve the tables for the rest. The graft costs
  about one store per byte the main graph keeps; that, with the page
  faults on fresh table memory, is what still bounds W = 8 (7.18).
- The parked step's group is remapped as well. Before that fix, a
  mismatched arrival resynchronised under a group id of another step,
  and a local definition outlived its group (found by the random test
  with three workers).

### 7.16 Measured (checkpoint 2)

Local and on a loaded machine, so ratios are indicative; accl numbers
come at checkpoint 3. Criterion benches are in `crates/phi/benches/phi.rs`.
The bench client: `i64` values versioned by value, `Add` leaves, an
unfold whose steps emit ten leaves and define one name.

| bench | time | per node | target |
|---|---|---|---|
| build 1 M root leaves (two operands each), cold | 93 ms | 93 ns | tens of ns: missed |
| build an unfold of 100 k steps × 12 nodes, cold | 263 ms | 220 ns | missed |
| rebuild after one leaf of 1 M changed | 0.11 µs | — | met |
| rebuild after one step of 100 k changed (1.2 M nodes) | 8.5 µs | — | met |
| scan resume after one element of 1 M changed | 28 µs | — | met |
| memory, root leaves | 144 B/node | | tens of B: missed |
| memory, unfold k = 10 | 206 B/node | | missed |

- *Cold build.* Where the cost goes (perf, root leaves):
  - set_opds 21% (operand and reverse-entry writes);
  - alloc 11%;
  - eval and set_val 20%;
  - page faults on fresh memory 10%.

  It is memory traffic: 144 B per node written. A step adds its own
  emission, key matching and definitions.

  Reaching tens of ns means writing fewer bytes:
  - a 32-byte header (`first` only for parents, `key` only under steps);
  - 8-byte operands, with the name in a side table;
  - reverse entries only where a reader is outside its source's step.

  This goes with the retained tiers.
- *Parallel cold build.* The toy client, 800 paragraphs at 4 workers:
  0.25 s → 0.115 s (2.2×), every segment accepted. With the bench
  client, whose steps are as cheap as grafting a node, there is no
  gain at 2–8 workers (209 ms → 216–233 ms): grafting copies each
  node, and on one thread that costs about as much as making it.
  Grafting by bulk column append (segment ids are contiguous: an
  offset, not a map) is the checkpoint-3 fix.
- *Rebuilds* are where the design pays. A changed element of a 100 k
  step unfold runs two steps, and a changed element of a 1 M scan steps
  to convergence, independent of size.

### 7.18 Measured (checkpoint 3a)

On accl (EPYC 7763, one node, 16 CPUs). The bench client is as in
7.16: a node is a logical node (each step's ten or twenty leaves, its
constant and the step), whether or not it is kept.

| | k = 10 | k = 20 | target |
|---|---|---|---|
| cold build, 100 k steps, sequential | 84.9 ms, 70.8 ns/node | 117 ms, 53.3 ns/node | ≤ 50 ns: missed |
| W = 2 | 48.3 ms (1.76×) | 64.5 ms (1.82×) | |
| W = 4 | 25.1 ms (3.38×) | 35.6 ms (3.29×) | ≥ 2.4×: met |
| W = 8 | 23.7 ms (3.58×) | 24.9 ms (4.71×) | ≥ 4.8×: missed |
| live memory | 29.7 B/node | 16.2 B/node | ≤ 32: met |
| retained (sealed, F = 16, compacted) | 1.7 B/node | 0.9 B/node | ≤ 16: met |
| sealing pass | 12.9 ms (10.8 ns/node) | 12.4 ms | |

- The toy client, 800 paragraphs: 88.7 ms in turn, 27.7 ms at W = 4
  (3.20×) and 15.7 ms at W = 8 (5.67×), equal to the sequential build.
- At 1 M steps (k = 10): 74 ns/node cold, 1.7 B/node retained,
  sealing 13.2 ns/node.
- *Sealed runs* (k = 10, 100 k): 6,177 runs, 16.2 steps on average,
  p99 65, max 117; 178 emissions on average, max 1,287.
- *Edits*, each changing one input element:
  - with sealing off: median 3 µs (2 steps);
  - a first edit in a sealed run: median 102 µs, max 487 µs (28 steps
    on average);
  - a second edit beside it: median 8 µs.

  At 1 M steps the first edit's median is 839 µs and its max 45 ms. The
  name index's sorted lists shift on each insertion, which is O(list):
  that is the checkpoint-3b item "reader runs in O(log R)".
- *Trade-off of transient interiors* (criterion, accl):
  - one changed step of 100 k: 5.13 µs at checkpoint 2, 3.86 µs now;
  - a name read by one leaf of a ten-leaf step: 3.78 µs (7.6 µs
    locally before);
  - one leaf of 1 M root leaves: 62 → 59 ns.

  The cost is that a changed leaf re-runs its step: ten leaves, not one.
- *Interiors* (`interior_sizes`): the bench client's step has 10
  transient emissions (max and p99). The toy's are the same for 10,
  100 and 1,000 words a paragraph.
- *What bounds the misses.*
  - Cold build: about 6,700 instructions a step at k = 10 (accl IPC
    about 2.5). A leaf costs about 450 instructions (emission record,
    eager evaluation, sweep visit). A step's fixed part is about 2,000:
    three name lookups, a node and its step record, its definition,
    operands, reverse edges and a reader entry. Page faults on fresh
    table memory are 5–10%.
  - W = 8: the main thread's graft, about 22 ms per 100 k steps. It
    writes about 390 bytes a step: two 64-byte headers, a 64-byte step
    record, values, operands, reverse edges, and a definition kept
    twice (step record and name index, 40 bytes each). The next steps
    are the byte diet: one definition record indexed by the name
    lists, a 40-byte step record, and narrower headers. They cut the
    graft and the cold build alike.

### 7.19 Measured (checkpoint 3b, the name index and the diet)

On accl, the same benches as 7.18.

| | before (7.18) | now | target |
|---|---|---|---|
| first edit in a sealed run, 1 M steps, max | 45 ms | 1.09 ms | ≤ 2 ms: met |
| the same, median | 839 µs | 132 µs | |
| second edit beside it, median | 40 µs | 9 µs | |
| cold build k = 10, sequential | 70.8 ns/node | 71.9 ns/node | ≤ 50: missed |
| cold build k = 20 | 53.3 | 56.0 | |
| W = 4, k = 10 | 3.38× | 3.29× | |
| W = 8, k = 10 | 3.58× | 6.29× | ≥ 4.8×: met |
| W = 8, k = 20 | 4.71× | 6.96× | |
| retained, 1 M steps sealed | 1.7 B/node | 1.5 B/node | |

- *The 45 ms* was not the name lists alone. At 1 M steps `Seq::diff`
  fell back to element-by-element comparison whenever a splice had
  changed a subtree's shape (69 ms for one edit). It now walks both
  trees with a stack per side and skips subtrees shared by pointer at
  any depth. A splice copies only its path, so that is O(log n). The
  rest was the O(R) list shifts (now runs) and the first edit after a
  compaction rebuilding the step index (compaction now keeps it).
- *W = 8:* the graft lost its per-read re-resolution: a segment's reads
  from before it all resolve as at its start, so each name is resolved
  once. Definitions now cost one 32-byte record plus a 4-byte index
  instead of 72 bytes.
- *Cold build:* 8,600 instructions per k = 10 step, at IPC about 3 on
  accl. Where they go:
  - emissions: about 320 each (12 per step: records, eager
    evaluation 80, sweep visit 36);
  - the step's own work: about 2,000 (its node and record, the
    successor's, operands and reverse edges, a definition, a reader
    entry, three name lookups at 125 each).

  50 ns/node would mean about 6,000 instructions a step. The remaining
  cuts are each a few percent (smaller emission records, a sweep that
  skips leaves done at emission, a cheaper `Args::get`), so it is not
  pursued until the TeX layer's step shapes say which costs matter.

### 7.20 Measured (checkpoint 3b, the rest: 10^8 nodes)

On accl (acclnode01, 16 CPUs), `prof` at 64e1af5. K10 is 12 nodes a
step and K20 is 22, so 8.4M K10 steps and 4.6M K20 steps are each
about 10^8 nodes.

| | K10, 8.4M steps | K20, 4.6M steps |
|---|---|---|
| cold, W=1 | 74.9 ns/node, 7.55 s | 57.6 ns/node, 5.83 s |
| cold, W=8 | 30.2 ns/node, 3.04 s (2.48x) | 15.8 ns/node, 1.59 s (3.65x) |
| peak RSS, W=1 / W=8 | 2.74 / 4.16 GB | 1.50 / 2.06 GB |

- Sealing all 8.4M K10 steps takes 1.85 s (18.3 ns/node) and makes
  519,252 runs (mean 16.2 steps, max 117). Retained: 1.5 B/node.
- First edit in a sealed run at 10^8 nodes: median 74 us, max 250 us
  (30 steps). Second edit beside it: median 7 us, max 116 us. At 1M
  steps: median 54 us, max 263 us. The edit cost no longer grows with
  the document.
- W=8 at 10^8 is below the 100k-step figure (6.29x, 7.19). The segments
  run in parallel, but the coordinator grafts them one after another,
  and at this size the graft and the memory bandwidth dominate. A
  parallel graft is the next lever if the 10^8 cold build matters.
- Preamble edit with parallel rounds (20k steps re-run, about 29 us a
  step): 589 ms at W=1, 199 ms at W=4, 111 ms at W=8 (5.3x). Every
  outcome was used (19,999 of 19,999).
- Memo, every Add memoized (1M steps): 136.6 ns/node against 77.5, at
  10M probes and 99.99% hits. A probe costs about 60 ns on accl.
- CSE, every Add probed (1M steps): 90.8 ns/node against 77.5. A probe
  costs about 13 ns on accl.

What the 10^8 runs found, all fixed:

1. **Reader lists rewrote their block starts on every change.** That
   is O(R/256) per insert or remove, so a name read by every step made
   each edit linear. perf at 4M steps: 37% in it. The starts are now a
   Fenwick tree over the block lengths. First edit at 10^8: median
   1148 -> 74 us.
2. **Compaction cut every table's capacity to its length.** The first
   edit after a seal then moved a whole table (12 ms at 10^8 nodes).
   Compaction now leaves an eighth of room, as address space that is
   not resident until used, and touches the next 2 MB at the seal.
   `mem()` counts lengths, so the retained figure is unchanged.
3. **The cold "regression" was the bench client.** Measured at 72.3 ->
   84.8 ns/node, it came from the client: an environment lookup per
   step and non-constant hooks per leaf. With part 1's client, the core
   is 72.3 -> 75.8 ns/node (K10) and 54.1 -> 55.3 (K20) on accl,
   +1.6% instructions. That is the cost of the rounds, cancellation,
   appends, memo and CSE plumbing.

### 7.21 Profiles and tier 2 (`profile.rs`, `graph/tune.rs`)

A `Graph<L, true>` profiles; `Graph<L>` (`P = false`, the default)
compiles the profile out.

**What a profile records.**

- Per op, by the op value and with `Lang::op_tag` as its stable
  identity:
  - leaf and scan-element evaluations and their mean cost;
  - steps run, and of those the ones run on an edit (after the first
    build);
  - memo store probes and hits (counted per tag by the store);
  - CSE probes and merges;
  - for watched ops, the evaluations whose key was seen before (the
    reuse a memo would find);
  - whether the op was ever emitted with a class other than `Pure`.
- Per region (a step's key): re-runs on edits, and the last run.

**How it is cheap.** Evaluations are sampled at random intervals, one in
128 on average.

- An unsampled evaluation costs an inlined countdown and a branch.
- A sampled one is timed (the clock's own cost is measured once and
  taken off) and stands for its interval, so counts are estimates and
  means are unbiased.
- Steps are counted exactly, through a one-op accumulator.
- `Graph::profile()` reports the ops by estimated total time and the 64
  regions that re-ran most.
- `snapshot()` gives a `Snapshot` keyed by tag, with a byte image
  (`phiprof1`). `load_profile` adds it to a later session: its ops count
  once seen by tag, its regions at once, and its memo decisions are
  applied by the next `tune`.

**Tier 2: `Graph::tune()`.** The client calls it when the scheduler is
idle; `run` never does, and with work queued it returns `busy`. Every
decision keeps results:

- A memoized op returns what it returned before for the same key: the
  op's tag and its operands' versions, and the op is pure.
- Sealing is exact (7.11).
- Check mode evaluates raw, never through the store.

The decisions:

- **Memo opt-in.**
  - A `Pure` leaf or scan op that costs at least `memo_probe_ns` (default
    100) is watched: every evaluation's key is looked at, at about one
    key hash each, and only for ops that cost more than a probe.
  - It is opted in when cost × reuse > `memo_probe_ns`, with
    `tune_min_samples` and `tune_min_keyed` as the evidence needed.
  - It is opted out when cost × the hit rate since opt-in falls below the
    probe cost; it then stays out for 64 runs.
  - The store gets `auto_memo_bytes` if it had no budget.
  - The membership test is a 64-bit filter, then a set.
- **Sealing by region.**
  - A region with at least `hot_runs` re-runs in the last 4 ×
    `seal_quiet` runs waits four times longer before sealing.
  - A region that re-ran once is sealed after `seal_quiet` / 8.
  - The hot queue is re-ordered by these quiet times, and what is due is
    sealed then and there, with the run end's pruning (`tidy`).

**Measured.** On accl, job 7549, `prof`:

| | K10, 1M steps | K20, 1M steps | K10, 8.4M steps (10^8 nodes) |
|---|---|---|---|
| main (b40d4bf) | 80.0 ns/node | 57.0 | 78.1 (off) |
| off | 81.8 | 61.9 | |
| on | 82.5 | 63.2 | 83.1 |

- On vs off: +0.3 to 1.2% at 1M steps, and +6% at 10^8.
- The off path had a +8% regression at K20. Its cause: once the hook
  gave the crate more callers of `Args::get`, LLVM stopped inlining it
  into client ops. It is now `inline(always)`; locally at K20,
  instructions are +2% against main and cycles are level.

Auto opt-in, with a preamble edit that re-runs 20k steps whose leaves
cost n ns more (`LEAFHEAVY`):

| n | before | after one tune |
|---|---|---|
| 100 | 50 ms | 39 ms |
| 300 | 117 ms | 39 ms |
| 1000 | 330 ms | 38 ms |

The run in which the op is watched costs more (keys): 66 ms at n = 100.

In the toy's random-edit test (40 seeds × 60 edits), a profiled and
tuned document with check mode on makes 80 opt-ins and 7 opt-outs and
seals 4281 steps, and equals the plain one after every edit.

What the tests found:

1. The tuned document's check-mode failure was `tune`'s sealing
   compacting without the run end's pruning. The public `Graph::seal()`
   had the same pattern; both now go through `tidy`.
2. A pathological toy macro made one random-edit seed take minutes. The
   toy now has a bound on pending input.

### 7.22 Queries, named sources and continuation calls (for the TeX layer)

**Reads after the step's own close.** `cx.read` after a group the step
closed resolves past the close, as a leaf's operand does (the TeX
layer's patch, taken as is).

**Unrecorded queries** for the TeX layer's frontier (its engine arrays
at a run's start):

- `cx.defined_reaching()` lists every name a definition reaches, with
  its value as `read` gives it. Groups and `\global` count, the step's
  own definitions are included, and in a segment the parent's names are
  listed too.
- `cx.here()` and `cx.defined_since(p0)` list the names whose reaching
  definition may differ since an earlier step, each with its value here
  (`None`: nothing reaches). p0 and here may be in different unfolds,
  across calls and returns: the walk goes by the two steps' paths from
  the root unfold, from where they part down to here. That is:
  - names defined from p0 on, nested unfolds included;
  - names defined in a group closed since, so a local definition made
    before p0 whose group closed is in the list.

  It is `None` if p0's step is gone; then use `defined_reaching`. Cost:
  the steps walked (a sealed run counts as one) plus their definitions.
- Neither query records a read: the step depends only on what it then
  reads.

**Named sources.**

- `g.source(key, value)` makes an input that is defined to a reserved
  name at the root (before everything); later edits are `g.set` on its
  node.
- `g.remove_source(key)` removes the definition.
- `cx.source(key)` reads that name. It is `None` when the source is
  absent, and the read is recorded either way, so a file appearing,
  changing or going wakes its readers (`\IfFileExists`, `\openin`).
- A source's elements (`\read`) are a read of the same name.

**Continuation calls.**

- `cx.call(op, input, init, args)` emits a nested unfold, and the step
  returns `Step::Call { key }`. The unfold runs in the step's sweep, and
  its result is the step's value and the state its successor starts
  from.
- The step also takes the unfold as an operand. An edit inside the file
  (the source's input) resumes the nested unfold at the edit, as any
  unfold resumes. If its result changed, the call step runs again (one
  step) and the document goes on after it until it converges; if not,
  nothing after the call runs.
- `Step` has no `Arg` (that would put a lifetime on `Lang::step`'s
  return type), hence `cx.call` plus `Step::Call`.
- Speculative entry works across calls. A book's chapters that are
  `\input`s build in parallel from root-level entries, and the result
  equals the sequential build. There is no speculation within one called
  file yet.
- Sealing never takes a step with an unfold child, so a call step stays
  live.

**What the tests found.** A segment packed its definitions with a step's
own before its call's steps'. In position order the call's come first,
so the name index was left unsorted and removed steps' definitions
stayed in it. The pack now sorts each name's definitions by position.

**Tests.**

- `an_input_file_is_a_call_the_document_resumes_after`, with check mode
  on:
  - an edit inside the file re-runs at most 6 steps;
  - an open conditional at the file's end resumes the document;
  - a file appearing and going wakes `\iffileexists`;
  - each state equals a fresh build.
- `chapters_called_from_a_book_build_in_parallel_as_in_turn`.
- The random-edit test's documents can `\input` two files, one with an
  open conditional, and test `\iffileexists`.

**Known, not yet fixed (on main too).** A cross-run slot can stay stale
after an edit. `\ref{k}` reads Unit where a fresh build reads 1, with
one random snippet set: seed 5, edit 51. The repro is in HANDOFF.md.

### 7.17 Dependencies

- `criterion` (dev only, default features off): asked for, and the
  repo has no bench harness to reuse.
- Nothing else. Threads are `std::thread::scope` and channels, the
  PRNG is a xorshift, hashing is the copied `Stable`, serialization is
  the client's `Codec` over a small framed format.

---

