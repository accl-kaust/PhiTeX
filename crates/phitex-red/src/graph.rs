//! A1: the node store, the environment's representation, and the reducer:
//! `pending(env; input)` → node ; pending(env', input'). Implements `Ctx`.
//!
//! * `EnvRep`: a persistent hash trie (HAMT) of definitions with TeX's levels,
//!   a persistent group chain (each frame holds the old entries its local
//!   definitions shadowed, undone at its end unless the current entry is
//!   global, and its `\aftergroup` tokens) and an independent persistent
//!   list chain (mode and the list-scope keys). `at` names the node that
//!   produced the env (the chain a list's emits are gathered along).
//! * `Graph`: the arena of nodes (each with its predecessor), `reduce_one`
//!   (exactly one node per call), `run`, memo hits.
//! * `Reducing`: the `Ctx` a node is reduced in. Every read of state from
//!   before the node is recorded once with the value seen; reads of what the
//!   node itself made are not. Structure read after the node closed groups or
//!   lists that were open before it is recorded under pseudo keys
//!   (`Key::Other(pk::TAG, …)`) that `env_lookup` / `Graph::holds` answer from
//!   the node's start thunk, so a node's reads always validate it.
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::ctx::Ctx;
use crate::env::{Change, Delta, Env, GroupKind, Mode};
use crate::item::Item;
use crate::prim::{Kind, PrimId};
use crate::term::{Emit, Input, Node, NodeId, Op, Out, Span, Src, TextId, Thunk, TokKind};
use crate::tok::{Names, Tok, Toks};
use crate::val::{Datum, Key, Meaning, Val};

/// No node (a thunk made by `Graph::start` from a fresh env).
pub const NO_NODE: NodeId = NodeId::MAX;
/// `ignore_depth` (tex.web 212).
pub const IGNORE_DEPTH: i32 = -65_536_000;

/// Pseudo keys (`Key::Other(pk::TAG, n)`) for structure a node read.
pub mod pk {
    pub const TAG: u8 = 0;
    /// Group depth of the start env.
    pub const GROUP_DEPTH: u32 = 0;
    /// List depth of the start env.
    pub const LIST_DEPTH: u32 = 1;
    /// + c: kind (`GroupKind as i32`) of the group c levels up.
    pub const GROUP_KIND: u32 = 0x1_0000;
    /// + c: the group frame c levels up (`FrameDatum`), read by closing it.
    pub const FRAME: u32 = 0x2_0000;
    /// + c: mode code of the list c levels up.
    pub const MODE: u32 = 0x3_0000;
    /// + 4c + w: list-scope key w (0 spacefactor 1 prevdepth 2 prevgraf 3 clang) c lists up.
    pub const LIST_KEY: u32 = 0x4_0000;
    /// + c: items of the list c levels up (`ItemsDatum`; c = 0 is `Key::Tail`).
    pub const CHAIN: u32 = 0x5_0000;
    pub fn class(n: u32) -> u32 {
        n >> 16
    }
}

// ===================================================================== HAMT

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub val: Val,
    /// TeX's level: 1 global/bottom, depth+1 inside groups.
    pub level: u32,
}

#[derive(Clone, Debug)]
enum Slot {
    Leaf(u64, Key, Entry),
    Many(u64, Arc<Vec<(Key, Entry)>>),
    Sub(Arc<HNode>),
}

#[derive(Clone, Debug, Default)]
struct HNode {
    bitmap: u32,
    slots: Vec<Slot>,
}

const BITS: u32 = 5;

fn hash_key(k: &Key) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    k.hash(&mut h);
    h.finish()
}

fn frag(h: u64, shift: u32) -> u32 {
    if shift >= 64 { 0 } else { ((h >> shift) & 31) as u32 }
}

fn slot_hash(s: &Slot) -> u64 {
    match s {
        Slot::Leaf(h, ..) | Slot::Many(h, _) => *h,
        Slot::Sub(_) => unreachable!(),
    }
}

impl HNode {
    fn get(&self, h: u64, shift: u32, k: &Key) -> Option<&Entry> {
        let bit = 1u32 << frag(h, shift);
        if self.bitmap & bit == 0 {
            return None;
        }
        let idx = (self.bitmap & (bit - 1)).count_ones() as usize;
        match &self.slots[idx] {
            Slot::Leaf(h2, k2, e) => (*h2 == h && k2 == k).then_some(e),
            Slot::Many(h2, v) => {
                if *h2 == h { v.iter().find(|(kk, _)| kk == k).map(|(_, e)| e) } else { None }
            }
            Slot::Sub(n) => n.get(h, shift + BITS, k),
        }
    }

    /// A node holding two leaf-ish slots of different hashes.
    fn pair(a: Slot, b: Slot, shift: u32) -> HNode {
        let (ha, hb) = (slot_hash(&a), slot_hash(&b));
        let (fa, fb) = (frag(ha, shift), frag(hb, shift));
        if fa == fb {
            HNode { bitmap: 1 << fa, slots: vec![Slot::Sub(Arc::new(HNode::pair(a, b, shift + BITS)))] }
        } else if fa < fb {
            HNode { bitmap: (1 << fa) | (1 << fb), slots: vec![a, b] }
        } else {
            HNode { bitmap: (1 << fa) | (1 << fb), slots: vec![b, a] }
        }
    }

    fn insert(&self, h: u64, shift: u32, k: Key, e: Entry) -> HNode {
        let bit = 1u32 << frag(h, shift);
        let idx = (self.bitmap & (bit - 1)).count_ones() as usize;
        let mut n = self.clone();
        if self.bitmap & bit == 0 {
            n.bitmap |= bit;
            n.slots.insert(idx, Slot::Leaf(h, k, e));
            return n;
        }
        let new = match &self.slots[idx] {
            Slot::Leaf(h2, k2, e2) => {
                if *h2 == h {
                    if *k2 == k {
                        Slot::Leaf(h, k, e)
                    } else {
                        Slot::Many(h, Arc::new(vec![(k2.clone(), e2.clone()), (k, e)]))
                    }
                } else {
                    let old = Slot::Leaf(*h2, k2.clone(), e2.clone());
                    Slot::Sub(Arc::new(HNode::pair(old, Slot::Leaf(h, k, e), shift + BITS)))
                }
            }
            Slot::Many(h2, v) => {
                if *h2 == h {
                    let mut v2 = (**v).clone();
                    match v2.iter_mut().find(|(kk, _)| *kk == k) {
                        Some(slot) => slot.1 = e,
                        None => v2.push((k, e)),
                    }
                    Slot::Many(h, Arc::new(v2))
                } else {
                    let old = Slot::Many(*h2, v.clone());
                    Slot::Sub(Arc::new(HNode::pair(old, Slot::Leaf(h, k, e), shift + BITS)))
                }
            }
            Slot::Sub(s) => Slot::Sub(Arc::new(s.insert(h, shift + BITS, k, e))),
        };
        n.slots[idx] = new;
        n
    }
}

/// A persistent map `Key → Entry` (hash array mapped trie). Clone is O(1).
#[derive(Clone, Debug, Default)]
pub struct Hamt {
    root: Arc<HNode>,
    len: usize,
}

impl Hamt {
    pub fn get(&self, k: &Key) -> Option<&Entry> {
        self.root.get(hash_key(k), 0, k)
    }
    pub fn insert(&self, k: Key, e: Entry) -> Hamt {
        let h = hash_key(&k);
        let fresh = self.root.get(h, 0, &k).is_none();
        Hamt { root: Arc::new(self.root.insert(h, 0, k, e)), len: self.len + usize::from(fresh) }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ============================================================ group / list

#[derive(Debug)]
pub struct SaveCell {
    pub key: Key,
    pub old: Entry,
    pub next: Option<Arc<SaveCell>>,
}

#[derive(Debug)]
pub struct AfterCell {
    pub tok: Tok,
    pub next: Option<Arc<AfterCell>>,
}

/// One open group: its kind, the entries its local definitions shadowed
/// (newest first), its `\aftergroup` tokens (newest first).
#[derive(Clone, Debug)]
pub struct GroupFrame {
    pub kind: GroupKind,
    pub depth: u32,
    pub saves: Option<Arc<SaveCell>>,
    pub after: Option<Arc<AfterCell>>,
    pub up: Option<Arc<GroupFrame>>,
}

impl GroupFrame {
    fn saves_vec(&self) -> Vec<(Key, Entry)> {
        let mut v = Vec::new();
        let mut s = self.saves.clone();
        while let Some(c) = s {
            v.push((c.key.clone(), c.old.clone()));
            s = c.next.clone();
        }
        v
    }
    fn after_toks(&self) -> Vec<Tok> {
        let mut v = Vec::new();
        let mut a = self.after.clone();
        while let Some(c) = a {
            v.push(c.tok);
            a = c.next.clone();
        }
        v.reverse();
        v
    }
}

/// One open list: its mode and list-scope values.
#[derive(Clone, Debug)]
pub struct ListFrame {
    pub mode: Mode,
    pub depth: u32,
    /// spacefactor, prevdepth, prevgraf, clang
    pub vals: [i32; 4],
    pub up: Option<Arc<ListFrame>>,
}

impl ListFrame {
    fn new(mode: Mode, depth: u32, up: Option<Arc<ListFrame>>) -> ListFrame {
        ListFrame { mode, depth, vals: [1000, IGNORE_DEPTH, 0, 0], up }
    }
    fn up_n(self: &Arc<Self>, c: u32) -> Option<Arc<ListFrame>> {
        let mut f = Some(self.clone());
        for _ in 0..c {
            f = f.and_then(|x| x.up.clone());
        }
        f
    }
}

fn list_which(k: &Key) -> Option<u8> {
    match k {
        Key::SpaceFactor => Some(0),
        Key::PrevDepth => Some(1),
        Key::PrevGraf => Some(2),
        Key::Clang => Some(3),
        _ => None,
    }
}

pub fn mode_code(m: Mode) -> i32 {
    match m {
        Mode::Vertical { inner } => if inner { -1 } else { 1 },
        Mode::Horizontal { inner } => if inner { -2 } else { 2 },
        Mode::Math { display } => if display { 3 } else { -3 },
    }
}

/// The environment's representation: definitions, group chain, list chain.
#[derive(Clone, Debug)]
pub struct EnvRep {
    pub defs: Hamt,
    pub group: Option<Arc<GroupFrame>>,
    pub list: Arc<ListFrame>,
    /// The node this env is the result of (`NO_NODE`: none).
    pub at: NodeId,
}

impl Default for EnvRep {
    fn default() -> Self {
        EnvRep {
            defs: Hamt::default(),
            group: None,
            list: Arc::new(ListFrame::new(Mode::Vertical { inner: false }, 0, None)),
            at: NO_NODE,
        }
    }
}

impl EnvRep {
    fn depth(&self) -> u32 {
        self.group.as_ref().map_or(0, |g| g.depth)
    }
    fn group_up(&self, c: u32) -> Option<Arc<GroupFrame>> {
        let mut f = self.group.clone();
        for _ in 0..c {
            f = f.and_then(|x| x.up.clone());
        }
        f
    }
    fn entry(&self, k: &Key) -> Entry {
        self.defs.get(k).cloned().unwrap_or_else(|| Entry { val: initial(k), level: 1 })
    }
    fn define(&mut self, key: &Key, val: &Val, global: bool) {
        if let Some(w) = list_which(key) {
            let mut f = (*self.list).clone();
            f.vals[w as usize] = val.int();
            self.list = Arc::new(f);
            return;
        }
        if global {
            self.defs = self.defs.insert(key.clone(), Entry { val: val.clone(), level: 1 });
            return;
        }
        let level = self.depth() + 1;
        if level > 1 {
            let cur = self.entry(key);
            if cur.level != level {
                if let Some(g) = &self.group {
                    let mut f = (**g).clone();
                    f.saves = Some(Arc::new(SaveCell { key: key.clone(), old: cur, next: g.saves.clone() }));
                    self.group = Some(Arc::new(f));
                }
            }
        }
        self.defs = self.defs.insert(key.clone(), Entry { val: val.clone(), level });
    }
    fn apply_change(&mut self, c: &Change) {
        match c {
            Change::Def { key, val, global } => self.define(key, val, *global),
            Change::Replace { key, val } => {
                let level = self.entry(key).level;
                self.defs = self.defs.insert(key.clone(), Entry { val: val.clone(), level });
            }
            Change::OpenGroup(kind) => {
                let depth = self.depth() + 1;
                self.group = Some(Arc::new(GroupFrame { kind: *kind, depth, saves: None, after: None, up: self.group.clone() }));
            }
            Change::CloseGroup => {
                let Some(f) = self.group.clone() else { return };
                self.group = f.up.clone();
                let mut s = f.saves.clone();
                while let Some(cell) = s {
                    let cur_level = self.defs.get(&cell.key).map_or(1, |e| e.level);
                    // A global definition made inside the group is retained.
                    if cur_level != 1 {
                        self.defs = self.defs.insert(cell.key.clone(), cell.old.clone());
                    }
                    s = cell.next.clone();
                }
            }
            Change::AfterGroup(t) => {
                // tex.web: `\aftergroup` at level one is ignored.
                if let Some(g) = &self.group {
                    let mut f = (**g).clone();
                    f.after = Some(Arc::new(AfterCell { tok: *t, next: g.after.clone() }));
                    self.group = Some(Arc::new(f));
                }
            }
            Change::OpenList(m) => {
                let depth = self.list.depth + 1;
                self.list = Arc::new(ListFrame::new(*m, depth, Some(self.list.clone())));
            }
            Change::CloseList => {
                if let Some(up) = self.list.up.clone() {
                    self.list = up;
                }
            }
        }
    }
}

/// What closing a group read: the frame's contents and the current entries
/// of the keys it shadowed (they decide restore vs. retain).
#[derive(Debug, PartialEq)]
pub struct FrameDatum {
    pub kind: GroupKind,
    pub saves: Vec<(Key, Entry)>,
    pub after: Vec<Tok>,
    pub cur: Vec<(Key, Entry)>,
}

impl Datum for FrameDatum {
    fn same(&self, o: &dyn Datum) -> bool {
        o.as_any().downcast_ref::<FrameDatum>().is_some_and(|o| self == o)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Items of a list read from before a node: the last `items.len()` items, or
/// all of them when `whole`.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemsDatum {
    pub items: Arc<[Item]>,
    pub whole: bool,
}

impl Datum for ItemsDatum {
    fn same(&self, o: &dyn Datum) -> bool {
        o.as_any().downcast_ref::<ItemsDatum>().is_some_and(|o| self == o)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// ======================================================== env_* (contract)

fn empty_toks() -> Toks {
    Arc::from(Vec::<Tok>::new())
}

fn init_cat(c: char) -> i32 {
    match c {
        '\\' => 0,
        '\r' => 5,
        ' ' => 10,
        '%' => 14,
        '\u{7f}' => 15,
        '\0' => 9,
        'a'..='z' | 'A'..='Z' => 11,
        _ => 12,
    }
}

/// INITEX's value of every key (tex.web 222–240, e-TeX, pdfTeX).
pub fn initial(k: &Key) -> Val {
    use Key::*;
    match k {
        Cs(_) | Active(_) => Val::Meaning(Meaning::Undefined),
        Cat(c) => Val::Int(init_cat(*c)),
        Lc(c) => Val::Int(match c {
            'a'..='z' => *c as i32,
            'A'..='Z' => *c as i32 + 32,
            _ => 0,
        }),
        Uc(c) => Val::Int(match c {
            'a'..='z' => *c as i32 - 32,
            'A'..='Z' => *c as i32,
            _ => 0,
        }),
        Sf(c) => Val::Int(if c.is_ascii_uppercase() { 999 } else { 1000 }),
        MathCode(c) => Val::Int(match c {
            '0'..='9' => *c as i32 + 0x7000,
            'a'..='z' | 'A'..='Z' => *c as i32 + 0x7100,
            _ => *c as i32,
        }),
        DelCode(c) => Val::Int(if *c == '.' { 0 } else { -1 }),
        IntPar(n) => Val::Int(match crate::prim::ip::NAMES.get(*n as usize).copied().unwrap_or("") {
            "mag" => 1000,
            "tolerance" => 10000,
            "hangafter" => 1,
            "maxdeadcycles" => 25,
            "escapechar" => 92,
            "endlinechar" => 13,
            // fix_date_and_time's defaults (tex.web 241); the build's givens redefine them.
            "time" => 720,
            "day" => 4,
            "month" => 7,
            "year" => 1776,
            // pdfTeX INITEX
            "pdfcompresslevel" => 9,
            "pdfdecimaldigits" => 4,
            "pdfimageresolution" => 72,
            "pdfminorversion" => 4,
            "pdfmajorversion" => 1,
            "pdfgamma" => 1000,
            "pdfimagegamma" => 2200,
            "pdfimagehicolor" => 1,
            _ => 0,
        }),
        DimenPar(_) | Count(_) | Dimen(_) | FontDimen(..) | FontCode(..) | PdfLast(_) | PdfRandom
        | Page(_) | Given(_) | LineNo | Ended | PrevGraf | Clang => Val::Int(0),
        GluePar(_) | MuGluePar(_) | Skip(_) | Muskip(_) => Val::Glue(crate::val::Glue::default()),
        TokPar(_) | Key::Toks(_) | AfterAssignment | Mark(..) => Val::Toks(empty_toks()),
        Key::Box(_) => Val::Box(None),
        Parshape => Val::Parshape(Arc::from(Vec::<(i32, i32)>::new())),
        PenShape(_) => Val::Ints(Arc::from(Vec::<i32>::new())),
        CurFont | FamFont(..) => Val::Font(0),
        HyphenChar(_) => Val::Int('-' as i32),
        SkewChar(_) => Val::Int(-1),
        Interaction => Val::Int(3),
        Conds => Val::Conds(Arc::from(Vec::<crate::val::Cond>::new())),
        SpaceFactor => Val::Int(1000),
        PrevDepth => Val::Int(IGNORE_DEPTH),
        JobName => Val::Name("texput".into()),
        Patterns(_) | Exceptions(_) | FontLoaded(..) | File(_) | ReadStream(_) | WriteOpen(_) | Tail
        | Other(..) => Val::Void,
    }
}

/// The INITEX environment: every primitive of the engine bound to its name.
pub fn initial_env(names: &Names, etex: bool, pdf: bool) -> Env {
    let reg = crate::prim::registry();
    let mut defs = Hamt::default();
    for (i, d) in reg.defs.iter().enumerate() {
        let on = match d.engine {
            0 => true,
            1 => etex,
            2 => pdf,
            _ => false,
        };
        if on {
            let key = Key::Cs(names.intern(d.name));
            defs = defs.insert(key, Entry { val: Val::Meaning(Meaning::Prim(i as PrimId)), level: 1 });
        }
    }
    Env { inner: Arc::new(EnvRep { defs, ..EnvRep::default() }) }
}

fn frame_datum(r: &EnvRep, f: &GroupFrame) -> FrameDatum {
    let saves = f.saves_vec();
    let cur = saves.iter().map(|(k, _)| (k.clone(), r.entry(k))).collect();
    FrameDatum { kind: f.kind, saves, after: f.after_toks(), cur }
}

pub fn env_lookup(e: &Env, k: &Key) -> Val {
    let r = &*e.inner;
    if let Some(w) = list_which(k) {
        return Val::Int(r.list.vals[w as usize]);
    }
    if let Key::Other(pk::TAG, n) = k {
        let c = n & 0xffff;
        return match pk::class(*n) {
            0 if *n == pk::GROUP_DEPTH => Val::Int(r.depth() as i32),
            0 if *n == pk::LIST_DEPTH => Val::Int(r.list.depth as i32),
            1 => Val::Int(r.group_up(c).map_or(0, |f| f.kind as i32)),
            2 => r.group_up(c).map_or(Val::Void, |f| Val::Data(Arc::new(frame_datum(r, &f)))),
            3 => Val::Int(r.list.up_n(c).map_or(0, |f| mode_code(f.mode))),
            4 => r.list.up_n(c / 4).map_or(Val::Void, |f| Val::Int(f.vals[(c % 4) as usize])),
            _ => Val::Void,
        };
    }
    match r.defs.get(k) {
        Some(e) => e.val.clone(),
        None => initial(k),
    }
}

pub fn env_apply(e: &Env, d: &Delta) -> Env {
    if d.0.is_empty() {
        return e.clone();
    }
    let mut r = (*e.inner).clone();
    for c in &d.0 {
        r.apply_change(c);
    }
    Env { inner: Arc::new(r) }
}

pub fn env_group_kind(e: &Env) -> GroupKind {
    e.inner.group.as_ref().map_or(GroupKind::Bottom, |g| g.kind)
}
pub fn env_group_depth(e: &Env) -> u32 {
    e.inner.depth()
}
pub fn env_mode(e: &Env) -> Mode {
    e.inner.list.mode
}
pub fn env_list_depth(e: &Env) -> u32 {
    e.inner.list.depth
}
pub fn env_after_group(e: &Env) -> Toks {
    e.inner.group.as_ref().map_or_else(empty_toks, |g| Arc::from(g.after_toks()))
}

/// The same env as the result of node `id`.
pub fn env_at(e: &Env, id: NodeId) -> Env {
    Env { inner: Arc::new(EnvRep { at: id, ..(*e.inner).clone() }) }
}

// ================================================================== lists

fn kind_matches(it: &Item, k: u8) -> bool {
    matches!(
        (it, k),
        (Item::Penalty(_), 0) | (Item::Kern { .. }, 1) | (Item::Glue { .. }, 2) | (Item::Box(_), 3)
    )
}

#[derive(Clone)]
enum Seg {
    Item(Item),
    Rem(u8),
}

/// Applies a segment to an empty list: the items left, and the removals
/// that met the empty list (they apply, in order, to what was before).
fn process(seg: Vec<Seg>) -> (Vec<Item>, Vec<u8>) {
    let mut own = Vec::new();
    let mut under = Vec::new();
    for s in seg {
        match s {
            Seg::Item(i) => own.push(i),
            Seg::Rem(k) => match own.last() {
                Some(l) => {
                    if kind_matches(l, k) {
                        own.pop();
                    }
                }
                None => under.push(k),
            },
        }
    }
    (own, under)
}

fn combine(mut ext: Vec<Item>, under: &[u8], own: Vec<Item>) -> Vec<Item> {
    for k in under {
        if ext.last().is_some_and(|l| kind_matches(l, *k)) {
            ext.pop();
        }
    }
    ext.extend(own);
    ext
}

// ================================================================== graph

/// Reads the graph does not answer from env: `Key::Page`, `Key::File`,
/// `Key::JobName`, `Key::Given` (page fold, fixpoint driver, build).
pub trait Answers: Send + Sync {
    /// `None`: not answered here (the graph falls back: files from disk,
    /// the rest from env).
    fn answer(&self, g: &Graph, key: &Key, at: &Thunk) -> Option<Val>;
}

#[derive(Clone)]
pub struct GraphOpts {
    pub etex: bool,
    pub pdf: bool,
    pub names: Arc<Names>,
    pub answers: Option<Arc<dyn Answers>>,
    /// Memoise nodes on (input, reads).
    pub memo: bool,
}

impl Default for GraphOpts {
    fn default() -> Self {
        GraphOpts { etex: true, pdf: true, names: Arc::new(Names::default()), answers: None, memo: true }
    }
}

#[derive(Default)]
struct Arena {
    nodes: Vec<Arc<Node>>,
    prev: Vec<NodeId>,
}

#[derive(Default)]
struct Texts {
    list: Vec<(Arc<str>, Arc<str>)>,
    ids: HashMap<(Arc<str>, Arc<str>), TextId>,
}

pub struct Graph {
    pub etex: bool,
    pub pdf: bool,
    pub names: Arc<Names>,
    answers: RwLock<Option<Arc<dyn Answers>>>,
    arena: RwLock<Arena>,
    texts: Mutex<Texts>,
    pub memo: crate::memo::Memo,
    memo_on: bool,
    halted: AtomicBool,
    relax: Option<PrimId>,
    // ---- the build's main chain (api.rs / fixpoint.rs / edit.rs)
    root: RwLock<Option<Thunk>>,
    chain: RwLock<Vec<Arc<Node>>>,
    /// The texts as the chain was last reduced from them.
    reduced_texts: RwLock<HashMap<TextId, Arc<str>>>,
    finder: RwLock<Option<Finder>>,
}

/// The file finder: a name as TeX asks for it → (the found name, its bytes).
pub type Finder = Arc<dyn Fn(&str) -> Option<(Arc<str>, Arc<[u8]>)> + Send + Sync>;

impl Graph {
    pub fn new(opts: GraphOpts) -> Graph {
        Graph {
            etex: opts.etex,
            pdf: opts.pdf,
            names: opts.names,
            answers: RwLock::new(opts.answers),
            arena: RwLock::new(Arena::default()),
            texts: Mutex::new(Texts::default()),
            memo: crate::memo::Memo::default(),
            memo_on: opts.memo,
            halted: AtomicBool::new(false),
            relax: crate::prim::registry().by_name("relax"),
            root: RwLock::new(None),
            chain: RwLock::new(Vec::new()),
            reduced_texts: RwLock::new(HashMap::new()),
            finder: RwLock::new(None),
        }
    }

    // ---- the build's main chain

    /// Files are looked up through `f` (else read from disk).
    pub fn set_finder(&self, f: Finder) {
        *self.finder.write().unwrap() = Some(f);
    }
    /// An input file's bytes through the finder (or the disk).
    pub fn find(&self, name: &str) -> Option<Arc<[u8]>> {
        let f = self.finder.read().unwrap().clone();
        match f {
            Some(f) => f(name).map(|(_, b)| b),
            None => std::fs::read(name).ok().map(Arc::from),
        }
    }
    /// Registers a source text; its id.
    pub fn add_text(&self, name: Arc<str>, data: Arc<str>) -> TextId {
        self.intern_text(name, data)
    }
    /// Text `id` is now `data` (the chain still holds the old one until re-reduced).
    pub fn set_text(&self, id: TextId, data: Arc<str>) {
        let mut t = self.texts.lock().unwrap();
        if let Some(e) = t.list.get_mut(id as usize) {
            e.1 = data;
        }
    }
    /// Text `id` as the chain was last reduced from it.
    pub fn reduced_text(&self, id: TextId) -> Option<Arc<str>> {
        self.reduced_texts.read().unwrap().get(&id).cloned()
    }
    /// The root thunk of the main chain; the chain is emptied.
    pub fn set_root(&self, t: Thunk) {
        *self.root.write().unwrap() = Some(t);
        self.chain.write().unwrap().clear();
    }
    /// The main chain's nodes, in order.
    pub fn nodes(&self) -> Vec<Arc<Node>> {
        self.chain.read().unwrap().clone()
    }
    /// The main chain as `edit.rs` takes it.
    pub fn chain(&self) -> crate::edit::Chain {
        let start = self.root.read().unwrap().clone().unwrap_or_else(|| self.start(self.initial_env(), Input::default()));
        crate::edit::Chain { start, nodes: self.chain.read().unwrap().iter().map(|n| (**n).clone()).collect() }
    }
    /// Replaces the main chain (after `edit::update`).
    pub fn set_chain(&self, c: crate::edit::Chain) {
        *self.root.write().unwrap() = Some(c.start);
        *self.chain.write().unwrap() = c.nodes.into_iter().map(Arc::new).collect();
    }
    /// Keeps the first `n` nodes of the main chain.
    pub fn truncate_chain(&self, n: usize) {
        self.chain.write().unwrap().truncate(n);
    }
    /// Nodes made in this graph so far (memo hits included).
    pub fn nodes_reduced(&self) -> usize {
        self.len()
    }
    /// The thunk after the main chain's last node.
    fn chain_end(&self) -> Option<Thunk> {
        match self.chain.read().unwrap().last() {
            Some(n) => Some(n.next.clone()),
            None => self.root.read().unwrap().clone(),
        }
    }
    /// The env after the main chain (the job's end env once reduced).
    pub fn end_env(&self) -> Env {
        self.chain_end().map_or_else(|| self.initial_env(), |t| t.env)
    }
    /// Reduces the main chain to the job's end (sequentially).
    pub fn reduce(&self) {
        {
            let t = self.texts.lock().unwrap();
            let mut r = self.reduced_texts.write().unwrap();
            for (i, (_, d)) in t.list.iter().enumerate() {
                r.insert(i as TextId, d.clone());
            }
        }
        let Some(mut t) = self.chain_end() else { return };
        while !Graph::done(&t) && !self.halted() {
            let n = self.reduce_one(&t);
            t = n.next.clone();
            self.chain.write().unwrap().push(n);
        }
    }

    pub fn set_answers(&self, a: Arc<dyn Answers>) {
        *self.answers.write().unwrap() = Some(a);
    }

    /// The INITEX env of this session's engine.
    pub fn initial_env(&self) -> Env {
        initial_env(&self.names, self.etex, self.pdf)
    }

    pub fn start(&self, env: Env, input: Input) -> Thunk {
        Thunk { env, input }
    }

    pub fn halt(&self) {
        self.halted.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.halted.store(false, Ordering::Relaxed);
    }
    pub fn halted(&self) -> bool {
        self.halted.load(Ordering::Relaxed)
    }

    // ---- texts
    pub fn intern_text(&self, name: Arc<str>, data: Arc<str>) -> TextId {
        let mut t = self.texts.lock().unwrap();
        if let Some(&id) = t.ids.get(&(name.clone(), data.clone())) {
            return id;
        }
        let id = t.list.len() as TextId;
        t.list.push((name.clone(), data.clone()));
        t.ids.insert((name, data), id);
        id
    }
    pub fn text(&self, id: TextId) -> Option<(Arc<str>, Arc<str>)> {
        self.texts.lock().unwrap().list.get(id as usize).cloned()
    }

    // ---- the arena
    pub fn node(&self, id: NodeId) -> Option<Arc<Node>> {
        self.arena.read().unwrap().nodes.get(id as usize).cloned()
    }
    /// The node before `id` in its chain.
    pub fn prev(&self, id: NodeId) -> NodeId {
        self.arena.read().unwrap().prev.get(id as usize).copied().unwrap_or(NO_NODE)
    }
    /// A node's id (its next env names it).
    pub fn id_of(n: &Node) -> NodeId {
        n.next.env.inner.at
    }
    pub fn len(&self) -> usize {
        self.arena.read().unwrap().nodes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Appends a node after thunk `at` (used for memo hits, and by the page
    /// builder / output for `Op::Fire`). Its next env is `at.env + delta`.
    pub fn push_node(&self, at: &Thunk, op: Op, spans: Vec<Span>, reads: Vec<(Key, Val)>, out: Out, input: Input) -> Arc<Node> {
        let env = at.env.apply(&out.delta);
        let mut a = self.arena.write().unwrap();
        let id = a.nodes.len() as NodeId;
        let n = Arc::new(Node { op, spans, reads, out, next: Thunk { env: env_at(&env, id), input } });
        a.nodes.push(n.clone());
        a.prev.push(at.env.inner.at);
        n
    }

    /// Reduces `t` into exactly one node.
    pub fn reduce_one(&self, t: &Thunk) -> Arc<Node> {
        if self.memo_on {
            if let Some(old) = self.memo.find(self, t) {
                return self.push_node(t, old.op.clone(), old.spans.clone(), old.reads.clone(), old.out.clone(), old.next.input.clone());
            }
        }
        let mut r = Reducing::new(self, t);
        let op = r.step();
        let n = r.finish(op);
        if self.memo_on && !self.halted() {
            self.memo.insert(&t.input, n.clone());
        }
        n
    }

    /// Reduces `t` into one node with answers `ans` over the graph's and a
    /// cancel flag (speculation); never memoised.
    pub fn reduce_with<'a>(&'a self, t: &Thunk, ans: &'a dyn Answers, cancel: &'a AtomicBool) -> Arc<Node> {
        let mut r = Reducing::new(self, t);
        r.over = Some(ans);
        r.cancel = Some(cancel);
        let op = r.step();
        r.finish(op)
    }

    /// The job has nothing more to reduce from `t`.
    pub fn done(t: &Thunk) -> bool {
        t.input.is_empty() || t.env.lookup(&Key::Ended).int() != 0
    }

    /// Reduces from `t` until the input is exhausted, the job ended, `stop`
    /// says so (the stopping node included), or the graph is halted.
    pub fn run(&self, t: Thunk, stop: &dyn Fn(&Node) -> bool) -> Vec<Arc<Node>> {
        let mut out = Vec::new();
        let mut t = t;
        while !Graph::done(&t) && !self.halted() {
            let n = self.reduce_one(&t);
            out.push(n.clone());
            if stop(&n) {
                break;
            }
            t = n.next.clone();
        }
        out
    }

    /// The items of the innermost list open at `t`.
    pub fn list_items(&self, t: &Thunk) -> Vec<Item> {
        self.ext_suffix(t.env.inner.at, 0, usize::MAX).items.to_vec()
    }
    /// The items of the list `c` levels out from the innermost at `t`.
    pub fn list_items_up(&self, t: &Thunk, c: u32) -> Vec<Item> {
        self.ext_suffix(t.env.inner.at, c, usize::MAX).items.to_vec()
    }

    /// The last `k` items (all when fewer) of the list `c` levels out at the
    /// end of node `at`, gathered back along the chain.
    pub fn ext_suffix(&self, at: NodeId, c: u32, k: usize) -> ItemsDatum {
        let mut d = 0u32;
        let mut u = 0u32;
        let mut pending: Vec<u8> = Vec::new();
        let mut out: Vec<Item> = Vec::new();
        let mut whole = true;
        let mut id = at;
        'chain: while id != NO_NODE {
            let Some(n) = self.node(id) else { break };
            for e in n.out.emits.iter().rev() {
                match e {
                    Emit::Close => d += 1,
                    Emit::Open(_) => {
                        if d > 0 {
                            d -= 1;
                        } else {
                            if u == c {
                                break 'chain;
                            }
                            u += 1;
                        }
                    }
                    Emit::Item(it) if d == 0 && u == c => {
                        let mut removed = false;
                        while let Some(kind) = pending.pop() {
                            if kind_matches(it, kind) {
                                removed = true;
                                break;
                            }
                        }
                        if !removed {
                            out.push(it.clone());
                            if out.len() >= k {
                                whole = false;
                                break 'chain;
                            }
                        }
                    }
                    Emit::RemoveTail(kind) if d == 0 && u == c => pending.push(*kind),
                    _ => {}
                }
            }
            id = self.prev(id);
        }
        out.reverse();
        ItemsDatum { items: out.into(), whole }
    }

    fn ext_holds(&self, t: &Thunk, c: u32, v: &Val) -> bool {
        let Val::Data(d) = v else { return false };
        let Some(rec) = d.as_any().downcast_ref::<ItemsDatum>() else { return false };
        let k = if rec.whole { usize::MAX } else { rec.items.len().max(1) };
        let cand = self.ext_suffix(t.env.inner.at, c, k);
        cand.items == rec.items && (!rec.whole || cand.whole)
    }

    /// The answer to an answered key at `t`.
    pub fn answer(&self, key: &Key, t: &Thunk) -> Val {
        let a = self.answers.read().unwrap().clone();
        if let Some(a) = a {
            if let Some(v) = a.answer(self, key, t) {
                return v;
            }
        }
        match key {
            Key::File(n) => match self.find(n) {
                Some(b) => Val::Bytes(b),
                None => Val::Void,
            },
            Key::LineNo => Val::Int(line_of(&t.input)),
            // GAP: with no page fold answering (no `sched::Sched` driving the
            // build), the page reads as empty.
            Key::Other(tg, crate::page::EMPTY) if *tg == crate::prim::tag::PAGE => Val::Int(1),
            _ => t.env.lookup(key),
        }
    }

    /// Does the read `(k, v)` still hold at thunk `t`?
    pub fn holds(&self, t: &Thunk, k: &Key, v: &Val) -> bool {
        match k {
            Key::Tail => self.ext_holds(t, 0, v),
            Key::Other(pk::TAG, n) if pk::class(*n) == pk::class(pk::CHAIN) => self.ext_holds(t, n & 0xffff, v),
            Key::LineNo => Val::Int(line_of(&t.input)) == *v,
            Key::Page(_) | Key::File(_) | Key::JobName | Key::Given(_) => self.answer(k, t) == *v,
            Key::Other(tg, _) if *tg == crate::prim::tag::PAGE => self.answer(k, t) == *v,
            _ => t.env.lookup(k) == *v,
        }
    }

    /// Do all of a node's reads hold at `t`?
    pub fn reads_hold(&self, t: &Thunk, reads: &[(Key, Val)]) -> bool {
        reads.iter().all(|(k, v)| self.holds(t, k, v))
    }
}

/// A node's consumed spans.
pub fn node_spans(n: &Node) -> &[Span] {
    &n.spans
}

/// TeX's `line` of the innermost text in `input` (0 before its first line).
pub fn line_of(input: &Input) -> i32 {
    let mut i = input.clone();
    loop {
        match i.top() {
            None => return 0,
            Some(Src::Text { data, line_end, pos, .. }) => {
                if *line_end == 0 && *pos == 0 {
                    return 0;
                }
                let b = data.as_bytes();
                let mut p = (*line_end as usize).min(b.len());
                if p > 0 && b[p - 1] == b'\n' {
                    p -= 1;
                }
                return b[..p].iter().filter(|&&c| c == b'\n').count() as i32 + 1;
            }
            Some(_) => i = i.below(),
        }
    }
}

fn merge_span(v: &mut Vec<Span>, s: Span) {
    if let Some(l) = v.last_mut() {
        if l.text == s.text && l.to == s.from {
            l.to = s.to;
            return;
        }
    }
    v.push(s);
}

// ============================================================== reducing

/// The read recorder (split from `Reducing` so the tokenizer's catcode
/// callback can borrow it beside the env).
#[derive(Default)]
struct Rec {
    reads: Vec<(Key, Val)>,
    read_set: HashSet<Key>,
    written: HashSet<Key>,
    written_list: HashSet<(u32, u8)>,
    g_opened: u32,
    g_popped: u32,
    l_opened: u32,
    l_popped: u32,
}

impl Rec {
    fn record(&mut self, k: Key, v: &Val) {
        if self.read_set.insert(k.clone()) {
            self.reads.push((k, v.clone()));
        }
    }
    fn get(&mut self, env: &Env, k: &Key) -> Val {
        let v = env.lookup(k);
        if let Some(w) = list_which(k) {
            if self.l_opened == 0 && !self.written_list.contains(&(self.l_popped, w)) {
                let rk = if self.l_popped == 0 {
                    k.clone()
                } else {
                    Key::Other(pk::TAG, pk::LIST_KEY + 4 * self.l_popped + u32::from(w))
                };
                self.record(rk, &v);
            }
            return v;
        }
        if !self.written.contains(k) {
            self.record(k.clone(), &v);
        }
        v
    }
}

pub struct Reducing<'g> {
    g: &'g Graph,
    start: Thunk,
    env: Env,
    input: Input,
    rec: Rec,
    spans: Vec<Span>,
    emits: Vec<Emit>,
    delta: Vec<Change>,
    /// Per-reduction answers (speculation's snapshot) over the graph's.
    over: Option<&'g dyn Answers>,
    /// Per-reduction cancel flag (a speculative chain's).
    cancel: Option<&'g AtomicBool>,
}

impl<'g> Reducing<'g> {
    fn stopped(&self) -> bool {
        self.g.halted() || self.cancel.is_some_and(|c| c.load(Ordering::Relaxed))
    }

    pub fn new(g: &'g Graph, t: &Thunk) -> Reducing<'g> {
        Reducing {
            g,
            start: t.clone(),
            env: t.env.clone(),
            input: t.input.clone(),
            rec: Rec::default(),
            spans: Vec::new(),
            emits: Vec::new(),
            delta: Vec::new(),
            over: None,
            cancel: None,
        }
    }

    fn start_at(&self) -> NodeId {
        self.start.env.inner.at
    }

    fn change(&mut self, c: Change) {
        let mut r = (*self.env.inner).clone();
        r.apply_change(&c);
        self.env = Env { inner: Arc::new(r) };
        self.delta.push(c);
    }

    /// One raw token with its span.
    fn take(&mut self) -> Option<(Tok, Option<Span>)> {
        if self.stopped() {
            return None;
        }
        let names = self.g.names.clone();
        let Reducing { env, rec, input, .. } = self;
        let mut f = |k: &Key| rec.get(env, k);
        match crate::tokenize::next_token(&mut f, &names, input) {
            Some((t, inp, sp)) => {
                self.input = inp;
                if let Some(s) = sp {
                    merge_span(&mut self.spans, s);
                }
                Some((t, sp))
            }
            None => {
                self.input = Input::default();
                None
            }
        }
    }

    fn relax_op(&self) -> Op {
        match self.g.relax {
            Some(id) => Op::Command(id),
            None => Op::Chars,
        }
    }

    fn tok_text(&self, t: &Tok) -> String {
        match t {
            Tok::Cs(c) => format!("\\{}", self.g.names.name(*c)),
            Tok::Active(c) => c.to_string(),
            Tok::Char { ch, .. } => ch.to_string(),
            Tok::Param(_) => "#".into(),
            Tok::DontExpand => "\\notexpanded:".into(),
        }
    }

    fn mode_name(m: Mode) -> &'static str {
        match m {
            Mode::Vertical { inner: false } => "vertical mode",
            Mode::Vertical { inner: true } => "internal vertical mode",
            Mode::Horizontal { inner: false } => "horizontal mode",
            Mode::Horizontal { inner: true } => "restricted horizontal mode",
            Mode::Math { display: true } => "display math mode",
            Mode::Math { display: false } => "math mode",
        }
    }

    fn expandable(&mut self, t: &Tok) -> bool {
        match self.meaning(t) {
            Meaning::Macro(_) => true,
            Meaning::Undefined => matches!(t, Tok::Cs(_) | Tok::Active(_)),
            Meaning::Prim(id) => {
                matches!(crate::prim::registry().get(id).kind, Kind::Expand(_) | Kind::If(_))
            }
            _ => false,
        }
    }

    fn undefined(&mut self) {
        crate::diag::error(
            self,
            "Undefined control sequence",
            &[
                "The control sequence at the end of the top line",
                "of your error message was never \\def'ed. If you have",
                "misspelled it (e.g., `\\hobx'), type `I' and the correct",
                "spelling (e.g., `I\\hbox'). Otherwise just continue,",
                "and I'll forget about whatever was undefined.",
            ],
        );
    }

    fn insert_dollar(&mut self, t: Tok) {
        self.back(t);
        self.push(Arc::from(vec![Tok::Char { cat: 3, ch: '$' }]), TokKind::Inserted);
        crate::diag::error(
            self,
            "Missing $ inserted",
            &["I've inserted a begin-math/end-math symbol since I think", "you left one out. Proceed, with fingers crossed."],
        );
    }

    /// main_control's dispatch for one token: the op of the node it makes.
    pub fn step(&mut self) -> Op {
        let Some(mut t) = self.next() else { return Op::Chars };
        if t == Tok::DontExpand {
            let Some(t2) = self.next() else { return Op::Chars };
            if self.expandable(&t2) {
                // `\noexpand`ed: means `\relax` here.
                return self.relax_op();
            }
            t = t2;
        }
        match self.meaning(&t) {
            Meaning::Macro(f) => self.apply(t, f),
            Meaning::Prim(id) => {
                let d = *crate::prim::registry().get(id);
                match d.kind {
                    Kind::Expand(f) => {
                        f(self, d.code);
                        Op::Expand(id)
                    }
                    Kind::If(_) => {
                        let (taken, untaken) = crate::cond::begin_if(self, id, d.code);
                        Op::Select { taken, untaken }
                    }
                    Kind::Command(f) => {
                        f(self, d.code);
                        Op::Command(id)
                    }
                    Kind::Quantity { .. } | Kind::Computed { .. } => {
                        crate::assign::prefixed_command(self, id, 0);
                        Op::Command(id)
                    }
                    Kind::Prefix => {
                        self.prefixes(d.code as u8);
                        Op::Command(id)
                    }
                }
            }
            Meaning::Undefined => match t {
                Tok::Cs(_) | Tok::Active(_) => {
                    self.undefined();
                    self.relax_op()
                }
                Tok::Char { cat, .. } => self.char_tok(t, cat),
                Tok::Param(_) => self.char_tok(t, 6),
                Tok::DontExpand => Op::Chars,
            },
            Meaning::Char { cat, .. } => self.char_tok(t, cat),
            Meaning::CharGiven(_) => {
                match self.mode() {
                    Mode::Vertical { .. } => {
                        self.back(t);
                        crate::list::new_graf(self, true);
                    }
                    _ => crate::list::char_tok(self, t),
                }
                Op::Chars
            }
            Meaning::MathCharGiven(v) => {
                match self.mode() {
                    Mode::Math { .. } => crate::math::math_given(self, v),
                    _ => self.insert_dollar(t),
                }
                Op::Chars
            }
            Meaning::Reg(..) | Meaning::Font(_) => {
                crate::assign::assign_meaning(self, t, 0);
                self.relax_op()
            }
        }
    }

    /// `\global` & co: gather prefixes, then the assignment they apply to.
    fn prefixes(&mut self, first: u8) {
        let mut p = first;
        loop {
            let Some(t) = self.next_x() else { return };
            if matches!(t, Tok::Char { cat: 10, .. }) {
                continue;
            }
            match self.meaning(&t) {
                Meaning::Prim(id) => {
                    let d = *crate::prim::registry().get(id);
                    if matches!(d.kind, Kind::Prefix) {
                        p |= d.code as u8;
                        continue;
                    }
                    if Some(id) == self.g.relax {
                        continue;
                    }
                    crate::assign::prefixed_command(self, id, p);
                    return;
                }
                Meaning::Reg(..) | Meaning::Font(_) => {
                    crate::assign::assign_meaning(self, t, p);
                    return;
                }
                _ => {
                    let msg = format!("You can't use a prefix with `{}'", self.tok_text(&t));
                    let help: &[&str] = if self.g.etex {
                        &["I'll pretend you didn't say \\long or \\outer or \\global or \\protected."]
                    } else {
                        &["I'll pretend you didn't say \\long or \\outer or \\global."]
                    };
                    self.back(t);
                    crate::diag::error(self, &msg, help);
                    return;
                }
            }
        }
    }

    /// A macro call at the top: its own `Apply` node.
    fn apply(&mut self, t: Tok, f: Arc<crate::val::Macro>) -> Op {
        match crate::macros::match_args(self, t, &f) {
            Some(args) => {
                let args: Arc<[Toks]> = args.into();
                let body = self.g.memo.apply(&f, &args, || crate::macros::substitute(&f.body, &args));
                self.push(body, TokKind::Macro);
                Op::Apply { f, args }
            }
            None => Op::Apply { f, args: Arc::from(Vec::<Toks>::new()) },
        }
    }

    /// A character token (or a `\let` copy of one) at the top.
    fn char_tok(&mut self, t: Tok, cat: u8) -> Op {
        let mode = self.mode();
        match cat {
            1 => crate::groups::left_brace(self),
            2 => crate::groups::right_brace(self),
            3 => match mode {
                Mode::Vertical { .. } => {
                    self.back(t);
                    crate::list::new_graf(self, true);
                }
                _ => crate::math::math_shift(self, t),
            },
            4 => crate::align::tab_mark(self, t),
            6 => {
                let msg = format!("You can't use `macro parameter character #' in {}", Self::mode_name(mode));
                crate::diag::error(self, &msg, &["Sorry, but I'm not programmed to handle this case;", "I'll just pretend that you didn't ask for it."]);
            }
            7 | 8 => match mode {
                Mode::Math { .. } => crate::math::sub_sup(self, t, cat == 7),
                _ => self.insert_dollar(t),
            },
            10 => {
                if let Mode::Horizontal { .. } = mode {
                    crate::list::char_tok(self, t);
                }
            }
            11 | 12 => match mode {
                Mode::Vertical { .. } => {
                    self.back(t);
                    crate::list::new_graf(self, true);
                }
                _ => crate::list::char_tok(self, t),
            },
            _ => {}
        }
        Op::Chars
    }

    /// The node: id, next thunk.
    pub fn finish(self, op: Op) -> Arc<Node> {
        let g = self.g;
        let mut a = g.arena.write().unwrap();
        let id = a.nodes.len() as NodeId;
        let n = Arc::new(Node {
            op,
            spans: self.spans,
            reads: self.rec.reads,
            out: Out { delta: Delta(self.delta), emits: self.emits },
            next: Thunk { env: env_at(&self.env, id), input: self.input },
        });
        a.nodes.push(n.clone());
        a.prev.push(self.start.env.inner.at);
        n
    }

    // ---- lists

    /// The node's own emits that belong to the current list: (the list was
    /// opened by this node, the segment).
    fn own_segment(&self) -> (bool, Vec<Seg>) {
        let mut d = 0u32;
        let mut open_at = None;
        for (i, e) in self.emits.iter().enumerate().rev() {
            match e {
                Emit::Close => d += 1,
                Emit::Open(_) => {
                    if d == 0 {
                        open_at = Some(i);
                        break;
                    }
                    d -= 1;
                }
                _ => {}
            }
        }
        let (from, target) = match open_at {
            Some(i) => (i + 1, 0),
            None => (0, self.rec.l_popped),
        };
        let mut seg = Vec::new();
        let (mut pos, mut depth) = (0u32, 0u32);
        for e in &self.emits[from..] {
            match e {
                Emit::Open(_) => depth += 1,
                Emit::Close => {
                    if depth > 0 {
                        depth -= 1;
                    } else {
                        pos += 1;
                    }
                }
                Emit::Item(it) if depth == 0 && pos == target => seg.push(Seg::Item(it.clone())),
                Emit::RemoveTail(k) if depth == 0 && pos == target => seg.push(Seg::Rem(*k)),
                _ => {}
            }
        }
        (open_at.is_some(), seg)
    }

    fn record_ext(&mut self, c: u32, ext: &ItemsDatum) {
        let key = if c == 0 { Key::Tail } else { Key::Other(pk::TAG, pk::CHAIN + c) };
        let v = Val::Data(Arc::new(ext.clone()));
        if let Some(slot) = self.rec.reads.iter_mut().find(|(k, _)| *k == key) {
            let more = match &slot.1 {
                Val::Data(d) => d.as_any().downcast_ref::<ItemsDatum>().is_none_or(|o| {
                    (ext.whole && !o.whole) || (!o.whole && ext.items.len() > o.items.len())
                }),
                _ => true,
            };
            if more {
                slot.1 = v;
            }
        } else {
            self.rec.record(key, &v);
        }
    }

    // ---- reads with a graph answer

    fn line_now(&self) -> i32 {
        line_of(&self.input)
    }
}

impl Ctx for Reducing<'_> {
    fn next(&mut self) -> Option<Tok> {
        self.take().map(|(t, _)| t)
    }

    fn next_x(&mut self) -> Option<Tok> {
        loop {
            let t = self.next()?;
            if t == Tok::DontExpand {
                // The marked token is not expanded (it acts as `\relax` if expandable).
                return self.next();
            }
            match self.meaning(&t) {
                Meaning::Macro(f) => crate::macros::expand_macro(self, t, &f),
                Meaning::Prim(id) => {
                    let d = *crate::prim::registry().get(id);
                    match d.kind {
                        Kind::Expand(f) => f(self, d.code),
                        Kind::If(_) => {
                            crate::cond::begin_if(self, id, d.code);
                        }
                        _ => return Some(t),
                    }
                }
                Meaning::Undefined if matches!(t, Tok::Cs(_) | Tok::Active(_)) => self.undefined(),
                _ => return Some(t),
            }
            if self.stopped() {
                return None;
            }
        }
    }

    fn back(&mut self, t: Tok) {
        self.input = self.input.push(Src::Toks { toks: Arc::from(vec![t]), at: 0, kind: TokKind::Backed });
    }

    fn push(&mut self, toks: Toks, kind: TokKind) {
        if toks.is_empty() && !matches!(kind, TokKind::UTemplate | TokKind::VTemplate) {
            return;
        }
        self.input = self.input.push(Src::Toks { toks, at: 0, kind });
    }

    fn push_text(&mut self, name: Arc<str>, data: Arc<str>) -> TextId {
        let id = self.g.intern_text(name, data.clone());
        self.input = self.input.push(crate::tokenize::text_source(id, data));
        id
    }

    fn end_input(&mut self) {
        let mut above = Vec::new();
        let mut cur = self.input.clone();
        loop {
            match cur.top() {
                None => return,
                Some(Src::Text { .. }) => break,
                Some(s) => {
                    above.push(s.clone());
                    cur = cur.below();
                }
            }
        }
        let Some(Src::Text { text, data, pos, line, line_end, eol, .. }) = cur.top().cloned() else { return };
        let mut inp = cur.below().push(Src::Text { text, data, pos, line, line_end, eol, ending: true });
        for s in above.into_iter().rev() {
            inp = inp.push(s);
        }
        self.input = inp;
    }

    fn reading(&self) -> Option<TokKind> {
        let mut i = self.input.clone();
        loop {
            match i.top() {
                Some(Src::Toks { toks, at, kind }) => {
                    if (*at as usize) < toks.len() {
                        return Some(*kind);
                    }
                    i = i.below();
                }
                _ => return None,
            }
        }
    }

    fn line_no(&mut self) -> i32 {
        let start = line_of(&self.start.input);
        self.rec.record(Key::LineNo, &Val::Int(start));
        self.line_now()
    }

    fn skip_raw(&mut self, stop: &mut dyn FnMut(&Tok, i32) -> bool, nest: &mut dyn FnMut(&Tok) -> i32) -> Vec<Span> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        while let Some((t, sp)) = self.take() {
            if let Some(s) = sp {
                merge_span(&mut out, s);
            }
            if stop(&t, depth) {
                break;
            }
            depth += nest(&t);
        }
        out
    }

    fn get(&mut self, key: &Key) -> Val {
        match key {
            Key::Tail => {
                let t = self.tail();
                Val::Data(Arc::new(ItemsDatum { items: t.into_iter().collect(), whole: false }))
            }
            Key::LineNo => Val::Int(self.line_no()),
            Key::Page(_) | Key::File(_) | Key::JobName | Key::Given(_) | Key::Other(crate::prim::tag::PAGE, _) => {
                if self.rec.written.contains(key) {
                    return self.env.lookup(key);
                }
                let v = match self.over.and_then(|a| a.answer(self.g, key, &self.start)) {
                    Some(v) => v,
                    None => self.g.answer(key, &self.start),
                };
                self.rec.record(key.clone(), &v);
                v
            }
            _ => self.rec.get(&self.env, key),
        }
    }

    fn meaning(&mut self, t: &Tok) -> Meaning {
        let key = match t {
            Tok::Cs(c) => Key::Cs(*c),
            Tok::Active(c) => Key::Active(*c),
            Tok::Char { cat, ch } => return Meaning::Char { cat: *cat, ch: *ch },
            Tok::Param(_) => return Meaning::Char { cat: 6, ch: '#' },
            Tok::DontExpand => return Meaning::Undefined,
        };
        match self.get(&key) {
            Val::Meaning(m) => m,
            _ => Meaning::Undefined,
        }
    }

    fn define(&mut self, key: Key, val: Val, global: bool) {
        if let Some(w) = list_which(&key) {
            if self.rec.l_opened == 0 {
                self.rec.written_list.insert((self.rec.l_popped, w));
            }
        } else {
            self.rec.written.insert(key.clone());
        }
        self.change(Change::Def { key, val, global });
    }

    fn replace(&mut self, key: Key, val: Val) {
        self.rec.written.insert(key.clone());
        self.change(Change::Replace { key, val });
    }

    fn open_group(&mut self, kind: GroupKind) {
        self.rec.g_opened += 1;
        self.change(Change::OpenGroup(kind));
    }

    fn close_group(&mut self) {
        let Some(frame) = self.env.inner.group.clone() else { return };
        if self.rec.g_opened > 0 {
            // Restored entries are values from before the group: read again if used.
            for (k, _) in frame.saves_vec() {
                self.rec.written.remove(&k);
            }
            self.rec.g_opened -= 1;
        } else {
            let c = self.rec.g_popped;
            let key = Key::Other(pk::TAG, pk::FRAME + c);
            let v = self.start.env.lookup(&key);
            if let Val::Data(d) = &v {
                if let Some(fd) = d.as_any().downcast_ref::<FrameDatum>() {
                    for (k, _) in &fd.saves {
                        self.rec.written.insert(k.clone());
                    }
                }
            }
            self.rec.record(key, &v);
            self.rec.g_popped += 1;
        }
        let toks: Toks = Arc::from(frame.after_toks());
        self.change(Change::CloseGroup);
        self.push(toks, TokKind::AfterGroup);
    }

    fn after_group(&mut self, t: Tok) {
        self.change(Change::AfterGroup(t));
    }

    fn group_kind(&mut self) -> GroupKind {
        let k = self.env.group_kind();
        if self.rec.g_opened == 0 {
            self.rec.record(Key::Other(pk::TAG, pk::GROUP_KIND + self.rec.g_popped), &Val::Int(k as i32));
        }
        k
    }

    fn group_depth(&mut self) -> u32 {
        let d = self.env.group_depth();
        let start = d + self.rec.g_popped - self.rec.g_opened;
        self.rec.record(Key::Other(pk::TAG, pk::GROUP_DEPTH), &Val::Int(start as i32));
        d
    }

    fn mode(&mut self) -> Mode {
        let m = self.env.mode();
        if self.rec.l_opened == 0 {
            self.rec.record(Key::Other(pk::TAG, pk::MODE + self.rec.l_popped), &Val::Int(mode_code(m)));
        }
        m
    }

    fn open_list(&mut self, mode: Mode) {
        self.rec.l_opened += 1;
        self.emits.push(Emit::Open(mode));
        self.change(Change::OpenList(mode));
    }

    fn close_list(&mut self) -> Vec<Item> {
        let (internal, seg) = self.own_segment();
        let (own, under) = process(seg);
        let items = if internal {
            own
        } else {
            let c = self.rec.l_popped;
            let ext = self.g.ext_suffix(self.start_at(), c, usize::MAX);
            self.record_ext(c, &ext);
            combine(ext.items.to_vec(), &under, own)
        };
        if self.env.list_depth() == 0 {
            return items;
        }
        self.emits.push(Emit::Close);
        self.change(Change::CloseList);
        if self.rec.l_opened > 0 {
            self.rec.l_opened -= 1;
        } else {
            self.rec.l_popped += 1;
        }
        items
    }

    fn tail(&mut self) -> Option<Item> {
        let (internal, seg) = self.own_segment();
        let (own, under) = process(seg);
        if let Some(l) = own.last() {
            return Some(l.clone());
        }
        if internal {
            return None;
        }
        let c = self.rec.l_popped;
        let ext = self.g.ext_suffix(self.start_at(), c, under.len() + 1);
        self.record_ext(c, &ext);
        combine(ext.items.to_vec(), &under, Vec::new()).last().cloned()
    }

    fn emit(&mut self, e: Emit) {
        self.emits.push(e);
    }

    fn names(&self) -> &Names {
        &self.g.names
    }

    fn file(&mut self, name: &str) -> Option<(Arc<str>, Arc<[u8]>)> {
        let cands: Vec<String> = if std::path::Path::new(name).extension().is_some() {
            vec![name.to_string()]
        } else {
            vec![format!("{name}.tex"), name.to_string()]
        };
        for c in cands {
            let n: Arc<str> = c.as_str().into();
            if let Val::Bytes(b) = self.get(&Key::File(n.clone())) {
                return Some((n, b));
            }
        }
        None
    }

    fn etex(&self) -> bool {
        self.g.etex
    }
    fn pdf(&self) -> bool {
        self.g.pdf
    }
    fn halted(&self) -> bool {
        self.stopped()
    }
}
