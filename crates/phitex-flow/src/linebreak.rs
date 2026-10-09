//! Line breaking: a pure function of a paragraph (Knuth–Plass, tex.web
//! part 38, without hyphenation, looseness or shapes yet), and packing.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::arith::{INF_BAD, badness, clamp};
use crate::piece::{HItem, Para};

/// How a box's glue is set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlueSet {
    /// 0 normal, 1 stretching, -1 shrinking.
    pub sign: i8,
    pub order: u8,
    pub ratio: f64,
}

impl Eq for GlueSet {}

impl Hash for GlueSet {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.sign.hash(h);
        self.order.hash(h);
        self.ratio.to_bits().hash(h);
    }
}

pub const NORMAL: GlueSet = GlueSet { sign: 0, order: 0, ratio: 0.0 };

/// A line: a box of width `hsize` with its glue set.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Line {
    pub items: Vec<HItem>,
    pub w: i32,
    pub h: i32,
    pub d: i32,
    pub glue: GlueSet,
}

/// The glue setting that makes `x` more (or less) fit, given the stretch
/// and shrink by order (tex.web 658, 664).
pub fn set_glue(x: i32, st: &[i64; 4], sh: &[i64; 4]) -> GlueSet {
    if x > 0 {
        let o = (0..4).rev().find(|&o| st[o] != 0).unwrap_or(0);
        if st[o] == 0 {
            NORMAL
        } else {
            GlueSet { sign: 1, order: o as u8, ratio: f64::from(x) / st[o] as f64 }
        }
    } else if x < 0 {
        let o = (0..4).rev().find(|&o| sh[o] != 0).unwrap_or(0);
        if sh[o] == 0 {
            NORMAL
        } else if o == 0 && -i64::from(x) > sh[0] {
            GlueSet { sign: -1, order: 0, ratio: 1.0 }
        } else {
            GlueSet { sign: -1, order: o as u8, ratio: f64::from(-x) / sh[o] as f64 }
        }
    } else {
        NORMAL
    }
}

/// `hpack(items, exactly target)`.
pub fn hpack(items: Vec<HItem>, target: i32) -> Line {
    let (mut w, mut h, mut d) = (0i64, 0, 0);
    let mut st = [0i64; 4];
    let mut sh = [0i64; 4];
    for it in &items {
        match *it {
            HItem::Char { w: cw, h: ch, d: cd, .. } | HItem::Box { w: cw, h: ch, d: cd } => {
                w += i64::from(cw);
                h = h.max(ch);
                d = d.max(cd);
            }
            HItem::Glue { w: gw, st: s, st_ord, sh: k, sh_ord } => {
                w += i64::from(gw);
                st[usize::from(st_ord)] += i64::from(s);
                sh[usize::from(sh_ord)] += i64::from(k);
            }
            HItem::Penalty(_) => {}
        }
    }
    let glue = set_glue(clamp(i64::from(target) - w), &st, &sh);
    Line { items, w: target, h, d, glue }
}

struct Node {
    pos: usize,
    start: usize,
    total: i64,
    prev: Option<usize>,
}

/// Breaks a paragraph into lines.
pub fn break_lines(p: &Para) -> Vec<Arc<Line>> {
    let items = &p.hlist;
    let n = items.len();
    let mut pw = vec![0i64; n + 1];
    let mut pst = vec![[0i64; 4]; n + 1];
    let mut psh = vec![0i64; n + 1];
    for (i, it) in items.iter().enumerate() {
        pw[i + 1] = pw[i];
        pst[i + 1] = pst[i];
        psh[i + 1] = psh[i];
        match *it {
            HItem::Char { w, .. } | HItem::Box { w, .. } => pw[i + 1] += i64::from(w),
            HItem::Glue { w, st, st_ord, sh, .. } => {
                pw[i + 1] += i64::from(w);
                pst[i + 1][usize::from(st_ord)] += i64::from(st);
                psh[i + 1] += i64::from(sh);
            }
            HItem::Penalty(_) => {}
        }
    }
    // (legal breakpoints: glue after a box, penalties < 10000, the end)
    let mut cands: Vec<(usize, i32)> = Vec::new();
    for i in 0..n {
        match items[i] {
            HItem::Glue { .. } if i > 0 && matches!(items[i - 1], HItem::Char { .. } | HItem::Box { .. }) => {
                cands.push((i, 0));
            }
            HItem::Penalty(pp) if pp < 10000 => cands.push((i, pp)),
            _ => {}
        }
    }
    cands.push((n, -10000));
    let start_after = |b: usize| -> usize {
        let mut j = b + 1;
        while j < n && matches!(items[j], HItem::Glue { .. } | HItem::Penalty(_)) {
            j += 1;
        }
        j
    };
    for pass in 0..2 {
        let threshold = if pass == 0 { p.tolerance } else { i32::MAX };
        let mut arena = vec![Node { pos: usize::MAX, start: 0, total: 0, prev: None }];
        let mut active: Vec<usize> = vec![0];
        let mut reached_end = false;
        for &(b, pen) in &cands {
            let mut best: [Option<(i64, usize)>; 4] = [None; 4];
            let mut keep = Vec::with_capacity(active.len());
            for &a in &active {
                let s = arena[a].start;
                let w = pw[b] - pw[s.min(b)];
                let stv: [i64; 4] = std::array::from_fn(|o| pst[b][o] - pst[s.min(b)][o]);
                let shv = psh[b] - psh[s.min(b)];
                let short = i64::from(p.hsize) - w;
                let (bad, fit, overfull) = if short > 0 {
                    if stv[1] + stv[2] + stv[3] > 0 {
                        (0, 2usize, false)
                    } else {
                        let bb = badness(clamp(short), clamp(stv[0]));
                        (bb, if bb > 99 { 0 } else if bb > 12 { 1 } else { 2 }, false)
                    }
                } else if short < 0 {
                    if -short > shv {
                        (INF_BAD + 1, 3, true)
                    } else {
                        let bb = badness(clamp(-short), clamp(shv));
                        (bb, if bb > 12 { 3 } else { 2 }, false)
                    }
                } else {
                    (0, 2, false)
                };
                let feasible = if overfull { pass == 1 } else { bad <= threshold };
                if feasible {
                    let b2 = i64::from(if overfull { INF_BAD } else { bad });
                    let mut d = (i64::from(p.linepenalty) + b2).pow(2);
                    if pen > 0 {
                        d += i64::from(pen).pow(2);
                    } else if pen > -10000 {
                        d -= i64::from(pen).pow(2);
                    }
                    let tot = arena[a].total + d;
                    if best[fit].is_none_or(|(t, _)| tot < t) {
                        best[fit] = Some((tot, a));
                    }
                }
                let drop = (short < 0 && (overfull || bad > threshold)) || pen <= -10000;
                if !drop {
                    keep.push(a);
                }
            }
            active = keep;
            for slot in best.iter().flatten() {
                let (tot, a) = *slot;
                let start = if b >= n { n } else { start_after(b) };
                arena.push(Node { pos: b, start, total: tot, prev: Some(a) });
                active.push(arena.len() - 1);
                if b == n {
                    reached_end = true;
                }
            }
            if active.is_empty() {
                break;
            }
        }
        if !reached_end {
            continue;
        }
        let fin = arena
            .iter()
            .enumerate()
            .filter(|(_, x)| x.pos == n && x.prev.is_some())
            .min_by_key(|(_, x)| x.total)
            .map(|(i, _)| i);
        if let Some(mut i) = fin {
            let mut breaks = Vec::new();
            while let Some(pr) = arena[i].prev {
                breaks.push(arena[i].pos);
                i = pr;
            }
            breaks.reverse();
            let mut lines = Vec::with_capacity(breaks.len());
            let mut s = 0;
            for &b in &breaks {
                let end = b.min(n);
                lines.push(Arc::new(hpack(items[s.min(end)..end].to_vec(), p.hsize)));
                s = if b < n { start_after(b) } else { n };
            }
            return lines;
        }
    }
    vec![Arc::new(hpack(items.clone(), p.hsize))]
}
