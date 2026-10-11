//! D1: the page builder as a fold over the main vertical list's items,
//! memoised on (state, item): insertions, marks, `\pagegoal`…, costs, fire.
//!
//! tex.web §980–1028 (with e-TeX's mark classes and `\pagediscards`). The
//! state is an immutable value (`Fold`); `step` is a pure function of the
//! state, the item contributed and the parameters it reads (through `rd`,
//! which the caller records as reads). A fire returns `Fire`: the caller
//! defines `\box255`, the insertion boxes, `\outputpenalty` and the marks,
//! then runs `\output` (or ships `box255` when there is none), then feeds
//! `Fire::back` to the fold again (after the output routine's material).
//! While `Fold::output_active` the caller does not step the fold.
use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use crate::arith::{badness, x_over_n};
use crate::ctx::Ctx;
use crate::diag::Diag;
use crate::item::{BoxNode, Ins, Item};
use crate::pack::{vpack, Params, Spec};
use crate::prim::{dp, gp, ip, tag, tp, Kind, Level, Registry};
use crate::tok::Toks;
use crate::val::{Datum, Glue, Key, Val};

const AWFUL_BAD: i32 = 0o7777777777;
const INF_BAD: i32 = 10000;
const INF_PENALTY: i32 = 10000;
const EJECT_PENALTY: i32 = -10000;
const DEPLORABLE: i32 = 100000;
const MAX_DIMEN: i32 = 0o7777777777;

/// `Key::Other(tag::PAGE, DISCARDS)`: `\pagediscards` (`Val::Data(Items)` or `Val::Void`).
pub const DISCARDS: u32 = 0;

/// `Key::Other(tag::PAGE, n)` read-backs of the fold, answered by it:
/// `\lastpenalty` `\lastkern` `\lastskip` `\lastnodetype` on the current page
/// (main vertical list empty), and whether the page is empty.
pub const LAST_PENALTY: u32 = 1;
pub const LAST_KERN: u32 = 2;
pub const LAST_GLUE: u32 = 3;
pub const LAST_NODE_TYPE: u32 = 4;
pub const EMPTY: u32 = 5;

pub fn last_penalty(c: &mut dyn Ctx) -> i32 {
    c.int(&Key::Other(tag::PAGE, LAST_PENALTY))
}
pub fn last_kern(c: &mut dyn Ctx) -> i32 {
    c.int(&Key::Other(tag::PAGE, LAST_KERN))
}
pub fn last_glue(c: &mut dyn Ctx) -> Option<Glue> {
    match c.get(&Key::Other(tag::PAGE, LAST_GLUE)) {
        Val::Glue(g) => Some(g),
        _ => None,
    }
}
pub fn last_node_type(c: &mut dyn Ctx) -> i32 {
    c.int(&Key::Other(tag::PAGE, LAST_NODE_TYPE))
}
/// `page_head=page_tail` (nothing on the page, nothing held).
pub fn page_empty(c: &mut dyn Ctx) -> bool {
    c.int(&Key::Other(tag::PAGE, EMPTY)) != 0
}

/// The page quantities, in `Key::Page` order.
pub const QUANTITIES: &[&str] = &[
    "pagegoal", "pagetotal", "pagestretch", "pagefilstretch", "pagefillstretch",
    "pagefilllstretch", "pageshrink", "pagedepth", "deadcycles", "insertpenalties",
];

pub fn register(r: &mut Registry) {
    for (i, n) in QUANTITIES.iter().enumerate() {
        let level = if i < 8 { Level::Dimen } else { Level::Int };
        r.add(n, i as u16, Kind::Computed { level, get: get_quantity, set: Some(set_quantity) }, 0);
    }
    r.add("pagediscards", 0, Kind::Command(page_discards), 1);
}

fn get_quantity(c: &mut dyn Ctx, code: u16) -> Val {
    Val::Int(c.int(&Key::Page(code as u8)))
}

/// `alter_page_so_far` / `alter_integer`: always global (no save stack in TeX either).
fn set_quantity(c: &mut dyn Ctx, code: u16, v: Val, _global: bool) {
    let n = match v {
        Val::Glue(g) => g.width,
        v => v.int(),
    };
    c.define(Key::Page(code as u8), Val::Int(n), true);
}

/// `\pagediscards`: the items discarded at the page top, then forgotten.
fn page_discards(c: &mut dyn Ctx, _code: u16) {
    let key = Key::Other(tag::PAGE, DISCARDS);
    if let Val::Data(d) = c.get(&key) {
        if let Some(l) = d.as_any().downcast_ref::<Items>() {
            for it in l.0.iter() {
                c.item(it.clone());
            }
        }
    }
    c.define(key, Val::Void, true);
}

/// A list of items as a `Val::Data`.
#[derive(Debug, PartialEq)]
pub struct Items(pub Arc<[Item]>);

impl Datum for Items {
    fn same(&self, o: &dyn Datum) -> bool {
        o.as_any().downcast_ref::<Items>().is_some_and(|x| x == self)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `page_contents`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, PartialOrd, Ord)]
pub enum Contents {
    #[default]
    Empty,
    InsertsOnly,
    BoxThere,
}

/// A page insertion record (`page_ins_head` list). Positions are indices into
/// the page's contents; `broken_ptr` an index into the broken insertion's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InsRec {
    pub class: u8,
    pub split_up: bool,
    pub height: i32,
    pub broken_ptr: Option<usize>,
    pub broken_ins: Option<usize>,
    pub last_ins: Option<usize>,
    pub best_ins: Option<usize>,
}

/// Something the caller must do besides a fire.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Diag(Arc<Diag>),
    /// `box_error`: `\box n` becomes void.
    VoidBox(u16),
}

/// The page builder's state. Immutable; `step` makes a new one.
#[derive(Clone, Debug, PartialEq)]
pub struct Fold {
    pub contents: Arc<Vec<Item>>,
    /// `precedes_break(page_tail)` (false at `page_head`).
    pub tail_breaks: bool,
    pub state: Contents,
    /// `page_so_far`: goal, total, stretch (normal fil fill filll), shrink, depth.
    pub so_far: [i32; 8],
    pub max_depth: i32,
    pub least_cost: i32,
    /// Index of the best break: `< contents.len()` an item on the page.
    pub best_break: Option<usize>,
    pub best_size: i32,
    pub ins: Arc<Vec<InsRec>>,
    pub insert_penalties: i32,
    pub dead_cycles: i32,
    pub last_glue: Option<Glue>,
    pub last_penalty: i32,
    pub last_kern: i32,
    pub last_node_type: i32,
    /// Per class: top, first, bot.
    pub marks: Arc<BTreeMap<u16, [Option<Toks>; 3]>>,
    pub discards: Arc<Vec<Item>>,
    /// A kern waiting for the next item (it is a breakpoint only before glue).
    pub held: Option<Item>,
    pub output_active: bool,
    /// What the last step asks the caller to do (diagnostics, voided boxes).
    pub effects: Arc<Vec<Effect>>,
}

impl Default for Fold {
    fn default() -> Self {
        Fold {
            contents: Arc::default(),
            tail_breaks: false,
            state: Contents::Empty,
            so_far: [0; 8],
            max_depth: 0,
            least_cost: 0,
            best_break: None,
            best_size: 0,
            ins: Arc::default(),
            insert_penalties: 0,
            dead_cycles: 0,
            last_glue: None,
            last_penalty: 0,
            last_kern: 0,
            last_node_type: -1,
            marks: Arc::default(),
            discards: Arc::default(),
            held: None,
            output_active: false,
            effects: Arc::default(),
        }
    }
}

/// A page break: what the output routine (or the default shipout) gets.
#[derive(Clone, Debug, PartialEq)]
pub struct Fire {
    pub box255: Option<Arc<BoxNode>>,
    /// Every class's (top, first, bot) after the fire (define `Key::Mark(class, 0..=2)`).
    pub marks: Vec<(u16, [Option<Toks>; 3])>,
    /// Insertion boxes to define (`\box n`), void when `None`.
    pub inserts: Vec<(u16, Option<Arc<BoxNode>>)>,
    /// Define `\outputpenalty` globally.
    pub output_penalty: i32,
    /// `Some(routine)`: run `\output` (box255 is defined); `None`: ship `box255`.
    pub output: Option<Toks>,
    /// Items to contribute again, in order, after the output routine's material.
    pub back: Vec<Item>,
    pub diags: Vec<Arc<Diag>>,
}

fn diag(msg: String, help: &[&str]) -> Arc<Diag> {
    Arc::new(Diag { msg, help: help.iter().map(|s| s.to_string()).collect(), line: 0 })
}

fn node_type(it: &Item) -> i32 {
    match it {
        Item::Box(b) => i32::from(b.vertical),
        Item::Rule { .. } => 2,
        Item::Ins(_) => 3,
        Item::Mark { .. } => 4,
        Item::Adjust(_) => 5,
        Item::Lig { .. } | Item::Char { .. } => 6,
        Item::Disc(_) => 7,
        Item::Whatsit(_) => 8,
        Item::Math { .. } => 9,
        Item::Glue { .. } => 10,
        Item::Kern { .. } | Item::MarginKern { .. } => 11,
        Item::Penalty(_) => 12,
        Item::Unset(_) => 13,
        Item::Noad(_) => 16,
    }
}

fn precedes_break(it: &Item) -> bool {
    node_type(it) < 9
}

fn scaled_by(h: i32, count: i32) -> i32 {
    if count == 1000 { h } else { x_over_n(h, 1000).0.wrapping_mul(count) }
}

fn get_box(rd: &mut dyn FnMut(&Key) -> Val, n: u16) -> Option<Arc<BoxNode>> {
    match rd(&Key::Box(n)) {
        Val::Box(b) => b,
        _ => None,
    }
}

fn natural(list: Vec<Item>, p: &Params) -> BoxNode {
    let mut b = vpack(list, Spec::Additional(0), MAX_DIMEN, p).b;
    b.vertical = true;
    b
}

enum Res {
    Done,
    Hold(Item),
    Fire(Item),
}

impl Fold {
    pub fn new() -> Fold {
        Fold::default()
    }

    /// `\pagegoal`…`\pagedepth` (0..=7), `\deadcycles` (8), `\insertpenalties` (9).
    pub fn quantity(&self, k: u8) -> i32 {
        match k {
            0..=7 => {
                if self.state == Contents::Empty && !self.output_active {
                    if k == 0 { MAX_DIMEN } else { 0 }
                } else {
                    self.so_far[k as usize]
                }
            }
            8 => self.dead_cycles,
            9 => self.insert_penalties,
            _ => 0,
        }
    }

    pub fn set_quantity(&self, k: u8, v: i32) -> Fold {
        let mut s = self.clone();
        match k {
            0..=7 => s.so_far[k as usize] = v,
            8 => s.dead_cycles = v,
            9 => s.insert_penalties = v,
            _ => {}
        }
        s
    }

    /// A definition the graph routes to the fold (`Key::Page`, `\pagediscards` cleared).
    pub fn apply_def(&self, key: &Key, val: &Val) -> Option<Fold> {
        match key {
            Key::Page(k) => Some(self.set_quantity(*k, val.int())),
            Key::Other(t, DISCARDS) if *t == tag::PAGE => {
                let mut s = self.clone();
                s.discards = Arc::default();
                Some(s)
            }
            _ => None,
        }
    }

    /// The fold's answer for a key it owns.
    pub fn answer(&self, key: &Key) -> Option<Val> {
        match key {
            Key::Page(k) => Some(Val::Int(self.quantity(*k))),
            Key::Mark(c, w) if *w < 3 => Some(Val::Toks(self.mark(*c, *w).unwrap_or_else(|| Arc::from(Vec::new())))),
            Key::Other(t, DISCARDS) if *t == tag::PAGE => Some(self.discards_val()),
            Key::Other(t, LAST_PENALTY) if *t == tag::PAGE => Some(Val::Int(self.last_penalty)),
            Key::Other(t, LAST_KERN) if *t == tag::PAGE => Some(Val::Int(self.last_kern)),
            Key::Other(t, LAST_GLUE) if *t == tag::PAGE => Some(self.last_glue.map_or(Val::Void, Val::Glue)),
            Key::Other(t, LAST_NODE_TYPE) if *t == tag::PAGE => Some(Val::Int(self.last_node_type)),
            Key::Other(t, EMPTY) if *t == tag::PAGE => Some(Val::Int(i32::from(self.is_empty()))),
            _ => None,
        }
    }

    pub fn mark(&self, class: u16, which: u8) -> Option<Toks> {
        self.marks.get(&class).and_then(|m| m.get(which as usize).cloned().flatten())
    }

    pub fn discards_val(&self) -> Val {
        if self.discards.is_empty() {
            Val::Void
        } else {
            Val::Data(Arc::new(Items(self.discards.iter().cloned().collect())))
        }
    }

    /// The output routine ended (`resume_page_builder`).
    pub fn output_done(&self) -> Fold {
        let mut s = self.clone();
        s.output_active = false;
        s
    }

    /// Nothing on the page and nothing held (used by `\end`'s `its_all_over`).
    pub fn is_empty(&self) -> bool {
        self.contents.is_empty() && self.held.is_none()
    }

    fn note(&mut self, e: Effect) {
        Arc::make_mut(&mut self.effects).push(e);
    }

    fn freeze(&mut self, s: Contents, rd: &mut dyn FnMut(&Key) -> Val) {
        self.state = s;
        self.so_far = [0; 8];
        self.so_far[0] = rd(&Key::DimenPar(dp::of("vsize"))).int();
        self.max_depth = rd(&Key::DimenPar(dp::of("maxdepth"))).int();
        self.least_cost = AWFUL_BAD;
    }

    fn update_last(&mut self, c: &Item) {
        self.last_glue = None;
        self.last_penalty = 0;
        self.last_kern = 0;
        self.last_node_type = node_type(c) + 1;
        match c {
            Item::Glue { g, .. } => self.last_glue = Some(*g),
            Item::Penalty(p) => self.last_penalty = *p,
            Item::Kern { w, .. } => self.last_kern = *w,
            _ => {}
        }
    }

    fn append(&mut self, c: Item) {
        if self.so_far[7] > self.max_depth {
            self.so_far[1] = self.so_far[1] + self.so_far[7] - self.max_depth;
            self.so_far[7] = self.max_depth;
        }
        self.tail_breaks = precedes_break(&c);
        Arc::make_mut(&mut self.contents).push(c);
    }

    fn discard(&mut self, c: Item, rd: &mut dyn FnMut(&Key) -> Val) {
        if rd(&Key::IntPar(ip::of("savingvdiscards"))).int() > 0 {
            Arc::make_mut(&mut self.discards).push(c);
        }
    }

    fn update_heights(&mut self, c: Item) {
        let mut c = c;
        match &mut c {
            Item::Glue { g, .. } => {
                self.so_far[2 + (g.stretch_order.min(3) as usize)] += g.stretch;
                self.so_far[6] += g.shrink;
                if g.shrink_order != 0 && g.shrink != 0 {
                    self.note(Effect::Diag(diag(
                        "Infinite glue shrinkage found on current page".into(),
                        &[
                            "The page about to be output contains some infinitely",
                            "shrinkable glue, e.g., `\\vss' or `\\vskip 0pt minus 1fil'.",
                            "Such glue doesn't belong there; but you can safely proceed,",
                            "since the offensive shrinkability has been made finite.",
                        ],
                    )));
                    g.shrink_order = 0;
                }
                self.so_far[1] += self.so_far[7] + g.width;
            }
            Item::Kern { w, .. } | Item::MarginKern { w, .. } => self.so_far[1] += self.so_far[7] + *w,
            _ => {}
        }
        self.so_far[7] = 0;
        self.append(c);
    }

    /// §1008–1011.
    fn append_insertion(&mut self, p: &Ins, rd: &mut dyn FnMut(&Key) -> Val) {
        if self.state == Contents::Empty {
            self.freeze(Contents::InsertsOnly, rd);
        }
        let n = p.class;
        let n16 = u16::from(n);
        let pos = self.ins.iter().position(|r| r.class >= n);
        let k = match pos {
            Some(k) if self.ins[k].class == n => k,
            _ => {
                let at = pos.unwrap_or(self.ins.len());
                // ensure_vbox(n)
                let mut bx = get_box(rd, n16);
                if bx.as_ref().is_some_and(|b| !b.vertical) {
                    self.note(Effect::Diag(diag(
                        "Insertions can only be added to a vbox".into(),
                        &[
                            "Tut tut: You're trying to \\insert into a",
                            "\\box register that now contains an \\hbox.",
                            "Proceed, and I'll discard its present contents.",
                        ],
                    )));
                    self.note(Effect::VoidBox(n16));
                    bx = None;
                }
                let height = bx.map_or(0, |b| b.height + b.depth);
                let q = rd(&Key::Skip(n16)).glue();
                let count = rd(&Key::Count(n16)).int();
                let h = scaled_by(height, count);
                self.so_far[0] = self.so_far[0] - h - q.width;
                self.so_far[2 + (q.stretch_order.min(3) as usize)] += q.stretch;
                self.so_far[6] += q.shrink;
                if q.shrink_order != 0 && q.shrink != 0 {
                    self.note(Effect::Diag(diag(
                        format!("Infinite glue shrinkage inserted from \\skip{n}"),
                        &[
                            "The correction glue for page breaking with insertions",
                            "must have finite shrinkability. But you may proceed,",
                            "since the offensive shrinkability has been made finite.",
                        ],
                    )));
                }
                Arc::make_mut(&mut self.ins).insert(
                    at,
                    InsRec { class: n, split_up: false, height, broken_ptr: None, broken_ins: None, last_ins: None, best_ins: None },
                );
                at
            }
        };
        if self.ins[k].split_up {
            self.insert_penalties += p.float_cost;
            return;
        }
        let here = self.contents.len();
        let count = rd(&Key::Count(n16)).int();
        let dimen = rd(&Key::Dimen(n16)).int();
        let delta = self.so_far[0] - self.so_far[1] - self.so_far[7] + self.so_far[6];
        let h = scaled_by(p.height, count);
        let rh = self.ins[k].height;
        let goal = self.so_far[0];
        let total = self.so_far[1] + self.so_far[7];
        let r = &mut Arc::make_mut(&mut self.ins)[k];
        r.last_ins = Some(here);
        if (h <= 0 || h <= delta) && p.height.saturating_add(rh) <= dimen {
            self.so_far[0] -= h;
            r.height += p.height;
        } else {
            // §1011: find the best way to split the insertion
            let mut w = if count <= 0 { MAX_DIMEN } else { goal - total };
            if count > 0 && count != 1000 {
                w = x_over_n(w, count).0.wrapping_mul(1000);
            }
            if w > dimen - rh {
                w = dimen - rh;
            }
            let mut l: Vec<Item> = p.list.to_vec();
            let vb = crate::boxes::vert_break(&mut l, w, p.split_max_depth);
            let q = (vb.at < l.len()).then_some(vb.at);
            let mut best_hd = vb.best_height_plus_depth;
            r.height += best_hd;
            if count != 1000 {
                best_hd = x_over_n(best_hd, 1000).0.wrapping_mul(count);
            }
            r.split_up = true;
            r.broken_ptr = q;
            r.broken_ins = Some(here);
            self.so_far[0] -= best_hd;
            match q {
                None => self.insert_penalties += EJECT_PENALTY,
                Some(i) => {
                    if let Some(Item::Penalty(v)) = p.list.get(i) {
                        self.insert_penalties += *v;
                    }
                }
            }
        }
    }

    /// One iteration of `build_page`'s loop for item `c` (§997–1005).
    fn contribute(&mut self, c: Item, next: Option<&Item>, rd: &mut dyn FnMut(&Key) -> Val) -> Res {
        self.update_last(&c);
        let pi = match &c {
            Item::Box(b) => {
                self.so_far[1] += self.so_far[7] + b.height;
                self.so_far[7] = b.depth;
                self.append(c);
                return Res::Done;
            }
            Item::Rule { h, d, .. } => {
                self.so_far[1] += self.so_far[7] + *h;
                self.so_far[7] = *d;
                self.append(c);
                return Res::Done;
            }
            Item::Ins(p) => {
                let p = p.clone();
                self.append_insertion(&p, rd);
                self.append(c);
                return Res::Done;
            }
            Item::Glue { .. } | Item::Kern { .. } | Item::MarginKern { .. } | Item::Penalty(_) => {
                if self.state < Contents::BoxThere {
                    self.discard(c, rd);
                    return Res::Done;
                }
                match &c {
                    Item::Glue { .. } => {
                        if self.tail_breaks {
                            0
                        } else {
                            self.update_heights(c);
                            return Res::Done;
                        }
                    }
                    Item::Penalty(v) => *v,
                    _ => match next {
                        None => return Res::Hold(c),
                        Some(Item::Glue { .. }) => 0,
                        Some(_) => {
                            self.update_heights(c);
                            return Res::Done;
                        }
                    },
                }
            }
            _ => {
                self.append(c);
                return Res::Done;
            }
        };
        if pi < INF_PENALTY {
            // §1007: the cost of breaking here
            let goal = self.so_far[0];
            let total = self.so_far[1];
            let b = if total < goal {
                if self.so_far[3] != 0 || self.so_far[4] != 0 || self.so_far[5] != 0 {
                    0
                } else {
                    badness(goal - total, self.so_far[2])
                }
            } else if total - goal > self.so_far[6] {
                AWFUL_BAD
            } else {
                badness(total - goal, self.so_far[6])
            };
            let mut cost = if b < AWFUL_BAD {
                if pi <= EJECT_PENALTY {
                    pi
                } else if b < INF_BAD {
                    b + pi + self.insert_penalties
                } else {
                    DEPLORABLE
                }
            } else {
                b
            };
            if self.insert_penalties >= 10000 {
                cost = AWFUL_BAD;
            }
            if cost <= self.least_cost {
                self.best_break = Some(self.contents.len());
                self.best_size = goal;
                self.least_cost = cost;
                for r in Arc::make_mut(&mut self.ins).iter_mut() {
                    r.best_ins = r.last_ins;
                }
            }
            if cost == AWFUL_BAD || pi <= EJECT_PENALTY {
                return Res::Fire(c);
            }
        }
        match c {
            Item::Penalty(_) => self.append(c),
            c => self.update_heights(c),
        }
        Res::Done
    }

    /// §1012–1023: break the page at the best break; `c` is the item that
    /// triggered it (still a contribution), `rest` the contributions after it.
    fn fire_up(&mut self, c: Item, rest: Vec<Item>, rd: &mut dyn FnMut(&Key) -> Val) -> Fire {
        let mut diags: Vec<Arc<Diag>> = Vec::new();
        let mut page: Vec<Item> = (*self.contents).clone();
        let len = page.len();
        let mut c = c;
        let break_at = self.best_break.unwrap_or(len).min(len);
        // §1013: \outputpenalty
        let mut output_penalty = INF_PENALTY;
        if break_at == len {
            if self.best_break.is_some() {
                if let Item::Penalty(v) = c {
                    output_penalty = v;
                    c = Item::Penalty(INF_PENALTY);
                }
            }
        } else if let Item::Penalty(v) = page[break_at] {
            output_penalty = v;
            page[break_at] = Item::Penalty(INF_PENALTY);
        }
        let mut marks = (*self.marks).clone();
        for m in marks.values_mut() {
            if m[2].is_some() {
                m[0] = m[2].clone();
                m[1] = None;
            }
        }
        // §1014
        if get_box(rd, 255).is_some() {
            diags.push(diag(
                "\\box255 is not void".into(),
                &["You shouldn't use \\box255 except in \\output routines.", "Proceed, and I'll discard its present contents."],
            ));
        }
        self.insert_penalties = 0;
        let holding = rd(&Key::IntPar(ip::of("holdinginserts"))).int() > 0;
        let pp = Params {
            hbadness: rd(&Key::IntPar(ip::of("hbadness"))).int(),
            vbadness: rd(&Key::IntPar(ip::of("vbadness"))).int(),
            hfuzz: rd(&Key::DimenPar(dp::of("hfuzz"))).int(),
            vfuzz: rd(&Key::DimenPar(dp::of("vfuzz"))).int(),
            overfull_rule: rd(&Key::DimenPar(dp::of("overfullrule"))).int(),
        };
        let mut ins: Vec<InsRec> = (*self.ins).clone();
        // §1018: prepare the insertion boxes as queues
        let mut queues: BTreeMap<u8, Vec<Item>> = BTreeMap::new();
        let mut inserts: Vec<(u16, Option<Arc<BoxNode>>)> = Vec::new();
        if !holding {
            for r in &ins {
                if r.best_ins.is_some() {
                    let n = u16::from(r.class);
                    let mut list = Vec::new();
                    match get_box(rd, n) {
                        Some(b) if !b.vertical => {
                            diags.push(diag(
                                "Insertions can only be added to a vbox".into(),
                                &[
                                    "Tut tut: You're trying to \\insert into a",
                                    "\\box register that now contains an \\hbox.",
                                    "Proceed, and I'll discard its present contents.",
                                ],
                            ));
                        }
                        Some(b) => list.extend(b.list.iter().cloned()),
                        None => {}
                    }
                    queues.insert(r.class, list);
                }
            }
        }
        // §1018–1022: walk the page up to the break
        let mut held: Vec<Item> = Vec::new();
        let mut body: Vec<Item> = Vec::new();
        for (k, it) in page[..break_at].iter().enumerate() {
            match it {
                Item::Ins(p) if !holding => {
                    let Some(r) = ins.iter_mut().find(|r| r.class == p.class) else {
                        body.push(it.clone());
                        continue;
                    };
                    let mut wait;
                    let mut keep = it.clone();
                    if r.best_ins.is_none() {
                        wait = true;
                    } else {
                        wait = false;
                        let buf = queues.entry(r.class).or_default();
                        buf.extend(p.list.iter().cloned());
                        if r.best_ins == Some(k) {
                            // §1021: wrap up the box, splitting p if called for
                            if r.split_up && r.broken_ins == Some(k) {
                                if let Some(bp) = r.broken_ptr {
                                    let cut = p.list.len().saturating_sub(bp);
                                    buf.truncate(buf.len() - cut);
                                    let tail: Vec<Item> = p.list.get(bp..).map(<[Item]>::to_vec).unwrap_or_default();
                                    let (pruned, _) = crate::boxes::prune_page_top(tail, p.split_top, false);
                                    if !pruned.is_empty() {
                                        let b = natural(pruned.clone(), &pp);
                                        let mut np = (**p).clone();
                                        np.height = b.height + b.depth;
                                        np.list = pruned.into();
                                        keep = Item::Ins(Arc::new(np));
                                        wait = true;
                                    }
                                }
                            }
                            r.best_ins = None;
                            let list = std::mem::take(buf);
                            let b = natural(list, &pp);
                            inserts.push((u16::from(r.class), Some(Arc::new(b))));
                        }
                    }
                    if wait {
                        held.push(keep);
                        self.insert_penalties += 1;
                    }
                }
                Item::Mark { class, toks } => {
                    let m = marks.entry(*class).or_default();
                    if m[1].is_none() {
                        m[1] = Some(toks.clone());
                    }
                    m[2] = Some(toks.clone());
                    body.push(it.clone());
                }
                _ => body.push(it.clone()),
            }
        }
        // classes queued but never wrapped (cannot happen in TeX; keep the box as it was)
        // §1017: box255, the rest back to the contributions
        let mut back: Vec<Item> = page[break_at..].to_vec();
        back.push(c);
        back.extend(rest);
        let p255 = Params { vbadness: INF_BAD, vfuzz: MAX_DIMEN, ..pp };
        let mut b255 = vpack(body, Spec::Exactly(self.best_size), self.max_depth, &p255).b;
        b255.vertical = true;
        let box255 = Some(Arc::new(b255));
        // start a new current page
        self.state = Contents::Empty;
        self.last_glue = None;
        self.last_penalty = 0;
        self.last_kern = 0;
        self.last_node_type = -1;
        self.so_far[7] = 0;
        self.max_depth = 0;
        self.best_break = None;
        self.held = None;
        self.ins = Arc::default();
        self.tail_breaks = !held.is_empty();
        self.contents = Arc::new(held);
        for m in marks.values_mut() {
            if m[0].is_some() && m[1].is_none() {
                m[1] = m[0].clone();
            }
        }
        self.marks = Arc::new(marks.clone());
        let marks: Vec<(u16, [Option<Toks>; 3])> = marks.into_iter().collect();
        // §1025: \output
        let routine = rd(&Key::TokPar(tp::of("output"))).toks();
        if !routine.is_empty() {
            let max_dead = rd(&Key::IntPar(ip::of("maxdeadcycles"))).int();
            if self.dead_cycles >= max_dead {
                diags.push(diag(
                    format!("Output loop---{} consecutive dead cycles", self.dead_cycles),
                    &[
                        "I've concluded that your \\output is awry; it never does a",
                        "\\shipout, so I'm shipping \\box255 out myself. Next time",
                        "increase \\maxdeadcycles if you want me to be more patient!",
                    ],
                ));
            } else {
                self.output_active = true;
                self.dead_cycles += 1;
                return Fire { box255, marks, inserts, output_penalty, output: Some(routine), back, diags };
            }
        }
        // §1023: perform the default output routine
        let mut all: Vec<Item> = (*self.contents).clone();
        all.extend(back);
        self.contents = Arc::default();
        self.tail_breaks = false;
        self.discards = Arc::default();
        self.dead_cycles = 0;
        Fire { box255, marks, inserts, output_penalty, output: None, back: all, diags }
    }
}

/// One contribution to the page: `(new state, fire)`. `rd` reads parameters
/// and registers (`\vsize`, `\maxdepth`, `\topskip`, `\count n`, `\dimen n`,
/// `\skip n`, `\box n`, `\holdinginserts`, `\output`, `\maxdeadcycles`, …);
/// the caller records them as the node's reads. Pure: memoisable on
/// (state, item, values read).
pub fn step(f: &Fold, item: &Item, rd: &mut dyn FnMut(&Key) -> Val) -> (Fold, Option<Fire>) {
    let mut s = f.clone();
    s.effects = Arc::default();
    let mut queue: VecDeque<Item> = VecDeque::new();
    if let Some(k) = s.held.take() {
        queue.push_back(k);
    }
    queue.push_back(item.clone());
    while let Some(c) = queue.pop_front() {
        // §1001: the first box or rule on the page gets \topskip before it
        let h = match &c {
            Item::Box(b) => Some(b.height),
            Item::Rule { h, .. } => Some(*h),
            _ => None,
        };
        if let Some(h) = h {
            if s.state < Contents::BoxThere {
                if s.state == Contents::Empty {
                    s.freeze(Contents::BoxThere, rd);
                } else {
                    s.state = Contents::BoxThere;
                }
                let topskip = gp::of("topskip");
                let mut g = rd(&Key::GluePar(topskip)).glue();
                if g.width > h { g.width -= h } else { g.width = 0 }
                s.update_last(&c);
                queue.push_front(c);
                queue.push_front(Item::Glue { g, sub: (topskip + 1) as u8, leader: None });
                continue;
            }
        }
        let next = queue.front().cloned();
        match s.contribute(c, next.as_ref(), rd) {
            Res::Done => {}
            Res::Hold(k) => s.held = Some(k),
            Res::Fire(c) => {
                let rest: Vec<Item> = queue.drain(..).collect();
                let mut fire = s.fire_up(c, rest, rd);
                let mut d: Vec<Arc<Diag>> = s
                    .effects
                    .iter()
                    .filter_map(|e| if let Effect::Diag(d) = e { Some(d.clone()) } else { None })
                    .collect();
                d.append(&mut fire.diags);
                fire.diags = d;
                return (s, Some(fire));
            }
        }
    }
    (s, None)
}
