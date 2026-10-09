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
//! may have changed: the nodes marked dirty (a definition they read was
//! remade) and the nodes the edited text touches. Everything before the
//! first of them is kept as it is. From there each old node is kept when
//! its input state is the state reached, its text is unchanged and every
//! definition it read is still equal; otherwise a node runs. Once the
//! chain is back on the old one at the same index with no scope changed,
//! old nodes are shared as they are up to the next dirty node, each read
//! of a name from before the piece still checked (only the first node
//! reading a name is marked, and every node reading it must read the same
//! definition).

use std::sync::Arc;

use crate::arith::{round_decimals, xn_over_d};
use crate::hash::{FxMap, FxSet};
use crate::tfm::Font;
use crate::val::{Key, Macro, Meaning, Param, Prim, Tok, Val};

/// A piece's identity, stable across edits.
pub type PieceId = u32;

/// Where a piece's reads of names from before it are answered.
pub trait Env {
    /// The definition of `key` reaching this piece's start, and where it
    /// was made.
    fn reach(&mut self, key: &Key) -> (Val, Src);
}

/// An item of a horizontal list.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum HItem {
    Char {
        c: u8,
        w: i32,
        h: i32,
        d: i32,
    },
    /// An empty box (the paragraph indentation).
    Box {
        w: i32,
        h: i32,
        d: i32,
    },
    Glue {
        w: i32,
        st: i32,
        st_ord: u8,
        sh: i32,
        sh_ord: u8,
    },
    Penalty(i32),
}

/// A paragraph: its list and the parameters it was read with. Line
/// breaking is a pure function of this; an equal paragraph has equal
/// lines.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Para {
    pub hlist: Vec<HItem>,
    pub hsize: i32,
    pub tolerance: i32,
    pub linepenalty: i32,
    pub baselineskip: i32,
    pub parskip: i32,
    pub vsize: i32,
    pub topskip: i32,
    pub maxdepth: i32,
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
}

/// The reader state between two commands: a value, the next node's
/// operand.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RState {
    pub pos: usize,
    tstate: TState,
    /// Token lists the input was rewritten with, each with how far it has
    /// been read (innermost last).
    pending: Vec<(Arc<[Tok]>, usize)>,
    mode: Mode,
    /// A `\global` prefix is waiting for its assignment.
    global: bool,
    depth: u32,
    /// Conditionals open.
    conds: u32,
    parskip: i32,
}

impl RState {
    fn start() -> RState {
        RState {
            pos: 0,
            tstate: TState::N,
            pending: Vec::new(),
            mode: Mode::V,
            global: false,
            depth: 0,
            conds: 0,
            parskip: 0,
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

/// A read: an operand.
#[derive(Clone, Debug)]
pub struct Read {
    pub key: Key,
    pub src: Src,
    pub val: Val,
}

/// A definition: a result.
#[derive(Clone, Debug)]
pub struct Def {
    pub key: Key,
    pub val: Val,
    pub global: bool,
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

/// What few nodes have: definitions, a paragraph, errors, a φ.
#[derive(Clone, Debug, Default)]
pub struct Rare {
    pub defs: Vec<Def>,
    pub para: Option<Arc<Para>>,
    pub errors: Vec<String>,
    pub phi: Option<Phi>,
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
    pub reads: Few<Read>,
    pub emit: Few<HItem>,
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
        self.rare.as_ref().and_then(|r| r.phi.as_ref())
    }
}

/// A piece's chain of nodes, with what resolving names over it needs.
#[derive(Clone, Debug, Default)]
pub struct Chain {
    pub nodes: Vec<Arc<Node>>,
    /// Nodes defining each key, in order.
    defs_of: FxMap<Key, Vec<u32>>,
    /// For each `Open` node, the index of its `Close`, once there is one.
    close_of: FxMap<u32, u32>,
    open: Vec<u32>,
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

    /// Whether node `i` begins in horizontal mode.
    fn input_h(&self, i: usize) -> bool {
        i > 0 && self.nodes[i - 1].out.mode == Mode::H
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
            let closed = n
                .group
                .is_some_and(|g| self.close_of.get(&g).is_some_and(|&c| (c as usize) < i));
            if def.global || !closed {
                return Some((d, def.val.clone()));
            }
        }
        None
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
        match n.kind {
            Kind::Open => self.open.push(i),
            Kind::Close => {
                if let Some(o) = self.open.pop() {
                    self.close_of.insert(o, i);
                }
            }
            _ => {}
        }
        for d in n.defs() {
            self.defs_of.entry(d.key.clone()).or_default().push(i);
        }
        self.nodes.push(n);
    }
}

/// What running a piece produced.
#[derive(Clone, Debug, Default)]
pub struct Run {
    pub chain: Chain,
    /// Every name read from before the piece: (key, the first node reading
    /// it, value). Every node reading it from before the piece read the
    /// same value, so the first stands for all.
    pub reads: Vec<(Key, u32, Val)>,
    /// Definitions reaching the piece's end, sorted by key.
    pub exports: Vec<(Key, Val)>,
    pub paras: Vec<Arc<Para>>,
    pub errors: Vec<String>,
    /// Nodes run this time (the others were kept).
    pub ran: u64,
}

/// Cuts the source into pieces at blank lines at brace depth 0. Lines lose
/// their trailing spaces and end in `\r` (TeX's end of line).
pub fn cut(src: &str) -> Vec<Arc<[char]>> {
    let mut pieces: Vec<Arc<[char]>> = Vec::new();
    let mut cur: Vec<char> = Vec::new();
    let mut depth: i32 = 0;
    let mut blank_run = false;
    let mut lines: Vec<&str> = src.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    for line in lines {
        let line = line.trim_end_matches([' ', '\r']);
        let blank = line.trim_start_matches(' ').is_empty();
        if blank {
            blank_run = true;
        } else {
            if blank_run && depth == 0 && cur.iter().any(|c| !c.is_whitespace()) {
                pieces.push(Arc::from(std::mem::take(&mut cur)));
            }
            blank_run = false;
            depth = brace_depth(line, depth);
        }
        cur.extend(line.chars());
        cur.push('\r');
    }
    if !cur.is_empty() {
        pieces.push(Arc::from(cur));
    }
    pieces
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

type Outside = FxMap<Key, (Val, Src)>;

fn outside_read(env: &mut dyn Env, outside: &mut Outside, k: &Key) -> (Val, Src) {
    if let Some(v) = outside.get(k) {
        return v.clone();
    }
    let v = env.reach(k);
    outside.insert(k.clone(), v.clone());
    v
}

/// The value of `key` for the next node of `chain`, and where from.
fn resolve_now(chain: &Chain, env: &mut dyn Env, outside: &mut Outside, key: &Key) -> (Val, Src) {
    match chain.resolve(key, chain.nodes.len()) {
        Some((d, v)) => (v, Src::Node(d)),
        None => outside_read(env, outside, key),
    }
}

/// Runs a piece: `old` is its previous chain (and the text that made it),
/// `dirty` the sorted indices of old nodes a definition of which was
/// remade. Returns the new chain, with every old node still holding kept.
pub fn run_piece(
    text: &[char],
    env: &mut dyn Env,
    old: Option<(&Chain, &[char])>,
    dirty: &[u32],
) -> Run {
    let mut outside: Outside = FxMap::default();
    let mut chain = Chain::default();
    let mut ran = 0u64;
    let edit = old.and_then(|(_, t)| Edit::between(t, text));
    let oldc = old.map(|(c, _)| c);
    // (where to start: the first dirty node, or the first the edit touches;
    // everything before it is kept as it is)
    let mut from = dirty.first().map_or(usize::MAX, |&d| d as usize);
    if let (Some(oc), Some(e)) = (oldc, edit) {
        let touched = oc
            .nodes
            .iter()
            .position(|n| n.out.pos >= e.start)
            .unwrap_or(oc.nodes.len());
        from = from.min(touched);
    }
    let from = oldc.map_or(0, |oc| from.min(oc.nodes.len().saturating_sub(1)));
    if let Some(oc) = oldc {
        for n in &oc.nodes[..from] {
            chain.push_arc(n.clone());
        }
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
            while oi < oc.nodes.len() && mapped(oc.input(oi).pos).is_none_or(|p| p < state.pos) {
                let n = &oc.nodes[oi];
                scope_changed |= !n.defs().is_empty() || n.kind.scopes();
                oi += 1;
            }
            if oi < oc.nodes.len() {
                let n = &oc.nodes[oi];
                let mut inp = oc.input(oi);
                let start_ok = mapped(inp.pos) == Some(state.pos);
                // (the text it read, through the character after it, unchanged)
                let text_ok = edit.is_none_or(|e| n.out.pos < e.start || inp.pos >= e.old_end);
                let par_ok = !(matches!(n.kind, Kind::Par | Kind::End) && para_fresh);
                inp.pos = state.pos;
                if start_ok && text_ok && inp == state {
                    cand = Some(oi);
                    let reads_ok = par_ok
                        && n.reads
                            .iter()
                            .all(|r| resolve_now(&chain, env, &mut outside, &r.key).0 == r.val);
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
                                r.src = resolve_now(&chain, env, &mut outside, &r.key).1;
                            }
                            kept = Some(n);
                        }
                    }
                }
            }
        }
        if let Some(stop) = copy_to {
            // (node `oi` was looked at; each node after it is shared as long
            // as nothing it read can differ: the definitions it read in the
            // piece are at the same nodes with the same values (no scope
            // changed), but a definition from before the piece is checked
            // read by read, since only the first node reading a name is
            // marked; a paragraph's node is looked at if a node of its
            // paragraph ran)
            let oc = oldc.expect("an old chain");
            let mut j = oi;
            while j < stop {
                let n = &oc.nodes[j];
                if j > oi {
                    if para_fresh && matches!(n.kind, Kind::Par | Kind::End) {
                        break;
                    }
                    let holds = n.reads.iter().all(|r| {
                        matches!(r.src, Src::Node(_))
                            || outside_read(env, &mut outside, &r.key).0 == r.val
                    });
                    if !holds {
                        break;
                    }
                }
                if oc.input(j).mode == Mode::V && n.out.mode == Mode::H {
                    para_fresh = false;
                }
                chain.push_arc(n.clone());
                j += 1;
            }
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
                x.command();
                chain.with_group(x.finish())
            }
        };
        if fresh {
            // (the same command run again, defining the same and leaving the
            // same state, changes nothing about how later names resolve)
            let same_as_old = cand.is_some_and(|ci| {
                let o = &oldc.expect("an old chain").nodes[ci];
                ci == chain.nodes.len()
                    && o.kind == n.kind
                    && o.out == n.out
                    && o.defs().len() == n.defs().len()
                    && o.defs()
                        .iter()
                        .zip(n.defs())
                        .all(|(a, b)| a.key == b.key && a.val == b.val && a.global == b.global)
            });
            if same_as_old {
                oi += 1;
            } else {
                scope_changed |= !n.defs().is_empty() || n.kind.scopes();
            }
        }
        let starts_para = state.mode == Mode::V && n.out.mode == Mode::H;
        if starts_para {
            para_fresh = fresh;
        } else if fresh {
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
    let end = chain.nodes.len();
    let mut exports: Vec<(Key, Val)> = chain
        .defs_of
        .keys()
        .filter_map(|k| chain.resolve(k, end).map(|(_, v)| (k.clone(), v)))
        .collect();
    exports.sort_by(|a, b| a.0.cmp(&b.0));
    let paras = chain
        .nodes
        .iter()
        .filter_map(|n| n.para().cloned())
        .collect();
    let errors = chain
        .nodes
        .iter()
        .flat_map(|n| n.errors().iter().cloned())
        .collect();
    let mut reads = Vec::new();
    let mut seen: FxSet<&Key> = FxSet::default();
    for (i, n) in chain.nodes.iter().enumerate() {
        for r in n.reads.iter() {
            if !matches!(r.src, Src::Node(_)) && seen.insert(&r.key) {
                reads.push((r.key.clone(), i as u32, r.val.clone()));
            }
        }
    }
    Run {
        chain,
        reads,
        exports,
        paras,
        errors,
        ran,
    }
}

/// Buffers reused from node to node (their capacity, never their
/// content).
#[derive(Default)]
struct Scratch {
    /// The node's reads so far (read once each), once there are many.
    seen: FxMap<Key, Val>,
    /// The node's reads and what it emitted (their capacity kept).
    reads: Vec<Read>,
    emit: Vec<HItem>,
    /// Shared copies of the names read in this piece.
    names: FxSet<Arc<str>>,
    buf: String,
}

/// One node's execution: one command, from the state its predecessor left.
struct Exec<'a> {
    text: &'a [char],
    chain: &'a Chain,
    env: &'a mut dyn Env,
    outside: &'a mut Outside,
    scratch: &'a mut Scratch,
    st: RState,
    reads: Vec<Read>,
    defs: Vec<Def>,
    emit: Vec<HItem>,
    para: Option<Arc<Para>>,
    errors: Vec<String>,
    phi: Option<Phi>,
    kind: Kind,
    cmd: Tok,
    /// How many of `reads` are in `scratch.seen`.
    seen_upto: usize,
}

/// Up to this many reads, a node's reads are looked up by a scan.
const FEW_READS: usize = 12;

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
            para: None,
            errors: Vec::new(),
            phi: None,
            kind: Kind::Other,
            cmd: Tok::Char(0, '\0'),
            seen_upto: 0,
        }
    }

    fn finish(mut self) -> Node {
        let reads = Few::take(&mut self.reads);
        let emit = Few::take(&mut self.emit);
        self.scratch.reads = self.reads;
        self.scratch.emit = self.emit;
        let rare = (!self.defs.is_empty()
            || self.para.is_some()
            || !self.errors.is_empty()
            || self.phi.is_some())
        .then(|| {
            Box::new(Rare {
                defs: self.defs,
                para: self.para,
                errors: self.errors,
                phi: self.phi,
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
        if let Some(d) = self.defs.iter().rev().find(|d| d.key == *key) {
            return d.val.clone();
        }
        if self.reads.len() <= FEW_READS {
            if let Some(r) = self.reads.iter().find(|r| r.key == *key) {
                return r.val.clone();
            }
        } else {
            for r in &self.reads[self.seen_upto..] {
                self.scratch
                    .seen
                    .entry(r.key.clone())
                    .or_insert_with(|| r.val.clone());
            }
            self.seen_upto = self.reads.len();
            if let Some(v) = self.scratch.seen.get(key) {
                return v.clone();
            }
        }
        let (v, src) = resolve_now(self.chain, self.env, self.outside, key);
        self.reads.push(Read {
            key: key.clone(),
            src,
            val: v.clone(),
        });
        v
    }

    fn define(&mut self, key: Key, val: Val, global: bool) {
        self.defs.push(Def { key, val, global });
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

    fn param(&mut self, p: Param) -> i32 {
        match self.get(&Key::Param(p)) {
            Val::Int(i) => i,
            _ => 0,
        }
    }

    fn font(&mut self) -> Arc<Font> {
        match self.get(&Key::Font) {
            Val::Font(f) => f,
            other => panic!("the font key holds {other:?}"),
        }
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

    // ------------------------------------------------------------ the input

    fn back(&mut self, t: Tok) {
        self.st.pending.push((Arc::from([t]), 0));
    }

    /// Rewrites the input: `toks`, then the rest.
    fn rewrite(&mut self, toks: Vec<Tok>) {
        if !toks.is_empty() {
            self.st.pending.push((Arc::from(toks), 0));
        }
    }

    /// The next token, unexpanded (tex.web 343-357, without `^^`).
    fn next_raw(&mut self) -> Option<Tok> {
        while let Some((list, i)) = self.st.pending.last_mut() {
            if *i < list.len() {
                let t = list[*i].clone();
                *i += 1;
                if *i == list.len() {
                    self.st.pending.pop();
                }
                return Some(t);
            }
            self.st.pending.pop();
        }
        loop {
            let c = *self.text.get(self.st.pos)?;
            let cat = self.catcode(c);
            self.st.pos += 1;
            match cat {
                0 => {
                    let Some(&c2) = self.text.get(self.st.pos) else {
                        self.st.tstate = TState::S;
                        return Some(self.cs(""));
                    };
                    let cat2 = self.catcode(c2);
                    if cat2 == 11 {
                        let start = self.st.pos;
                        while self.st.pos < self.text.len() {
                            let c3 = self.text[self.st.pos];
                            if self.catcode(c3) != 11 {
                                break;
                            }
                            self.st.pos += 1;
                        }
                        self.scratch.buf.clear();
                        self.scratch.buf.extend(&self.text[start..self.st.pos]);
                        self.st.tstate = TState::S;
                        return Some(Tok::Cs(intern(&mut self.scratch.names, &self.scratch.buf)));
                    }
                    self.st.pos += 1;
                    self.st.tstate = if cat2 == 10 { TState::S } else { TState::M };
                    let mut b = [0u8; 4];
                    return Some(self.cs(c2.encode_utf8(&mut b)));
                }
                5 => {
                    let ts = self.st.tstate;
                    self.st.tstate = TState::N;
                    match ts {
                        TState::N => return Some(self.cs("par")),
                        TState::M => return Some(Tok::Char(10, ' ')),
                        TState::S => {}
                    }
                }
                10 => {
                    if self.st.tstate == TState::M {
                        self.st.tstate = TState::S;
                        return Some(Tok::Char(10, ' '));
                    }
                }
                14 => {
                    while self.st.pos < self.text.len() && self.text[self.st.pos] != '\r' {
                        self.st.pos += 1;
                    }
                    self.st.pos += 1;
                    self.st.tstate = TState::N;
                }
                9 => {}
                15 => self.err("Text line contains an invalid character".into()),
                13 => {
                    self.st.tstate = TState::M;
                    return Some(Tok::Active(c));
                }
                _ => {
                    self.st.tstate = TState::M;
                    return Some(Tok::Char(cat, c));
                }
            }
        }
    }

    /// The next token, macro calls expanded; conditionals too unless `top`
    /// (at the top, a conditional is a command: a node of its own).
    fn next_x_in(&mut self, top: bool) -> Option<Tok> {
        loop {
            let t = self.next_raw()?;
            let Some(key) = t.key() else { return Some(t) };
            match self.meaning(&key) {
                Meaning::Macro(m) => self.call(&t, &m),
                Meaning::Prim(p) if p.expandable() && !top => self.conditional(p),
                _ => return Some(t),
            }
        }
    }

    fn next_x(&mut self) -> Option<Tok> {
        self.next_x_in(false)
    }

    // ------------------------------------------------------------ macros

    /// A macro call: matches the arguments and rewrites the input with the
    /// body (tex.web 389-399).
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
                args[usize::from(n)] = if delim.is_empty() {
                    self.undelimited()
                } else {
                    self.delimited(delim)
                };
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

    fn undelimited(&mut self) -> Vec<Tok> {
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning an argument".into());
                return Vec::new();
            };
            match t.cat() {
                Some(10) => {}
                Some(1) => return self.group_body(),
                _ => return vec![t],
            }
        }
    }

    fn group_body(&mut self) -> Vec<Tok> {
        let mut out = Vec::new();
        let mut depth = 1;
        while let Some(t) = self.next_raw() {
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        return out;
                    }
                }
                _ => {}
            }
            out.push(t);
        }
        self.err("File ended while scanning an argument".into());
        out
    }

    fn delimited(&mut self, delim: &[Tok]) -> Vec<Tok> {
        let mut buf: Vec<Tok> = Vec::new();
        let mut depth = 0;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a delimited argument".into());
                break;
            };
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => depth -= 1,
                _ => {}
            }
            buf.push(t);
            if depth == 0 && buf.len() >= delim.len() && buf[buf.len() - delim.len()..] == *delim {
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
        buf
    }

    // ------------------------------------------------------------ conditionals

    /// A conditional primitive (as a command node at the top, or inline
    /// inside another command's scanning).
    fn conditional(&mut self, p: Prim) {
        match p {
            Prim::Ifx => {
                let a = self.next_raw();
                let b = self.next_raw();
                let t = match (a, b) {
                    (Some(a), Some(b)) => self.tok_meaning(&a) == self.tok_meaning(&b),
                    _ => false,
                };
                self.begin_cond(t);
            }
            Prim::Iftrue => self.begin_cond(true),
            Prim::Iffalse => self.begin_cond(false),
            Prim::Else => {
                if self.st.conds == 0 {
                    self.err("Extra \\else".into());
                    return;
                }
                self.skip(false);
                self.end_cond();
            }
            Prim::Fi => {
                if self.st.conds == 0 {
                    self.err("Extra \\fi".into());
                    return;
                }
                self.end_cond();
            }
            _ => {}
        }
    }

    fn begin_cond(&mut self, test: bool) {
        if test {
            self.st.conds += 1;
            self.kind = Kind::If;
            self.phi = Some(Phi {
                test: true,
                keys: Vec::new(),
            });
            return;
        }
        let at_else = self.skip(true);
        if at_else {
            self.st.conds += 1;
            self.kind = Kind::If;
        } else {
            self.kind = Kind::IfClosed;
        }
        self.phi = Some(Phi {
            test: false,
            keys: Vec::new(),
        });
    }

    /// Skips an arm: up to the matching `\fi`, or `\else` if asked.
    /// Returns whether it stopped at an `\else`.
    fn skip(&mut self, stop_at_else: bool) -> bool {
        let mut depth = 0usize;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("Incomplete \\if".into());
                return false;
            };
            let Some(k) = t.key() else { continue };
            match self.meaning(&k) {
                Meaning::Prim(Prim::Ifx | Prim::Iftrue | Prim::Iffalse) => depth += 1,
                Meaning::Prim(Prim::Fi) => {
                    if depth == 0 {
                        return false;
                    }
                    depth -= 1;
                }
                Meaning::Prim(Prim::Else) if depth == 0 && stop_at_else => return true,
                _ => {}
            }
        }
    }

    /// The end of a conditional: a φ for each key its taken arm defined
    /// (found over the nodes since its `\if`).
    fn end_cond(&mut self) {
        self.st.conds -= 1;
        self.kind = Kind::EndIf;
        let nodes = &self.chain.nodes;
        let here = nodes.len();
        // (the matching `\if`, walking back)
        let mut depth = 0usize;
        let mut at = None;
        for j in (0..here).rev() {
            match nodes[j].kind {
                Kind::EndIf => depth += 1,
                Kind::If => {
                    if depth == 0 {
                        at = Some(j);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        let Some(at) = at else {
            self.phi = Some(Phi {
                test: false,
                keys: Vec::new(),
            });
            return;
        };
        let test = nodes[at].phi().is_some_and(|p| p.test);
        let mut keys: Vec<Key> = Vec::new();
        for n in &nodes[at + 1..here] {
            for d in n.defs() {
                if !keys.contains(&d.key) {
                    keys.push(d.key.clone());
                }
            }
        }
        let mut phis = Vec::new();
        for k in keys {
            let Some((d, after)) = self.chain.resolve(&k, here) else {
                continue;
            };
            self.reads.push(Read {
                key: k.clone(),
                src: Src::Node(d),
                val: after.clone(),
            });
            let (before, src) = match self.chain.resolve(&k, at) {
                Some((d, v)) => (v, Src::Node(d)),
                None => outside_read(self.env, self.outside, &k),
            };
            self.reads.push(Read {
                key: k.clone(),
                src,
                val: before.clone(),
            });
            // (the φ is a definition: readers after the `\fi` read it)
            self.defs.push(Def {
                key: k.clone(),
                val: after.clone(),
                global: false,
            });
            phis.push((k, after, before));
        }
        self.phi = Some(Phi { test, keys: phis });
    }

    // ------------------------------------------------------------ commands

    /// One command.
    fn command(&mut self) {
        let Some(t) = self.next_x_in(true) else {
            self.end();
            return;
        };
        self.cmd = t.clone();
        let global = std::mem::take(&mut self.st.global);
        match &t {
            Tok::Char(cat, c) => self.char_tok(*cat, *c),
            Tok::Param(_) => self.err("Illegal parameter number in a paragraph".into()),
            Tok::Cs(_) | Tok::Active(_) => {
                let key = t.key().expect("a name");
                match self.meaning(&key) {
                    Meaning::Prim(Prim::Global) => self.st.global = true,
                    Meaning::Prim(p) if p.expandable() => self.conditional(p),
                    Meaning::Prim(p) => self.prim(p, global),
                    Meaning::Char(cat, c) => self.char_tok(cat, c),
                    Meaning::Undefined => self.err(format!("Undefined control sequence {t}")),
                    Meaning::Macro(_) => {}
                }
            }
        }
    }

    /// The end of the piece: its last paragraph, its open groups and
    /// conditionals.
    fn end(&mut self) {
        self.end_par();
        self.kind = Kind::End;
        if self.st.depth > 0 {
            self.err("Missing } inserted at the end of a piece".into());
        }
        if self.st.conds > 0 {
            self.err("Incomplete \\if at the end of a piece".into());
        }
    }

    fn char_tok(&mut self, cat: u8, c: char) {
        match cat {
            1 => {
                self.st.depth += 1;
                self.kind = Kind::Open;
            }
            2 => {
                if self.st.depth > 0 {
                    self.st.depth -= 1;
                    self.kind = Kind::Close;
                } else {
                    self.err("Too many }'s".into());
                }
            }
            10 => {
                if self.st.mode == Mode::H {
                    self.space();
                }
            }
            11 | 12 => self.letter(c),
            _ => self.err(format!(
                "Character {c} of category {cat} is not supported yet"
            )),
        }
    }

    fn prim(&mut self, p: Prim, global: bool) {
        match p {
            Prim::Def => self.def(global),
            Prim::Gdef => self.def(true),
            Prim::Let => self.let_(global),
            Prim::Catcode => self.catcode_assign(global),
            Prim::Param(q) => self.param_assign(q, global),
            Prim::Par => self.end_par(),
            Prim::Indent => {
                if self.st.mode == Mode::V {
                    self.begin_par(true);
                } else {
                    let w = self.param(Param::Parindent);
                    self.emit.push(HItem::Box { w, h: 0, d: 0 });
                }
            }
            Prim::Noindent => {
                if self.st.mode == Mode::V {
                    self.begin_par(false);
                }
            }
            Prim::Space => {
                if self.st.mode == Mode::V {
                    self.begin_par(true);
                }
                self.space();
            }
            Prim::Relax
            | Prim::Global
            | Prim::Ifx
            | Prim::Iftrue
            | Prim::Iffalse
            | Prim::Else
            | Prim::Fi => {}
        }
    }

    fn def(&mut self, global: bool) {
        let Some(name) = self.next_raw() else { return };
        let Some(key) = name.key() else {
            self.err("Missing control sequence inserted".into());
            return;
        };
        let mut params = Vec::new();
        loop {
            let Some(t) = self.next_raw() else { return };
            match t.cat() {
                Some(1) => break,
                Some(6) => match self.next_raw() {
                    Some(Tok::Char(_, d)) if d.is_ascii_digit() && d != '0' => {
                        params.push(Tok::Param(d as u8 - b'0'));
                    }
                    _ => self.err("Parameters must be numbered consecutively".into()),
                },
                _ => params.push(t),
            }
        }
        let mut body = Vec::new();
        let mut depth = 1;
        loop {
            let Some(t) = self.next_raw() else {
                self.err("File ended while scanning a definition".into());
                break;
            };
            match t.cat() {
                Some(1) => {
                    depth += 1;
                    body.push(t);
                }
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    body.push(t);
                }
                Some(6) => match self.next_raw() {
                    Some(Tok::Char(_, d)) if d.is_ascii_digit() && d != '0' => {
                        body.push(Tok::Param(d as u8 - b'0'));
                    }
                    Some(x) if x.cat() == Some(6) => body.push(x),
                    other => {
                        self.err("Illegal parameter number in a definition".into());
                        if let Some(x) = other {
                            body.push(x);
                        }
                    }
                },
                _ => body.push(t),
            }
        }
        self.define(
            key,
            Val::Meaning(Meaning::Macro(Arc::new(Macro { params, body }))),
            global,
        );
    }

    fn let_(&mut self, global: bool) {
        let Some(name) = self.next_raw() else { return };
        let Some(key) = name.key() else {
            self.err("Missing control sequence inserted".into());
            return;
        };
        let mut t = self.next_raw();
        while matches!(t, Some(Tok::Char(10, _))) {
            t = self.next_raw();
        }
        if matches!(t, Some(Tok::Char(12, '='))) {
            t = self.next_raw();
            if matches!(t, Some(Tok::Char(10, _))) {
                t = self.next_raw();
            }
        }
        let Some(t) = t else { return };
        let m = self.tok_meaning(&t);
        self.define(key, Val::Meaning(m), global);
    }

    fn catcode_assign(&mut self, global: bool) {
        let c = self.scan_int();
        self.optional_equals();
        let v = self.scan_int();
        let Some(ch) = char::from_u32(c as u32) else {
            self.err("Bad character code".into());
            return;
        };
        if !(0..=15).contains(&v) {
            self.err("Invalid code".into());
            return;
        }
        self.define(Key::Cat(ch), Val::Cat(v as u8), global);
    }

    fn param_assign(&mut self, p: Param, global: bool) {
        self.optional_equals();
        let v = if p.is_int() {
            self.scan_int()
        } else {
            self.scan_dimen()
        };
        self.define(Key::Param(p), Val::Int(v), global);
    }

    // ------------------------------------------------------------ scanning

    fn skip_spaces_x(&mut self) -> Option<Tok> {
        loop {
            let t = self.next_x()?;
            if t.cat() != Some(10) {
                return Some(t);
            }
        }
    }

    fn optional_equals(&mut self) {
        if let Some(t) = self.skip_spaces_x()
            && !t.is_char(12, '=')
        {
            self.back(t);
        }
    }

    fn optional_space(&mut self) {
        if let Some(t) = self.next_x()
            && t.cat() != Some(10)
        {
            self.back(t);
        }
    }

    fn signs(&mut self) -> (bool, Option<Tok>) {
        let mut neg = false;
        loop {
            let t = self.skip_spaces_x();
            match &t {
                Some(x) if x.is_char(12, '-') => neg = !neg,
                Some(x) if x.is_char(12, '+') => {}
                _ => return (neg, t),
            }
        }
    }

    fn internal(&mut self, t: &Tok) -> Option<(i32, bool)> {
        let k = t.key()?;
        match self.meaning(&k) {
            Meaning::Prim(Prim::Param(p)) => Some((self.param(p), p.is_int())),
            _ => None,
        }
    }

    /// An integer (tex.web 440-447).
    fn scan_int(&mut self) -> i32 {
        let (neg, t) = self.signs();
        let Some(t) = t else {
            self.err("Missing number, treated as zero".into());
            return 0;
        };
        let v: i64 = if t.is_char(12, '`') {
            let c = match self.next_raw() {
                Some(Tok::Char(_, c) | Tok::Active(c)) => i64::from(u32::from(c)),
                Some(Tok::Cs(n)) => {
                    let mut it = n.chars();
                    if let (Some(c), None) = (it.next(), it.next()) {
                        i64::from(u32::from(c))
                    } else {
                        self.err("Improper alphabetic constant".into());
                        0
                    }
                }
                _ => 0,
            };
            self.optional_space();
            c
        } else if let Some((v, _)) = self.internal(&t) {
            i64::from(v)
        } else {
            let (radix, mut first) = if t.is_char(12, '\'') {
                (8u32, None)
            } else if t.is_char(12, '"') {
                (16, None)
            } else {
                (10, Some(t))
            };
            let mut v: i64 = 0;
            let mut any = false;
            loop {
                let x = match first.take() {
                    Some(x) => Some(x),
                    None => self.next_x(),
                };
                let Some(x) = x else { break };
                let d = match &x {
                    Tok::Char(12, c) if c.is_ascii_digit() => c.to_digit(10),
                    Tok::Char(11 | 12, c) if radix == 16 && ('A'..='F').contains(c) => {
                        c.to_digit(16)
                    }
                    _ => None,
                };
                match d {
                    Some(d) if d < radix => {
                        v = (v * i64::from(radix) + i64::from(d)).min(i64::from(i32::MAX));
                        any = true;
                    }
                    _ => {
                        if x.cat() != Some(10) {
                            self.back(x);
                        }
                        break;
                    }
                }
            }
            if !any {
                self.err("Missing number, treated as zero".into());
            }
            v
        };
        let v = v as i32;
        if neg { -v } else { v }
    }

    fn keyword(&mut self, kw: &str) -> bool {
        let kws: Vec<char> = kw.chars().collect();
        let mut buf = Vec::new();
        let mut i = 0;
        while i < kws.len() {
            let Some(t) = self.next_x() else {
                for x in buf.into_iter().rev() {
                    self.back(x);
                }
                return false;
            };
            let hit = matches!(&t, Tok::Char(_, c) if c.to_ascii_lowercase() == kws[i]);
            if hit {
                buf.push(t);
                i += 1;
            } else if buf.is_empty() && t.cat() == Some(10) {
            } else {
                self.back(t);
                for x in buf.into_iter().rev() {
                    self.back(x);
                }
                return false;
            }
        }
        true
    }

    /// A dimension (tex.web 448-461).
    fn scan_dimen(&mut self) -> i32 {
        let (neg, t) = self.signs();
        let Some(t) = t else {
            self.err("Missing number, treated as zero".into());
            return 0;
        };
        if let Some((v, is_int)) = self.internal(&t) {
            if !is_int {
                return if neg { -v } else { v };
            }
            return self.units(neg, v, 0);
        }
        let mut ip: i64 = 0;
        let mut digits: Vec<u8> = Vec::new();
        let mut point = false;
        let mut any = false;
        let mut first = Some(t);
        loop {
            let x = match first.take() {
                Some(x) => Some(x),
                None => self.next_x(),
            };
            let Some(x) = x else { break };
            match &x {
                Tok::Char(12, c) if c.is_ascii_digit() => {
                    let d = c.to_digit(10).unwrap_or(0);
                    any = true;
                    if point {
                        if digits.len() < 17 {
                            digits.push(d as u8);
                        }
                    } else {
                        ip = (ip * 10 + i64::from(d)).min(i64::from(i32::MAX));
                    }
                }
                Tok::Char(12, '.' | ',') if !point => point = true,
                _ => {
                    self.back(x);
                    break;
                }
            }
        }
        if !any && !point {
            self.err("Missing number, treated as zero".into());
        }
        let f = if digits.is_empty() {
            0
        } else {
            round_decimals(&digits)
        };
        self.units(neg, ip as i32, f)
    }

    fn units(&mut self, neg: bool, mut v: i32, mut f: i32) -> i32 {
        let font_unit = if self.keyword("em") {
            Some(6)
        } else if self.keyword("ex") {
            Some(5)
        } else {
            None
        };
        if let Some(n) = font_unit {
            let q = self.font().param(n);
            let r = i64::from(v) * i64::from(q) + i64::from(xn_over_d(q, f, 65536).0);
            self.optional_space();
            let r = crate::arith::clamp(r);
            return if neg { -r } else { r };
        }
        let _ = self.keyword("true");
        let (num, den) = if self.keyword("pt") {
            (1, 1)
        } else if self.keyword("in") {
            (7227, 100)
        } else if self.keyword("pc") {
            (12, 1)
        } else if self.keyword("cm") {
            (7227, 254)
        } else if self.keyword("mm") {
            (7227, 2540)
        } else if self.keyword("bp") {
            (7227, 7200)
        } else if self.keyword("dd") {
            (1238, 1157)
        } else if self.keyword("cc") {
            (14856, 1157)
        } else if self.keyword("sp") {
            self.optional_space();
            return if neg { -v } else { v };
        } else {
            self.err("Illegal unit of measure (pt inserted)".into());
            (1, 1)
        };
        if num != 1 || den != 1 {
            let (q, rem) = xn_over_d(v, num, den);
            f = ((i64::from(num) * i64::from(f) + 65536 * i64::from(rem)) / i64::from(den)) as i32;
            v = q + f / 65536;
            f %= 65536;
        }
        if v >= 0o40000 {
            self.err("Dimension too large".into());
            v = 0o37777;
            f = 0o177_777;
        }
        let r = v * 65536 + f;
        self.optional_space();
        if neg { -r } else { r }
    }

    // ------------------------------------------------------------ material

    fn begin_par(&mut self, indent: bool) {
        self.st.mode = Mode::H;
        self.st.parskip = self.param(Param::Parskip);
        if indent {
            let w = self.param(Param::Parindent);
            self.emit.push(HItem::Box { w, h: 0, d: 0 });
        }
    }

    fn letter(&mut self, c: char) {
        if self.st.mode == Mode::V {
            self.begin_par(true);
        }
        let font = self.font();
        match font.char(u32::from(c)) {
            Some(d) if u32::from(c) < 256 => self.emit.push(HItem::Char {
                c: c as u8,
                w: d.w,
                h: d.h,
                d: d.d,
            }),
            _ => self.err(format!("Missing character {c} in font {}", font.name)),
        }
    }

    fn space(&mut self) {
        let f = self.font();
        self.emit.push(HItem::Glue {
            w: f.param(2),
            st: f.param(3),
            st_ord: 0,
            sh: f.param(4),
            sh_ord: 0,
        });
    }

    /// `\par`: the paragraph, from the material its nodes emitted, with the
    /// parameters line breaking and pages read (tex.web 1096).
    fn end_par(&mut self) {
        if self.st.mode != Mode::H {
            return;
        }
        self.kind = Kind::Par;
        self.st.mode = Mode::V;
        // (the paragraph's nodes: back to the one that began it)
        let nodes = &self.chain.nodes;
        let mut first = nodes.len();
        for j in (0..nodes.len()).rev() {
            first = j;
            if !self.chain.input_h(j) {
                break;
            }
        }
        let mut hlist: Vec<HItem> = nodes[first..]
            .iter()
            .flat_map(|n| n.emit.iter().copied())
            .collect();
        hlist.append(&mut self.emit);
        if matches!(hlist.last(), Some(HItem::Glue { .. })) {
            hlist.pop();
        }
        hlist.push(HItem::Penalty(10000));
        hlist.push(HItem::Glue {
            w: 0,
            st: 65536,
            st_ord: 1,
            sh: 0,
            sh_ord: 0,
        });
        let para = Para {
            hlist,
            hsize: self.param(Param::Hsize),
            tolerance: self.param(Param::Tolerance),
            linepenalty: self.param(Param::Linepenalty),
            baselineskip: self.param(Param::Baselineskip),
            parskip: self.st.parskip,
            vsize: self.param(Param::Vsize),
            topskip: self.param(Param::Topskip),
            maxdepth: self.param(Param::Maxdepth),
        };
        self.para = Some(Arc::new(para));
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
