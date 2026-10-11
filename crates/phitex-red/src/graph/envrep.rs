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

/// A name's binding: the definition reaching it (an SSA value) and TeX's
/// level it was made at.
#[derive(Clone, Debug)]
pub struct Entry {
    pub cell: Arc<crate::env::DefCell>,
    /// TeX's level: 1 global/bottom, depth+1 inside groups.
    pub level: u32,
}

impl Entry {
    pub fn val(&self) -> Val {
        self.cell.val()
    }
}

impl PartialEq for Entry {
    /// The same definition, or an equal value at the same level.
    fn eq(&self, o: &Self) -> bool {
        self.level == o.level && (self.cell.id_now() == o.cell.id_now() || self.cell.val() == o.cell.val())
    }
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

/// A persistent list (newest first).
#[derive(Debug)]
pub struct Cons<T> {
    pub head: T,
    pub next: Option<Arc<Cons<T>>>,
}

/// A persistent list's elements, oldest first.
pub fn cons_list<T: Clone>(c: &Option<Arc<Cons<T>>>) -> Vec<T> {
    cons_vec(c.clone())
}

fn cons_vec<T: Clone>(mut c: Option<Arc<Cons<T>>>) -> Vec<T> {
    let mut v = Vec::new();
    while let Some(x) = c {
        v.push(x.head.clone());
        c = x.next.clone();
    }
    v.reverse();
    v
}

fn cons_push<T>(c: &Option<Arc<Cons<T>>>, head: T) -> Option<Arc<Cons<T>>> {
    Some(Arc::new(Cons { head, next: c.clone() }))
}

/// One open group: the bindings when it opened (`snap`), the global
/// definitions made inside it (oldest first when read), the keys defined
/// locally at its level, its `\aftergroup` tokens. Its end is
/// `snap + globals`: no entry-by-entry restore of a save stack.
#[derive(Clone, Debug)]
pub struct GroupFrame {
    pub kind: GroupKind,
    pub depth: u32,
    pub snap: Hamt,
    pub globals: Option<Arc<Cons<(Key, Entry)>>>,
    pub locals: Option<Arc<Cons<Key>>>,
    pub after: Option<Arc<Cons<Tok>>>,
    pub up: Option<Arc<GroupFrame>>,
}

impl GroupFrame {
    pub fn after_toks(&self) -> Vec<Tok> {
        cons_vec(self.after.clone())
    }
    fn local_keys(&self) -> Vec<Key> {
        let mut v = cons_vec(self.locals.clone());
        v.sort();
        v.dedup();
        v
    }
}

/// One open list: its mode and its list-scope definitions (spacefactor,
/// prevdepth, prevgraf, clang), each a definition cell like any other.
#[derive(Clone, Debug)]
pub struct ListFrame {
    pub mode: Mode,
    pub depth: u32,
    pub vals: [Arc<crate::env::DefCell>; 4],
    pub up: Option<Arc<ListFrame>>,
}

impl ListFrame {
    fn new(mode: Mode, depth: u32, up: Option<Arc<ListFrame>>, mk: &mut dyn FnMut(Val) -> Arc<crate::env::DefCell>) -> ListFrame {
        let vals = [mk(Val::Int(1000)), mk(Val::Int(IGNORE_DEPTH)), mk(Val::Int(0)), mk(Val::Int(0))];
        ListFrame { mode, depth, vals, up }
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

/// The environment's representation: the reaching-definition map (key →
/// definition cell), the group chain, the list chain.
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
        let mut mk = |v: Val| crate::env::DefCell::new(v);
        EnvRep {
            defs: Hamt::default(),
            group: None,
            list: Arc::new(ListFrame::new(Mode::Vertical { inner: false }, 0, None, &mut mk)),
            at: NO_NODE,
        }
    }
}

impl EnvRep {
    pub fn depth(&self) -> u32 {
        self.group.as_ref().map_or(0, |g| g.depth)
    }
    fn group_up(&self, c: u32) -> Option<Arc<GroupFrame>> {
        let mut f = self.group.clone();
        for _ in 0..c {
            f = f.and_then(|x| x.up.clone());
        }
        f
    }
    fn entry(&self, k: &Key) -> Option<Entry> {
        self.defs.get(k).cloned()
    }
    fn with_frame(&mut self, f: impl FnOnce(&mut GroupFrame)) {
        if let Some(g) = &self.group {
            let mut n = (**g).clone();
            f(&mut n);
            self.group = Some(Arc::new(n));
        }
    }
    fn define(&mut self, key: &Key, cell: Arc<crate::env::DefCell>, global: bool) {
        if let Some(w) = list_which(key) {
            let mut f = (*self.list).clone();
            f.vals[w as usize] = cell;
            self.list = Arc::new(f);
            return;
        }
        if global {
            let e = Entry { cell, level: 1 };
            self.defs = self.defs.insert(key.clone(), e.clone());
            let k = key.clone();
            self.with_frame(|f| f.globals = cons_push(&f.globals, (k, e)));
            return;
        }
        let level = self.depth() + 1;
        self.defs = self.defs.insert(key.clone(), Entry { cell, level });
        if level > 1 {
            let k = key.clone();
            self.with_frame(|f| f.locals = cons_push(&f.locals, k));
        }
    }
    /// TeX's in-place change (`box(n):=null`, `\wd n:=`): a new definition at
    /// the level the current one lives at, seen by every group end that would
    /// restore that level's binding.
    fn replace(&mut self, key: &Key, cell: Arc<crate::env::DefCell>) {
        if list_which(key).is_some() {
            self.define(key, cell, false);
            return;
        }
        let level = self.entry(key).map_or(1, |e| e.level);
        if level <= 1 {
            self.define(key, cell, true);
            return;
        }
        let e = Entry { cell, level };
        self.defs = self.defs.insert(key.clone(), e.clone());
        // frames opened at depth ≥ level hold the level's binding in their snapshot
        fn fix(f: &Option<Arc<GroupFrame>>, key: &Key, e: &Entry, level: u32) -> Option<Arc<GroupFrame>> {
            let g = f.as_ref()?;
            if g.depth < level + 1 {
                return Some(g.clone());
            }
            let mut n = (**g).clone();
            n.snap = n.snap.insert(key.clone(), e.clone());
            n.up = fix(&g.up, key, e, level);
            Some(Arc::new(n))
        }
        self.group = fix(&self.group, key, &e, level);
    }
    fn apply_change(&mut self, c: &Change, mk: &mut dyn FnMut(Val) -> Arc<crate::env::DefCell>) {
        match c {
            Change::Def { key, val, global } => {
                let cell = mk(val.clone());
                self.define(key, cell, *global);
            }
            Change::Replace { key, val } => {
                let cell = mk(val.clone());
                self.replace(key, cell);
            }
            Change::OpenGroup(kind) => {
                let depth = self.depth() + 1;
                self.group = Some(Arc::new(GroupFrame {
                    kind: *kind,
                    depth,
                    snap: self.defs.clone(),
                    globals: None,
                    locals: None,
                    after: None,
                    up: self.group.clone(),
                }));
            }
            Change::CloseGroup => {
                let Some(f) = self.group.clone() else { return };
                let globals = cons_vec(f.globals.clone());
                let mut defs = f.snap.clone();
                for (k, e) in &globals {
                    defs = defs.insert(k.clone(), e.clone());
                }
                self.defs = defs;
                self.group = f.up.clone();
                // the globals stay global through the enclosing groups' ends
                self.with_frame(|p| {
                    for g in globals {
                        p.globals = cons_push(&p.globals, g);
                    }
                });
            }
            Change::AfterGroup(t) => {
                // tex.web: `\aftergroup` at level one is ignored.
                let t = *t;
                self.with_frame(|f| f.after = cons_push(&f.after, t));
            }
            Change::OpenList(m) => {
                let depth = self.list.depth + 1;
                self.list = Arc::new(ListFrame::new(*m, depth, Some(self.list.clone()), mk));
            }
            Change::CloseList => {
                if let Some(up) = self.list.up.clone() {
                    self.list = up;
                }
            }
        }
    }
}

/// What closing a group opened before a node read: the group's kind, its
/// `\aftergroup` tokens, and the values its end restores (the keys defined
/// locally in it, as the snapshot binds them).
#[derive(Debug, PartialEq)]
pub struct FrameDatum {
    pub kind: GroupKind,
    pub after: Vec<Tok>,
    pub restored: Vec<(Key, Val)>,
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
            defs = defs.insert(key, Entry { cell: crate::env::DefCell::new(Val::Meaning(Meaning::Prim(i as PrimId))), level: 1 });
        }
    }
    Env { inner: Arc::new(EnvRep { defs, ..EnvRep::default() }) }
}

fn frame_datum(f: &GroupFrame) -> FrameDatum {
    let restored = f
        .local_keys()
        .into_iter()
        .map(|k| {
            let v = f.snap.get(&k).map_or_else(|| initial(&k), Entry::val);
            (k, v)
        })
        .collect();
    FrameDatum { kind: f.kind, after: f.after_toks(), restored }
}

pub fn env_lookup(e: &Env, k: &Key) -> Val {
    let r = &*e.inner;
    if let Some(w) = list_which(k) {
        return r.list.vals[w as usize].val();
    }
    if let Key::Other(pk::TAG, n) = k {
        let c = n & 0xffff;
        return match pk::class(*n) {
            0 if *n == pk::GROUP_DEPTH => Val::Int(r.depth() as i32),
            0 if *n == pk::LIST_DEPTH => Val::Int(r.list.depth as i32),
            1 => Val::Int(r.group_up(c).map_or(0, |f| f.kind as i32)),
            2 => r.group_up(c).map_or(Val::Void, |f| Val::Data(Arc::new(frame_datum(&f)))),
            3 => Val::Int(r.list.up_n(c).map_or(0, |f| mode_code(f.mode))),
            4 => r.list.up_n(c / 4).map_or(Val::Void, |f| f.vals[(c % 4) as usize].val()),
            _ => Val::Void,
        };
    }
    match r.defs.get(k) {
        Some(e) => e.val(),
        None => initial(k),
    }
}

/// The definition reaching `k` in `e` (0: the initial value; pseudo keys have none).
pub fn env_def(e: &Env, k: &Key) -> crate::env::DefId {
    let r = &*e.inner;
    if let Some(w) = list_which(k) {
        return r.list.vals[w as usize].id_now();
    }
    if matches!(k, Key::Other(pk::TAG, _)) {
        return 0;
    }
    r.defs.get(k).map_or(0, |e| e.cell.id_now())
}

pub fn env_apply(e: &Env, d: &Delta) -> Env {
    if d.0.is_empty() {
        return e.clone();
    }
    let mut r = (*e.inner).clone();
    let mut mk = |v: Val| crate::env::DefCell::new(v);
    for c in &d.0 {
        r.apply_change(c, &mut mk);
    }
    Env { inner: Arc::new(r) }
}

/// `env_apply` binding the delta's definitions to `cells` (in order); fresh
/// cells past their end.
pub fn env_apply_cells(e: &Env, d: &Delta, cells: &[Arc<crate::env::DefCell>]) -> Env {
    let mut r = (*e.inner).clone();
    let mut i = 0usize;
    let mut mk = |v: Val| {
        let c = match cells.get(i) {
            Some(c) => c.clone(),
            None => crate::env::DefCell::new(v),
        };
        i += 1;
        c
    };
    for c in &d.0 {
        r.apply_change(c, &mut mk);
    }
    Env { inner: Arc::new(r) }
}

/// `env_apply` returning the cells it made, in order.
pub fn env_apply_collect(e: &Env, d: &Delta) -> (Env, Vec<Arc<crate::env::DefCell>>) {
    let mut r = (*e.inner).clone();
    let mut made = Vec::new();
    let mut mk = |v: Val| {
        let c = crate::env::DefCell::new(v);
        made.push(c.clone());
        c
    };
    for c in &d.0 {
        r.apply_change(c, &mut mk);
    }
    (Env { inner: Arc::new(r) }, made)
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
