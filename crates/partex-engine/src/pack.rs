//! Packaging: `hpack` and `vpack` (tex.web part 33, §644–§679).
//!
//! Pure functions: a list and a specification in, a box and what TeX
//! would report about it out. Printing the report (and the box) is the
//! caller's business.

use alloc::vec::Vec;

use crate::Scaled;
use crate::font::Font;
use crate::lr;
use crate::node::{BoxNode, FontId, GlueSign, GlueSpec, Node, Order, RUNNING};
use crate::scaled::{MAX_DIMEN, badness};

/// §644: the size a box is packed to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spec {
    /// `to` this size.
    Exactly(Scaled),
    /// The natural size plus this much (`spread`; 0 for natural).
    Additional(Scaled),
}

impl Spec {
    pub const NATURAL: Spec = Spec::Additional(0);
}

/// The parameters packaging reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Params {
    /// `\hbadness` or `\vbadness`.
    pub badness: i32,
    /// `\hfuzz` or `\vfuzz`.
    pub fuzz: Scaled,
    /// `\overfullrule` (hpack only).
    pub overfull_rule: Scaled,
    /// e-TeX's `\TeXXeTstate>0`: check the text directions (hpack only).
    pub texxet: bool,
}

/// §660–§667, §674–§678: what TeX reports about a packed box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    Underfull {
        badness: i32,
    },
    Loose {
        badness: i32,
    },
    Tight {
        badness: i32,
    },
    /// `excess` too wide or too high.
    Overfull {
        excess: Scaled,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Packed {
    pub node: BoxNode,
    /// `last_badness`.
    pub badness: i32,
    pub report: Option<Report>,
    /// The shrinkability of the list by order (§646's `total_shrink`;
    /// §1201 looks at it after packaging a display).
    pub total_shrink: [Scaled; 4],
    /// Likewise the stretchability (`total_stretch`, which §796 records
    /// in an alignment entry).
    pub total_stretch: [Scaled; 4],
    /// `TeXXeT`: unbalanced text directions. The missing closing nodes are
    /// not yet in `node` (TeX appends them after the report above).
    pub lr: lr::Problems,
}

impl Packed {
    /// Append the closing nodes `TeXXeT` found missing.
    pub fn close_lr(&mut self) {
        self.node.list.append(&mut self.lr.missing);
    }
}

/// Font metrics by internal font number.
pub trait Fonts {
    fn font(&self, f: FontId) -> &Font;
}

/// An internal inconsistency (TeX's `confusion`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Confusion(pub &'static str);

/// §646: the glue totals by order.
#[derive(Default)]
struct Totals {
    stretch: [Scaled; 4],
    shrink: [Scaled; 4],
}

impl Totals {
    fn add(&mut self, g: &GlueSpec) {
        self.stretch[g.stretch_order.index()] += g.stretch;
        self.shrink[g.shrink_order.index()] += g.shrink;
    }
}

/// §659, §665: the order of infinity in `t`.
#[must_use]
pub fn order(t: &[Scaled; 4]) -> Order {
    if t[3] != 0 {
        Order::Filll
    } else if t[2] != 0 {
        Order::Fill
    } else if t[1] != 0 {
        Order::Fil
    } else {
        Order::Normal
    }
}

/// Horizontal natural dimensions being accumulated.
#[derive(Default)]
struct HDims {
    h: Scaled,
    d: Scaled,
    x: Scaled,
    totals: Totals,
}

impl HDims {
    fn char(&mut self, fonts: &impl Fonts, font: FontId, ch: u8) {
        // §654: incorporate character dimensions.
        if let Some(g) = fonts.font(font).glyph(i32::from(ch)) {
            self.x += g.width;
            self.h = self.h.max(g.height);
            self.d = self.d.max(g.depth);
        }
    }

    fn node(&mut self, fonts: &impl Fonts, n: &Node) {
        match n {
            Node::Glyphs(g) => {
                let font = fonts.font(g.font);
                for &ch in g.chars() {
                    // §654: incorporate character dimensions.
                    if let Some(g) = font.glyph(i32::from(ch)) {
                        self.x += g.width;
                        self.h = self.h.max(g.height);
                        self.d = self.d.max(g.depth);
                    }
                }
            }
            Node::Ligature(l) => self.char(fonts, l.font, l.ch),
            Node::Box(b) => self.boxed(b.width, b.height, b.depth, b.shift),
            Node::Rule {
                width,
                height,
                depth,
                ..
            } => self.boxed(*width, *height, *depth, 0),
            Node::Unset(u) => self.boxed(u.width, u.height, u.depth, 0),
            Node::Glue { spec, .. } => {
                // §656: incorporate glue into the horizontal totals.
                self.x += spec.width;
                self.totals.add(spec);
            }
            Node::Leaders(l) => {
                self.x += l.spec.width;
                self.totals.add(&l.spec);
                let (h, d) = height_depth(&l.leader);
                self.h = self.h.max(h);
                self.d = self.d.max(d);
            }
            Node::Kern { width, .. }
            | Node::Math { width, .. }
            | Node::MarginKern { width, .. } => self.x += width,
            Node::Disc(d) => {
                // (tex.web has the replaced nodes after the discretionary)
                for n in &d.replace {
                    self.node(fonts, n);
                }
            }
            Node::Penalty(_) | Node::Ins(_) | Node::Mark(_) | Node::Adjust(_) => {}
            // pdfTeX §1629
            Node::Whatsit(w) => {
                if let Some(d) = w.ref_dims() {
                    self.boxed(d.width, d.height, d.depth, 0);
                }
            }
        }
    }

    /// §653: incorporate box dimensions.
    fn boxed(&mut self, width: Scaled, height: Scaled, depth: Scaled, s: Scaled) {
        self.x += width;
        self.h = self.h.max(height - s);
        self.d = self.d.max(depth + s);
    }
}

/// The height and depth `hpack` gives `list` at its natural width, without
/// packing it.
pub fn natural_height_depth(list: &[Node], fonts: &impl Fonts) -> (Scaled, Scaled) {
    let mut dims = HDims {
        h: 0,
        d: 0,
        x: 0,
        totals: Totals::default(),
    };
    for n in list {
        dims.node(fonts, n);
    }
    (dims.h, dims.d)
}

/// The natural width `hpack` gives `list`, and its glue's stretch and
/// shrink by order.
pub fn natural_width(list: &[Node], fonts: &impl Fonts) -> (Scaled, [Scaled; 4], [Scaled; 4]) {
    let mut dims = HDims::default();
    for n in list {
        dims.node(fonts, n);
    }
    (dims.x, dims.totals.stretch, dims.totals.shrink)
}

/// The height and depth of a box or rule node (for leaders).
fn height_depth(n: &Node) -> (Scaled, Scaled) {
    match n {
        Node::Box(b) => (b.height, b.depth),
        Node::Rule { height, depth, .. } => (*height, *depth),
        _ => (RUNNING, RUNNING),
    }
}

fn width_of(n: &Node) -> Scaled {
    match n {
        Node::Box(b) => b.width,
        Node::Rule { width, .. } => *width,
        _ => RUNNING,
    }
}

/// §649: package hlist `list` as TeX's `hpack`. With `adjust`, insertions,
/// marks and `\vadjust` material move out of the list into it (§655);
/// pdfTeX's and `XeTeX`'s `\vadjust pre` material (their
/// `pre_adjust_tail`'s) stays wrapped in its adjust node there, for
/// [`split_migrated`].
pub fn hpack(
    mut list: Vec<Node>,
    spec: Spec,
    params: &Params,
    fonts: &impl Fonts,
    adjust: Option<&mut Vec<Node>>,
) -> Packed {
    if let Some(adjust) = adjust {
        // §655: transfer insertions, marks and adjustments.
        let mut kept = Vec::with_capacity(list.len());
        for n in list {
            match n {
                Node::Ins(_) | Node::Mark(_) => adjust.push(n),
                Node::Adjust(a) if a.pre => adjust.push(Node::Adjust(a)),
                Node::Adjust(a) => adjust.extend(a.list),
                n => kept.push(n),
            }
        }
        list = kept;
    }
    let mut dims = HDims::default();
    for n in &list {
        dims.node(fonts, n);
    }
    let lr = if params.texxet {
        lr::balance(&mut list)
    } else {
        lr::Problems::default()
    };
    let natural = dims.x;
    let w = match spec {
        Spec::Exactly(w) => w,
        Spec::Additional(w) => w + natural,
    };
    let mut node = BoxNode {
        vertical: false,
        width: w,
        height: dims.h,
        depth: dims.d,
        list,
        ..BoxNode::default()
    };
    // §657: determine the glue setting.
    let (badness, report) = set_glue(&mut node, w - natural, &dims.totals, params, true);
    Packed {
        node,
        badness,
        report,
        total_shrink: dims.totals.shrink,
        total_stretch: dims.totals.stretch,
        lr,
    }
}

/// Material `hpack` moved out of a list (§655), split into the
/// pre-adjustment list (`\vadjust pre`), which goes before the box in the
/// enclosing vertical list, and the rest, which goes after it.
#[must_use]
pub fn split_migrated(migrated: Vec<Node>) -> (Vec<Node>, Vec<Node>) {
    let mut pre = Vec::new();
    let mut post = Vec::with_capacity(migrated.len());
    for n in migrated {
        match n {
            Node::Adjust(a) if a.pre => pre.extend(a.list),
            n => post.push(n),
        }
    }
    (pre, post)
}

/// §668: package vlist `list` as TeX's `vpackage`, with the depth at most
/// `max_depth` (`vpack` is `max_depth` = `max_dimen`).
pub fn vpack(
    list: Vec<Node>,
    spec: Spec,
    max_depth: Scaled,
    params: &Params,
) -> Result<Packed, Confusion> {
    let (mut w, mut d, mut x) = (0, 0, 0);
    let mut totals = Totals::default();
    for n in &list {
        // §669: examine node `n` in the vlist.
        match n {
            Node::Glyphs(_) | Node::Ligature(_) => return Err(Confusion("vpack")),
            Node::Box(b) => {
                // §670: incorporate box dimensions.
                x += d + b.height;
                d = b.depth;
                w = w.max(b.width + b.shift);
            }
            Node::Rule {
                width,
                height,
                depth,
                ..
            } => {
                x += d + height;
                d = *depth;
                w = w.max(*width);
            }
            Node::Unset(u) => {
                x += d + u.height;
                d = u.depth;
                w = w.max(u.width);
            }
            Node::Glue { spec, .. } => {
                // §671: incorporate glue into the vertical totals.
                x += d + spec.width;
                d = 0;
                totals.add(spec);
            }
            Node::Leaders(l) => {
                x += d + l.spec.width;
                d = 0;
                totals.add(&l.spec);
                w = w.max(width_of(&l.leader));
            }
            Node::Kern { width, .. } => {
                x += d + width;
                d = 0;
            }
            // pdfTeX §1629
            Node::Whatsit(wh) => {
                if let Some(r) = wh.ref_dims() {
                    x += d + r.height;
                    d = r.depth;
                    w = w.max(r.width);
                }
            }
            _ => {}
        }
    }
    let depth = if d > max_depth {
        x += d - max_depth;
        max_depth
    } else {
        d
    };
    // §672: determine the value of `height(r)` and the glue setting.
    let h = match spec {
        Spec::Exactly(h) => h,
        Spec::Additional(h) => h + x,
    };
    let mut node = BoxNode {
        vertical: true,
        width: w,
        height: h,
        depth,
        list,
        ..BoxNode::default()
    };
    let (badness, report) = set_glue(&mut node, h - x, &totals, params, false);
    Ok(Packed {
        node,
        badness,
        report,
        total_shrink: totals.shrink,
        total_stretch: totals.stretch,
        lr: lr::Problems::default(),
    })
}

/// `vpack` with no depth limit.
pub fn vpack_natural(list: Vec<Node>, spec: Spec, params: &Params) -> Result<Packed, Confusion> {
    vpack(list, spec, MAX_DIMEN, params)
}

/// §657–§667, §672–§678: set the glue of `b` for excess `x`; the badness
/// and the report, if any.
fn set_glue(
    b: &mut BoxNode,
    x: Scaled,
    t: &Totals,
    params: &Params,
    horizontal: bool,
) -> (i32, Option<Report>) {
    if x == 0 {
        return (0, None);
    }
    let empty = b.list.is_empty();
    if x > 0 {
        // §658, §673: determine glue stretch setting.
        let o = order(&t.stretch);
        b.glue_order = o;
        if t.stretch[o.index()] == 0 {
            b.glue_sign = GlueSign::Normal; // there's nothing to stretch
        } else {
            b.glue_sign = GlueSign::Stretching;
            b.glue_set = f64::from(x) / f64::from(t.stretch[o.index()]);
        }
        if o == Order::Normal && !empty {
            // §660, §674: report an underfull box, if sufficiently bad.
            let bad = badness(x, t.stretch[0]);
            if bad > params.badness {
                let report = if bad > 100 {
                    Report::Underfull { badness: bad }
                } else {
                    Report::Loose { badness: bad }
                };
                return (bad, Some(report));
            }
            return (bad, None);
        }
        return (0, None);
    }
    // §664, §676: determine glue shrink setting.
    let o = order(&t.shrink);
    b.glue_order = o;
    if t.shrink[o.index()] == 0 {
        b.glue_sign = GlueSign::Normal; // there's nothing to shrink
    } else {
        b.glue_sign = GlueSign::Shrinking;
        b.glue_set = f64::from(-x) / f64::from(t.shrink[o.index()]);
    }
    if t.shrink[o.index()] < -x && o == Order::Normal && !empty {
        b.glue_set = 1.0; // use the maximum shrinkage
        // §666, §677: report an overfull box, if sufficiently bad.
        let excess = -x - t.shrink[0];
        if excess > params.fuzz || params.badness < 100 {
            if horizontal && params.overfull_rule > 0 && excess > params.fuzz {
                b.list.push(Node::Rule {
                    width: params.overfull_rule,
                    height: RUNNING,
                    depth: RUNNING,
                    sync: crate::origin::Side(0),
                });
            }
            return (1_000_000, Some(Report::Overfull { excess }));
        }
        return (1_000_000, None);
    }
    if o == Order::Normal && !empty {
        // §667, §678: report a tight box, if sufficiently bad.
        let bad = badness(-x, t.shrink[0]);
        if bad > params.badness {
            return (bad, Some(Report::Tight { badness: bad }));
        }
        return (bad, None);
    }
    (0, None)
}
