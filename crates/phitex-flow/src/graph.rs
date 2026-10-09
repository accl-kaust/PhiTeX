//! The graph: a node per command.
//!
//! The text is cut at blank lines (brace depth 0) into **pieces**: places
//! where reading can start, because the reader state there is known
//! (vertical mode, no group, no conditional, no rewritten input). Reading
//! a piece makes a chain of **nodes, one per command**. A node's operands
//! are the reader state it started from (its predecessor's output:
//! position, tokenizer state, rewritten input, mode, group depth, open
//! conditionals) and the exact definitions it read, each from the node
//! that made it: a node earlier in the piece ([`Src::Node`]), a node of an
//! earlier piece ([`Src::Piece`]), or the value before any definition
//! ([`Src::Initial`]). Its results are its definitions, what it emitted
//! and the reader state after it. A macro call is a step inside the node
//! whose command it produces: it rewrites the input and defines nothing.
//!
//! Names resolve over the nodes: the definition of a key reaching node
//! `i` is the latest before `i` still in scope (its group not closed
//! before `i`, or global). A `}` is a node that defines nothing. At `\fi`
//! (or the `\else` ending the taken arm) a node defines each key the arm
//! defined as an explicit φ of the value after the arm and the value
//! before the `\if`. `\par` is a node whose operands are the material the
//! paragraph's nodes emitted and the parameters it read; its result is the
//! paragraph, a value line breaking is a function of.
//!
//! [`run_piece`] runs a piece again from exactly the nodes whose operands
//! may have changed: the dirty nodes (every node reading a name from before
//! the piece whose definition was remade) and the nodes the edited text
//! touches. Everything before the first of them is kept as it is. From
//! there each old node is kept when its input state is the state reached,
//! its text is unchanged and every definition it read is still equal;
//! otherwise a node runs. Once the chain is back on the old one at the same
//! index with no scope changed, old nodes are shared as they are up to the
//! next dirty node.

use std::sync::Arc;

use crate::hash::{FxMap, FxSet};
pub use crate::item::{Item, Para};
use crate::page::IGNORE_DEPTH;
use crate::tfm::Font;
use crate::val::{DimenPar, IntPar, Key, Macro, Meaning, Prim, Tok, Val};

mod assign;
mod boxes;
pub(crate) mod diag;
mod etex;
mod expand;
mod fonts;
mod io;
mod lang;
mod material;
mod math;
mod scan;

pub use expand::JOBNAME;
pub use io::{ends_document, file_text, page_io, whatsits};

/// A piece's identity, stable across edits.
pub type PieceId = u32;

/// Where a piece's reads of names from before it are answered.
pub trait Env {
    /// The definition of `key` reaching node `node` of this piece from
    /// before the piece (or from an element between its nodes), where it
    /// was made, and the nodes it reaches, `lo..hi` (`0..u32::MAX`: all of
    /// them, as when no element between the piece's nodes defines `key`).
    fn reach(&mut self, key: &Key, node: u32) -> (Val, Src, u32, u32);

    /// The bytes of the TFM file `name.tfm`, if there is one (a pure
    /// function of the name during a build).
    fn font_file(&mut self, _name: &str) -> Option<std::sync::Arc<[u8]>> {
        None
    }

    /// The bytes of the input file of exactly this name, if there is one:
    /// what a read of `Key::File(name)` answers (a pure function of the
    /// name during a build).
    fn file(&mut self, _name: &str) -> Option<Arc<[u8]>> {
        None
    }

    /// The job's name (`\jobname`): what a read of `Key::Job` answers.
    fn jobname(&mut self) -> Arc<str> {
        Arc::from(expand::JOBNAME)
    }

    /// The lines of its file before this piece (`Key::LineBase`).
    fn line_base(&mut self) -> i32 {
        0
    }
}

/// The tokenizer's state (tex.web 303).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TState {
    N,
    M,
    S,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    V,
    H,
    /// Math (`RState::inner`: an inline formula or a subformula; else a
    /// display).
    M,
}

/// What opened a group (only to report mismatches: the scoping itself is
/// the reaching definitions over the `Open`/`Close` nodes).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GroupKind {
    /// `{`.
    Simple,
    /// `\begingroup`.
    SemiSimple,
    /// A box's or a discretionary's braces (`Group::boxg`).
    Box,
    /// A formula, a subformula, `\left`…`\right`, a `\mathchoice` part
    /// (`Group::boxg`, `math::MathKind`).
    Math,
}

/// An open conditional: the node whose step opened it, which ends may end
/// it now (tex.web 489: 1 while its test is read, 2 `\fi`, 3 `\else` or
/// `\fi`, 4 `\or` too), and its test.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Cond {
    open: u32,
    limit: u8,
    test: bool,
    /// Which conditional (`IfKind::code`, plus 32 after `\unless`:
    /// etex.web's `cur_if`), for `\currentiftype`.
    code: u8,
}

/// A text the reader is in above the piece's (TeX's input levels that
/// read characters, as a value of the reader state): e-TeX's
/// `\scantokens` text, an `\input` file (read in the piece until the core
/// cuts files into pieces: CORE.md, "`\input` (decided)") or a `\read`
/// line. Read with the category codes in force as it is read, line by
/// line, each line ending in the `\endlinechar` in force when it is begun.
/// It lies above the first `at` rewritten token lists: those above it are
/// read first, then it, then those below. Where its reading is: `pos`,
/// `tstate`, `eol` (as the piece's own are kept in `RState`); `eof`: its
/// end was met once and `\everyeof` went in (etex.web §362); `endinput`:
/// `\endinput` was met, it ends with its current line (tex.web 362).
#[derive(Clone, PartialEq, Eq, Debug)]
struct PendingText {
    text: Arc<[char]>,
    at: usize,
    pos: usize,
    tstate: TState,
    eol: i16,
    eof: bool,
    kind: TextKind,
    endinput: bool,
}

/// What a [`PendingText`] is.
#[derive(Clone, PartialEq, Eq, Debug)]
enum TextKind {
    /// `\scantokens`'s pseudo file.
    Scan,
    /// An `\input` file, as the log names it (`./sub.tex`): its end is
    /// `)` in the log.
    File(Arc<str>),
    /// A `\read` line: its end ends the reading (`next_raw` is `None`);
    /// `\read` leaves it.
    Read,
}

/// An open group: its kind and the tokens `\aftergroup` gave it (its
/// close rewrites the input with them).
#[derive(Clone, PartialEq, Eq, Debug)]
struct Group {
    kind: GroupKind,
    after: Vec<Tok>,
    /// A box group's specification and context (tex.web 1083).
    boxg: Option<Arc<material::BoxGroup>>,
}

/// A stack in the reader state, shared until changed: handing the state
/// from a node to the next copies nothing.
#[derive(Clone, Debug)]
struct Stack<T>(Option<Arc<Vec<T>>>);

impl<T: Clone> Stack<T> {
    const fn new() -> Self {
        Stack(None)
    }
    fn len(&self) -> usize {
        self.0.as_ref().map_or(0, |v| v.len())
    }
    fn is_empty(&self) -> bool {
        self.0.is_none()
    }
    fn last(&self) -> Option<&T> {
        self.0.as_ref().and_then(|v| v.last())
    }
    fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        self.0.as_mut().and_then(|v| Arc::make_mut(v).get_mut(i))
    }
    fn last_mut(&mut self) -> Option<&mut T> {
        let n = self.len();
        n.checked_sub(1).and_then(|i| self.get_mut(i))
    }
    fn push(&mut self, x: T) {
        Arc::make_mut(self.0.get_or_insert_with(Default::default)).push(x);
    }
    fn pop(&mut self) -> Option<T> {
        let v = Arc::make_mut(self.0.as_mut()?);
        let x = v.pop();
        if v.is_empty() {
            self.0 = None;
        }
        x
    }
}

impl<T: PartialEq> PartialEq for Stack<T> {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        }
    }
}

impl<T: Eq> Eq for Stack<T> {}

/// The value `RState::eol` holds before the line's end-of-line character
/// has been read.
const EOL_UNREAD: i16 = i16::MIN;

/// The reader state between two commands: a value, the next node's
/// operand.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RState {
    pub pos: usize,
    tstate: TState,
    /// Token lists the input was rewritten with, each with how far it has
    /// been read (innermost last).
    /// (and whether it was backed up, tex.web 325's `backed_up`: an error's
    /// context shows it as such; a list read to its end stays until the
    /// next token is read, as TeX's input level does)
    pending: Vec<(Arc<[Tok]>, usize, bool)>,
    mode: Mode,
    /// The list being built is internal: a box's (TeX's negative modes).
    inner: bool,
    /// Boxes open: the level of the list being built (0: the main vertical
    /// list and its paragraphs).
    level: u16,
    /// A box group just closed: what its packaging node reads.
    boxing: Option<Arc<material::BoxEnd>>,
    /// A `\global` prefix is waiting for its assignment.
    global: bool,
    /// Conditionals open (innermost last).
    conds: Stack<Cond>,
    /// `\long`, `\outer` prefixes waiting for their definition.
    long: bool,
    outer: bool,
    /// The groups open, innermost last.
    groups: Stack<Group>,
    /// The token `\afterassignment` gave: the next assignment rewrites the
    /// input with it.
    after_assignment: Option<Box<Tok>>,
    /// The list being built's `\spacefactor`, `\prevdepth`, `\prevgraf`.
    space_factor: i32,
    prev_depth: i32,
    prev_graf: i32,
    /// The current line's end-of-line character (`\endlinechar` as the line
    /// was begun; -1: none), or `EOL_UNREAD`.
    eol: i16,
    /// e-TeX's `\protected` prefix is waiting for its definition.
    protected: bool,
    /// The texts the reader is in above the piece's (`\scantokens`
    /// texts, `\input` files, `\read` lines), innermost last.
    texts: Stack<PendingText>,
    /// A paragraph's language state (tex.web 1091, 1376): `clang`, and
    /// the language and hyphen minima it began with.
    hyph_lang: [u8; 4],
}

impl RState {
    fn start() -> RState {
        RState {
            pos: 0,
            tstate: TState::N,
            pending: Vec::new(),
            mode: Mode::V,
            inner: false,
            level: 0,
            boxing: None,
            global: false,
            conds: Stack::new(),
            long: false,
            outer: false,
            groups: Stack::new(),
            after_assignment: None,
            space_factor: 1000,
            prev_depth: IGNORE_DEPTH,
            prev_graf: 0,
            eol: EOL_UNREAD,
            protected: false,
            texts: Stack::new(),
            hyph_lang: [0, 0, 1, 1],
        }
    }
}

/// What kind of command a node was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Other,
    /// `{`: opens a scope.
    Open,
    /// `}`: closes one (and defines nothing).
    Close,
    /// An `\if...` whose taken arm follows (the conditional is open).
    If,
    /// An `\if...` that skipped to its `\fi` (closed at once).
    IfClosed,
    /// `\else` or `\fi`: ends the conditional, with its φ.
    EndIf,
    /// `\par`: a paragraph.
    Par,
    /// The end of the piece.
    End,
}

impl Kind {
    /// Whether the node changes how later names resolve.
    fn scopes(self) -> bool {
        matches!(
            self,
            Kind::Open | Kind::Close | Kind::If | Kind::IfClosed | Kind::EndIf
        )
    }
}

/// Where a definition a node read was made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Src {
    /// Node `.0` of the same piece.
    Node(u32),
    /// A node of piece `.0`.
    Piece(PieceId),
    /// The value before any definition.
    Initial,
}

impl RState {
    /// Equal but for the position.
    fn eq_but_pos(&self, o: &RState) -> bool {
        self.tstate == o.tstate
            && self.mode == o.mode
            && self.inner == o.inner
            && self.level == o.level
            && self.boxing == o.boxing
            && self.global == o.global
            && self.conds == o.conds
            && self.long == o.long
            && self.outer == o.outer
            && self.groups == o.groups
            && self.after_assignment == o.after_assignment
            && self.space_factor == o.space_factor
            && self.prev_depth == o.prev_depth
            && self.prev_graf == o.prev_graf
            && self.eol == o.eol
            && self.pending == o.pending
            && self.protected == o.protected
            && self.texts == o.texts
            && self.hyph_lang == o.hyph_lang
    }

    /// Whether a node going from `self` to `out` begins a paragraph of the
    /// main vertical list. (Inside a box no paragraph start resets what
    /// changed: the box's packaging reads all its nodes.)
    fn starts_para(&self, out: &RState) -> bool {
        self.mode == Mode::V
            && out.mode == Mode::H
            && self.level == 0
            && out.level == 0
            && !out.inner
    }
}

/// A read: an operand.
#[derive(Clone, Debug)]
pub struct Read {
    pub key: Key,
    pub src: Src,
    pub val: Val,
    /// The scope of the definition read, as the reading node sees it
    /// ([`Chain::scope_of`]): what a definition at the level of the one it
    /// replaces (`Def::level`) is made from.
    pub scope: Option<u16>,
}

/// A shared [`Read`].
pub type ReadRef = Arc<Read>;

impl Read {
    /// Whether `r` read the definition this one did (by identity, then
    /// equality of the value and of its scope).
    fn same(this: &ReadRef, r: &ReadRef) -> bool {
        Arc::ptr_eq(this, r) || (this.val == r.val && this.scope == r.scope)
    }
}

/// A definition: a result.
#[derive(Clone, Debug)]
pub struct Def {
    pub key: Key,
    pub val: Val,
    pub global: bool,
    /// A definition made at the level of the one it replaces (TeX's
    /// assignments without the save stack, CORE.md Conflict 7): `Some(k)`,
    /// it lives as long as the `k`-th group enclosing its node's own group
    /// is open (`Some(0)`: its own group, as `None`). Counted from the
    /// node, not an index, so it survives nodes moving.
    pub level: Option<u16>,
}

/// A φ at the end of a conditional: the test, and per key the value after
/// the taken arm and the value before the `\if`.
#[derive(Clone, Debug)]
pub struct Phi {
    pub test: bool,
    pub keys: Vec<(Key, Val, Val)>,
}

/// A short list: up to two items in place (what most nodes read and
/// emit), more on the heap.
#[derive(Clone, Debug, Default)]
pub enum Few<T> {
    #[default]
    Zero,
    One(T),
    Two([T; 2]),
    Many(Box<[T]>),
}

impl<T> Few<T> {
    /// Takes the items out of `v` (which keeps its capacity).
    fn take(v: &mut Vec<T>) -> Few<T> {
        match v.len() {
            0 => Few::Zero,
            1 => Few::One(v.pop().expect("one item")),
            2 => {
                let b = v.pop().expect("two items");
                let a = v.pop().expect("two items");
                Few::Two([a, b])
            }
            _ => Few::Many(v.drain(..).collect()),
        }
    }
}

impl<T> std::ops::Deref for Few<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        match self {
            Few::Zero => &[],
            Few::One(x) => std::slice::from_ref(x),
            Few::Two(a) => a,
            Few::Many(b) => b,
        }
    }
}

impl<T> std::ops::DerefMut for Few<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        match self {
            Few::Zero => &mut [],
            Few::One(x) => std::slice::from_mut(x),
            Few::Two(a) => a,
            Few::Many(b) => b,
        }
    }
}

/// What few nodes have: definitions, a paragraph (or the vertical
/// material it carries), errors, a φ, vertical material inside a box,
/// what it wrote to the log.
#[derive(Clone, Debug, Default)]
pub struct Rare {
    pub defs: Vec<Def>,
    pub para: Option<Arc<Para>>,
    pub errors: Vec<String>,
    pub phi: Option<Box<Phi>>,
    pub vemit: Vec<Item>,
    pub log: Option<Box<str>>,
}

/// A command node. Its input state is its predecessor's `out` (the
/// piece's start for the first node): [`Chain::input`].
#[derive(Clone, Debug)]
pub struct Node {
    pub kind: Kind,
    /// The innermost group open at it (its `Open` node's index), if any.
    group: Option<u32>,
    /// The command, for printing.
    pub cmd: Tok,
    /// The reader state after it (its successor's input).
    pub out: RState,
    pub reads: Few<ReadRef>,
    /// The items it emitted into the horizontal list being built (at
    /// `out.level`); into the vertical list inside a box: `vemit` (at the
    /// outer level they are carried by `para`).
    pub emit: Few<Item>,
    pub rare: Option<Box<Rare>>,
}

impl Node {
    pub fn defs(&self) -> &[Def] {
        self.rare.as_ref().map_or(&[], |r| &r.defs)
    }
    pub fn para(&self) -> Option<&Arc<Para>> {
        self.rare.as_ref().and_then(|r| r.para.as_ref())
    }
    pub fn errors(&self) -> &[String] {
        self.rare.as_ref().map_or(&[], |r| &r.errors)
    }
    pub fn phi(&self) -> Option<&Phi> {
        self.rare.as_ref().and_then(|r| r.phi.as_deref())
    }
    pub fn vemit(&self) -> &[Item] {
        self.rare.as_ref().map_or(&[], |r| &r.vemit)
    }
    /// What it wrote to the log (`\message`; `\immediate\write`, ending
    /// in a newline), the diagnostic value.
    pub fn log(&self) -> Option<&str> {
        self.rare.as_ref().and_then(|r| r.log.as_deref())
    }
}

/// A stretch of a chain's nodes, shared with the chain it was made in.
#[derive(Clone, Debug)]
struct Seg {
    v: Arc<Vec<Arc<Node>>>,
    start: u32,
    end: u32,
}

/// A chain's nodes: stretches shared with the chains that made them (a
/// kept stretch is shared by one pointer, not node by node), then the
/// nodes added since.
#[derive(Clone, Debug, Default)]
pub struct Nodes {
    segs: Vec<Seg>,
    /// Where each segment ends in this chain.
    ends: Vec<u32>,
    tail: Vec<Arc<Node>>,
}

/// Past this many stretches, a chain's nodes are made one again.
const MAX_SEGS: usize = 16;

impl Nodes {
    pub fn len(&self) -> usize {
        self.in_segs() + self.tail.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn in_segs(&self) -> usize {
        self.ends.last().map_or(0, |&e| e as usize)
    }

    pub fn get(&self, i: usize) -> &Arc<Node> {
        let b = self.in_segs();
        if i >= b {
            return &self.tail[i - b];
        }
        let s = self.ends.partition_point(|&e| e as usize <= i);
        let lo = if s == 0 { 0 } else { self.ends[s - 1] as usize };
        let seg = &self.segs[s];
        &seg.v[seg.start as usize + i - lo]
    }

    fn push(&mut self, n: Arc<Node>) {
        self.tail.push(n);
    }

    /// The nodes added since become a stretch others can share.
    fn freeze(&mut self) {
        if self.segs.len() > MAX_SEGS {
            let all: Vec<Arc<Node>> = self.iter().cloned().collect();
            self.segs.clear();
            self.ends.clear();
            self.tail = all;
        }
        if self.tail.is_empty() {
            return;
        }
        let v = Arc::new(std::mem::take(&mut self.tail));
        let end = v.len() as u32;
        self.add_seg(Seg { v, start: 0, end });
    }

    fn add_seg(&mut self, s: Seg) {
        let len = s.end - s.start;
        if len == 0 {
            return;
        }
        if let Some(last) = self.segs.last_mut()
            && Arc::ptr_eq(&last.v, &s.v)
            && last.end == s.start
        {
            last.end = s.end;
            *self.ends.last_mut().expect("a segment") += len;
            return;
        }
        let e = self.in_segs() as u32 + len;
        self.segs.push(s);
        self.ends.push(e);
    }

    /// Appends `other`'s nodes `a..b`, shared (`other` is frozen).
    fn share(&mut self, other: &Nodes, a: usize, b: usize) {
        debug_assert!(other.tail.is_empty(), "a frozen chain");
        if a >= b {
            return;
        }
        self.freeze();
        let mut lo = 0usize;
        for (s, &e) in other.segs.iter().zip(&other.ends) {
            let e = e as usize;
            let (x, y) = (a.max(lo), b.min(e));
            if x < y {
                self.add_seg(Seg {
                    v: s.v.clone(),
                    start: s.start + (x - lo) as u32,
                    end: s.start + (y - lo) as u32,
                });
            }
            lo = e;
            if lo >= b {
                break;
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Node>> {
        self.range(0, self.len())
    }

    /// Nodes `a..b`.
    pub fn range(&self, a: usize, b: usize) -> impl Iterator<Item = &Arc<Node>> {
        let mut lo = 0usize;
        let segs = self.segs.iter().zip(&self.ends).filter_map(move |(s, &e)| {
            let e = e as usize;
            let (x, y) = (a.max(lo), b.min(e));
            let r = (x < y).then(|| &s.v[s.start as usize + x - lo..s.start as usize + y - lo]);
            lo = e;
            r
        });
        let (base, n) = (self.in_segs(), self.tail.len());
        let ta = a.clamp(base, base + n) - base;
        let tb = b.clamp(base, base + n) - base;
        segs.flatten().chain(self.tail[ta..tb.max(ta)].iter())
    }
}

impl std::ops::Index<usize> for Nodes {
    type Output = Arc<Node>;
    fn index(&self, i: usize) -> &Arc<Node> {
        self.get(i)
    }
}

/// A piece's chain of nodes, with what resolving names over it needs (each
/// index a cache of the nodes).
#[derive(Clone, Debug, Default)]
pub struct Chain {
    pub nodes: Nodes,
    /// Nodes defining each key, in order.
    defs_of: FxMap<Key, Vec<u32>>,
    /// For each `Open` node, the index of its `Close`, once there is one.
    close_of: FxMap<u32, u32>,
    open: Vec<u32>,
    /// Every `Open` and `Close` node, in order.
    scopes: Vec<u32>,
    /// The nodes with a paragraph, and with errors.
    para_at: Vec<u32>,
    err_at: Vec<u32>,
    /// For each `Open` node, the `Open` node of the group enclosing it.
    parent_of: FxMap<u32, u32>,
}

impl Chain {
    /// The input state of node `i` (`i == len`: of the next node).
    pub fn input(&self, i: usize) -> RState {
        if i == 0 {
            RState::start()
        } else {
            self.nodes[i - 1].out.clone()
        }
    }

    /// The input position of node `i`.
    fn input_pos(&self, i: usize) -> usize {
        if i == 0 { 0 } else { self.nodes[i - 1].out.pos }
    }

    /// Whether node `i` begins a paragraph of the main vertical list.
    fn starts_para(&self, i: usize) -> bool {
        let out = &self.nodes[i].out;
        if i == 0 {
            RState::start().starts_para(out)
        } else {
            self.nodes[i - 1].out.starts_para(out)
        }
    }

    /// The definition of `key` reaching node `i` (`i == len`: the next
    /// node): the latest still in scope.
    pub fn resolve(&self, key: &Key, i: usize) -> Option<(u32, Val)> {
        let ds = self.defs_of.get(key)?;
        for &d in ds.iter().rev() {
            if d as usize >= i {
                continue;
            }
            let n = &self.nodes[d as usize];
            let Some(def) = n.defs().iter().rev().find(|x| x.key == *key) else {
                continue;
            };
            let closed = self
                .def_group(n, def)
                .is_some_and(|g| self.close_of.get(&g).is_some_and(|&c| (c as usize) < i));
            if def.global || !closed {
                return Some((d, def.val.clone()));
            }
        }
        None
    }

    /// The group a definition of node `n` is local to (its `Open` node;
    /// `None`: the piece's outer level).
    pub fn def_group(&self, n: &Node, def: &Def) -> Option<u32> {
        let mut g = n.group;
        for _ in 0..def.level.unwrap_or(0) {
            g = g.and_then(|x| self.parent_of.get(&x).copied());
        }
        g
    }

    /// The scope of node `d`'s definition of `key` seen from the next
    /// node: `Some(k)` local to the group `k` levels out from the innermost
    /// one open; `None` global, or made at the piece's outer level.
    pub fn scope_of(&self, d: u32, key: &Key) -> Option<u16> {
        let n = &self.nodes[d as usize];
        let def = n.defs().iter().rev().find(|x| x.key == *key)?;
        if def.global {
            return None;
        }
        let g = self.def_group(n, def)?;
        let p = self.open.iter().rposition(|&o| o == g)?;
        Some((self.open.len() - 1 - p) as u16)
    }

    /// The groups open at the next node, outermost first (their `Open`
    /// nodes).
    pub fn open_groups(&self) -> &[u32] {
        &self.open
    }

    /// A node about to be added, given the group it is in.
    fn with_group(&self, mut n: Node) -> Arc<Node> {
        n.group = self.open.last().copied();
        if n.kind == Kind::Close {
            // (a close belongs to the group it closes' parent)
            n.group = self.open.len().checked_sub(2).map(|i| self.open[i]);
        }
        Arc::new(n)
    }

    /// Adds a node whose `group` is already right (kept at its old index).
    fn push_arc(&mut self, n: Arc<Node>) {
        let i = self.nodes.len() as u32;
        self.index(&n, i);
        self.nodes.push(n);
    }

    /// Node `n`, at `i`, in the indices.
    fn index(&mut self, n: &Node, i: u32) {
        match n.kind {
            Kind::Open => {
                if let Some(&p) = self.open.last() {
                    self.parent_of.insert(i, p);
                }
                self.open.push(i);
                self.scopes.push(i);
            }
            Kind::Close => {
                if let Some(o) = self.open.pop() {
                    self.close_of.insert(o, i);
                }
                self.scopes.push(i);
            }
            _ => {}
        }
        if let Some(r) = &n.rare {
            for d in &r.defs {
                self.defs_of.entry(d.key.clone()).or_default().push(i);
            }
            if r.para.is_some() {
                self.para_at.push(i);
            }
            if !r.errors.is_empty() {
                self.err_at.push(i);
            }
        }
    }

    /// The first `from` nodes of `old`, shared.
    fn prefix(old: &Chain, from: usize) -> Chain {
        let mut c = Chain::default();
        c.share(old, 0, from);
        c
    }

    /// Appends `old`'s nodes `a..b`, shared, at the same indices (the chain
    /// has `a` nodes, and how names resolve at `a` is the same in both):
    /// their entries in `old`'s indices are copied, not found again.
    fn share(&mut self, old: &Chain, a: usize, b: usize) {
        debug_assert_eq!(self.nodes.len(), a);
        if a >= b {
            return;
        }
        self.nodes.share(&old.nodes, a, b);
        let within = |v: &[u32]| {
            let s = v.partition_point(|&i| (i as usize) < a);
            let e = v.partition_point(|&i| (i as usize) < b);
            s..e
        };
        for (k, v) in &old.defs_of {
            let r = within(v);
            if !r.is_empty() {
                self.defs_of
                    .entry(k.clone())
                    .or_default()
                    .extend_from_slice(&v[r]);
            }
        }
        for &i in &old.scopes[within(&old.scopes)] {
            if old.nodes[i as usize].kind == Kind::Open {
                if let Some(&p) = self.open.last() {
                    self.parent_of.insert(i, p);
                }
                self.open.push(i);
            } else if let Some(o) = self.open.pop() {
                self.close_of.insert(o, i);
            }
            self.scopes.push(i);
        }
        self.para_at
            .extend_from_slice(&old.para_at[within(&old.para_at)]);
        self.err_at
            .extend_from_slice(&old.err_at[within(&old.err_at)]);
    }
}

/// What running a piece produced.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub chain: Chain,
    /// Every definition read from before the piece: (key, the first node
    /// reading it, value), one entry per key and value, sorted by key then
    /// node. A key read with two values (a node kept from an earlier run
    /// and one run now) has two entries, and validating them all finds the
    /// stale one.
    pub reads: Vec<(Key, u32, Val)>,
    /// Definitions reaching the piece's end, sorted by key.
    pub exports: Vec<(Key, Val)>,
    pub paras: Vec<Arc<Para>>,
    pub errors: Vec<String>,
    /// Nodes run this time (the others were kept).
    pub ran: u64,
    /// The fonts this piece loads first (`Key::FontLoaded` definitions), in
    /// node order: the document's font numbering is these, piece by piece.
    pub fonts: Vec<Arc<str>>,
}

/// The `Open` node of the `k`-th group enclosing node `i` (0: its own
/// innermost), for the log's line numbers.
pub fn enclosing_open(chain: &Chain, i: usize, k: usize) -> Option<usize> {
    let mut g = if i < chain.nodes.len() { chain.nodes[i].group } else { return None };
    for _ in 0..k {
        g = g.and_then(|x| chain.parent_of.get(&x).copied());
    }
    g.map(|x| x as usize)
}

/// The node that began the paragraph at `level` that node `i` is in.
pub fn paragraph_start_at(chain: &Chain, i: usize, level: u16) -> usize {
    material::para_start(chain, i, level)
}

/// The node that began the paragraph node `i` of `chain` ended (or is
/// in): for the log's line numbers.
pub fn paragraph_start(chain: &Chain, i: usize) -> usize {
    material::para_start(chain, i, chain.nodes[i].out.level)
}

/// Cuts the source into pieces at blank lines at brace depth 0. Lines lose
/// their trailing spaces and end in `\r` (TeX's end of line).
pub fn cut(src: &str) -> Vec<Arc<[char]>> {
    cut_from(src, 0, |_| false).0
}

/// [`cut`] from byte `from` of `src` (0, or the start of a piece's first
/// line: the cut there is the same as from the start), giving each piece
/// with the byte offset of its first line. `stop(p)` is asked at each
/// piece start `p` after the first; if it says so, the cut ends before it
/// and `p` is returned too.
pub fn cut_from(
    src: &str,
    from: usize,
    mut stop: impl FnMut(usize) -> bool,
) -> (Vec<Arc<[char]>>, Vec<usize>, Option<usize>) {
    let mut pieces: Vec<Arc<[char]>> = Vec::new();
    let mut starts = vec![from];
    let mut cur: Vec<char> = Vec::new();
    let mut depth: i32 = 0;
    let mut blank_run = false;
    let mut off = from;
    let mut lines = src[from..].split('\n').peekable();
    while let Some(raw) = lines.next() {
        let line_start = off;
        off += raw.len() + 1;
        if raw.is_empty() && lines.peek().is_none() {
            // (the source's final newline)
            break;
        }
        let line = raw.trim_end_matches([' ', '\r']);
        let blank = line.trim_start_matches(' ').is_empty();
        if blank {
            blank_run = true;
        } else {
            if blank_run && depth == 0 && cur.iter().any(|c| !c.is_whitespace()) {
                pieces.push(Arc::from(std::mem::take(&mut cur)));
                if stop(line_start) {
                    return (pieces, starts, Some(line_start));
                }
                starts.push(line_start);
            }
            blank_run = false;
            depth = brace_depth(line, depth);
        }
        cur.extend(line.chars());
        cur.push('\r');
    }
    if cur.is_empty() {
        starts.truncate(pieces.len());
    } else {
        pieces.push(Arc::from(cur));
    }
    (pieces, starts, None)
}

fn brace_depth(line: &str, mut depth: i32) -> i32 {
    let mut it = line.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => {
                it.next();
            }
            '%' => break,
            '{' => depth += 1,
            '}' => depth = (depth - 1).max(0),
            _ => {}
        }
    }
    depth
}

/// The text edit between a piece's previous text and its new one: the
/// common prefix length, and the end of the changed region in each.
#[derive(Clone, Copy, Debug)]
struct Edit {
    start: usize,
    old_end: usize,
    new_end: usize,
}

impl Edit {
    fn between(old: &[char], new: &[char]) -> Option<Edit> {
        if old == new {
            return None;
        }
        let start = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let tail = old[start..]
            .iter()
            .rev()
            .zip(new[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        Some(Edit {
            start,
            old_end: old.len() - tail,
            new_end: new.len() - tail,
        })
    }

    /// An old position in the new text, if it is outside the changed
    /// region.
    fn map(self, pos: usize) -> Option<usize> {
        if pos < self.start {
            Some(pos)
        } else if pos >= self.old_end {
            Some(pos - self.old_end + self.new_end)
        } else {
            None
        }
    }
}

/// The definitions from before the piece read in this run, each read once
/// for the nodes it reaches (`lo..hi`: all of them unless an element
/// between the piece's nodes defines the name).
type Outside = FxMap<Key, (ReadRef, u32, u32)>;

/// The definition of `k` from before the piece reaching node `node`.
fn outside_read(env: &mut dyn Env, outside: &mut Outside, k: &Key, node: u32) -> ReadRef {
    if let Some((v, lo, hi)) = outside.get(k)
        && (*lo..*hi).contains(&node)
    {
        return v.clone();
    }
    // (what the build answers itself: a file's bytes, the job's name, the
    // lines before the piece)
    let (val, src, lo, hi) = match k {
        Key::File(n) => (env.file(n).map_or(Val::Void, Val::Bytes), Src::Initial, 0, u32::MAX),
        Key::Job => (Val::Name(env.jobname()), Src::Initial, 0, u32::MAX),
        Key::LineBase => (Val::Int(env.line_base()), Src::Initial, 0, u32::MAX),
        _ => env.reach(k, node),
    };
    let r = Arc::new(Read {
        key: k.clone(),
        src,
        val,
        scope: None,
    });
    outside.insert(k.clone(), (r.clone(), lo, hi));
    r
}

/// The definition of `key` for the next node of `chain`, and where from.
fn resolve_now(chain: &Chain, env: &mut dyn Env, outside: &mut Outside, key: &Key) -> ReadRef {
    match chain.resolve(key, chain.nodes.len()) {
        Some((d, val)) => Arc::new(Read {
            key: key.clone(),
            src: Src::Node(d),
            val,
            scope: chain.scope_of(d, key),
        }),
        None => outside_read(env, outside, key, chain.nodes.len() as u32),
    }
}

/// Runs a piece: `old` is its previous run (and the text that made it),
/// `dirty` the names from before the piece a definition of which was
/// remade: every old node reading one of them is dirty. Returns the new
/// chain, with every old node still holding kept.
pub fn run_piece(
    text: &[char],
    env: &mut dyn Env,
    old: Option<(&Run, &[char])>,
    dirty: &[Key],
) -> Run {
    let mut outside: Outside = FxMap::default();
    let mut chain = Chain::default();
    let mut ran = 0u64;
    let edit = old.and_then(|(_, t)| Edit::between(t, text));
    let oldc = old.map(|(r, _)| &r.chain);
    // (the dirty old nodes: those reading a dirty name from before the
    // piece, in order)
    let dirty: Vec<u32> = match oldc {
        Some(oc) if !dirty.is_empty() => oc
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.reads
                    .iter()
                    .any(|r| !matches!(r.src, Src::Node(_)) && dirty.contains(&r.key))
            })
            .map(|(i, _)| i as u32)
            .collect(),
        _ => Vec::new(),
    };
    // (where to start: the first dirty node, or the first the edit touches;
    // everything before it is kept as it is)
    let mut from = dirty.first().map_or(usize::MAX, |&d| d as usize);
    if let (Some(oc), Some(e)) = (oldc, edit) {
        // (positions only grow along a chain)
        let (mut lo, mut hi) = (0, oc.nodes.len());
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            if oc.nodes[mid].out.pos >= e.start {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        let touched = lo;
        from = from.min(touched);
    }
    let from = oldc.map_or(0, |oc| from.min(oc.nodes.len().saturating_sub(1)));
    if let Some(oc) = oldc {
        chain = Chain::prefix(oc, from);
    }
    let mut state = chain.input(from);
    let mut scratch = Scratch::default();
    let mut oi = from;
    // (a paragraph's node runs again if any node of its paragraph did)
    let mut para_fresh = false;
    // (a node ran or was dropped that changes how later names resolve:
    // later old nodes' reads must be looked at one by one)
    let mut scope_changed = false;
    loop {
        let mut kept: Option<Node> = None;
        let shared: Option<Arc<Node>> = None;
        // (the old node at this very input, if any: the same command)
        let mut cand: Option<usize> = None;
        let mut copy_to: Option<usize> = None;
        if let Some(oc) = oldc {
            let mapped = |p: usize| edit.map_or(Some(p), |e| e.map(p));
            while oi < oc.nodes.len() && mapped(oc.input_pos(oi)).is_none_or(|p| p < state.pos) {
                let n = &oc.nodes[oi];
                scope_changed |= !n.defs().is_empty() || n.kind.scopes();
                oi += 1;
            }
            if oi < oc.nodes.len() {
                let n = &oc.nodes[oi];
                let inp_pos = oc.input_pos(oi);
                let start_ok = mapped(inp_pos) == Some(state.pos);
                // (the text it read, through the character after it, unchanged)
                let text_ok = edit.is_none_or(|e| n.out.pos < e.start || inp_pos >= e.old_end);
                let par_ok = !(matches!(n.kind, Kind::Par | Kind::End) && para_fresh);
                let state_ok = if oi == 0 {
                    RState::start().eq_but_pos(&state)
                } else {
                    oc.nodes[oi - 1].out.eq_but_pos(&state)
                };
                if start_ok && text_ok && state_ok {
                    cand = Some(oi);
                    let reads_ok = par_ok
                        && n.reads.iter().all(|r| {
                            Read::same(&resolve_now(&chain, env, &mut outside, &r.key), r)
                        });
                    if reads_ok {
                        let in_place = edit.is_none() && !scope_changed && oi == chain.nodes.len();
                        if in_place {
                            // (the same node at the same index, its operands
                            // from the same nodes: shared, not copied; and so
                            // on up to the next node that must be looked at)
                            copy_to = Some(
                                dirty
                                    .iter()
                                    .find(|&&d| d as usize > oi)
                                    .map_or(oc.nodes.len(), |&d| d as usize),
                            );
                        } else {
                            let mut n = (**n).clone();
                            if let Some(e) = edit {
                                n.out.pos = e.map(n.out.pos).unwrap_or(n.out.pos);
                            }
                            // (re-point reads at the nodes now defining them)
                            for r in n.reads.iter_mut() {
                                *r = resolve_now(&chain, env, &mut outside, &r.key);
                            }
                            kept = Some(n);
                        }
                    }
                }
            }
        }
        if let Some(stop) = copy_to {
            // (node `oi` was looked at; each node after it up to the next
            // dirty one is shared: the definitions it read in the piece are
            // at the same nodes with the same values (no scope changed), and
            // every node reading a remade definition from before the piece
            // is dirty (a definition remade since, not yet marked, is
            // caught when the run is validated: its reads list every value
            // read); a paragraph's node is looked at if a node of its
            // paragraph ran)
            let oc = oldc.expect("an old chain");
            let mut j = oi;
            while j < stop {
                let n = &oc.nodes[j];
                if j > oi && para_fresh && matches!(n.kind, Kind::Par | Kind::End) {
                    break;
                }
                if oc.starts_para(j) {
                    para_fresh = false;
                }
                j += 1;
            }
            chain.share(oc, oi, j);
            if j == oc.nodes.len() {
                break;
            }
            oi = j;
            state = chain.input(j);
            continue;
        }
        let fresh = kept.is_none() && shared.is_none();
        let n: Arc<Node> = match (shared, kept) {
            (Some(n), _) => {
                oi += 1;
                n
            }
            (None, Some(n)) => {
                // (kept at another index: later nodes' reads in the piece
                // and their groups would name other nodes)
                if oi != chain.nodes.len() && (!n.defs().is_empty() || n.kind.scopes()) {
                    scope_changed = true;
                }
                oi += 1;
                chain.with_group(n)
            }
            (None, None) => {
                ran += 1;
                let mut x = Exec::new(text, &chain, &state, env, &mut outside, &mut scratch);
                // (the old node at this input's paragraph: an equal one is
                // shared)
                x.old_para = cand.and_then(|ci| oldc.and_then(|oc| oc.nodes[ci].para().cloned()));
                x.command();
                chain.with_group(x.finish())
            }
        };
        let mut same_material = !fresh;
        if fresh {
            // (the same command run again, defining the same and leaving the
            // same state, changes nothing about how later names resolve)
            let same_as_old = cand.is_some_and(|ci| {
                let o = &oldc.expect("an old chain").nodes[ci];
                ci == chain.nodes.len()
                    && o.kind == n.kind
                    && o.out == n.out
                    && o.defs().len() == n.defs().len()
                    && o.defs().iter().zip(n.defs()).all(|(a, b)| {
                        a.key == b.key
                            && a.val == b.val
                            && a.global == b.global
                            && a.level == b.level
                    })
            });
            if same_as_old {
                // (and emitting the same, the paragraph's material is the
                // same: a re-run producing equal results stops there)
                let o = &oldc.expect("an old chain").nodes[oi];
                same_material = o.emit[..] == n.emit[..]
                    && o.vemit() == n.vemit()
                    && match (o.para(), n.para()) {
                        (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                        (None, None) => true,
                        _ => false,
                    };
                oi += 1;
            } else {
                scope_changed |= !n.defs().is_empty() || n.kind.scopes();
            }
        }
        let starts_para = state.starts_para(&n.out);
        if starts_para {
            para_fresh = !same_material;
        } else if !same_material {
            para_fresh = true;
        }
        let end = n.kind == Kind::End;
        state = n.out.clone();
        chain.push_arc(n);
        if end {
            break;
        }
    }
    // (exports: each key defined in the piece, as it reaches the end)
    chain.nodes.freeze();
    let end = chain.nodes.len();
    let mut exports: Vec<(Key, Val)> = chain
        .defs_of
        .keys()
        .filter_map(|k| chain.resolve(k, end).map(|(_, v)| (k.clone(), v)))
        .collect();
    exports.sort_by(|a, b| a.0.cmp(&b.0));
    let paras = chain
        .para_at
        .iter()
        .filter_map(|&i| chain.nodes[i as usize].para().cloned())
        .collect();
    let errors = chain
        .err_at
        .iter()
        .flat_map(|&i| chain.nodes[i as usize].errors().iter().cloned())
        .collect();
    // (the kept nodes before `from` read what they read in the old run:
    // its entries whose first reader is among them; then each read of the
    // other nodes, its identity seen once)
    let mut reads: Vec<(Key, u32, Val)> = match old {
        Some((r, _)) => r
            .reads
            .iter()
            .filter(|(_, i, _)| (*i as usize) < from)
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    let mut seen: FxSet<usize> = FxSet::default();
    for (d, n) in chain.nodes.range(from, end).enumerate() {
        let i = from + d;
        for r in n.reads.iter() {
            if matches!(r.src, Src::Node(_)) || !seen.insert(Arc::as_ptr(r) as usize) {
                continue;
            }
            reads.push((r.key.clone(), i as u32, r.val.clone()));
        }
    }
    reads.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    // (one entry per (name, value): its first reader — the first of its
    // name's run with that value)
    let mut kept: Vec<(Key, u32, Val)> = Vec::with_capacity(reads.len());
    let mut run_start = 0;
    for e in reads {
        if kept.last().is_none_or(|l| l.0 != e.0) {
            run_start = kept.len();
        } else if kept[run_start..].iter().any(|x| x.2 == e.2) {
            continue;
        }
        kept.push(e);
    }
    let reads = kept;
    let mut fonts: Vec<(u32, Arc<str>)> = chain
        .defs_of
        .iter()
        .filter_map(|(k, v)| match k {
            Key::FontLoaded(f) => v.first().map(|&i| (i, f.clone())),
            _ => None,
        })
        .collect();
    fonts.sort_by_key(|x| x.0);
    let fonts = fonts.into_iter().map(|(_, f)| f).collect();
    Run {
        chain,
        reads,
        exports,
        paras,
        errors,
        ran,
        fonts,
    }
}

/// Buffers reused from node to node (their capacity, never their
/// content).
#[derive(Default)]
struct Scratch {
    /// The node's reads so far (read once each, by index), once there are
    /// many.
    seen: FxMap<Key, u32>,
    /// The node's reads of `\catcode` and `\sfcode` of characters below
    /// 256, by index, while their stamp is the node's.
    chars: Vec<(u32, u32)>,
    stamp: u32,
    /// The node's reads and what it emitted (their capacity kept).
    reads: Vec<ReadRef>,
    emit: Vec<Item>,
    /// Shared copies of the names read in this piece.
    names: FxSet<Arc<str>>,
    buf: String,
    /// The glue `scan_glue` just gave is an internal quantity's own (for
    /// TeX's shared zero glue, which `\showbox` and the short display
    /// tell apart).
    glue_shared: bool,
}

/// One node's execution: one command, from the state its predecessor left.
struct Exec<'a> {
    text: &'a [char],
    chain: &'a Chain,
    env: &'a mut dyn Env,
    outside: &'a mut Outside,
    scratch: &'a mut Scratch,
    st: RState,
    reads: Vec<ReadRef>,
    defs: Vec<Def>,
    emit: Vec<Item>,
    vemit: Vec<Item>,
    para: Option<Arc<Para>>,
    errors: Vec<String>,
    phi: Option<Box<Phi>>,
    log: Option<Box<str>>,
    kind: Kind,
    cmd: Tok,
    /// The token `next_raw` just gave came after `\noexpand`'s mark.
    noexp: bool,
    /// Expanding a `\write`'s text (no mode: tex.web 1370).
    in_write: bool,
    /// How many of `reads` are in `scratch.seen`.
    seen_upto: usize,
    /// This node's stamp in `scratch.chars`.
    stamp: u32,
    /// The paragraph the old node at this place made, if any: an equal
    /// paragraph is that one (the same `Arc`).
    old_para: Option<Arc<Para>>,
    /// While a rewritten text (`\scantokens`) is read: its characters,
    /// which the tokenizer reads instead of the piece's.
    ptext: Option<Arc<[char]>>,
}

/// Up to this many reads, a node's reads are looked up by a scan.
const FEW_READS: usize = 12;

/// Where a node found a definition: its own `defs[i]`, or `reads[i]`.
enum Found {
    Own(usize),
    Read(usize),
}

impl<'a> Exec<'a> {
    fn new(
        text: &'a [char],
        chain: &'a Chain,
        input: &RState,
        env: &'a mut dyn Env,
        outside: &'a mut Outside,
        scratch: &'a mut Scratch,
    ) -> Exec<'a> {
        if !scratch.seen.is_empty() {
            scratch.seen.clear();
        }
        if scratch.chars.is_empty() {
            scratch.chars = vec![(0, 0); 512];
        }
        scratch.stamp = scratch.stamp.wrapping_add(1);
        if scratch.stamp == 0 {
            scratch.chars.fill((0, 0));
            scratch.stamp = 1;
        }
        let stamp = scratch.stamp;
        let reads = std::mem::take(&mut scratch.reads);
        let emit = std::mem::take(&mut scratch.emit);
        Exec {
            text,
            chain,
            env,
            outside,
            scratch,
            st: input.clone(),
            reads,
            defs: Vec::new(),
            emit,
            vemit: Vec::new(),
            para: None,
            errors: Vec::new(),
            phi: None,
            log: None,
            kind: Kind::Other,
            cmd: Tok::Char(0, '\0'),
            noexp: false,
            in_write: false,
            seen_upto: 0,
            stamp,
            old_para: None,
            ptext: None,
        }
    }

    fn finish(mut self) -> Node {
        // (vertical material at the outer level is carried by `para`, in
        // the order the build's page fold reads it)
        if self.st.level == 0 && !self.vemit.is_empty() {
            let v = std::mem::take(&mut self.vemit);
            match &mut self.para {
                Some(p) => Arc::make_mut(p).after.extend(v),
                None => {
                    self.para = Some(Arc::new(Para {
                        before: v,
                        ..Para::default()
                    }));
                }
            }
        }
        let reads = Few::take(&mut self.reads);
        let emit = Few::take(&mut self.emit);
        self.scratch.reads = self.reads;
        self.scratch.emit = self.emit;
        let rare = (!self.defs.is_empty()
            || self.para.is_some()
            || !self.errors.is_empty()
            || self.phi.is_some()
            || !self.vemit.is_empty()
            || self.log.is_some())
        .then(|| {
            Box::new(Rare {
                defs: self.defs,
                para: self.para,
                errors: self.errors,
                phi: self.phi,
                vemit: self.vemit,
                log: self.log,
            })
        });
        Node {
            kind: self.kind,
            group: None,
            cmd: self.cmd,
            out: self.st,
            reads,
            emit,
            rare,
        }
    }

    fn err(&mut self, s: String) {
        self.errors.push(s);
    }

    // ------------------------------------------------------------ names

    /// The definition of `key` reaching this node: this node's own, else a
    /// node before it in the piece, else from before the piece.
    fn get(&mut self, key: &Key) -> Val {
        match self.find(key) {
            Found::Own(i) => self.defs[i].val.clone(),
            Found::Read(i) => self.reads[i].val.clone(),
        }
    }

    /// [`Exec::get`], as the read (the value not copied out of it).
    fn lookup(&mut self, key: &Key) -> ReadRef {
        match self.find(key) {
            // (its own definition: not a read)
            Found::Own(i) => Arc::new(Read {
                key: key.clone(),
                src: Src::Node(self.chain.nodes.len() as u32),
                val: self.defs[i].val.clone(),
                scope: None,
            }),
            Found::Read(i) => self.reads[i].clone(),
        }
    }

    /// Where the definition of `key` reaching this node is: its own, or
    /// one of its reads (read now if it was not).
    fn find(&mut self, key: &Key) -> Found {
        // (a character's code, read again in the node: by the character)
        let slot = match key {
            Key::Cat(c) if (*c as u32) < 256 && self.defs.is_empty() => Some(*c as usize),
            Key::Sfcode(c) if (*c as u32) < 256 && self.defs.is_empty() => Some(256 + *c as usize),
            _ => None,
        };
        if let Some(s) = slot {
            let (stamp, i) = self.scratch.chars[s];
            if stamp == self.stamp {
                return Found::Read(i as usize);
            }
        }
        let f = self.find_slow(key);
        if let (Some(s), Found::Read(i)) = (slot, &f) {
            self.scratch.chars[s] = (self.stamp, *i as u32);
        }
        f
    }

    fn find_slow(&mut self, key: &Key) -> Found {
        if let Some(i) = self.defs.iter().rposition(|d| d.key == *key) {
            return Found::Own(i);
        }
        if self.reads.len() <= FEW_READS {
            if let Some(i) = self.reads.iter().position(|r| r.key == *key) {
                return Found::Read(i);
            }
        } else {
            for (j, r) in self.reads[self.seen_upto..].iter().enumerate() {
                self.scratch
                    .seen
                    .entry(r.key.clone())
                    .or_insert((self.seen_upto + j) as u32);
            }
            self.seen_upto = self.reads.len();
            if let Some(&i) = self.scratch.seen.get(key) {
                return Found::Read(i as usize);
            }
        }
        let r = resolve_now(self.chain, self.env, self.outside, key);
        self.reads.push(r);
        Found::Read(self.reads.len() - 1)
    }

    fn define(&mut self, key: Key, val: Val, global: bool) {
        self.defs.push(Def {
            key,
            val,
            global,
            level: None,
        });
    }

    fn meaning(&mut self, key: &Key) -> Meaning {
        match self.get(key) {
            Val::Meaning(m) => m,
            _ => Meaning::Undefined,
        }
    }

    fn catcode(&mut self, c: char) -> u8 {
        match self.get(&Key::Cat(c)) {
            Val::Cat(k) => k,
            _ => 12,
        }
    }

    fn int(&mut self, key: &Key) -> i32 {
        match self.get(key) {
            Val::Int(i) => i,
            Val::Glue(g) => g.width,
            _ => 0,
        }
    }

    fn int_par(&mut self, p: IntPar) -> i32 {
        self.int(&Key::IntPar(p))
    }

    fn dimen_par(&mut self, p: DimenPar) -> i32 {
        self.int(&Key::DimenPar(p))
    }

    /// The current font (through its read: the read is shared, the font's
    /// `Arc` is copied once).
    fn font(&mut self) -> Arc<Font> {
        let r = self.lookup(&Key::Font);
        match &r.val {
            Val::Font(f) => f.clone(),
            other => panic!("the font key holds {other:?}"),
        }
    }

    /// `\fontdimen n` of the current font.
    fn font_param(&mut self, n: u16) -> i32 {
        let f = self.font();
        self.int(&Key::FontDimen(f.key.clone(), n))
    }

    fn tok_meaning(&mut self, t: &Tok) -> Meaning {
        match t {
            Tok::Char(c, x) => Meaning::Char(*c, *x),
            _ => t.key().map_or(Meaning::Undefined, |k| self.meaning(&k)),
        }
    }

    fn cs(&mut self, name: &str) -> Tok {
        Tok::Cs(intern(&mut self.scratch.names, name))
    }

    /// Sets the node's kind from a conditional's step, unless its command
    /// already gave it a kind that matters more (a group, a paragraph).
    fn cond_kind(&mut self, k: Kind) {
        if matches!(
            self.kind,
            Kind::Other | Kind::If | Kind::IfClosed | Kind::EndIf
        ) {
            self.kind = k;
        }
    }

    // ------------------------------------------------------------ the input

    fn back(&mut self, t: Tok) {
        // (tex.web 325: lists read to their end go first, not those under
        // a rewritten text)
        let floor = self.st.texts.last().map_or(0, |x| x.at);
        while self.st.pending.len() > floor && self.st.pending.last().is_some_and(|(l, i, _)| *i >= l.len()) {
            self.st.pending.pop();
        }
        self.st.pending.push((Arc::from([t]), 0, true));
    }

    /// Rewrites the input: `toks`, then the rest.
    fn rewrite(&mut self, toks: Vec<Tok>) {
        if !toks.is_empty() {
            self.st.pending.push((Arc::from(toks), 0, false));
        }
    }

    /// The current line's end-of-line character (tex.web 360: the value of
    /// `\endlinechar` when the line was begun), if active.
    fn eol(&mut self) -> Option<char> {
        if self.st.eol == EOL_UNREAD {
            self.read_eol();
        }
        u8::try_from(self.st.eol).ok().map(char::from)
    }

    /// Reads `\endlinechar` for the line being begun.
    fn read_eol(&mut self) {
        let v = self.int_par(IntPar::Endlinechar);
        self.st.eol = if (0..=255).contains(&v) { v as i16 } else { -1 };
    }

    /// The reader has passed a line's end: the next line begins (its
    /// end-of-line character is read when the line is: as its first
    /// character is).
    fn new_line(&mut self) {
        self.st.tstate = TState::N;
        self.st.eol = EOL_UNREAD;
    }

    /// The characters the tokenizer reads: the piece's, or a rewritten
    /// text's while one is read.
    fn txt(&self) -> &[char] {
        match &self.ptext {
            Some(t) => t,
            None => self.text,
        }
    }

    /// Moves to `np`; past a line's end, the next line begins (and a text
    /// whose `\endinput` was met ends, tex.web 362).
    fn advance(&mut self, np: usize) {
        let crossed = self.txt()[self.st.pos..np].contains(&'\r');
        self.st.pos = np;
        if crossed {
            self.new_line();
            if self.ptext.is_some() && self.st.texts.last().is_some_and(|x| x.endinput) {
                self.st.pos = self.txt().len();
            }
        }
    }

    /// The character at `pos`, the position after it and its category
    /// (tex.web 355): a line's end is its end-of-line character (none if
    /// inactive), and `^^` reduces the character or the two hex digits
    /// after it.
    fn char_at(&mut self, pos: usize) -> Option<(char, usize, u8)> {
        let raw = *self.txt().get(pos)?;
        let c = if raw == '\r' { self.eol()? } else { raw };
        let cat = self.catcode(c);
        // (`^^`: the second circumflex on the line, and a character after it)
        if raw == '\r' || cat != 7 || self.txt().get(pos + 1) != Some(&c) {
            return Some((c, pos + 1, cat));
        }
        let at = |x: &mut Self, p: usize| -> Option<char> {
            let ch = x.txt().get(p).copied();
            match ch {
                Some('\r') => x.eol(),
                other => other,
            }
        };
        let Some(c2) = at(self, pos + 2) else {
            return Some((c, pos + 1, cat));
        };
        if u32::from(c2) >= 128 {
            return Some((c, pos + 1, cat));
        }
        let hex = |ch: char| ch.is_ascii_digit() || ('a'..='f').contains(&ch);
        let (r, np) = match at(self, pos + 3) {
            Some(c3) if hex(c2) && hex(c3) && self.txt().get(pos + 2) != Some(&'\r') => (
                (c2.to_digit(16).unwrap_or(0) << 4) | c3.to_digit(16).unwrap_or(0),
                pos + 4,
            ),
            _ => {
                let v = u32::from(c2);
                (if v < 64 { v + 64 } else { v - 64 }, pos + 3)
            }
        };
        let r = char::from_u32(r).unwrap_or('\0');
        Some((r, np, self.catcode(r)))
    }

    /// The position just after the current line's end.
    fn line_end_after(&self, pos: usize) -> usize {
        let text = self.txt();
        text[pos..]
            .iter()
            .position(|&c| c == '\r')
            .map_or(text.len(), |k| pos + k + 1)
    }

    /// The next token, unexpanded (tex.web 343-357). After `\noexpand`'s
    /// mark, the token after it, with `noexp` set.
    fn next_raw(&mut self) -> Option<Tok> {
        self.noexp = false;
        loop {
            // (the token lists above the innermost rewritten text; a list
            // read to its end stays until the next read, tex.web 325)
            let floor = self.st.texts.last().map_or(0, |x| x.at);
            while self.st.pending.len() > floor {
                let Some((list, i, _)) = self.st.pending.last_mut() else { break };
                if *i < list.len() {
                    let t = list[*i].clone();
                    *i += 1;
                    if t == Tok::DontExpand {
                        let n = self.next_raw();
                        self.noexp = n.as_ref().is_some_and(|x| x.key().is_some());
                        return n;
                    }
                    return Some(t);
                }
                self.st.pending.pop();
            }
            if self.st.texts.is_empty() {
                return self.tokenize();
            }
            // (a `\read` line's end ends the reading)
            if self.st.texts.last().is_some_and(|x| x.kind == TextKind::Read) {
                return self.text_token();
            }
            if let Some(t) = self.text_token() {
                return Some(t);
            }
        }
    }

    /// The next token of the innermost rewritten text, read as the piece's
    /// text is; at its end `\everyeof` goes in once (etex.web §362), then
    /// the text is done (`None`: read on below it).
    fn text_token(&mut self) -> Option<Tok> {
        let x = self.st.texts.last()?.clone();
        let outer = (self.st.pos, self.st.tstate, self.st.eol);
        (self.st.pos, self.st.tstate, self.st.eol) = (x.pos, x.tstate, x.eol);
        let prev = self.ptext.replace(x.text.clone());
        let t = self.tokenize();
        self.ptext = prev;
        let here = (self.st.pos, self.st.tstate, self.st.eol);
        (self.st.pos, self.st.tstate, self.st.eol) = outer;
        let Some(x) = self.st.texts.last_mut() else { return t };
        (x.pos, x.tstate, x.eol) = here;
        if t.is_some() || x.kind == TextKind::Read {
            return t;
        }
        // (`\endinput` forces the end: no `\everyeof`)
        if !x.eof && !x.endinput {
            x.eof = true;
            let every = match self.get(&Key::TokPar(crate::val::TokPar::Everyeof)) {
                Val::Toks(l) => l,
                _ => Arc::new(Vec::new()),
            };
            if !every.is_empty() {
                self.rewrite(every.to_vec());
                return None;
            }
        }
        if let Some(PendingText { kind: TextKind::File(_), .. }) = self.st.texts.pop() {
            // (an `\input` file is closed, tex.web 362)
            self.log_raw(&format!("{}{}", crate::show::RAW, ')'));
        }
        None
    }

    /// The next token of the text being read (the piece's, or a rewritten
    /// one's: `txt`), unexpanded (tex.web 343-357).
    fn tokenize(&mut self) -> Option<Tok> {
        loop {
            let pos = self.st.pos;
            let raw = *self.txt().get(pos)?;
            if self.st.eol == EOL_UNREAD {
                self.read_eol();
            }
            if raw == '\r' && self.eol().is_none() {
                // (an inactive end-of-line character: the line just ends)
                self.advance(pos + 1);
                continue;
            }
            let (c, np, cat) = self.char_at(pos)?;
            match cat {
                0 => {
                    // (a control sequence: a run of letters, or one character)
                    let line_end = self.line_end_after(pos);
                    let first = if np < line_end {
                        self.char_at(np)
                    } else {
                        None
                    };
                    let Some((c2, mut q, cat2)) = first else {
                        self.advance(np);
                        self.st.tstate = TState::S;
                        return Some(self.cs(""));
                    };
                    if cat2 == 11 {
                        self.scratch.buf.clear();
                        self.scratch.buf.push(c2);
                        while q < line_end {
                            let Some((c3, q3, cat3)) = self.char_at(q) else {
                                break;
                            };
                            if cat3 != 11 {
                                break;
                            }
                            self.scratch.buf.push(c3);
                            q = q3;
                        }
                        self.st.tstate = TState::S;
                        let t = Tok::Cs(intern(&mut self.scratch.names, &self.scratch.buf));
                        self.advance(q);
                        return Some(t);
                    }
                    self.st.tstate = if cat2 == 10 { TState::S } else { TState::M };
                    let mut b = [0u8; 4];
                    let t = self.cs(c2.encode_utf8(&mut b));
                    self.advance(q);
                    return Some(t);
                }
                5 => {
                    // (the rest of the line is discarded)
                    let ts = self.st.tstate;
                    let end = self.line_end_after(pos);
                    self.advance(end.max(np));
                    match ts {
                        TState::N => return Some(self.cs("par")),
                        TState::M => return Some(Tok::Char(10, ' ')),
                        TState::S => {}
                    }
                }
                10 => {
                    let ts = self.st.tstate;
                    self.st.tstate = TState::S;
                    self.advance(np);
                    if ts == TState::M {
                        return Some(Tok::Char(10, ' '));
                    }
                }
                14 => {
                    let end = self.line_end_after(pos);
                    self.advance(end.max(np));
                }
                9 => self.advance(np),
                15 => {
                    self.advance(np);
                    self.err("Text line contains an invalid character".into());
                }
                13 => {
                    self.st.tstate = TState::M;
                    self.advance(np);
                    return Some(Tok::Active(c));
                }
                _ => {
                    self.st.tstate = TState::M;
                    self.advance(np);
                    return Some(Tok::Char(cat, c));
                }
            }
        }
    }

    // ------------------------------------------------------------ macros

    /// A macro call: matches the arguments and rewrites the input with the
    /// body (tex.web 389-399). A step: it defines nothing.
    fn call(&mut self, name: &Tok, m: &Macro) {
        let mut args: Vec<Vec<Tok>> = vec![Vec::new(); 10];
        let p = &m.params;
        let mut i = 0;
        while i < p.len() {
            if let Tok::Param(n) = p[i] {
                let mut j = i + 1;
                while j < p.len() && !matches!(p[j], Tok::Param(_)) {
                    j += 1;
                }
                let delim = &p[i + 1..j];
                let arg = if delim.is_empty() {
                    self.undelimited(name, m.long)
                } else {
                    self.delimited(name, delim, m.long)
                };
                let Some(arg) = arg else { return };
                args[usize::from(n)] = arg;
                i = j;
            } else {
                match self.next_raw() {
                    Some(x) if x == p[i] => {}
                    other => {
                        self.err(format!("Use of {name} doesn't match its definition"));
                        if let Some(x) = other {
                            self.back(x);
                        }
                        return;
                    }
                }
                i += 1;
            }
        }
        let mut out = Vec::with_capacity(m.body.len());
        for t in &m.body {
            match t {
                Tok::Param(n) => out.extend(args[usize::from(*n)].iter().cloned()),
                _ => out.push(t.clone()),
            }
        }
        self.rewrite(out);
    }

    /// A `\par` in a short macro's argument ends the call (tex.web 392).
    fn runaway(&mut self, name: &Tok, t: &Tok, long: bool) -> bool {
        if !long && matches!(t, Tok::Cs(n) if &**n == "par") {
            self.err(format!("Paragraph ended before {name} was complete"));
            self.back(t.clone());
            return true;
        }
        false
    }

    fn undelimited(&mut self, name: &Tok, long: bool) -> Option<Vec<Tok>> {
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning an argument".into());
                return Some(Vec::new());
            };
            if self.runaway(name, &t, long) {
                return None;
            }
            match t.cat() {
                Some(10) => {}
                Some(1) => return self.group_body(name, long),
                _ => return Some(vec![t]),
            }
        }
    }

    fn group_body(&mut self, name: &Tok, long: bool) -> Option<Vec<Tok>> {
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(t) = self.next_raw() {
            if self.runaway(name, &t, long) {
                return None;
            }
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(out);
                    }
                }
                _ => {}
            }
            out.push(t);
        }
        self.err("File ended while scanning an argument".into());
        Some(out)
    }

    fn delimited(&mut self, name: &Tok, delim: &[Tok], long: bool) -> Option<Vec<Tok>> {
        let mut buf: Vec<Tok> = Vec::new();
        let mut depth = 0;
        // (`#{`: the left brace delimits, and stays in the input)
        let brace_delim = delim.last().is_some_and(|t| t.cat() == Some(1));
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a delimited argument".into());
                break;
            };
            if self.runaway(name, &t, long) {
                return None;
            }
            let at_zero = depth == 0;
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => depth -= 1,
                _ => {}
            }
            buf.push(t);
            let check = if brace_delim { at_zero } else { depth == 0 };
            if check && buf.len() >= delim.len() && buf[buf.len() - delim.len()..] == *delim {
                // (a `#{` brace is consumed: the body ends with it)
                buf.truncate(buf.len() - delim.len());
                break;
            }
        }
        if buf.len() >= 2 && buf[0].cat() == Some(1) && buf[buf.len() - 1].cat() == Some(2) {
            let mut d = 0;
            let mut whole = true;
            for (k, t) in buf.iter().enumerate() {
                match t.cat() {
                    Some(1) => d += 1,
                    Some(2) => d -= 1,
                    _ => {}
                }
                if d == 0 && k + 1 < buf.len() {
                    whole = false;
                    break;
                }
            }
            if whole {
                buf.pop();
                buf.remove(0);
            }
        }
        Some(buf)
    }

    // ------------------------------------------------------------ commands

    /// One command.
    fn command(&mut self) {
        let Some((t, m)) = self.next_x_m(true) else {
            self.end();
            return;
        };
        self.cmd = t.clone();
        let (global, long, outer) = (
            std::mem::take(&mut self.st.global),
            std::mem::take(&mut self.st.long),
            std::mem::take(&mut self.st.outer),
        );
        let protected = std::mem::take(&mut self.st.protected);
        if self.st.mode == Mode::M && self.math_command(&t, &m) {
            return;
        }
        match m {
            Meaning::Prim(p) if p.conditional() => self.conditional(p, &t),
            Meaning::Prim(Prim::Global | Prim::Long | Prim::Outer | Prim::Protected) => {
                let Meaning::Prim(p) = m else { unreachable!() };
                self.prefix(p, global, long, outer, protected);
            }
            _ if assign::prefixable(&m) => self.prefixed(&t, &m, global, long, outer, protected),
            Meaning::Prim(Prim::Mat(x)) => self.mat(x, &t),
            Meaning::Prim(Prim::Math(x)) => self.math_prim(x, &t),
            Meaning::Prim(p) => self.prim(p),
            Meaning::Char(cat, c) => self.char_tok(cat, c, &t),
            Meaning::CharGiven(c) => self.char_cmd(c, &t),
            Meaning::MathCharGiven(_) => self.insert_dollar_sign(&t),
            Meaning::Undefined => self.err(format!("Undefined control sequence {t}")),
            Meaning::Macro(_)
            | Meaning::CountRef(_)
            | Meaning::DimenRef(_)
            | Meaning::SkipRef(_)
            | Meaning::MuskipRef(_)
            | Meaning::ToksRef(_)
            | Meaning::Font(_) => {}
        }
    }

    /// The end of the piece: its last paragraph, its open groups and
    /// conditionals.
    fn end(&mut self) {
        self.end_par();
        self.kind = Kind::End;
        if !self.st.groups.is_empty() {
            self.err("Missing } inserted at the end of a piece".into());
        }
        if !self.st.conds.is_empty() {
            self.err("Incomplete \\if at the end of a piece".into());
        }
    }

    /// A group opens: a scope (tex.web 1063).
    fn open_group(&mut self, g: GroupKind) {
        self.st.groups.push(Group {
            kind: g,
            after: Vec::new(),
            boxg: None,
        });
        self.kind = Kind::Open;
    }

    /// The innermost group closes: it defines nothing; the tokens
    /// `\aftergroup` gave it rewrite the input (tex.web 282).
    fn close_group(&mut self) {
        let after = self.st.groups.pop().map(|g| g.after).unwrap_or_default();
        self.rewrite(after);
        self.kind = Kind::Close;
    }

    fn char_tok(&mut self, cat: u8, c: char, t: &Tok) {
        match cat {
            1 => self.open_group(GroupKind::Simple),
            2 => match self.st.groups.last().map(|g| g.kind) {
                Some(GroupKind::Simple) => self.close_group(),
                Some(GroupKind::Box) => self.close_box(t),
                Some(GroupKind::Math) => self.math_right_brace(),
                Some(GroupKind::SemiSimple) => self.err("Extra }, or forgotten \\endgroup".into()),
                None => self.err("Too many }'s".into()),
            },
            10 => {
                if self.st.mode == Mode::H {
                    self.space_cmd();
                }
            }
            11 | 12 => self.char_cmd(u32::from(c), t),
            3 => self.math_shift(t),
            7 | 8 => self.sub_sup_cmd(cat == 7, t),
            _ => self.err(format!(
                "Character {c} of category {cat} is not supported yet"
            )),
        }
    }

    /// A primitive that is neither expanded, a conditional nor an
    /// assignment.
    fn prim(&mut self, p: Prim) {
        match p {
            Prim::Par => self.par_cmd(),
            Prim::Indent | Prim::Noindent => self.start_par(p == Prim::Indent),
            Prim::Space => self.ex_space(),
            Prim::Char => {
                let t = self.cmd.clone();
                self.char_cmd_num(&t);
            }
            Prim::BeginGroup => self.open_group(GroupKind::SemiSimple),
            Prim::EndGroup => match self.st.groups.last().map(|g| g.kind) {
                Some(GroupKind::SemiSimple) => self.close_group(),
                Some(GroupKind::Math) => {
                    let t = self.cmd.clone();
                    self.math_off_save(&t);
                }
                Some(GroupKind::Simple | GroupKind::Box) => {
                    // (tex.web 1064: the missing `}` goes in first)
                    self.err("Missing } inserted".into());
                    let t = self.cmd.clone();
                    self.rewrite(vec![Tok::Char(2, '}'), t]);
                }
                None => self.err("Extra \\endgroup".into()),
            },
            Prim::AfterGroup => {
                if let Some(t) = self.next_raw()
                    && let Some(g) = self.st.groups.last_mut()
                {
                    g.after.push(t);
                }
            }
            Prim::AfterAssignment => self.st.after_assignment = self.next_raw().map(Box::new),
            Prim::IgnoreSpaces => {
                // (the next non-blank token, expanded, is the next command)
                loop {
                    match self.next_x_m(false) {
                        Some((t, _)) if t.cat() == Some(10) => {}
                        Some((t, _)) => {
                            self.back(t);
                            break;
                        }
                        None => break,
                    }
                }
            }
            Prim::Message | Prim::ErrMessage => self.message(p == Prim::ErrMessage),
            Prim::SetLanguage => self.set_language(),
            Prim::Show(c) => self.show_cmd(c),
            Prim::Immediate => self.immediate(),
            Prim::Write => self.write_cmd(),
            Prim::OpenOut | Prim::CloseOut | Prim::Special => self.extension(p),
            Prim::InStream(open) => self.open_in(open),
            Prim::Input => self.start_input(),
            Prim::Uppercase | Prim::Lowercase => self.case_shift(p == Prim::Uppercase),
            Prim::EndCsName => self.err("Extra \\endcsname".into()),
            Prim::End | Prim::Dump => self.stop_job(p == Prim::Dump),
            Prim::Relax => {}
            Prim::Marks | Prim::Discards(_) | Prim::ShowE(_) | Prim::Middle | Prim::Lr(_) => self.etex_command(p),
            Prim::LastItem(_)
            | Prim::SpaceFactor
            | Prim::PrevDepth
            | Prim::PrevGraf
            | Prim::PageInt(_)
            | Prim::PageDimen(_)
            | Prim::BoxDimen(_) => self.err(format!("You can't use `\\{}' here", p.name())),
            _ => self.err(format!("\\{} is not supported yet", p.name())),
        }
    }
}

/// The shared copy of a name.
fn intern(names: &mut FxSet<Arc<str>>, s: &str) -> Arc<str> {
    if let Some(a) = names.get(s) {
        return a.clone();
    }
    let a: Arc<str> = Arc::from(s);
    names.insert(a.clone());
    a
}
