//! The pages the last build shipped as SyncTeX sees them (CORE.md, "Source
//! places and item provenance"): each page's boxes, characters, kerns,
//! glue, math nodes and rules where shipping puts them (tex.web's
//! `hlist_out` and `vlist_out`, §619–§637, the records of pdfTeX's
//! `synctex.c`), each with the source place of the node that made it. A
//! function of the pages and of the provenance the build keeps (the tables
//! [`Build::material_node`] reads), computed when asked: never a read of
//! the graph, no node, nothing kept.
//!
//! A box on a page is traced to its maker by what it is: a line of a
//! paragraph of a piece's material, a box among its vertical items, or a
//! box a node put in a register (`Run::box_defs`, with the boxes in it)
//! that an output routine moved; the items in a box by its origins. The
//! boxes an output routine made itself (the page's own, a headline) and
//! what is in them have the place of the page builder's call that fired it
//! (pdfTeX's: the line being read then). Not yet: what is in an insert
//! (all at the place of the node that made the insert), alignments' and
//! math lists' items (the place of the node that made the whole),
//! TeXXeT's reversed segments (in their order in the list).

use std::sync::Arc;

use super::Build;
use crate::graph::Run;
use crate::hash::FxMap;
use crate::item::{A_LEADERS, BoxNode, C_LEADERS, Item, Leaders, Para, RUNNING};
use crate::linebreak::Line;
use crate::show::zround;
use crate::val::Glue;

/// What a record is (`synctex.c`'s records, by their letters).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// `(`: an hbox's list begins, at its reference point (`w`, `ht`, `dp`
    /// its dimensions).
    HBox,
    /// `[`: a vbox's list begins, at its reference point.
    VBox,
    /// `)`, `]`: the list ends.
    EndH,
    EndV,
    /// `h`, `v`: an empty box, at its reference point.
    VoidH,
    VoidV,
    /// A character (a ligature: its character) at its reference point,
    /// `w` its width: written as an `x` record of its own place.
    Glyph,
    /// `x`: a run of characters ends here (`synctexcurrent`), with the
    /// place of its last character.
    RunEnd,
    /// `k`: just past a kern of width `w`.
    Kern,
    /// `g`: just past glue (glue with rule leaders too).
    Glue,
    /// `$`: a math node (at its start).
    Math,
    /// `r`: just past a rule `w` wide, `ht` its height plus depth, `dp`
    /// its depth.
    Rule,
}

/// A source place as a page's records name it: a piece the page depends
/// on (an index into [`PageSync::deps`]) and the line in that piece (from
/// 1: the line of the piece's text the node began reading in). The
/// place's line in its file is the lines of the file before the piece
/// ([`SyncPiece::base`]) plus that.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Place {
    pub dep: u32,
    pub line: u32,
}

/// No place known.
pub const NOWHERE: Place = Place { dep: u32::MAX, line: 0 };

/// A record: where on the page (scaled points: TeX's `cur_h`, `cur_v` as
/// shipping has them, from the page's corner in PDF mode) and which place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rec {
    pub kind: Kind,
    pub place: Place,
    pub h: i32,
    pub v: i32,
    pub w: i32,
    pub ht: i32,
    pub dp: i32,
}

/// A piece a page's records name: its id, and its run. While a piece
/// has the same run (the same `Arc`), the lines its nodes began reading in
/// are the same.
#[derive(Clone, Debug)]
pub struct Dep {
    pub id: u32,
    pub run: Arc<Run>,
}

/// A shipped page's records.
#[derive(Clone, Debug, Default)]
pub struct PageSync {
    /// pdfTeX's PDF mode (`\pdfoutput` positive where it was shipped):
    /// positions from the page's top left corner, `\pdfhorigin` and
    /// `\pdfvorigin` included; else DVI's (TeX's, without the inch).
    pub pdf: bool,
    /// `\mag` where it was shipped.
    pub mag: i32,
    pub deps: Vec<Dep>,
    pub recs: Vec<Rec>,
}

/// A piece of the last build as places name it: its id, its run, its
/// file (`None`: the main file) and the lines of that file before it.
#[derive(Clone, Debug)]
pub struct SyncPiece {
    pub id: u32,
    pub run: Option<Arc<Run>>,
    pub file: Option<Arc<str>>,
    pub base: u32,
}

/// How a box found on a page by its address is known.
#[derive(Clone, Copy, Debug)]
enum Anchor {
    /// Line `li` of paragraph `j` of piece `k`.
    Line { k: u32, j: u32, li: u32 },
    /// A box node `maker` of piece `k` made (its origins back from it).
    Made { k: u32, maker: u32 },
    /// A box in an insert node `node` emitted: all of it at that place.
    Fixed { k: u32, node: u32 },
}

/// The boxes of the last build's material and registers, by address: how
/// a page's boxes are traced to their makers ([`Build::sync_index`]); and
/// the registers' boxes by their content too, for a box an output routine
/// moved that is an equal box, not the same (`Arc`).
pub struct Index(FxMap<usize, Anchor>, FxMap<u64, Vec<(Arc<BoxNode>, Anchor)>>);

fn addr(b: &Arc<BoxNode>) -> usize {
    Arc::as_ptr(b) as usize
}

/// What [`Index`]'s second table finds a box by: its kind, dimensions,
/// glue setting and length.
fn shape(b: &BoxNode) -> u64 {
    let mut x: u64 = u64::from(b.vertical);
    for v in [b.w, b.h, b.d, b.list.len() as i32, i32::from(b.glue.sign), i32::from(b.glue.order)] {
        x = (x ^ u64::from(v as u32)).wrapping_mul(0x0100_0000_01b3).rotate_left(23);
    }
    x
}

/// Equal boxes but for where they are shifted.
fn same_box(a: &BoxNode, b: &BoxNode) -> bool {
    a.vertical == b.vertical && a.w == b.w && a.h == b.h && a.d == b.d && a.glue == b.glue && a.list == b.list
}

/// An item of piece `k`'s vertical material, carried by node `pn`.
fn anchor_item(m: &mut FxMap<usize, Anchor>, it: &Item, k: u32, pn: u32) {
    match it {
        Item::Box(b) | Item::Append(b, _) => {
            m.entry(addr(b)).or_insert(Anchor::Made { k, maker: pn });
        }
        Item::Shipout(sh) => {
            m.entry(addr(&sh.b)).or_insert(Anchor::Made { k, maker: pn });
        }
        Item::Ins(ins) => {
            for x in &ins.list {
                if let Item::Box(b) | Item::Append(b, _) = x {
                    m.entry(addr(b)).or_insert(Anchor::Fixed { k, node: pn });
                }
            }
        }
        _ => {}
    }
}

/// Box `b`, made by node `maker` of piece `k`, and the boxes in it (their
/// makers by its origins), each known once, by address and by content.
fn descend(idx: &mut Index, b: &Arc<BoxNode>, k: u32, maker: u32) {
    if idx.0.contains_key(&addr(b)) {
        return;
    }
    let a = Anchor::Made { k, maker };
    idx.0.insert(addr(b), a);
    if !b.list.is_empty() {
        idx.1.entry(shape(b)).or_default().push((b.clone(), a));
    }
    for (i, it) in b.list.iter().enumerate() {
        if let Item::Box(c) | Item::Append(c, _) = it {
            descend(idx, c, k, b.origins.node_of(maker, i).unwrap_or(maker));
        }
    }
}

/// §625, §634 (as `dvi.rs`'s): the glue of a box set so far.
#[derive(Default)]
struct SetGlue {
    cur_g: i32,
    cur_glue: f64,
}

impl SetGlue {
    fn set(&mut self, b: &BoxNode, g: &Glue) -> i32 {
        let vet = |x: f64| x.clamp(-1_000_000_000.0, 1_000_000_000.0);
        let mut wd = g.width - self.cur_g;
        if b.glue.sign == 1 {
            if g.stretch_order == b.glue.order {
                self.cur_glue += f64::from(g.stretch);
                self.cur_g = zround(vet(b.glue.ratio * self.cur_glue));
            }
        } else if b.glue.sign == -1 && g.shrink_order == b.glue.order {
            self.cur_glue -= f64::from(g.shrink);
            self.cur_g = zround(vet(b.glue.ratio * self.cur_glue));
        }
        wd += self.cur_g;
        wd
    }
}

/// How a box's items are traced to their nodes.
enum Ctx<'a> {
    /// A line of a paragraph: by the line's and the paragraph's tables.
    Line { k: u32, para: &'a Para, pn: u32, line: &'a Line },
    /// A box node `maker` made: by its origins.
    Made { k: u32, maker: u32 },
    /// Not known: everything at one place.
    Fixed(Place),
}

/// The walk over a page (tex.web's ship, recording).
struct Walk<'a> {
    b: &'a Build,
    idx: &'a Index,
    deps: Vec<Dep>,
    dep_of: FxMap<u32, u32>,
    /// Each piece's line ends, once asked.
    ends: FxMap<u32, Vec<u32>>,
    recs: Vec<Rec>,
    h: i32,
    v: i32,
}

impl<'a> Walk<'a> {
    /// Node `node` of piece `k`'s place (as [`Build::source_place`]).
    fn place(&mut self, k: u32, node: u32) -> Place {
        let b = self.b;
        let Some(kept) = b.pieces.get(k as usize) else { return NOWHERE };
        let Some(run) = kept.run.as_ref() else { return NOWHERE };
        let Some(pos) = run.node_pos(node as usize) else { return NOWHERE };
        let dep = if let Some(&d) = self.dep_of.get(&k) {
            d
        } else {
            let d = self.deps.len() as u32;
            self.deps.push(Dep { id: kept.id, run: run.clone() });
            self.dep_of.insert(k, d);
            d
        };
        let ends = self.ends.entry(k).or_insert_with(|| {
            kept.text.iter().enumerate().filter(|&(_, &c)| c == '\r').map(|(i, _)| i as u32).collect()
        });
        let p = pos.min(kept.text.len()) as u32;
        Place { dep, line: 1 + ends.partition_point(|&x| x < p) as u32 }
    }

    fn rec(&mut self, kind: Kind, place: Place, h: i32, v: i32, dims: [i32; 3]) {
        let [w, ht, dp] = dims;
        self.recs.push(Rec { kind, place, h, v, w, ht, dp });
    }

    /// The context of a box `a` names, and the box's own place.
    fn anchored(&mut self, a: Anchor) -> Option<(Ctx<'a>, Place)> {
        let b = self.b;
        match a {
            Anchor::Line { k, j, li } => {
                let kept = b.pieces.get(k as usize)?;
                let run = kept.run.as_ref()?;
                let para = run.paras.get(j as usize)?;
                let pn = *run.para_nodes().get(j as usize)?;
                let line = kept.lines.get(j as usize)?.as_ref()?.get(li as usize)?;
                let p = self.place(k, pn);
                Some((Ctx::Line { k, para: &**para, pn, line: &**line }, p))
            }
            Anchor::Made { k, maker } => {
                let p = self.place(k, maker);
                Some((Ctx::Made { k, maker }, p))
            }
            Anchor::Fixed { k, node } => {
                let p = self.place(k, node);
                Some((Ctx::Fixed(p), p))
            }
        }
    }

    /// The node that made item `i` of box `this` (traced by `ctx`).
    fn item_node(ctx: &Ctx<'_>, this: &BoxNode, i: usize) -> Option<(u32, u32)> {
        match ctx {
            Ctx::Line { k, para, pn, line } => Some((*k, line.node_of(para, *pn, i).unwrap_or(*pn))),
            Ctx::Made { k, maker } => Some((*k, this.origins.node_of(*maker, i).unwrap_or(*maker))),
            Ctx::Fixed(_) => None,
        }
    }

    fn item_place(&mut self, ctx: &Ctx<'_>, this: &BoxNode, i: usize) -> Place {
        match Self::item_node(ctx, this, i) {
            Some((k, n)) => self.place(k, n),
            None => match ctx {
                Ctx::Fixed(p) => *p,
                _ => NOWHERE,
            },
        }
    }

    /// The context of box `c`, item `i` of `this`, and its own place: as
    /// the material or a register knows it, else by `this`'s tables.
    fn child(&mut self, ctx: &Ctx<'a>, this: &BoxNode, i: usize, c: &Arc<BoxNode>) -> (Ctx<'a>, Place) {
        if let Some(&a) = self.idx.0.get(&addr(c))
            && let Some(r) = self.anchored(a)
        {
            return r;
        }
        // (a box not known by address where nothing else says: an equal
        // register's box)
        if matches!(ctx, Ctx::Fixed(_))
            && let Some(&(_, a)) = self.idx.1.get(&shape(c)).and_then(|v| v.iter().find(|(x, _)| same_box(x, c)))
            && let Some(r) = self.anchored(a)
        {
            return r;
        }
        match Self::item_node(ctx, this, i) {
            Some((k, n)) => {
                let p = self.place(k, n);
                (Ctx::Made { k, maker: n }, p)
            }
            None => {
                let p = match ctx {
                    Ctx::Fixed(p) => *p,
                    _ => NOWHERE,
                };
                (Ctx::Fixed(p), p)
            }
        }
    }

    fn list_out(&mut self, b: &BoxNode, ctx: &Ctx<'a>, place: Place) {
        if b.vertical {
            self.vlist_out(b, ctx, place);
        } else {
            self.hlist_out(b, ctx, place);
        }
    }

    /// §619–§628 (TeXXeT's reversals left out).
    fn hlist_out(&mut self, this: &BoxNode, ctx: &Ctx<'a>, place: Place) {
        let base_line = self.v;
        let left_edge = self.h;
        self.rec(Kind::HBox, place, self.h, self.v, [this.w, this.h, this.d]);
        let mut glue = SetGlue::default();
        // (the place of the last character of the run of them open)
        let mut run: Option<Place> = None;
        for (i, it) in this.list.iter().enumerate() {
            let width = match it {
                Item::Char(f, c) => Some(f.glyph(i32::from(*c)).map_or(0, |g| g.width)),
                Item::Lig(l) => Some(l.font.glyph(i32::from(l.c)).map_or(0, |g| g.width)),
                _ => None,
            };
            if let Some(wd) = width {
                let p = self.item_place(ctx, this, i);
                self.rec(Kind::Glyph, p, self.h, base_line, [wd, 0, 0]);
                self.h += wd;
                run = Some(p);
                continue;
            }
            if let Some(p) = run.take() {
                self.rec(Kind::RunEnd, p, self.h, base_line, [0; 3]);
            }
            match it {
                Item::Box(b) | Item::Append(b, _) => {
                    if b.list.is_empty() {
                        let p = self.item_place(ctx, this, i);
                        let kind = if b.vertical { Kind::VoidV } else { Kind::VoidH };
                        self.rec(kind, p, self.h, base_line, [b.w, b.h, b.d]);
                        self.h += b.w;
                    } else {
                        let (c, p) = self.child(ctx, this, i, b);
                        self.v = base_line + b.shift;
                        let edge = self.h + b.w;
                        self.list_out(b, &c, p);
                        self.h = edge;
                        self.v = base_line;
                    }
                }
                Item::Rule { w, h, d } => {
                    let ht = if *h == RUNNING { this.h } else { *h };
                    let dp = if *d == RUNNING { this.d } else { *d };
                    self.h += *w;
                    let p = self.item_place(ctx, this, i);
                    self.rec(Kind::Rule, p, self.h, base_line, [*w, ht + dp, dp]);
                }
                Item::Glue { g, .. } => {
                    self.h += glue.set(this, g);
                    let p = self.item_place(ctx, this, i);
                    self.rec(Kind::Glue, p, self.h, base_line, [0; 3]);
                }
                Item::Kern { w, .. } => {
                    self.h += *w;
                    let p = self.item_place(ctx, this, i);
                    self.rec(Kind::Kern, p, self.h, base_line, [*w, 0, 0]);
                }
                Item::Math { w, .. } => {
                    let p = self.item_place(ctx, this, i);
                    self.rec(Kind::Math, p, self.h, base_line, [0; 3]);
                    self.h += *w;
                }
                Item::Leaders(l) => {
                    let rule_wd = glue.set(this, &l.g);
                    self.hleaders(this, ctx, i, l, rule_wd, base_line, left_edge);
                }
                _ => {}
            }
        }
        if let Some(p) = run {
            self.rec(Kind::RunEnd, p, self.h, base_line, [0; 3]);
        }
        self.rec(Kind::EndH, place, self.h, self.v, [0; 3]);
    }

    /// §626–§628.
    #[allow(clippy::too_many_arguments)]
    fn hleaders(&mut self, this: &BoxNode, ctx: &Ctx<'a>, i: usize, l: &Leaders, rule_wd: i32, base_line: i32, left_edge: i32) {
        match &l.b {
            Item::Box(lb) if lb.w > 0 && rule_wd > 0 => {
                let leader_wd = lb.w;
                let rule_wd = rule_wd + 10;
                let edge = self.h + rule_wd;
                let mut lx = 0;
                if l.sub == A_LEADERS {
                    let save_h = self.h;
                    self.h = left_edge + leader_wd * ((self.h - left_edge) / leader_wd);
                    if self.h < save_h {
                        self.h += leader_wd;
                    }
                } else {
                    let lq = rule_wd / leader_wd;
                    let lr = rule_wd % leader_wd;
                    if l.sub == C_LEADERS {
                        self.h += lr / 2;
                    } else {
                        lx = lr / (lq + 1);
                        self.h += (lr - (lq - 1) * lx) / 2;
                    }
                }
                let (c, p) = self.child(ctx, this, i, lb);
                while self.h + leader_wd <= edge {
                    self.v = base_line + lb.shift;
                    let save_h = self.h;
                    self.list_out(lb, &c, p);
                    self.v = base_line;
                    self.h = save_h + leader_wd + lx;
                }
                self.h = edge - 10;
            }
            _ => {
                self.h += rule_wd;
                let p = self.item_place(ctx, this, i);
                self.rec(Kind::Glue, p, self.h, base_line, [0; 3]);
            }
        }
    }

    /// §629–§637.
    fn vlist_out(&mut self, this: &BoxNode, ctx: &Ctx<'a>, place: Place) {
        self.rec(Kind::VBox, place, self.h, self.v, [this.w, this.h, this.d]);
        let left_edge = self.h;
        self.v -= this.h;
        let top_edge = self.v;
        let mut glue = SetGlue::default();
        for (i, it) in this.list.iter().enumerate() {
            match it {
                Item::Box(b) | Item::Append(b, _) => {
                    if b.list.is_empty() {
                        self.v += b.h;
                        let p = self.item_place(ctx, this, i);
                        let kind = if b.vertical { Kind::VoidV } else { Kind::VoidH };
                        self.rec(kind, p, self.h, self.v, [b.w, b.h, b.d]);
                        self.v += b.d;
                    } else {
                        self.v += b.h;
                        let save_v = self.v;
                        self.h = left_edge + b.shift;
                        let (c, p) = self.child(ctx, this, i, b);
                        self.list_out(b, &c, p);
                        self.v = save_v + b.d;
                        self.h = left_edge;
                    }
                }
                Item::Rule { h, d, .. } => self.v += *h + *d,
                Item::Glue { g, .. } => self.v += glue.set(this, g),
                Item::Leaders(l) => {
                    let rule_ht = glue.set(this, &l.g);
                    self.vleaders(this, ctx, i, l, rule_ht, left_edge, top_edge);
                }
                Item::Kern { w, .. } => self.v += *w,
                _ => {}
            }
        }
        self.rec(Kind::EndV, place, self.h, self.v, [0; 3]);
    }

    /// §635–§637.
    #[allow(clippy::too_many_arguments)]
    fn vleaders(&mut self, this: &BoxNode, ctx: &Ctx<'a>, i: usize, l: &Leaders, rule_ht: i32, left_edge: i32, top_edge: i32) {
        match &l.b {
            Item::Box(lb) if lb.h + lb.d > 0 && rule_ht > 0 => {
                let leader_ht = lb.h + lb.d;
                let rule_ht = rule_ht + 10;
                let edge = self.v + rule_ht;
                let mut lx = 0;
                if l.sub == A_LEADERS {
                    let save_v = self.v;
                    self.v = top_edge + leader_ht * ((self.v - top_edge) / leader_ht);
                    if self.v < save_v {
                        self.v += leader_ht;
                    }
                } else {
                    let lq = rule_ht / leader_ht;
                    let lr = rule_ht % leader_ht;
                    if l.sub == C_LEADERS {
                        self.v += lr / 2;
                    } else {
                        lx = lr / (lq + 1);
                        self.v += (lr - (lq - 1) * lx) / 2;
                    }
                }
                let (c, p) = self.child(ctx, this, i, lb);
                while self.v + leader_ht <= edge {
                    self.h = left_edge + lb.shift;
                    self.v += lb.h;
                    let save_v = self.v;
                    self.list_out(lb, &c, p);
                    self.h = left_edge;
                    self.v = save_v - lb.h + leader_ht + lx;
                }
                self.v = edge - 10;
            }
            _ => self.v += rule_ht,
        }
    }
}

impl Build {
    /// The pages the last build shipped.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Page `j`'s box: a page kept from the build before has the same
    /// (`Arc`).
    pub fn page_box(&self, j: usize) -> Option<Arc<BoxNode>> {
        self.pages.get(j).map(|(p, _)| p.b.clone())
    }

    /// The pieces of the last build, as [`PageSync`]'s places name them.
    pub fn sync_pieces(&self) -> Vec<SyncPiece> {
        self.pieces
            .iter()
            .map(|kept| {
                let (file, base) = self.places.get(&kept.id).cloned().unwrap_or((None, 0));
                SyncPiece { id: kept.id, run: kept.run.clone(), file, base }
            })
            .collect()
    }

    /// The boxes of the last build's material (its lines, its vertical
    /// items' boxes, the boxes in its inserts) and the boxes its nodes
    /// put in registers, with the boxes in those: what
    /// [`Build::page_sync`] traces a page's boxes by.
    pub fn sync_index(&self) -> Index {
        let mut idx = Index(FxMap::default(), FxMap::default());
        for (k, kept) in self.pieces.iter().enumerate() {
            let m = &mut idx.0;
            let Some(run) = kept.run.as_ref() else { continue };
            let k = k as u32;
            let nodes = run.para_nodes();
            for (j, para) in run.paras.iter().enumerate() {
                let Some(&pn) = nodes.get(j) else { break };
                for it in para.before.iter().chain(&para.after) {
                    anchor_item(m, it, k, pn);
                }
                if let Some(Some(lines)) = kept.lines.get(j) {
                    // (the inserts the lines moved out: by the node that
                    // made each in the paragraph's list)
                    let mut ins: FxMap<usize, u32> = FxMap::default();
                    for (i, it) in para.hlist.iter().enumerate() {
                        if let (Item::Ins(x), Some(d)) = (it, para.origins.of(i)) {
                            ins.insert(Arc::as_ptr(x) as usize, pn.saturating_sub(d));
                        }
                    }
                    for (li, l) in lines.iter().enumerate() {
                        m.entry(addr(&l.b)).or_insert(Anchor::Line { k, j: j as u32, li: li as u32 });
                        for it in &l.adjust {
                            let node = match it {
                                Item::Ins(x) => ins.get(&(Arc::as_ptr(x) as usize)).copied().unwrap_or(pn),
                                _ => pn,
                            };
                            anchor_item(m, it, k, node);
                        }
                    }
                }
            }
            for (d, b) in run.box_defs() {
                descend(&mut idx, &b, k, d);
            }
        }
        idx
    }

    /// Page `j`'s records, as shipping it walks it (`idx`: this build's
    /// [`Build::sync_index`]).
    pub fn page_sync(&self, j: usize, idx: &Index) -> Option<PageSync> {
        let (page, _) = self.pages.get(j)?;
        let ship = &page.ship;
        let pdf = ship.pdf.as_ref().filter(|s| s.output > 0);
        let mut w = Walk {
            b: self,
            idx,
            deps: Vec::new(),
            dep_of: FxMap::default(),
            ends: FxMap::default(),
            recs: Vec::new(),
            h: 0,
            v: 0,
        };
        // (what an output routine made: the place of the page builder's
        // call that fired it, as pdfTeX's line being read then)
        let fallback = self
            .page_orgs
            .get(j)
            .and_then(|o| {
                let node = self.material_node(o.call.0 as usize, o.call.1, &[])?;
                Some(w.place(o.call.0, node))
            })
            .unwrap_or(NOWHERE);
        let (h0, v0) = match pdf {
            Some(s) => (s.horigin + ship.hoffset, s.vorigin + ship.voffset),
            None => (ship.hoffset, ship.voffset),
        };
        w.h = h0;
        w.v = v0 + page.b.h;
        let (ctx, place) = w.child(&Ctx::Fixed(fallback), &BoxNode::default(), 0, &page.b);
        w.list_out(&page.b, &ctx, place);
        // (boxes with no place: the page's first place)
        if let Some(first) = w.recs.iter().map(|r| r.place).find(|&p| p != NOWHERE) {
            for r in &mut w.recs {
                if r.place == NOWHERE && matches!(r.kind, Kind::HBox | Kind::VBox | Kind::EndH | Kind::EndV) {
                    r.place = first;
                }
            }
        }
        Some(PageSync { pdf: pdf.is_some(), mag: ship.mag, deps: w.deps, recs: w.recs })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::{Options, Order, Reads};
    use crate::tfm::Font;

    fn font() -> Arc<Font> {
        let b = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/cmr10.tfm")).unwrap();
        Arc::new(Font::parse("cmr10", &b).unwrap())
    }

    /// Each character on the page has the line it was read on, at the
    /// point shipping puts it: one after the other along the baseline.
    #[test]
    fn glyphs_have_their_lines() {
        let src = "\\catcode`\\{=1 \\catcode`\\}=2 \\font\\x=cmr10 \\x \\hsize=300pt \\vsize=200pt \\parfillskip=0pt plus 1fil\nAlpha\nBeta \\hbox{Gamma}\nDelta\\par\n\\end\n";
        for lean in [true, false] {
            let mut b = Build::new(font());
            b.set_lean(lean);
            let out = b.update(src, Options { threads: 1, reads: Reads::Suspend, order: Order::Priority, ..Options::default() });
            assert!(out.errors.is_empty(), "{:?}", out.errors);
            let ps = b.page_sync(0, &b.sync_index()).expect("page 1");
            let pieces = b.sync_pieces();
            let line = |p: Place| {
                let d = &ps.deps[p.dep as usize];
                pieces.iter().find(|x| x.id == d.id).expect("its piece").base + p.line
            };
            let glyphs: Vec<&Rec> = ps.recs.iter().filter(|r| r.kind == Kind::Glyph).collect();
            assert_eq!(glyphs.len(), "AlphaBetaGammaDelta".len(), "lean {lean}: {:?}", ps.recs);
            let mut lines: Vec<u32> = glyphs.iter().map(|r| line(r.place)).collect();
            lines.dedup();
            assert_eq!(lines, [2, 3, 4], "lean {lean}");
            assert!(glyphs.windows(2).all(|g| g[0].v == g[1].v && g[0].h + g[0].w <= g[1].h), "lean {lean}: {glyphs:?}");
            assert!(!ps.pdf);
            // (the boxes open and close in pairs)
            let opens = ps.recs.iter().filter(|r| matches!(r.kind, Kind::HBox | Kind::VBox)).count();
            let ends = ps.recs.iter().filter(|r| matches!(r.kind, Kind::EndH | Kind::EndV)).count();
            assert_eq!(opens, ends);
        }
    }
}
