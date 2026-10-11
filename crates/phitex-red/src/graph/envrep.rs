// The environment's representation (included into graph.rs: one module).
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


/// `Env::hash` (P3: maintain incrementally in `EnvRep`; every `Change` folds in).
pub fn env_hash(_e: &Env) -> u128 {
    todo!("P3: env hash")
}
