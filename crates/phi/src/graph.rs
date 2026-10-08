//! The graph (DESIGN 7.3–7.7, 7.11–7.12): one arena of nodes, built by
//! evaluating it, kept up to date by pushing changes in position order.
//!
//! - Nodes live in columns indexed by [`NodeId`]. A node's position is
//!   its Dewey path: its parent and its order label among its siblings.
//! - Every edge has a reverse entry on its source, so a change wakes
//!   exactly the readers of what changed, field by field.
//! - Steps are the only nodes that make nodes. Their emissions are
//!   matched to their old children by key; their successors are matched
//!   by key, cursor, group and state (convergence).
//! - Names are an index of definitions by position with scopes.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::marker::PhantomData;

use crate::lang::{Chain, Class, Lang, Slot, Step};
use crate::seq::{ElemId, Seq};
use crate::value::{Proj, Sel, Value, project};
use crate::ver::{Ver, hash64};

/// A node's index in the arena.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u32);

/// An interned name.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NameId(pub u32);

pub(crate) const NONE: u32 = u32::MAX;
const ROOT: u32 = 0;
/// No group: definitions at the outer level live to the end.
const NOGROUP: u64 = 0;
/// The cursor past the last element.
pub const END: ElemId = ElemId(u64::MAX);

/// What a node is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    Root,
    /// A value the client sets between runs.
    Input,
    /// A constant a step emitted.
    Const,
    Leaf,
    Unfold,
    Step,
    Scan,
    /// A cross-run slot's value.
    Cross,
    /// A chain's payloads.
    ChainRead,
}

const DIRTY: u8 = 1;
const QUEUED: u8 = 2;
const DEAD: u8 = 4;

/// An operand: what it reads (`src`, through `sel`), and the name it
/// was resolved from, if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Opd {
    pub src: u32,
    pub sel: Sel,
    pub name: u32,
}

/// A reverse edge: `node` reads the list's owner (valid while `node`'s
/// generation is `era`).
#[derive(Clone, Copy)]
struct Rev {
    node: u32,
    era: u32,
    next: u32,
}

/// A position: a node's (its parent, its label), or a definition's or a
/// scope event's (the step, its emission order).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Pos {
    pub parent: u32,
    pub ord: u64,
}

#[derive(Clone, Debug)]
struct DefRec {
    name: u32,
    sub: u64,
    src: u32,
    sel: Sel,
    group: u64,
    global: bool,
}

/// A step's own record.
#[derive(Clone, Debug, Default)]
pub(crate) struct StepInfo {
    pub unfold: u32,
    pub key: u64,
    /// The cursor at the step's start.
    pub at: ElemId,
    /// Input elements consumed.
    pub took: u32,
    pub grp_in: u64,
    pub grp_out: u64,
    /// The version of the state it ran from.
    pub in_ver: Ver,
    defs: Vec<DefRec>,
    opened: Vec<u64>,
    closed: Vec<u64>,
}

pub(crate) struct UnfoldInfo<V> {
    pub keys: HashMap<u64, u32>,
    /// Each input element that starts a step's consumption → the step.
    pub owners: HashMap<ElemId, u32>,
    pub input: Option<Seq<V>>,
    pub args_ver: Ver,
    /// During a resume after an input edit: where the changed elements
    /// end (new indices); a successor starting before it runs again.
    pub hunk_end: Option<usize>,
    /// The group open where the unfold was made.
    pub grp0: u64,
}

pub(crate) struct ScanInfo<V> {
    pub input: Seq<V>,
    pub states: Vec<V>,
    pub outs: Seq<V>,
    pub init_ver: Ver,
    pub args_ver: Ver,
    pub done: bool,
}

#[derive(Clone, Debug)]
struct DefEntry {
    pos: Pos,
    src: u32,
    sel: Sel,
    group: u64,
    global: bool,
}

#[derive(Default)]
struct NameTab {
    by_hash: HashMap<u64, u32>,
    spell: Vec<Box<[u8]>>,
    defs: Vec<Vec<DefEntry>>,
    /// Nodes that read the name, with their generation (lazily pruned).
    readers: Vec<Vec<(u32, u32)>>,
}

#[derive(Clone, Debug)]
struct Group {
    parent: u64,
    close: Option<Pos>,
    names: Vec<u32>,
}

/// The counts of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Leaves, constants, crosses and chain reads evaluated.
    pub evals: u64,
    /// Steps run.
    pub steps: u64,
    /// Scan elements stepped.
    pub scanned: u64,
    pub created: u64,
    pub removed: u64,
    /// Nodes woken by a change they read.
    pub woken: u64,
    /// Runs of the cross-run loop beyond the first.
    pub iterations: u32,
    /// Slots still changing when the loop's bound was reached, each with
    /// the versions it took, in order.
    pub oscillating: Vec<(Slot, Vec<Ver>)>,
    /// Groups closed with none open (a client error).
    pub unbalanced: u64,
    /// The largest step state seen, in bytes ([`Config::debug`]).
    pub state_max: usize,
    /// The sum of step states' bytes, over `steps`.
    pub state_sum: u64,
}

/// Switches.
#[derive(Clone, Debug)]
pub struct Config {
    /// Bound on cross-run iterations.
    pub max_iters: u32,
    /// Evaluate every leaf again after the run and compare (impurity or a
    /// missed wake panics).
    pub check: bool,
    /// Report step state sizes.
    pub debug: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_iters: 5,
            check: false,
            debug: false,
        }
    }
}

/// The node columns.
pub struct Nodes<L: Lang> {
    pub(crate) kind: Vec<Kind>,
    pub(crate) op: Vec<L::Op>,
    pub(crate) class: Vec<Class>,
    pub(crate) parent: Vec<u32>,
    pub(crate) ord: Vec<u64>,
    pub(crate) depth: Vec<u16>,
    pub(crate) next: Vec<u32>,
    pub(crate) first: Vec<u32>,
    pub(crate) key: Vec<u32>,
    pub(crate) a0: Vec<u32>,
    pub(crate) an: Vec<u16>,
    rd: Vec<u32>,
    pub(crate) val: Vec<L::Val>,
    pub(crate) flags: Vec<u8>,
    era: Vec<u32>,
    /// Index into the kind's table (steps, unfolds, scans), or the slot
    /// or chain.
    pub(crate) aux: Vec<u64>,
    pub(crate) opds: Vec<Opd>,
    revs: Vec<Rev>,
    absent: L::Val,
}

impl<L: Lang> Nodes<L> {
    #[inline]
    pub(crate) fn pos(&self, n: u32) -> Pos {
        Pos {
            parent: self.parent[n as usize],
            ord: self.ord[n as usize],
        }
    }

    #[inline]
    fn pdepth(&self, p: Pos) -> u16 {
        self.depth[p.parent as usize] + 1
    }

    /// Position order: pre-order of the region tree.
    pub(crate) fn cmp_pos(&self, a: Pos, b: Pos) -> Ordering {
        if a.parent == b.parent {
            return a.ord.cmp(&b.ord);
        }
        let (mut a, mut b) = (a, b);
        let (mut da, mut db) = (self.pdepth(a), self.pdepth(b));
        let mut lifted_a = false;
        let mut lifted_b = false;
        while da > db {
            a = self.pos(a.parent);
            da -= 1;
            lifted_a = true;
        }
        while db > da {
            b = self.pos(b.parent);
            db -= 1;
            lifted_b = true;
        }
        if a == b {
            // (one is inside the other's subtree: the ancestor first)
            return match (lifted_a, lifted_b) {
                (true, _) => Ordering::Greater,
                (_, true) => Ordering::Less,
                _ => Ordering::Equal,
            };
        }
        while a.parent != b.parent {
            a = self.pos(a.parent);
            b = self.pos(b.parent);
        }
        a.ord.cmp(&b.ord)
    }

    #[inline]
    pub(crate) fn cmp_node(&self, a: u32, b: u32) -> Ordering {
        self.cmp_pos(self.pos(a), self.pos(b))
    }

    #[inline]
    pub(crate) fn opds_of(&self, n: u32) -> &[Opd] {
        let a = self.a0[n as usize] as usize;
        &self.opds[a..a + self.an[n as usize] as usize]
    }

    #[inline]
    fn read(&self, o: &Opd) -> Proj<'_, L::Val> {
        if o.src == NONE {
            Proj::Ref(&self.absent)
        } else {
            project(&self.val[o.src as usize], o.sel, &self.absent)
        }
    }

    #[inline]
    fn read_ver(&self, o: &Opd) -> Ver {
        if o.src == NONE {
            self.absent.ver()
        } else {
            o.sel.ver(&self.val[o.src as usize])
        }
    }

    pub(crate) fn is_dead(&self, n: u32) -> bool {
        self.flags[n as usize] & DEAD != 0
    }

    /// The children (or steps) of `n`, in order.
    pub(crate) fn children(&self, n: u32) -> Vec<u32> {
        let mut out = Vec::new();
        let mut c = self.first[n as usize];
        while c != NONE {
            out.push(c);
            c = self.next[c as usize];
        }
        out
    }
}

/// The operand values an op sees.
pub struct Args<'a, L: Lang> {
    g: &'a Nodes<L>,
    opds: &'a [Opd],
}

impl<'a, L: Lang> Args<'a, L> {
    #[must_use]
    pub fn len(&self) -> usize {
        self.opds.len()
    }
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.opds.is_empty()
    }
    /// Operand `i`'s value.
    ///
    /// # Panics
    ///
    /// If there is no operand `i`.
    #[must_use]
    pub fn get(&self, i: usize) -> Proj<'a, L::Val> {
        self.g.read(&self.opds[i])
    }
}

/// A handle on a node a step emitted, valid only in that step (the
/// brand `'s` cannot escape it).
#[derive(Clone, Copy)]
pub struct Local<'s> {
    ix: u32,
    _brand: PhantomData<fn(&'s ()) -> &'s ()>,
}

/// An operand of a node a step emits.
#[derive(Clone, Copy)]
pub enum Arg<'s> {
    Local(Local<'s>),
    Field(Local<'s>, u32),
    /// The unfold's operand `i` (0 its input, 1 its initial state, then
    /// its arguments).
    Up(u16),
    UpField(u16, u32),
    /// A name, resolved at the emitted node's position.
    Name(NameId),
}

#[derive(Clone, Copy)]
enum SArg {
    Local(u32, Sel),
    Node(u32, Sel),
    Name(u32),
}

struct Spec<V, O> {
    kind: Kind,
    op: O,
    class: Class,
    key: u32,
    args: (u32, u16),
    lit: Option<V>,
    aux: u64,
    sub: u64,
    grp: u64,
}

enum Event {
    /// A group opened, and the group open before it.
    Open(u64, u64),
    Close(u64),
}

/// A step's emissions.
struct Emit<L: Lang> {
    specs: Vec<Spec<L::Val, L::Op>>,
    args: Vec<SArg>,
    defs: Vec<(u32, SArg, bool, u64, u64)>,
    events: Vec<(Event, u64)>,
    reads: Vec<(u32, Opd)>,
    new_names: Vec<(u64, Box<[u8]>)>,
    next_key: Option<u64>,
    sub: u64,
}

impl<L: Lang> Default for Emit<L> {
    fn default() -> Self {
        Emit {
            specs: Vec::new(),
            args: Vec::new(),
            defs: Vec::new(),
            events: Vec::new(),
            reads: Vec::new(),
            new_names: Vec::new(),
            next_key: None,
            sub: 0,
        }
    }
}

impl<L: Lang> Emit<L> {
    fn clear(&mut self) {
        self.specs.clear();
        self.args.clear();
        self.defs.clear();
        self.events.clear();
        self.reads.clear();
        self.new_names.clear();
        self.next_key = None;
        self.sub = 0;
    }
}

/// What a step sees and does besides its state and operands: its input,
/// names, and the nodes, definitions and scopes it makes.
pub struct StepCx<'s, L: Lang> {
    g: &'s Nodes<L>,
    names: &'s NameTab,
    groups: &'s HashMap<u64, Group>,
    unfold_opds: &'s [Opd],
    input: Option<&'s Seq<L::Val>>,
    idx: usize,
    start: usize,
    step: u32,
    pos: Pos,
    grp: u64,
    opened: u32,
    em: &'s mut Emit<L>,
    _brand: PhantomData<fn(&'s ()) -> &'s ()>,
}

impl<'s, L: Lang> StepCx<'s, L> {
    /// Consume the next input element.
    pub fn next(&mut self) -> Option<&'s L::Val> {
        let x = self.input?.get(self.idx)?.1;
        self.idx += 1;
        Some(x)
    }

    /// The input element `k` ahead, not consumed.
    #[must_use]
    pub fn peek(&self, k: usize) -> Option<&'s L::Val> {
        Some(self.input?.get(self.idx + k)?.1)
    }

    /// The identity of the next input element ([`END`] past the last).
    #[must_use]
    pub fn cursor(&self) -> ElemId {
        self.input.and_then(|s| s.get(self.idx)).map_or(END, |e| e.0)
    }

    /// The elements consumed so far by this step.
    #[must_use]
    pub fn consumed(&self) -> usize {
        self.idx - self.start
    }

    /// The name spelled `s` (interned by content).
    pub fn name(&mut self, s: &[u8]) -> NameId {
        let h = hash64(s);
        if let Some(&i) = self.names.by_hash.get(&h) {
            return NameId(i);
        }
        let base = u32::try_from(self.names.spell.len()).expect("names fit u32");
        if let Some(k) = self.em.new_names.iter().position(|e| e.0 == h) {
            return NameId(base + u32::try_from(k).expect("names fit u32"));
        }
        self.em.new_names.push((h, s.into()));
        NameId(base + u32::try_from(self.em.new_names.len() - 1).expect("names fit u32"))
    }

    /// Name `n`'s value at this step's start, or a constant this step
    /// defined it to; `None` if undefined. The read is recorded: the step
    /// runs again when it changes.
    pub fn read(&mut self, n: NameId) -> Option<Proj<'s, L::Val>> {
        // (its own definitions first: a constant's value is known now)
        if let Some(d) = self.em.defs.iter().rev().find(|d| d.0 == n.0) {
            if let SArg::Local(ix, sel) = d.1
                && let Some(v) = &self.em.specs[ix as usize].lit
            {
                debug_assert!(sel.is_whole());
                // (a constant of this step: the borrow ends with the step's
                // buffer, so the value is cloned)
                return Some(Proj::Owned(v.clone()));
            }
        }
        let o = if (n.0 as usize) < self.names.defs.len() {
            resolve(self.g, self.names, self.groups, n.0, self.pos)
        } else {
            Opd {
                src: NONE,
                sel: Sel::WHOLE,
                name: n.0,
            }
        };
        self.em.reads.push((n.0, o));
        if o.src == NONE {
            None
        } else {
            Some(self.g.read(&o))
        }
    }

    fn arg(&self, a: Arg<'s>) -> SArg {
        match a {
            Arg::Local(l) => SArg::Local(l.ix, Sel::WHOLE),
            Arg::Field(l, f) => SArg::Local(l.ix, Sel::field(f)),
            Arg::Up(i) => {
                let o = self.unfold_opds[i as usize];
                SArg::Node(o.src, o.sel)
            }
            Arg::UpField(i, f) => {
                let o = self.unfold_opds[i as usize];
                debug_assert!(o.sel.is_whole());
                SArg::Node(o.src, Sel::field(f))
            }
            Arg::Name(n) => SArg::Name(n.0),
        }
    }

    fn push(&mut self, kind: Kind, op: L::Op, class: Class, args: &[Arg<'s>], lit: Option<L::Val>, aux: u64) -> Local<'s> {
        let a0 = u32::try_from(self.em.args.len()).expect("args fit u32");
        for &a in args {
            let s = self.arg(a);
            self.em.args.push(s);
        }
        let ix = u32::try_from(self.em.specs.len()).expect("specs fit u32");
        let key = match self.em.next_key.take() {
            // (explicit keys and ordinals apart)
            Some(k) => {
                #[allow(clippy::cast_possible_truncation)]
                let h = (hash64(&k) as u32) | 0x8000_0000;
                h
            }
            None => ix & 0x7fff_ffff,
        };
        let sub = self.em.sub;
        self.em.sub += 1;
        self.em.specs.push(Spec {
            kind,
            op,
            class,
            key,
            args: (a0, u16::try_from(args.len()).expect("at most 65535 operands")),
            lit,
            aux,
            sub,
            grp: self.grp,
        });
        Local {
            ix,
            _brand: PhantomData,
        }
    }

    /// The next node emitted is keyed by `k` instead of its ordinal.
    pub fn key(&mut self, k: u64) {
        self.em.next_key = Some(k);
    }

    /// A constant.
    pub fn lit(&mut self, v: L::Val) -> Local<'s> {
        self.push(Kind::Const, L::Op::default(), Class::Pure, &[], Some(v), 0)
    }

    /// A leaf.
    pub fn leaf(&mut self, op: L::Op, class: Class, args: &[Arg<'s>]) -> Local<'s> {
        self.push(Kind::Leaf, op, class, args, None, 0)
    }

    /// A nested unfold over `input` from `init`.
    pub fn unfold(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) -> Local<'s> {
        let mut all = vec![input, init];
        all.extend_from_slice(args);
        self.push(Kind::Unfold, op, Class::Pure, &all, None, 0)
    }

    /// A scan over `input` from `init`.
    pub fn scan(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) -> Local<'s> {
        let mut all = vec![input, init];
        all.extend_from_slice(args);
        self.push(Kind::Scan, op, Class::Pure, &all, None, 0)
    }

    /// Slot `s`'s value from the last run (predicted).
    pub fn cross(&mut self, s: Slot) -> Local<'s> {
        self.push(Kind::Cross, L::Op::default(), Class::Pure, &[], None, s.0)
    }

    /// The payloads of chain `c` (all of it: a chain read is placed after
    /// what it reads by the client).
    pub fn chain(&mut self, c: Chain) -> Local<'s> {
        self.push(Kind::ChainRead, L::Op::default(), Class::Pure, &[], None, u64::from(c.0))
    }

    /// Define name `n` here as `v`'s value, local to the open group
    /// unless `global`.
    pub fn define(&mut self, n: NameId, v: Arg<'s>, global: bool) {
        let s = self.arg(v);
        let sub = self.em.sub;
        self.em.sub += 1;
        let grp = if global { NOGROUP } else { self.grp };
        self.em.defs.push((n.0, s, global, sub, grp));
    }

    /// Open a group: local definitions after it end at its close.
    pub fn open_group(&mut self) {
        self.opened += 1;
        let g = (u64::from(self.step) << 32) | u64::from(self.opened);
        let sub = self.em.sub;
        self.em.sub += 1;
        self.em.events.push((Event::Open(g, self.grp), sub));
        self.grp = g;
    }

    /// Close the innermost open group.
    pub fn close_group(&mut self) {
        let sub = self.em.sub;
        self.em.sub += 1;
        let g = self.grp;
        self.em.events.push((Event::Close(g), sub));
        if g != NOGROUP {
            // (opened in this step, or before: its parent from the table)
            let local = self.em.events.iter().find_map(|(e, _)| match e {
                Event::Open(x, p) if *x == g => Some(*p),
                _ => None,
            });
            self.grp = local.unwrap_or_else(|| self.groups.get(&g).map_or(NOGROUP, |x| x.parent));
        }
    }

    /// Whether the run was cancelled (long steps poll it).
    #[must_use]
    pub fn cancelled(&self) -> bool {
        false
    }
}

/// Where name `m` at `at` resolves: the latest definition before `at`
/// alive there.
fn resolve<L: Lang>(g: &Nodes<L>, names: &NameTab, groups: &HashMap<u64, Group>, m: u32, at: Pos) -> Opd {
    let none = Opd {
        src: NONE,
        sel: Sel::WHOLE,
        name: m,
    };
    let Some(defs) = names.defs.get(m as usize) else {
        return none;
    };
    let k = defs.partition_point(|d| g.cmp_pos(d.pos, at) == Ordering::Less);
    for d in defs[..k].iter().rev() {
        let alive = d.global
            || d.group == NOGROUP
            || groups
                .get(&d.group)
                .and_then(|x| x.close)
                .is_none_or(|c| g.cmp_pos(c, at) == Ordering::Greater);
        if alive {
            return Opd {
                src: d.src,
                sel: d.sel,
                name: m,
            };
        }
    }
    none
}

/// The graph.
pub struct Graph<L: Lang> {
    pub(crate) n: Nodes<L>,
    names: NameTab,
    groups: HashMap<u64, Group>,
    pub(crate) steps: Vec<StepInfo>,
    pub(crate) unfolds: Vec<UnfoldInfo<L::Val>>,
    pub(crate) scans: Vec<ScanInfo<L::Val>>,
    free: Vec<u32>,
    to_free: Vec<u32>,
    heap: Vec<u32>,
    chains: HashMap<u32, Vec<u32>>,
    chain_readers: HashMap<u32, Vec<u32>>,
    pubs: HashMap<u64, Vec<u32>>,
    crosses: HashMap<u64, Vec<u32>>,
    pred: HashMap<u64, L::Val>,
    hunks: Vec<u32>,
    root_ord: u64,
    grp_hint: u64,
    em: Emit<L>,
    keymap: HashMap<u32, u32>,
    pub cfg: Config,
    rep: Report,
}

impl<L: Lang> Default for Graph<L> {
    fn default() -> Self {
        Self::new()
    }
}

/// The spacing of step labels.
const GAP: u64 = 1 << 32;

impl<L: Lang> Graph<L> {
    #[must_use]
    pub fn new() -> Self {
        let mut g = Graph {
            n: Nodes {
                kind: Vec::new(),
                op: Vec::new(),
                class: Vec::new(),
                parent: Vec::new(),
                ord: Vec::new(),
                depth: Vec::new(),
                next: Vec::new(),
                first: Vec::new(),
                key: Vec::new(),
                a0: Vec::new(),
                an: Vec::new(),
                rd: Vec::new(),
                val: Vec::new(),
                flags: Vec::new(),
                era: Vec::new(),
                aux: Vec::new(),
                opds: Vec::new(),
                revs: Vec::new(),
                absent: L::Val::default(),
            },
            names: NameTab::default(),
            groups: HashMap::new(),
            steps: Vec::new(),
            unfolds: Vec::new(),
            scans: Vec::new(),
            free: Vec::new(),
            to_free: Vec::new(),
            heap: Vec::new(),
            chains: HashMap::new(),
            chain_readers: HashMap::new(),
            pubs: HashMap::new(),
            crosses: HashMap::new(),
            pred: HashMap::new(),
            hunks: Vec::new(),
            root_ord: 0,
            grp_hint: NOGROUP,
            em: Emit::default(),
            keymap: HashMap::new(),
            cfg: Config::default(),
            rep: Report::default(),
        };
        // (node 0 is the root region)
        let r = g.alloc(Kind::Root, L::Op::default(), Class::Pure, NONE, 0, 0, 0);
        debug_assert_eq!(r, ROOT);
        g.n.depth[0] = 0;
        g
    }

    /// Reserve room for `nodes` nodes and `opds` operands.
    pub fn reserve(&mut self, nodes: usize, opds: usize) {
        let n = &mut self.n;
        n.kind.reserve(nodes);
        n.op.reserve(nodes);
        n.class.reserve(nodes);
        n.parent.reserve(nodes);
        n.ord.reserve(nodes);
        n.depth.reserve(nodes);
        n.next.reserve(nodes);
        n.first.reserve(nodes);
        n.key.reserve(nodes);
        n.a0.reserve(nodes);
        n.an.reserve(nodes);
        n.rd.reserve(nodes);
        n.val.reserve(nodes);
        n.flags.reserve(nodes);
        n.era.reserve(nodes);
        n.aux.reserve(nodes);
        n.opds.reserve(opds);
        n.revs.reserve(opds);
    }

    #[allow(clippy::too_many_arguments)]
    fn alloc(&mut self, kind: Kind, op: L::Op, class: Class, parent: u32, ord: u64, key: u32, aux: u64) -> u32 {
        let depth = if parent == NONE { 0 } else { self.n.depth[parent as usize] + 1 };
        self.rep.created += 1;
        let n = &mut self.n;
        if let Some(i) = self.free.pop() {
            let u = i as usize;
            n.kind[u] = kind;
            n.op[u] = op;
            n.class[u] = class;
            n.parent[u] = parent;
            n.ord[u] = ord;
            n.depth[u] = depth;
            n.next[u] = NONE;
            n.first[u] = NONE;
            n.key[u] = key;
            n.a0[u] = 0;
            n.an[u] = 0;
            n.rd[u] = NONE;
            n.val[u] = L::Val::default();
            n.flags[u] = 0;
            n.era[u] = n.era[u].wrapping_add(1);
            n.aux[u] = aux;
            return i;
        }
        let i = u32::try_from(n.kind.len()).expect("fewer than 2^32 nodes");
        assert!(i != NONE, "fewer than 2^32 - 1 nodes");
        n.kind.push(kind);
        n.op.push(op);
        n.class.push(class);
        n.parent.push(parent);
        n.ord.push(ord);
        n.depth.push(depth);
        n.next.push(NONE);
        n.first.push(NONE);
        n.key.push(key);
        n.a0.push(0);
        n.an.push(0);
        n.rd.push(NONE);
        n.val.push(L::Val::default());
        n.flags.push(0);
        n.era.push(0);
        n.aux.push(aux);
        i
    }

    /// Give `n` operands `os` (a new generation: its old reverse entries
    /// go stale).
    fn set_opds(&mut self, n: u32, os: &[Opd]) {
        let u = n as usize;
        let era = self.n.era[u].wrapping_add(1);
        self.n.era[u] = era;
        self.n.a0[u] = u32::try_from(self.n.opds.len()).expect("operands fit u32");
        self.n.an[u] = u16::try_from(os.len()).expect("at most 65535 operands");
        for o in os {
            self.n.opds.push(*o);
            if o.src != NONE {
                let s = o.src as usize;
                let r = u32::try_from(self.n.revs.len()).expect("edges fit u32");
                self.n.revs.push(Rev {
                    node: n,
                    era,
                    next: self.n.rd[s],
                });
                self.n.rd[s] = r;
            }
            if o.name != NONE {
                let rs = &mut self.names.readers[o.name as usize];
                if rs.len() >= 64 && rs.len().is_power_of_two() {
                    let g = &self.n;
                    rs.retain(|&(r, gn)| g.era[r as usize] == gn && g.flags[r as usize] & DEAD == 0);
                }
                rs.push((n, era));
            }
        }
    }

    fn add_rev(&mut self, src: u32, reader: u32) {
        let r = u32::try_from(self.n.revs.len()).expect("edges fit u32");
        self.n.revs.push(Rev {
            node: reader,
            era: self.n.era[reader as usize],
            next: self.n.rd[src as usize],
        });
        self.n.rd[src as usize] = r;
    }

    // ---- the worklist ----

    fn heap_less(&self, a: u32, b: u32) -> bool {
        self.n.cmp_node(a, b) == Ordering::Less
    }

    fn push_dirty(&mut self, n: u32) {
        let f = &mut self.n.flags[n as usize];
        *f |= DIRTY;
        if *f & (QUEUED | DEAD) != 0 {
            return;
        }
        *f |= QUEUED;
        self.heap.push(n);
        let mut i = self.heap.len() - 1;
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap_less(self.heap[i], self.heap[p]) {
                self.heap.swap(i, p);
                i = p;
            } else {
                break;
            }
        }
    }

    fn pop(&mut self) -> Option<u32> {
        let top = *self.heap.first()?;
        let last = self.heap.pop().expect("non-empty");
        if !self.heap.is_empty() {
            self.heap[0] = last;
            let mut i = 0;
            loop {
                let (l, r) = (2 * i + 1, 2 * i + 2);
                let mut m = i;
                if l < self.heap.len() && self.heap_less(self.heap[l], self.heap[m]) {
                    m = l;
                }
                if r < self.heap.len() && self.heap_less(self.heap[r], self.heap[m]) {
                    m = r;
                }
                if m == i {
                    break;
                }
                self.heap.swap(i, m);
                i = m;
            }
        }
        self.n.flags[top as usize] &= !QUEUED;
        Some(top)
    }

    // ---- values and waking ----

    /// Set `n`'s value; wake its readers whose read changed. Returns
    /// whether the version changed.
    fn set_val(&mut self, n: u32, v: L::Val) -> bool {
        let u = n as usize;
        if self.n.val[u].ver() == v.ver() {
            return false;
        }
        let old = std::mem::replace(&mut self.n.val[u], v);
        self.wake(n, &old);
        if let Class::Effect(c) = self.n.class[u] {
            self.wake_chain(c.0);
        }
        true
    }

    fn wake_chain(&mut self, c: u32) {
        if let Some(rs) = self.chain_readers.get(&c) {
            let rs = rs.clone();
            for r in rs {
                if !self.n.is_dead(r) {
                    self.rep.woken += 1;
                    self.push_dirty(r);
                }
            }
        }
    }

    /// Wake the readers of `n` whose read of it changed from `old`.
    fn wake(&mut self, n: u32, old: &L::Val) {
        let mut prev = NONE;
        let mut e = self.n.rd[n as usize];
        while e != NONE {
            let Rev { node: r, era, next } = self.n.revs[e as usize];
            let ru = r as usize;
            if self.n.era[ru] != era || self.n.flags[ru] & DEAD != 0 {
                // (stale: unlinked)
                if prev == NONE {
                    self.n.rd[n as usize] = next;
                } else {
                    self.n.revs[prev as usize].next = next;
                }
                e = next;
                continue;
            }
            let new = &self.n.val[n as usize];
            let changed = self
                .n
                .opds_of(r)
                .iter()
                .any(|o| o.src == n && o.sel.ver(old) != o.sel.ver(new));
            if changed {
                self.rep.woken += 1;
                self.push_dirty(r);
            }
            prev = e;
            e = next;
        }
    }

    // ---- the client's root region ----

    fn root_node(&mut self, kind: Kind, op: L::Op, class: Class, aux: u64) -> u32 {
        self.root_ord += 1;
        let ord = self.root_ord;
        let key = u32::try_from(ord & 0x7fff_ffff).unwrap_or(0);
        let n = self.alloc(kind, op, class, ROOT, ord, key, aux);
        // (root nodes in order: linked at the end)
        self.link_last(ROOT, n);
        n
    }

    fn link_last(&mut self, parent: u32, n: u32) {
        // root regions are appended to; keep a tail in `aux` of the root
        let tail = self.n.aux[parent as usize];
        if self.n.first[parent as usize] == NONE {
            self.n.first[parent as usize] = n;
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let t = tail as u32;
            self.n.next[t as usize] = n;
        }
        self.n.aux[parent as usize] = u64::from(n);
    }

    /// An input: a value the client sets between runs.
    pub fn input(&mut self, v: L::Val) -> NodeId {
        let n = self.root_node(Kind::Input, L::Op::default(), Class::Pure, 0);
        self.n.val[n as usize] = v;
        NodeId(n)
    }

    /// Set an input's value.
    pub fn set(&mut self, n: NodeId, v: L::Val) {
        debug_assert_eq!(self.n.kind[n.0 as usize], Kind::Input);
        self.set_val(n.0, v);
    }

    fn opd(&self, (n, sel): (NodeId, Sel)) -> Opd {
        Opd { src: n.0, sel, name: NONE }
    }

    /// A leaf of the root region.
    pub fn leaf(&mut self, op: L::Op, class: Class, args: &[(NodeId, Sel)]) -> NodeId {
        let n = self.root_node(Kind::Leaf, op, class, 0);
        let os: Vec<Opd> = args.iter().map(|&a| self.opd(a)).collect();
        self.set_opds(n, &os);
        self.register(n);
        self.settle_new(n);
        NodeId(n)
    }

    /// An unfold of the root region.
    pub fn unfold(&mut self, op: L::Op, input: NodeId, init: NodeId, args: &[NodeId]) -> NodeId {
        let n = self.root_node(Kind::Unfold, op, Class::Pure, 0);
        let mut os = vec![self.opd((input, Sel::WHOLE)), self.opd((init, Sel::WHOLE))];
        os.extend(args.iter().map(|&a| self.opd((a, Sel::WHOLE))));
        self.set_opds(n, &os);
        self.register(n);
        self.push_dirty(n);
        NodeId(n)
    }

    /// A scan of the root region.
    pub fn scan(&mut self, op: L::Op, input: (NodeId, Sel), init: NodeId, args: &[NodeId]) -> NodeId {
        let n = self.root_node(Kind::Scan, op, Class::Pure, 0);
        let mut os = vec![self.opd(input), self.opd((init, Sel::WHOLE))];
        os.extend(args.iter().map(|&a| self.opd((a, Sel::WHOLE))));
        self.set_opds(n, &os);
        self.register(n);
        self.push_dirty(n);
        NodeId(n)
    }

    /// A read of chain `c`, in the root region (place it after what it
    /// reads).
    pub fn chain_read(&mut self, c: Chain) -> NodeId {
        let n = self.root_node(Kind::ChainRead, L::Op::default(), Class::Pure, u64::from(c.0));
        self.register(n);
        self.push_dirty(n);
        NodeId(n)
    }

    /// Set up a new node's side tables (unfold, scan, chains, slots).
    fn register(&mut self, n: u32) {
        let u = n as usize;
        match self.n.kind[u] {
            Kind::Unfold => {
                let grp0 = self.cur_grp_for(n);
                self.n.aux[u] = self.unfolds.len() as u64;
                self.unfolds.push(UnfoldInfo {
                    keys: HashMap::new(),
                    owners: HashMap::new(),
                    input: None,
                    args_ver: Ver::ABSENT,
                    hunk_end: None,
                    grp0,
                });
            }
            Kind::Scan => {
                self.n.aux[u] = self.scans.len() as u64;
                self.scans.push(ScanInfo {
                    input: Seq::new(),
                    states: Vec::new(),
                    outs: Seq::new(),
                    init_ver: Ver::ABSENT,
                    args_ver: Ver::ABSENT,
                    done: false,
                });
            }
            Kind::Cross => {
                self.crosses.entry(self.n.aux[u]).or_default().push(n);
            }
            Kind::ChainRead => {
                #[allow(clippy::cast_possible_truncation)]
                let c = self.n.aux[u] as u32;
                self.chain_readers.entry(c).or_default().push(n);
            }
            _ => {}
        }
        match self.n.class[u] {
            Class::Effect(c) => self.chains.entry(c.0).or_default().push(n),
            Class::Publish(s) => self.pubs.entry(s.0).or_default().push(n),
            _ => {}
        }
    }

    fn cur_grp_for(&self, n: u32) -> u64 {
        // (an unfold a step made: the group open at its emission, kept by
        // the step processing in `grp_hint`)
        let _ = n;
        self.grp_hint
    }
}

impl<L: Lang> Graph<L> {
    /// A new node: evaluated now if it is a leaf whose operands are all
    /// settled (in position order nothing before it changes again), else
    /// queued.
    fn settle_new(&mut self, n: u32) {
        let u = n as usize;
        let leafy = matches!(self.n.kind[u], Kind::Leaf | Kind::Const | Kind::Cross);
        let clean = self
            .n
            .opds_of(n)
            .iter()
            .all(|o| o.src == NONE || self.n.flags[o.src as usize] & DIRTY == 0);
        if leafy && clean {
            self.n.flags[u] &= !DIRTY;
            self.process_kind(n);
        } else {
            self.push_dirty(n);
        }
    }

    fn process(&mut self, n: u32) {
        let f = self.n.flags[n as usize];
        if f & DEAD != 0 || f & DIRTY == 0 {
            return;
        }
        self.n.flags[n as usize] &= !DIRTY;
        self.process_kind(n);
    }

    fn process_kind(&mut self, n: u32) {
        match self.n.kind[n as usize] {
            Kind::Leaf => self.eval_leaf(n),
            Kind::Root | Kind::Input | Kind::Const => {}
            Kind::Cross => {
                let v = self.pred.get(&self.n.aux[n as usize]).cloned().unwrap_or_default();
                self.rep.evals += 1;
                self.set_val(n, v);
            }
            Kind::ChainRead => self.eval_chain(n),
            Kind::Step => self.run_step(n),
            Kind::Unfold => self.run_unfold(n),
            Kind::Scan => self.run_scan(n),
        }
    }

    fn eval_leaf(&mut self, n: u32) {
        let v = {
            let args = Args {
                g: &self.n,
                opds: self.n.opds_of(n),
            };
            L::eval(self.n.op[n as usize], &args)
        };
        self.rep.evals += 1;
        self.set_val(n, v);
    }

    /// The live nodes of chain `c` in position order.
    fn chain_nodes(&self, c: u32) -> Vec<u32> {
        let mut ns: Vec<u32> = self
            .chains
            .get(&c)
            .map(|v| v.iter().copied().filter(|&x| !self.n.is_dead(x)).collect())
            .unwrap_or_default();
        ns.sort_by(|&a, &b| self.n.cmp_node(a, b));
        ns.dedup();
        ns
    }

    fn eval_chain(&mut self, n: u32) {
        #[allow(clippy::cast_possible_truncation)]
        let c = self.n.aux[n as usize] as u32;
        let items: Vec<(ElemId, L::Val)> = self
            .chain_nodes(c)
            .into_iter()
            .map(|x| (ElemId(u64::from(x)), self.n.val[x as usize].clone()))
            .collect();
        let v = L::chain_val(Seq::from_vec(items));
        self.rep.evals += 1;
        self.set_val(n, v);
    }

    // ---- unfolds ----

    fn args_ver(&self, n: u32, from: usize) -> Ver {
        let vs: Vec<Ver> = self.n.opds_of(n)[from..].iter().map(|o| self.n.read_ver(o)).collect();
        Ver::node(0x6172_6773, &vs)
    }

    fn run_unfold(&mut self, u: u32) {
        let ui = self.n.aux[u as usize] as usize;
        let uo: Vec<Opd> = self.n.opds_of(u).to_vec();
        let input = L::as_seq(&self.n.read(&uo[0])).cloned().unwrap_or_default();
        let args_ver = self.args_ver(u, 2);
        let first = self.n.first[u as usize];
        let init = Opd {
            src: uo[1].src,
            sel: uo[1].sel,
            name: NONE,
        };
        if first == NONE {
            let at = input.get(0).map_or(END, |e| e.0);
            let grp = self.unfolds[ui].grp0;
            let s = self.new_step(u, NONE, 0, at, grp, init);
            self.unfolds[ui].input = Some(input);
            self.unfolds[ui].args_ver = args_ver;
            self.push_dirty(s);
            return;
        }
        if self.n.opds_of(first)[0] != init {
            let mut os: Vec<Opd> = self.n.opds_of(first).to_vec();
            os[0] = init;
            self.set_opds(first, &os);
            self.push_dirty(first);
        }
        if self.unfolds[ui].args_ver != args_ver {
            self.unfolds[ui].args_ver = args_ver;
            for s in self.n.children(u) {
                self.push_dirty(s);
            }
        }
        let old = self.unfolds[ui].input.take().unwrap_or_default();
        if old.ver() != input.ver() || old.len() != input.len() {
            let h = input.diff(&old);
            self.unfolds[ui].hunk_end = Some(input.len() - h.suffix);
            self.hunks.push(u);
            let mut s = first;
            let mut i = h.prefix;
            while i > 0 {
                i -= 1;
                let id = old.get(i).expect("in the old input").0;
                if let Some(&o) = self.unfolds[ui].owners.get(&id)
                    && !self.n.is_dead(o)
                {
                    s = o;
                    break;
                }
            }
            if s == first {
                // (the first step starts at the input's start, wherever that is now)
                let fi = self.n.aux[first as usize] as usize;
                self.steps[fi].at = input.get(0).map_or(END, |e| e.0);
            }
            self.push_dirty(s);
        }
        self.unfolds[ui].input = Some(input);
    }

    /// A new step of unfold `u` after `prev` (`NONE`: the first).
    fn new_step(&mut self, u: u32, prev: u32, key: u64, at: ElemId, grp: u64, opd0: Opd) -> u32 {
        let ui = self.n.aux[u as usize] as usize;
        let after = if prev == NONE { NONE } else { self.n.next[prev as usize] };
        let ord = self.step_ord(u, prev, after);
        #[allow(clippy::cast_possible_truncation)]
        let k32 = key as u32;
        let si = self.steps.len() as u64;
        let s = self.alloc(Kind::Step, self.n.op[u as usize], Class::Pure, u, ord, k32, si);
        self.steps.push(StepInfo {
            unfold: u,
            key,
            at,
            took: 0,
            grp_in: grp,
            grp_out: grp,
            in_ver: Ver::ABSENT,
            defs: Vec::new(),
            opened: Vec::new(),
            closed: Vec::new(),
        });
        if prev == NONE {
            self.n.next[s as usize] = self.n.first[u as usize];
            self.n.first[u as usize] = s;
        } else {
            self.n.next[s as usize] = after;
            self.n.next[prev as usize] = s;
        }
        self.unfolds[ui].keys.insert(key, s);
        self.set_opds(s, &[opd0]);
        if after != NONE {
            // (the old successor now follows the new step)
            let mut os: Vec<Opd> = self.n.opds_of(after).to_vec();
            os[0] = Opd {
                src: s,
                sel: Sel::WHOLE,
                name: NONE,
            };
            self.set_opds(after, &os);
        }
        s
    }

    /// A label between `prev` and `after` among `u`'s steps, relabeling
    /// them all if there is no room.
    fn step_ord(&mut self, u: u32, prev: u32, after: u32) -> u64 {
        let lo = if prev == NONE { 0 } else { self.n.ord[prev as usize] };
        if after == NONE {
            return lo + GAP;
        }
        let hi = self.n.ord[after as usize];
        if hi - lo > 1 {
            return lo + (hi - lo) / 2;
        }
        // (no room: spread every step of `u` again, keeping the order)
        let mut c = self.n.first[u as usize];
        let mut k = 0u64;
        let mut found = 0;
        while c != NONE {
            k += 1;
            self.n.ord[c as usize] = k * GAP;
            if c == prev {
                k += 1;
                found = k * GAP;
            }
            c = self.n.next[c as usize];
        }
        if prev == NONE {
            // (before the first: shift all by one gap)
            let mut c = self.n.first[u as usize];
            while c != NONE {
                self.n.ord[c as usize] += GAP;
                c = self.n.next[c as usize];
            }
            return GAP;
        }
        found
    }

    /// Run step `s`: its op, its emissions matched to its old children,
    /// its definitions and scopes, then its successor.
    #[allow(clippy::too_many_lines)]
    fn run_step(&mut self, s: u32) {
        self.rep.steps += 1;
        let si = self.n.aux[s as usize] as usize;
        let u = self.steps[si].unfold;
        let ui = self.n.aux[u as usize] as usize;
        let input = self.unfolds[ui].input.clone();
        let at = self.steps[si].at;
        let start = match &input {
            None => 0,
            Some(inp) if at == END => inp.len(),
            Some(inp) => inp.index_of(at).unwrap_or(inp.len()),
        };
        let prev_o = self.n.opds_of(s)[0];
        self.em.clear();
        let (res, idx, grp_out, end_cursor, in_ver) = {
            let Graph { n, names, groups, em, steps, .. } = self;
            let uo = n.opds_of(u);
            let mut cx = StepCx {
                g: n,
                names,
                groups,
                unfold_opds: uo,
                input: input.as_ref(),
                idx: start,
                start,
                step: s,
                pos: n.pos(s),
                grp: steps[si].grp_in,
                opened: 0,
                em,
                _brand: PhantomData,
            };
            let args = Args { g: n, opds: &uo[2..] };
            let st = n.read(&prev_o);
            let in_ver = st.ver();
            let res = L::step(n.op[s as usize], &st, &args, &mut cx);
            (res, cx.idx, cx.grp, cx.cursor(), in_ver)
        };
        if self.cfg.debug {
            let b = match &res {
                Step::Next { st, .. } => st.bytes(),
                Step::Done(v) => v.bytes(),
            };
            self.rep.state_max = self.rep.state_max.max(b);
            self.rep.state_sum += b as u64;
        }
        // names the step made
        let new_names = std::mem::take(&mut self.em.new_names);
        for (h, sp) in new_names {
            let id = u32::try_from(self.names.spell.len()).expect("names fit u32");
            self.names.by_hash.insert(h, id);
            self.names.spell.push(sp);
            self.names.defs.push(Vec::new());
            self.names.readers.push(Vec::new());
        }
        let ids = self.match_children(s);
        self.grp_hint = NOGROUP;
        self.apply_groups(s, si);
        self.apply_defs(s, si, &ids);
        self.settle_children(s, &ids);
        // the step's own operands: its state, then the names it read
        let mut os = vec![prev_o];
        os.extend(self.em.reads.iter().map(|r| r.1));
        self.set_opds(s, &os);
        // its input range
        let took = u32::try_from(idx - start).expect("took fits u32");
        let old_at = self.steps[si].at;
        if self.steps[si].took > 0 && self.unfolds[ui].owners.get(&old_at) == Some(&s) {
            self.unfolds[ui].owners.remove(&old_at);
        }
        if took > 0 {
            self.unfolds[ui].owners.insert(at, s);
        }
        let st_info = &mut self.steps[si];
        st_info.took = took;
        st_info.grp_out = grp_out;
        st_info.in_ver = in_ver;
        match res {
            Step::Done(v) => {
                self.set_val(s, v.clone());
                let mut c = self.n.next[s as usize];
                while c != NONE {
                    let nx = self.n.next[c as usize];
                    self.remove(c);
                    c = nx;
                }
                self.n.next[s as usize] = NONE;
                self.set_val(u, v);
            }
            Step::Next { st, key } => {
                let ver = st.ver();
                self.set_val(s, st);
                self.successor(s, u, ui, key, end_cursor, idx, grp_out, ver);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn successor(&mut self, s: u32, u: u32, ui: usize, key: u64, cursor: ElemId, idx: usize, grp: u64, ver: Ver) {
        let input_ok = self.unfolds[ui].hunk_end.is_none_or(|h| idx >= h);
        let q = self.n.next[s as usize];
        let fits = |g: &Self, j: u32| {
            let sj = &g.steps[g.n.aux[j as usize] as usize];
            sj.key == key && sj.at == cursor && sj.grp_in == grp
        };
        if q != NONE && fits(self, q) {
            let qi = self.n.aux[q as usize] as usize;
            if self.steps[qi].in_ver != ver || !input_ok {
                self.push_dirty(q);
            }
            return;
        }
        if let Some(&j) = self.unfolds[ui].keys.get(&key)
            && j != s
            && !self.n.is_dead(j)
            && self.n.parent[j as usize] == u
            && self.n.ord[j as usize] > self.n.ord[s as usize]
            && fits(self, j)
        {
            let mut c = q;
            while c != j {
                let nx = self.n.next[c as usize];
                self.remove(c);
                c = nx;
            }
            self.n.next[s as usize] = j;
            let mut os: Vec<Opd> = self.n.opds_of(j).to_vec();
            os[0] = Opd {
                src: s,
                sel: Sel::WHOLE,
                name: NONE,
            };
            self.set_opds(j, &os);
            let ji = self.n.aux[j as usize] as usize;
            if self.steps[ji].in_ver != ver || !input_ok {
                self.push_dirty(j);
            }
            return;
        }
        let n = self.new_step(
            u,
            s,
            key,
            cursor,
            grp,
            Opd {
                src: s,
                sel: Sel::WHOLE,
                name: NONE,
            },
        );
        self.push_dirty(n);
    }

    /// Match the step's emissions to its old children by key: the ids,
    /// in emission order. Old children not matched are removed.
    fn match_children(&mut self, s: u32) -> Vec<u32> {
        self.keymap.clear();
        let mut c = self.n.first[s as usize];
        while c != NONE {
            self.keymap.insert(self.n.key[c as usize], c);
            c = self.n.next[c as usize];
        }
        let specs = std::mem::take(&mut self.em.specs);
        let mut ids = Vec::with_capacity(specs.len());
        let mut gone: Vec<u32> = Vec::new();
        for sp in &specs {
            let cand = self.keymap.remove(&sp.key);
            let reuse = cand.filter(|&o| {
                let u = o as usize;
                self.n.kind[u] == sp.kind && self.n.op[u] == sp.op && self.n.class[u] == sp.class && {
                    // (an unfold's or scan's table index is its aux)
                    matches!(sp.kind, Kind::Unfold | Kind::Scan | Kind::Leaf | Kind::Const) || self.n.aux[u] == sp.aux
                }
            });
            if reuse.is_none()
                && let Some(o) = cand
            {
                gone.push(o);
            }
            let id = if let Some(o) = reuse {
                self.n.ord[o as usize] = sp.sub;
                if sp.kind == Kind::Unfold {
                    let ui = self.n.aux[o as usize] as usize;
                    if self.unfolds[ui].grp0 != sp.grp {
                        self.unfolds[ui].grp0 = sp.grp;
                        let f = self.n.first[o as usize];
                        if f != NONE {
                            let fi = self.n.aux[f as usize] as usize;
                            self.steps[fi].grp_in = sp.grp;
                            self.push_dirty(f);
                        }
                    }
                }
                o
            } else {
                self.grp_hint = sp.grp;
                let o = self.alloc(sp.kind, sp.op, sp.class, s, sp.sub, sp.key, sp.aux);
                self.n.flags[o as usize] |= DIRTY;
                self.register(o);
                o
            };
            ids.push(id);
        }
        // the old ones not matched
        gone.extend(self.keymap.values().copied());
        for o in gone {
            self.remove(o);
        }
        // link in order
        self.n.first[s as usize] = ids.first().copied().unwrap_or(NONE);
        for w in ids.windows(2) {
            self.n.next[w[0] as usize] = w[1];
        }
        if let Some(&l) = ids.last() {
            self.n.next[l as usize] = NONE;
        }
        // constants take their values now
        for (sp, &id) in specs.iter().zip(&ids) {
            if let Some(v) = &sp.lit {
                self.set_val(id, v.clone());
                self.n.flags[id as usize] &= !DIRTY;
            }
        }
        self.em.specs = specs;
        ids
    }

    fn sarg(&self, a: SArg, ids: &[u32], at: Pos) -> Opd {
        match a {
            SArg::Local(ix, sel) => Opd {
                src: ids[ix as usize],
                sel,
                name: NONE,
            },
            SArg::Node(src, sel) => Opd { src, sel, name: NONE },
            SArg::Name(m) => resolve(&self.n, &self.names, &self.groups, m, at),
        }
    }

    /// The emitted children's operands, set where they changed; then each
    /// new or changed child evaluated or queued, in order.
    fn settle_children(&mut self, s: u32, ids: &[u32]) {
        let specs = std::mem::take(&mut self.em.specs);
        let mut os: Vec<Opd> = Vec::new();
        for (sp, &id) in specs.iter().zip(ids) {
            os.clear();
            let at = self.n.pos(id);
            for k in 0..sp.args.1 {
                let a = self.em.args[(sp.args.0 + u32::from(k)) as usize];
                os.push(self.sarg(a, ids, at));
            }
            if self.n.opds_of(id) != os.as_slice() {
                self.set_opds(id, &os);
                self.n.flags[id as usize] |= DIRTY;
            }
            if self.n.flags[id as usize] & (DIRTY | QUEUED) == DIRTY {
                self.settle_new(id);
            }
        }
        let _ = s;
        self.em.specs = specs;
    }
}

impl<L: Lang> Graph<L> {
    /// The step's scope events, applied to the group table.
    fn apply_groups(&mut self, s: u32, si: usize) {
        let events = std::mem::take(&mut self.em.events);
        let mut opened = Vec::new();
        let mut closed = Vec::new();
        for (e, sub) in &events {
            match *e {
                Event::Open(g, p) => {
                    self.groups
                        .entry(g)
                        .or_insert_with(|| Group {
                            parent: p,
                            close: None,
                            names: Vec::new(),
                        })
                        .parent = p;
                    opened.push(g);
                }
                Event::Close(g) => {
                    if g == NOGROUP {
                        self.rep.unbalanced += 1;
                        continue;
                    }
                    let pos = Pos { parent: s, ord: *sub };
                    let gr = self.groups.entry(g).or_insert_with(|| Group {
                        parent: NOGROUP,
                        close: None,
                        names: Vec::new(),
                    });
                    let old = gr.close;
                    if old != Some(pos) {
                        gr.close = Some(pos);
                        let from = match old {
                            Some(o) if self.n.cmp_pos(o, pos) == Ordering::Less => o,
                            _ => pos,
                        };
                        self.reresolve_group(g, from);
                    }
                    closed.push(g);
                }
            }
        }
        let old_closed = std::mem::take(&mut self.steps[si].closed);
        for g in old_closed {
            if closed.contains(&g) {
                continue;
            }
            if let Some(gr) = self.groups.get_mut(&g)
                && let Some(c) = gr.close
                && c.parent == s
            {
                gr.close = None;
                self.reresolve_group(g, c);
            }
        }
        self.steps[si].opened = opened;
        self.steps[si].closed = closed;
        self.em.events = events;
    }

    fn reresolve_group(&mut self, g: u64, from: Pos) {
        let names = self.groups.get(&g).map(|x| x.names.clone()).unwrap_or_default();
        for m in names {
            self.reresolve(m, from);
        }
    }

    /// The step's definitions, applied to the name index.
    fn apply_defs(&mut self, s: u32, si: usize, ids: &[u32]) {
        let defs = std::mem::take(&mut self.em.defs);
        let new: Vec<DefRec> = defs
            .iter()
            .map(|&(m, a, global, sub, grp)| {
                let o = self.sarg(a, ids, Pos { parent: s, ord: sub });
                DefRec {
                    name: m,
                    sub,
                    src: o.src,
                    sel: o.sel,
                    group: grp,
                    global,
                }
            })
            .collect();
        self.em.defs = defs;
        let old = std::mem::take(&mut self.steps[si].defs);
        let same = old.len() == new.len()
            && old.iter().zip(&new).all(|(a, b)| {
                a.name == b.name && a.sub == b.sub && a.src == b.src && a.sel == b.sel && a.group == b.group && a.global == b.global
            });
        if same {
            self.steps[si].defs = new;
            return;
        }
        let mut touched: Vec<u32> = Vec::new();
        for d in &old {
            let pos = Pos { parent: s, ord: d.sub };
            self.names.defs[d.name as usize].retain(|e| e.pos != pos);
            touched.push(d.name);
        }
        for d in &new {
            let e = DefEntry {
                pos: Pos { parent: s, ord: d.sub },
                src: d.src,
                sel: d.sel,
                group: d.group,
                global: d.global,
            };
            let list = &self.names.defs[d.name as usize];
            let k = list.partition_point(|x| self.n.cmp_pos(x.pos, e.pos) == Ordering::Less);
            self.names.defs[d.name as usize].insert(k, e);
            if !d.global
                && d.group != NOGROUP
                && let Some(gr) = self.groups.get_mut(&d.group)
                && !gr.names.contains(&d.name)
            {
                gr.names.push(d.name);
            }
            touched.push(d.name);
        }
        self.steps[si].defs = new;
        touched.sort_unstable();
        touched.dedup();
        let from = self.n.pos(s);
        for m in touched {
            self.reresolve(m, from);
        }
    }

    /// Readers of name `m` after `from` resolved again; those whose value
    /// changed are woken.
    fn reresolve(&mut self, m: u32, from: Pos) {
        let rs = std::mem::take(&mut self.names.readers[m as usize]);
        let mut keep = Vec::with_capacity(rs.len());
        for (r, era) in rs {
            let ru = r as usize;
            if self.n.era[ru] != era || self.n.flags[ru] & DEAD != 0 {
                continue;
            }
            keep.push((r, era));
            let at = self.n.pos(r);
            if self.n.cmp_pos(at, from) != Ordering::Greater {
                continue;
            }
            let a0 = self.n.a0[ru] as usize;
            for k in 0..self.n.an[ru] as usize {
                let o = self.n.opds[a0 + k];
                if o.name != m {
                    continue;
                }
                let nw = resolve(&self.n, &self.names, &self.groups, m, at);
                if nw.src != o.src || nw.sel != o.sel {
                    let ov = self.n.read_ver(&o);
                    self.n.opds[a0 + k] = nw;
                    if nw.src != NONE {
                        self.add_rev(nw.src, r);
                    }
                    if ov != self.n.read_ver(&nw) {
                        self.rep.woken += 1;
                        self.push_dirty(r);
                    }
                }
            }
        }
        let rest = std::mem::take(&mut self.names.readers[m as usize]);
        keep.extend(rest);
        self.names.readers[m as usize] = keep;
    }

    /// Remove `n` and everything under it.
    fn remove(&mut self, n: u32) {
        let u = n as usize;
        if self.n.flags[u] & DEAD != 0 {
            return;
        }
        let mut c = self.n.first[u];
        while c != NONE {
            let nx = self.n.next[c as usize];
            self.remove(c);
            c = nx;
        }
        self.n.flags[u] |= DEAD;
        self.n.era[u] = self.n.era[u].wrapping_add(1);
        self.rep.removed += 1;
        self.to_free.push(n);
        if self.n.kind[u] == Kind::Step {
            let si = self.n.aux[u] as usize;
            let su = self.steps[si].unfold;
            let ui = self.n.aux[su as usize] as usize;
            let (key, at) = (self.steps[si].key, self.steps[si].at);
            if self.unfolds[ui].keys.get(&key) == Some(&n) {
                self.unfolds[ui].keys.remove(&key);
            }
            if self.unfolds[ui].owners.get(&at) == Some(&n) {
                self.unfolds[ui].owners.remove(&at);
            }
            let defs = std::mem::take(&mut self.steps[si].defs);
            let from = self.n.pos(n);
            let mut touched = Vec::new();
            for d in &defs {
                let pos = Pos { parent: n, ord: d.sub };
                self.names.defs[d.name as usize].retain(|e| e.pos != pos);
                touched.push(d.name);
            }
            touched.sort_unstable();
            touched.dedup();
            for m in touched {
                self.reresolve(m, from);
            }
            for g in std::mem::take(&mut self.steps[si].closed) {
                if let Some(gr) = self.groups.get_mut(&g)
                    && let Some(c) = gr.close
                    && c.parent == n
                {
                    gr.close = None;
                    self.reresolve_group(g, c);
                }
            }
        }
        if let Class::Effect(c) = self.n.class[u] {
            self.wake_chain(c.0);
        }
    }

    // ---- scans ----

    fn run_scan(&mut self, sc: u32) {
        let ix = self.n.aux[sc as usize] as usize;
        let uo: Vec<Opd> = self.n.opds_of(sc).to_vec();
        let input = L::as_seq(&self.n.read(&uo[0])).cloned().unwrap_or_default();
        let init = self.n.read(&uo[1]).clone();
        let args_ver = self.args_ver(sc, 2);
        let op = self.n.op[sc as usize];
        let info = &mut self.scans[ix];
        let full = !info.done || info.init_ver != init.ver() || info.args_ver != args_ver;
        let (old_input, old_states, old_outs) = (
            std::mem::take(&mut info.input),
            std::mem::take(&mut info.states),
            std::mem::take(&mut info.outs),
        );
        let (p, sfx) = if full {
            (0, 0)
        } else {
            let h = input.diff(&old_input);
            (h.prefix, h.suffix)
        };
        let (n_new, n_old) = (input.len(), old_input.len());
        let mut st = if p == 0 { init.clone() } else { old_states[p - 1].clone() };
        let mut states: Vec<L::Val> = Vec::with_capacity(n_new);
        states.extend_from_slice(&old_states[..p]);
        let mut fresh: Vec<(ElemId, L::Val)> = Vec::new();
        let mut met = None;
        {
            let args = Args { g: &self.n, opds: &uo[2..] };
            for (i, (id, x)) in input.iter_from(p).enumerate().map(|(k, e)| (k + p, e)) {
                let (s2, out) = L::scan(op, &st, x, &args);
                st = s2;
                self.rep.scanned += 1;
                fresh.push((id, out));
                states.push(st.clone());
                if !full && i >= n_new - sfx {
                    let oi = i + n_old - n_new;
                    if old_states[oi].ver() == st.ver() {
                        met = Some(oi);
                        break;
                    }
                }
            }
        }
        let outs = match met {
            Some(oi) => {
                states.extend_from_slice(&old_states[oi + 1..]);
                old_outs.splice(p, oi + 1 - p, fresh)
            }
            None => old_outs.splice(p, n_old - p.min(n_old), fresh),
        };
        let last = states.last().cloned().unwrap_or_else(|| init.clone());
        let v = {
            let args = Args { g: &self.n, opds: &uo[2..] };
            L::scan_result(op, &last, &outs, &args)
        };
        let info = &mut self.scans[ix];
        info.input = input;
        info.states = states;
        info.outs = outs;
        info.init_ver = init.ver();
        info.args_ver = args_ver;
        info.done = true;
        self.rep.evals += 1;
        self.set_val(sc, v);
    }

    // ---- the run ----

    /// The value slot `s` takes in the next run.
    fn published(&self, s: u64) -> L::Val {
        let mut best: Option<u32> = None;
        for &p in self.pubs.get(&s).into_iter().flatten() {
            if self.n.is_dead(p) {
                continue;
            }
            if best.is_none_or(|b| self.n.cmp_node(p, b) == Ordering::Greater) {
                best = Some(p);
            }
        }
        best.map(|b| self.n.val[b as usize].clone()).unwrap_or_default()
    }

    /// Evaluate what is new or changed, to quiescence, then iterate the
    /// cross-run slots to their fixed point.
    pub fn run(&mut self) -> Report {
        self.rep = Report::default();
        let mut hist: HashMap<u64, Vec<Ver>> = HashMap::new();
        loop {
            while let Some(n) = self.pop() {
                self.process(n);
            }
            let mut slots: Vec<u64> = self.pubs.keys().chain(self.crosses.keys()).copied().collect();
            slots.sort_unstable();
            slots.dedup();
            let mut changed = Vec::new();
            for sl in slots {
                let v = self.published(sl);
                let cur = self.pred.get(&sl).map_or_else(|| L::Val::default().ver(), Value::ver);
                if cur != v.ver() {
                    changed.push((sl, v));
                }
            }
            if changed.is_empty() {
                break;
            }
            if self.rep.iterations >= self.cfg.max_iters {
                for (sl, v) in changed {
                    let mut h = hist.remove(&sl).unwrap_or_default();
                    h.push(v.ver());
                    self.rep.oscillating.push((Slot(sl), h));
                }
                break;
            }
            self.rep.iterations += 1;
            for (sl, v) in changed {
                hist.entry(sl).or_default().push(v.ver());
                self.pred.insert(sl, v);
                for c in self.crosses.get(&sl).cloned().unwrap_or_default() {
                    if !self.n.is_dead(c) {
                        self.push_dirty(c);
                    }
                }
            }
        }
        for u in std::mem::take(&mut self.hunks) {
            let ui = self.n.aux[u as usize] as usize;
            if !self.n.is_dead(u) {
                self.unfolds[ui].hunk_end = None;
            }
        }
        if self.cfg.check {
            self.check();
        }
        self.sweep();
        self.rep.clone()
    }

    /// Registries pruned of the dead; removed slots freed for reuse.
    fn sweep(&mut self) {
        let dead = |n: &Nodes<L>, x: u32| n.flags[x as usize] & DEAD != 0;
        for v in self.chains.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.chain_readers.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.pubs.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        for v in self.crosses.values_mut() {
            v.retain(|&x| !dead(&self.n, x));
        }
        // (a freed node keeps its DEAD flag until reused)
        let tf = std::mem::take(&mut self.to_free);
        for n in tf {
            self.n.first[n as usize] = NONE;
            self.n.val[n as usize] = L::Val::default();
            self.free.push(n);
        }
    }

    /// Every live leaf and scan evaluated again: a value that differs is
    /// an impure op or a missed wake.
    fn check(&mut self) {
        let mut seen = vec![false; self.n.kind.len()];
        let mut stack = vec![ROOT];
        while let Some(x) = stack.pop() {
            seen[x as usize] = true;
            let mut c = self.n.first[x as usize];
            while c != NONE {
                assert!(!self.n.is_dead(c), "check: dead node %{c} linked under %{x}");
                assert_eq!(self.n.parent[c as usize], x, "check: %{c} linked under %{x}");
                stack.push(c);
                c = self.n.next[c as usize];
            }
        }
        for (i, s) in seen.iter().enumerate() {
            assert!(
                *s || self.n.flags[i] & DEAD != 0,
                "check: live node %{i} ({:?} {:?}, parent %{}) is not in the tree",
                self.n.kind[i],
                self.n.op[i],
                self.n.parent[i]
            );
        }
        for n in 1..self.n.kind.len() {
            let n32 = u32::try_from(n).expect("fits");
            if self.n.is_dead(n32) || self.n.kind[n] != Kind::Leaf {
                continue;
            }
            let v = {
                let args = Args {
                    g: &self.n,
                    opds: self.n.opds_of(n32),
                };
                L::eval(self.n.op[n], &args)
            };
            assert!(
                v.ver() == self.n.val[n].ver(),
                "check: node %{n} ({:?}) is {:?} but evaluates to {:?}",
                self.n.op[n],
                self.n.val[n],
                v
            );
            for o in self.n.opds_of(n32) {
                if o.name != NONE {
                    let at = self.n.pos(n32);
                    let want = resolve(&self.n, &self.names, &self.groups, o.name, at);
                    assert!(
                        want.src == o.src && want.sel == o.sel,
                        "check: node %{n} reads name {} at %{} but it resolves to %{}",
                        o.name,
                        o.src,
                        want.src
                    );
                }
            }
        }
    }

    // ---- reading the results ----

    /// A node's value.
    #[must_use]
    pub fn value(&self, n: NodeId) -> &L::Val {
        &self.n.val[n.0 as usize]
    }

    /// Chain `c`'s payloads in order.
    #[must_use]
    pub fn chain(&self, c: Chain) -> Vec<L::Val> {
        self.chain_nodes(c.0).into_iter().map(|x| self.n.val[x as usize].clone()).collect()
    }

    /// The value slot `s` was predicted to have in this run.
    #[must_use]
    pub fn slot(&self, s: Slot) -> L::Val {
        self.pred.get(&s.0).cloned().unwrap_or_default()
    }

    /// Predict slot `s` (a cold start from a persisted run).
    pub fn predict(&mut self, s: Slot, v: L::Val) {
        self.pred.insert(s.0, v);
    }

    /// The number of live nodes.
    #[must_use]
    pub fn live(&self) -> usize {
        (1..self.n.kind.len()).filter(|&i| self.n.flags[i] & DEAD == 0).count()
    }

    /// The name spelled `s`, if any node has used it.
    #[must_use]
    pub fn name_id(&self, s: &[u8]) -> Option<NameId> {
        self.names.by_hash.get(&hash64(s)).map(|&i| NameId(i))
    }

    /// The spelling of name `n`.
    #[must_use]
    pub fn spelling(&self, n: NameId) -> &[u8] {
        &self.names.spell[n.0 as usize]
    }

    /// Name `n`'s value at the end of the program.
    #[must_use]
    pub fn name_value(&self, n: NameId) -> Option<L::Val> {
        let end = Pos {
            parent: ROOT,
            ord: u64::MAX,
        };
        let o = resolve(&self.n, &self.names, &self.groups, n.0, end);
        (o.src != NONE).then(|| self.n.read(&o).clone())
    }

    pub(crate) fn names_spell(&self, m: u32) -> Vec<u8> {
        self.names.spell[m as usize].to_vec()
    }

    /// A step's definitions: (source, selector, name, global).
    pub(crate) fn step_defs(&self, s: u32) -> Vec<(u32, Sel, u32, bool)> {
        self.steps[self.n.aux[s as usize] as usize]
            .defs
            .iter()
            .map(|d| (d.src, d.sel, d.name, d.global))
            .collect()
    }

    /// The root region's nodes, in order.
    #[must_use]
    pub fn roots(&self) -> Vec<NodeId> {
        self.n.children(ROOT).into_iter().map(NodeId).collect()
    }
}
