//! C2: `hpack`, `vpack` as pure functions (tex.web 649–679).
//!
//! Totals by glue order, the glue set ratio as a real (`x / total`, TeX's
//! `unfloat`), badness, underfull/loose/tight/overfull reports (the message
//! up to its `)`, the caller appends `detected at line n` or its own place),
//! the overfull rule, and adjust material migration (`\vadjust`, `\insert`,
//! `\mark` out of an hlist when packed for a vertical list).
use std::sync::Arc;

use crate::diag::Diag;
use crate::item::{BoxNode, GlueSet, Item, RUNNING};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spec { Exactly(i32), Additional(i32) }

/// What packing reads from env, read by the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params { pub hbadness: i32, pub vbadness: i32, pub hfuzz: i32, pub vfuzz: i32, pub overfull_rule: i32 }

#[derive(Clone, Debug)]
pub struct Packed { pub b: BoxNode, pub badness: i32, pub report: Option<Arc<crate::diag::Diag>>, pub adjust: Vec<Item> }

/// `\maxdimen`.
pub const MAX_DIMEN: i32 = 0o7777777777;

/// TeX's `print_scaled` (tex.web 103), without the unit.
pub fn print_scaled(s: i32) -> String {
    let mut o = String::new();
    let mut s = i64::from(s);
    if s < 0 {
        o.push('-');
        s = -s;
    }
    o.push_str(&(s / 65536).to_string());
    o.push('.');
    let mut s = 10 * (s % 65536) + 5;
    let mut delta = 10i64;
    loop {
        if delta > 65536 {
            s = s + 0o100000 - 50000;
        }
        o.push((b'0' + (s / 65536) as u8) as char);
        s = 10 * (s % 65536);
        delta *= 10;
        if s <= delta {
            break;
        }
    }
    o
}

fn highest(t: &[i32; 4]) -> u8 {
    if t[3] != 0 {
        3
    } else if t[2] != 0 {
        2
    } else if t[1] != 0 {
        1
    } else {
        0
    }
}

struct Fit {
    set: GlueSet,
    badness: i32,
    report: Option<String>,
    /// Overfull excess (`-x - total_shrink[normal]`), 0 if not overfull.
    over: i32,
}

/// tex.web 657–667 / 673–678: the glue setting for excess `x`.
#[allow(clippy::too_many_arguments)]
fn fit(x: i32, st: &[i32; 4], sh: &[i32; 4], nonempty: bool, bad_lim: i32, fuzz: i32, what: &str, too: &str) -> Fit {
    let mut f = Fit { set: GlueSet::default(), badness: 0, report: None, over: 0 };
    if x == 0 {
        return f;
    }
    if x > 0 {
        let o = highest(st);
        f.set.order = o;
        f.set.sign = 1;
        if st[o as usize] != 0 {
            f.set.ratio = f64::from(x) / f64::from(st[o as usize]);
        } else {
            f.set.sign = 0;
            f.set.ratio = 0.0;
        }
        if o == 0 && nonempty {
            f.badness = crate::arith::badness(x, st[0]);
            if f.badness > bad_lim {
                let w = if f.badness > 100 { "Underfull" } else { "Loose" };
                f.report = Some(format!("{w} \\{what} (badness {})", f.badness));
            }
        }
    } else {
        let nx = x.wrapping_neg();
        let o = highest(sh);
        f.set.order = o;
        f.set.sign = -1;
        if sh[o as usize] != 0 {
            f.set.ratio = f64::from(nx) / f64::from(sh[o as usize]);
        } else {
            f.set.sign = 0;
            f.set.ratio = 0.0;
        }
        if sh[o as usize] < nx && o == 0 && nonempty {
            f.badness = 1_000_000;
            f.set.ratio = 1.0;
            let ex = nx.wrapping_sub(sh[0]);
            f.over = ex;
            if ex > fuzz || bad_lim < 100 {
                f.report = Some(format!("Overfull \\{what} ({}pt too {too})", print_scaled(ex)));
            }
        } else if o == 0 && nonempty {
            f.badness = crate::arith::badness(nx, sh[0]);
            if f.badness > bad_lim {
                f.report = Some(format!("Tight \\{what} (badness {})", f.badness));
            }
        }
    }
    f
}

fn diag(msg: Option<String>) -> Option<Arc<Diag>> {
    msg.map(|msg| Arc::new(Diag { msg, help: Vec::new(), line: 0 }))
}

/// `hpack` with adjust material migrating (as for lines of a paragraph and
/// an `\hbox` built in vertical mode).
pub fn hpack(list: Vec<Item>, spec: Spec, p: &Params, fonts: &dyn crate::font::Fonts) -> Packed {
    hpack_with(list, spec, p, fonts, true)
}

/// `hpack`; `migrate`: TeX's `adjust_tail <> null`.
pub fn hpack_with(list: Vec<Item>, spec: Spec, p: &Params, fonts: &dyn crate::font::Fonts, migrate: bool) -> Packed {
    let (mut h, mut d, mut x) = (0i32, 0i32, 0i32);
    let mut st = [0i32; 4];
    let mut sh = [0i32; 4];
    let mut out: Vec<Item> = Vec::with_capacity(list.len());
    let mut adjust = Vec::new();
    for it in list {
        match &it {
            Item::Char { font, c } | Item::Lig { font, c, .. } => {
                let f = fonts.font(*font);
                x = x.wrapping_add(f.width(*c));
                h = h.max(f.height(*c));
                d = d.max(f.depth(*c));
            }
            Item::Box(b) => {
                x = x.wrapping_add(b.width);
                let s = b.shift;
                h = h.max(b.height.wrapping_sub(s));
                d = d.max(b.depth.wrapping_add(s));
            }
            Item::Rule { w, h: rh, d: rd } => {
                x = x.wrapping_add(*w);
                h = h.max(*rh);
                d = d.max(*rd);
            }
            Item::Unset(u) => {
                x = x.wrapping_add(u.width);
                h = h.max(u.height);
                d = d.max(u.depth);
            }
            Item::Ins(_) | Item::Mark { .. } if migrate => {
                adjust.push(it);
                continue;
            }
            Item::Adjust(l) if migrate => {
                adjust.extend(l.iter().cloned());
                continue;
            }
            Item::Glue { g, leader, .. } => {
                x = x.wrapping_add(g.width);
                let so = (g.stretch_order & 3) as usize;
                let ko = (g.shrink_order & 3) as usize;
                st[so] = st[so].wrapping_add(g.stretch);
                sh[ko] = sh[ko].wrapping_add(g.shrink);
                if let Some(l) = leader {
                    match &**l {
                        Item::Box(b) => {
                            h = h.max(b.height);
                            d = d.max(b.depth);
                        }
                        Item::Rule { h: rh, d: rd, .. } => {
                            h = h.max(*rh);
                            d = d.max(*rd);
                        }
                        _ => {}
                    }
                }
            }
            Item::Kern { w, .. } | Item::Math { w, .. } | Item::MarginKern { w, .. } => {
                x = x.wrapping_add(*w);
            }
            _ => {}
        }
        out.push(it);
    }
    let w = match spec {
        Spec::Exactly(w) => w,
        Spec::Additional(e) => x.wrapping_add(e),
    };
    let excess = w.wrapping_sub(x);
    let f = fit(excess, &st, &sh, !out.is_empty(), p.hbadness, p.hfuzz, "hbox", "wide");
    if f.over > 0 && p.overfull_rule > 0 && f.over > p.hfuzz {
        out.push(Item::Rule { w: p.overfull_rule, h: RUNNING, d: RUNNING });
    }
    Packed {
        b: BoxNode { vertical: false, width: w, height: h, depth: d, shift: 0, set: f.set, lr: 0, list: out.into() },
        badness: f.badness,
        report: diag(f.report),
        adjust,
    }
}

/// `vpackage(list, spec, max_depth)`.
pub fn vpack(list: Vec<Item>, spec: Spec, max_depth: i32, p: &Params) -> Packed {
    let (mut w, mut d, mut x) = (0i32, 0i32, 0i32);
    let mut st = [0i32; 4];
    let mut sh = [0i32; 4];
    for it in list.iter() {
        match it {
            Item::Box(b) => {
                x = x.wrapping_add(d).wrapping_add(b.height);
                d = b.depth;
                w = w.max(b.width.wrapping_add(b.shift));
            }
            Item::Rule { w: rw, h: rh, d: rd } => {
                x = x.wrapping_add(d).wrapping_add(*rh);
                d = *rd;
                w = w.max(*rw);
            }
            Item::Unset(u) => {
                x = x.wrapping_add(d).wrapping_add(u.height);
                d = u.depth;
                w = w.max(u.width);
            }
            Item::Glue { g, leader, .. } => {
                x = x.wrapping_add(d);
                d = 0;
                x = x.wrapping_add(g.width);
                let so = (g.stretch_order & 3) as usize;
                let ko = (g.shrink_order & 3) as usize;
                st[so] = st[so].wrapping_add(g.stretch);
                sh[ko] = sh[ko].wrapping_add(g.shrink);
                if let Some(l) = leader {
                    match &**l {
                        Item::Box(b) => w = w.max(b.width),
                        Item::Rule { w: rw, .. } => w = w.max(*rw),
                        _ => {}
                    }
                }
            }
            Item::Kern { w: k, .. } => {
                x = x.wrapping_add(d).wrapping_add(*k);
                d = 0;
            }
            _ => {}
        }
    }
    let depth = if d > max_depth {
        x = x.wrapping_add(d - max_depth);
        max_depth
    } else {
        d
    };
    let h = match spec {
        Spec::Exactly(h) => h,
        Spec::Additional(e) => x.wrapping_add(e),
    };
    let excess = h.wrapping_sub(x);
    let f = fit(excess, &st, &sh, !list.is_empty(), p.vbadness, p.vfuzz, "vbox", "high");
    Packed {
        b: BoxNode { vertical: true, width: w, height: h, depth, shift: 0, set: f.set, lr: 0, list: list.into() },
        badness: f.badness,
        report: diag(f.report),
        adjust: Vec::new(),
    }
}
