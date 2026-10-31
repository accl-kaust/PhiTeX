// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Collider.cpp, src/inc/Collider.h),
// LGPL-2.1-or-later.

//! Collision avoidance: each slot's collision attributes
//! ([`SlotCollision`]), the shift collider (moving a glyph off its
//! neighbours along four axes) and the kern collider (kerning a cluster
//! away from the next).

use alloc::vec::Vec;

use crate::intervals::{Zones, gmax, gmin};
use crate::position::{Position, Rect};
use crate::segment::{NIL, Segment};

pub(crate) const COLL_FIX: u16 = 1;
pub(crate) const COLL_IGNORE: u16 = 2;
pub(crate) const COLL_START: u16 = 4;
pub(crate) const COLL_END: u16 = 8;
pub(crate) const COLL_KERN: u16 = 16;
pub(crate) const COLL_ISCOL: u16 = 32;
pub(crate) const COLL_KNOWN: u16 = 64;
pub(crate) const COLL_ISSPACE: u16 = 128;
pub(crate) const COLL_TEMPLOCK: u16 = 256;

const SEQ_ORDER_LEFTDOWN: i32 = 1;
const SEQ_ORDER_RIGHTUP: i32 = 2;
const SEQ_ORDER_NOABOVE: i32 = 4;
const SEQ_ORDER_NOBELOW: i32 = 8;
const SEQ_ORDER_NOLEFT: i32 = 16;
const SEQ_ORDER_NORIGHT: i32 = 32;

const ISQRT2: f32 = 0.707_106_781;

#[inline]
fn sqr(x: f32) -> f32 {
    x * x
}

/// `SlotCollision`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SlotCollision {
    pub limit: Rect,
    pub shift: Position,
    pub offset: Position,
    pub excl_offset: Position,
    pub margin: u16,
    pub margin_wt: u16,
    pub flags: u16,
    pub excl_glyph: u16,
    pub seq_class: u16,
    pub seq_prox_class: u16,
    pub seq_order: u16,
    pub seq_above_xoff: i16,
    pub seq_above_wt: u16,
    pub seq_below_xlim: i16,
    pub seq_below_wt: u16,
    pub seq_valign_ht: u16,
    pub seq_valign_wt: u16,
}

impl SlotCollision {
    /// `SlotCollision(seg, slot)` (`initFromSlot`): `None` for a slot with
    /// no glyph (its fields stay zero).
    pub(crate) fn from_slot(seg: &Segment, s: u32) -> Option<SlotCollision> {
        let gid = seg.s(s).glyphid;
        let a = u16::from(seg.silf.a_collision);
        let g = seg.face.glyphs.glyph_safe(gid)?;
        let p = |k: u16| g.attrs.get(a.wrapping_add(k));
        let f = |k: u16| f32::from(p(k) as i16);
        Some(SlotCollision {
            flags: p(0),
            limit: Rect::new(Position::new(f(1), f(2)), Position::new(f(3), f(4))),
            margin: p(5),
            margin_wt: p(6),
            seq_class: p(7),
            seq_prox_class: p(8),
            seq_order: p(9),
            seq_above_xoff: p(10) as i16,
            seq_above_wt: p(11),
            seq_below_xlim: p(12) as i16,
            seq_below_wt: p(13),
            seq_valign_ht: p(14),
            seq_valign_wt: p(15),
            ..SlotCollision::default()
        })
    }

    /// `ignore()`.
    pub(crate) fn ignore(&self) -> bool {
        self.flags & COLL_IGNORE != 0 || self.flags & COLL_ISSPACE != 0
    }
}

/// `ShiftCollider`.
#[derive(Default)]
pub(crate) struct ShiftCollider {
    ranges: [Zones; 4],
    target: u32,
    limit: Rect,
    curr_shift: Position,
    curr_offset: Position,
    origin: Position,
    margin: f32,
    margin_wt: f32,
    len: [f32; 4],
    seq_class: u16,
    seq_prox_class: u16,
    seq_order: u16,
}

/// A slot's box as `BBox` (xi, yi, xa, ya) and slanted box as `SlantBox`
/// (si, di, sa, da).
#[derive(Clone, Copy)]
struct BB {
    xi: f32,
    yi: f32,
    xa: f32,
    ya: f32,
}

impl BB {
    fn of(r: Rect) -> BB {
        BB {
            xi: r.bl.x,
            yi: r.bl.y,
            xa: r.tr.x,
            ya: r.tr.y,
        }
    }
}

#[derive(Clone, Copy)]
struct SB {
    si: f32,
    di: f32,
    sa: f32,
    da: f32,
}

impl SB {
    fn of(r: Rect) -> SB {
        SB {
            si: r.bl.x,
            di: r.bl.y,
            sa: r.tr.x,
            da: r.tr.y,
        }
    }
}

fn sdm(vi: f32, va: f32, mx: f32, my: f32, greater: bool) -> f32 {
    let op = |a: f32, b: f32| if greater { a > b } else { a < b };
    let mut res = 2.0 * mx - vi;
    if op(res, vi + 2.0 * my) {
        res = va + 2.0 * my;
        if op(res, 2.0 * mx - va) {
            res = mx + my;
        }
    }
    res
}

impl ShiftCollider {
    pub(crate) fn new() -> ShiftCollider {
        ShiftCollider {
            target: NIL,
            ..ShiftCollider::default()
        }
    }

    /// `initSlot`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn init_slot(
        &mut self,
        seg: &Segment,
        a_slot: u32,
        limit: &Rect,
        margin: f32,
        margin_weight: f32,
        curr_shift: Position,
        curr_offset: Position,
        dir: i32,
    ) -> bool {
        let gc = &seg.face.glyphs;
        let gid = seg.s(a_slot).glyphid;
        if !gc.check(gid) {
            return false;
        }
        let bb = BB::of(gc.bbox(gid));
        let sb = SB::of(gc.slant(gid));
        self.limit = if curr_offset.x != 0.0 || curr_offset.y != 0.0 {
            Rect::new(limit.bl - curr_offset, limit.tr - curr_offset)
        } else {
            *limit
        };
        let l = self.limit;
        for i in 0..4 {
            match i {
                0 => {
                    let mn = l.bl.x + curr_offset.x;
                    let mx = l.tr.x + curr_offset.x;
                    self.len[0] = bb.xa - bb.xi;
                    let a = curr_offset.y + curr_shift.y;
                    self.ranges[0].initialise(false, mn, mx, margin, margin_weight, a);
                }
                1 => {
                    let mn = l.bl.y + curr_offset.y;
                    let mx = l.tr.y + curr_offset.y;
                    self.len[1] = bb.ya - bb.yi;
                    let a = curr_offset.x + curr_shift.x;
                    self.ranges[1].initialise(false, mn, mx, margin, margin_weight, a);
                }
                2 => {
                    let shift = curr_offset.x + curr_offset.y + curr_shift.x + curr_shift.y;
                    let mn = -2.0 * gmin(curr_shift.x - l.bl.x, curr_shift.y - l.bl.y) + shift;
                    let mx = 2.0 * gmin(l.tr.x - curr_shift.x, l.tr.y - curr_shift.y) + shift;
                    self.len[2] = sb.sa - sb.si;
                    let a = curr_offset.x - curr_offset.y + curr_shift.x - curr_shift.y;
                    self.ranges[2].initialise(true, mn, mx, margin / ISQRT2, margin_weight, a);
                }
                _ => {
                    let shift = curr_offset.x - curr_offset.y + curr_shift.x - curr_shift.y;
                    let mn = -2.0 * gmin(curr_shift.x - l.bl.x, l.tr.y - curr_shift.y) + shift;
                    let mx = 2.0 * gmin(l.tr.x - curr_shift.x, curr_shift.y - l.bl.y) + shift;
                    self.len[3] = sb.da - sb.di;
                    let a = curr_offset.x + curr_offset.y + curr_shift.x + curr_shift.y;
                    self.ranges[3].initialise(true, mn, mx, margin / ISQRT2, margin_weight, a);
                }
            }
        }
        self.target = a_slot;
        if dir & 1 == 0 {
            self.limit.bl.x = -limit.tr.x;
        }
        self.curr_offset = curr_offset;
        self.curr_shift = curr_shift;
        self.origin = seg.s(a_slot).position - curr_offset;
        self.margin = margin;
        self.margin_wt = margin_weight;
        let c = seg.collision(a_slot).copied().unwrap_or_default();
        self.seq_class = c.seq_class;
        self.seq_prox_class = c.seq_prox_class;
        self.seq_order = c.seq_order;
        true
    }

    /// `addBox_slope`.
    #[allow(clippy::too_many_arguments)]
    fn add_box_slope(
        &mut self,
        isx: bool,
        bx: &Rect,
        bb: BB,
        sb: SB,
        org: Position,
        weight: f32,
        m: f32,
        minright: bool,
        axis: usize,
    ) {
        match axis {
            0 => {
                if bx.bl.y < org.y + bb.ya && bx.tr.y > org.y + bb.yi && bx.width() > 0.0 {
                    let a = org.y + 0.5 * (bb.yi + bb.ya);
                    let c = 0.5 * (bb.xi + bb.xa);
                    if isx {
                        self.ranges[0].weighted(
                            false,
                            bx.bl.x - c,
                            bx.tr.x - c,
                            weight,
                            a,
                            m,
                            (if minright { bx.tr.x } else { bx.bl.x }) - c,
                            a,
                            0.0,
                            false,
                        );
                    } else {
                        self.ranges[0].weighted(
                            false,
                            bx.bl.x - c,
                            bx.tr.x - c,
                            weight,
                            a,
                            0.0,
                            0.0,
                            org.y,
                            m * (a * a
                                + sqr((if minright { bx.tr.y } else { bx.bl.y })
                                    - 0.5 * (bb.yi + bb.ya))),
                            false,
                        );
                    }
                }
            }
            1 => {
                if bx.bl.x < org.x + bb.xa && bx.tr.x > org.x + bb.xi && bx.height() > 0.0 {
                    let a = org.x + 0.5 * (bb.xi + bb.xa);
                    let c = 0.5 * (bb.yi + bb.ya);
                    if isx {
                        self.ranges[1].weighted(
                            false,
                            bx.bl.y - c,
                            bx.tr.y - c,
                            weight,
                            a,
                            0.0,
                            0.0,
                            org.x,
                            m * (a * a
                                + sqr((if minright { bx.tr.x } else { bx.bl.x })
                                    - 0.5 * (bb.xi + bb.xa))),
                            false,
                        );
                    } else {
                        self.ranges[1].weighted(
                            false,
                            bx.bl.y - c,
                            bx.tr.y - c,
                            weight,
                            a,
                            m,
                            (if minright { bx.tr.y } else { bx.bl.y }) - c,
                            a,
                            0.0,
                            false,
                        );
                    }
                }
            }
            2 => {
                if bx.bl.x - bx.tr.y < org.x - org.y + sb.da
                    && bx.tr.x - bx.bl.y > org.x - org.y + sb.di
                {
                    let d = org.x - org.y + 0.5 * (sb.di + sb.da);
                    let c = 0.5 * (sb.si + sb.sa);
                    let smax = gmin(2.0 * bx.tr.x - d, 2.0 * bx.tr.y + d);
                    let smin = gmax(2.0 * bx.bl.x - d, 2.0 * bx.bl.y + d);
                    if smin > smax {
                        return;
                    }
                    let a = d;
                    let si = if isx {
                        2.0 * (if minright { bx.tr.x } else { bx.bl.x }) - a
                    } else {
                        2.0 * (if minright { bx.tr.y } else { bx.bl.y }) + a
                    };
                    self.ranges[2].weighted(
                        true,
                        smin - c,
                        smax - c,
                        weight / 2.0,
                        a,
                        m / 2.0,
                        si,
                        0.0,
                        0.0,
                        isx,
                    );
                }
            }
            3 => {
                if bx.bl.x + bx.bl.y < org.x + org.y + sb.sa
                    && bx.tr.x + bx.tr.y > org.x + org.y + sb.si
                {
                    let s = org.x + org.y + 0.5 * (sb.si + sb.sa);
                    let c = 0.5 * (sb.di + sb.da);
                    let dmax = gmin(2.0 * bx.tr.x - s, s - 2.0 * bx.bl.y);
                    let dmin = gmax(2.0 * bx.bl.x - s, s - 2.0 * bx.tr.y);
                    if dmin > dmax {
                        return;
                    }
                    let a = s;
                    let di = if isx {
                        2.0 * (if minright { bx.tr.x } else { bx.bl.x }) - a
                    } else {
                        2.0 * (if minright { bx.tr.y } else { bx.bl.y }) + a
                    };
                    self.ranges[3].weighted(
                        true,
                        dmin - c,
                        dmax - c,
                        weight / 2.0,
                        a,
                        m / 2.0,
                        di,
                        0.0,
                        0.0,
                        !isx,
                    );
                }
            }
            _ => {}
        }
    }

    /// `removeBox`.
    fn remove_box(&mut self, bx: &Rect, bb: BB, sb: SB, org: Position, axis: usize) {
        match axis {
            0 => {
                if bx.bl.y < org.y + bb.ya && bx.tr.y > org.y + bb.yi && bx.width() > 0.0 {
                    let c = 0.5 * (bb.xi + bb.xa);
                    self.ranges[0].exclude(bx.bl.x - c, bx.tr.x - c);
                }
            }
            1 => {
                if bx.bl.x < org.x + bb.xa && bx.tr.x > org.x + bb.xi && bx.height() > 0.0 {
                    let c = 0.5 * (bb.yi + bb.ya);
                    self.ranges[1].exclude(bx.bl.y - c, bx.tr.y - c);
                }
            }
            2 => {
                if bx.bl.x - bx.tr.y < org.x - org.y + sb.da
                    && bx.tr.x - bx.bl.y > org.x - org.y + sb.di
                    && bx.width() > 0.0
                    && bx.height() > 0.0
                {
                    let di = org.x - org.y + sb.di;
                    let da = org.x - org.y + sb.da;
                    let smax = sdm(di, da, bx.tr.x, bx.tr.y, true);
                    let smin = sdm(da, di, bx.bl.x, bx.bl.y, false);
                    let c = 0.5 * (sb.si + sb.sa);
                    self.ranges[2].exclude(smin - c, smax - c);
                }
            }
            3 => {
                if bx.bl.x + bx.bl.y < org.x + org.y + sb.sa
                    && bx.tr.x + bx.tr.y > org.x + org.y + sb.si
                    && bx.width() > 0.0
                    && bx.height() > 0.0
                {
                    let si = org.x + org.y + sb.si;
                    let sa = org.x + org.y + sb.sa;
                    let dmax = sdm(si, sa, bx.tr.x, -bx.bl.y, true);
                    let dmin = sdm(sa, si, bx.bl.x, -bx.tr.y, false);
                    let c = 0.5 * (sb.di + sb.da);
                    self.ranges[3].exclude(dmin - c, dmax - c);
                }
            }
            _ => {}
        }
    }

    /// `mergeSlot`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn merge_slot(
        &mut self,
        seg: &mut Segment,
        slot: u32,
        cslot: &SlotCollision,
        curr_shift: Position,
        is_after: bool,
        same_cluster: bool,
        has_col: &mut bool,
        is_exclusion: bool,
    ) -> bool {
        let mut is_col = false;
        let spos = seg.s(slot).position;
        let sx = spos.x - self.origin.x + curr_shift.x;
        let sy = spos.y - self.origin.y + curr_shift.y;
        let sd = sx - sy;
        let ss = sx + sy;
        let gid = seg.s(slot).glyphid;
        {
            let gc = &seg.face.glyphs;
            if !gc.check(gid) {
                return false;
            }
            let bb = BB::of(gc.bbox(gid));
            let mut order_flags: i32 = 0;
            let same_class = self.seq_prox_class == 0 && cslot.seq_class == self.seq_class;
            if same_cluster
                && self.seq_class != 0
                && (same_class
                    || (self.seq_prox_class != 0 && cslot.seq_class == self.seq_prox_class))
            {
                order_flags = i32::from(self.seq_order);
            }
            if order_flags != 0
                || (sx + bb.xa + self.margin >= self.limit.bl.x
                    && sx + bb.xi - self.margin <= self.limit.tr.x)
                || (sy + bb.ya + self.margin >= self.limit.bl.y
                    && sy + bb.yi - self.margin <= self.limit.tr.y)
            {
                let tx = self.curr_offset.x + self.curr_shift.x;
                let ty = self.curr_offset.y + self.curr_shift.y;
                let td = tx - ty;
                let ts = tx + ty;
                let sb = SB::of(gc.slant(gid));
                let tgid = seg.s(self.target).glyphid;
                let tbb = BB::of(gc.bbox(tgid));
                let tsb = SB::of(gc.slant(tgid));
                let seq_above_wt = f32::from(cslot.seq_above_wt);
                let seq_below_wt = f32::from(cslot.seq_below_wt);
                let seq_valign_wt = f32::from(cslot.seq_valign_wt);
                let valign_ht = f32::from(cslot.seq_valign_ht);
                if is_after {
                    order_flags ^= if same_class { 0x3F } else { 0x3 };
                    order_flags ^= ((order_flags >> 1) & order_flags & 0x15) * 3;
                }
                for i in 0..4usize {
                    let (vmin, vmax, otmin, otmax, omin, omax, cmin, cmax, lmargin);
                    let l = self.limit;
                    let co = self.curr_offset;
                    match i {
                        0 => {
                            vmin = gmax(
                                gmax(bb.xi - tbb.xa + sx, sb.di - tsb.da + ty + sd),
                                sb.si - tsb.sa - ty + ss,
                            );
                            vmax = gmin(
                                gmin(bb.xa - tbb.xi + sx, sb.da - tsb.di + ty + sd),
                                sb.sa - tsb.si - ty + ss,
                            );
                            otmin = tbb.yi + ty;
                            otmax = tbb.ya + ty;
                            omin = bb.yi + sy;
                            omax = bb.ya + sy;
                            let torg = co.x;
                            cmin = l.bl.x + torg;
                            cmax = l.tr.x - tbb.xi + tbb.xa + torg;
                            lmargin = self.margin;
                        }
                        1 => {
                            vmin = gmax(
                                gmax(bb.yi - tbb.ya + sy, tsb.di - sb.da + tx - sd),
                                sb.si - tsb.sa - tx + ss,
                            );
                            vmax = gmin(
                                gmin(bb.ya - tbb.yi + sy, tsb.da - sb.di + tx - sd),
                                sb.sa - tsb.si - tx + ss,
                            );
                            otmin = tbb.xi + tx;
                            otmax = tbb.xa + tx;
                            omin = bb.xi + sx;
                            omax = bb.xa + sx;
                            let torg = co.y;
                            cmin = l.bl.y + torg;
                            cmax = l.tr.y - tbb.yi + tbb.ya + torg;
                            lmargin = self.margin;
                        }
                        2 => {
                            vmin = gmax(
                                gmax(sb.si - tsb.sa + ss, 2.0 * (bb.yi - tbb.ya + sy) + td),
                                2.0 * (bb.xi - tbb.xa + sx) - td,
                            );
                            vmax = gmin(
                                gmin(sb.sa - tsb.si + ss, 2.0 * (bb.ya - tbb.yi + sy) + td),
                                2.0 * (bb.xa - tbb.xi + sx) - td,
                            );
                            otmin = tsb.di + td;
                            otmax = tsb.da + td;
                            omin = sb.di + sd;
                            omax = sb.da + sd;
                            let torg = co.x + co.y;
                            cmin = l.bl.x + l.bl.y + torg;
                            cmax = l.tr.x + l.tr.y - tsb.si + tsb.sa + torg;
                            lmargin = self.margin / ISQRT2;
                        }
                        _ => {
                            vmin = gmax(
                                gmax(sb.di - tsb.da + sd, 2.0 * (bb.xi - tbb.xa + sx) - ts),
                                -2.0 * (bb.ya - tbb.yi + sy) + ts,
                            );
                            vmax = gmin(
                                gmin(sb.da - tsb.di + sd, 2.0 * (bb.xa - tbb.xi + sx) - ts),
                                -2.0 * (bb.yi - tbb.ya + sy) + ts,
                            );
                            otmin = tsb.si + ts;
                            otmax = tsb.sa + ts;
                            omin = sb.si + ss;
                            omax = sb.sa + ss;
                            let torg = co.x - co.y;
                            cmin = l.bl.x - l.tr.y + torg;
                            cmax = l.tr.x - l.bl.y - tsb.di + tsb.da + torg;
                            lmargin = self.margin / ISQRT2;
                        }
                    }
                    if order_flags != 0 {
                        let org = Position::new(tx, ty);
                        let xminf = l.bl.x + co.x + tbb.xi;
                        let xpinf = l.tr.x + co.x + tbb.xa;
                        let ypinf = l.tr.y + co.y + tbb.ya;
                        let yminf = l.bl.y + co.y + tbb.yi;
                        let r = |a: f32, b: f32, c: f32, d: f32| {
                            Rect::new(Position::new(a, b), Position::new(c, d))
                        };
                        match order_flags {
                            SEQ_ORDER_RIGHTUP => {
                                let r1x =
                                    f32::from(cslot.seq_above_xoff) + 0.5 * (bb.xi + bb.xa) + sx;
                                let r3x = f32::from(cslot.seq_below_xlim)
                                    + bb.xa
                                    + sx
                                    + 0.5 * (tbb.xa - tbb.xi);
                                let r2y = 0.5 * (bb.yi + bb.ya) + sy;
                                self.add_box_slope(
                                    true,
                                    &r(xminf, r2y, r1x, ypinf),
                                    tbb,
                                    tsb,
                                    org,
                                    0.0,
                                    seq_above_wt,
                                    true,
                                    i,
                                );
                                self.remove_box(&r(xminf, yminf, r3x, r2y), tbb, tsb, org, i);
                                self.add_box_slope(
                                    true,
                                    &r(r3x, yminf, xpinf, r2y - valign_ht),
                                    tbb,
                                    tsb,
                                    org,
                                    seq_below_wt,
                                    0.0,
                                    true,
                                    i,
                                );
                                self.add_box_slope(
                                    false,
                                    &r(sx + bb.xi, r2y, xpinf, r2y + valign_ht),
                                    tbb,
                                    tsb,
                                    org,
                                    0.0,
                                    seq_valign_wt,
                                    true,
                                    i,
                                );
                                self.add_box_slope(
                                    false,
                                    &r(sx + bb.xi, r2y - valign_ht, xpinf, r2y),
                                    tbb,
                                    tsb,
                                    org,
                                    seq_below_wt,
                                    seq_valign_wt,
                                    false,
                                    i,
                                );
                            }
                            SEQ_ORDER_LEFTDOWN => {
                                let r1x =
                                    0.5 * (bb.xi + bb.xa) + f32::from(cslot.seq_above_xoff) + sx;
                                let r3x = bb.xi - f32::from(cslot.seq_below_xlim) + sx
                                    - 0.5 * (tbb.xa - tbb.xi);
                                let r2y = 0.5 * (bb.yi + bb.ya) + sy;
                                self.add_box_slope(
                                    true,
                                    &r(r1x, yminf, xpinf, r2y),
                                    tbb,
                                    tsb,
                                    org,
                                    0.0,
                                    seq_above_wt,
                                    false,
                                    i,
                                );
                                self.remove_box(&r(r3x, r2y, xpinf, ypinf), tbb, tsb, org, i);
                                self.add_box_slope(
                                    true,
                                    &r(xminf, r2y - valign_ht, r3x, ypinf),
                                    tbb,
                                    tsb,
                                    org,
                                    seq_below_wt,
                                    0.0,
                                    false,
                                    i,
                                );
                                self.add_box_slope(
                                    false,
                                    &r(xminf, r2y, sx + bb.xa, r2y + valign_ht),
                                    tbb,
                                    tsb,
                                    org,
                                    0.0,
                                    seq_valign_wt,
                                    true,
                                    i,
                                );
                                self.add_box_slope(
                                    false,
                                    &r(xminf, r2y - valign_ht, sx + bb.xa, r2y),
                                    tbb,
                                    tsb,
                                    org,
                                    seq_below_wt,
                                    seq_valign_wt,
                                    false,
                                    i,
                                );
                            }
                            SEQ_ORDER_NOABOVE => {
                                self.remove_box(
                                    &r(bb.xi - tbb.xa + sx, sy + bb.ya, bb.xa - tbb.xi + sx, ypinf),
                                    tbb,
                                    tsb,
                                    org,
                                    i,
                                );
                            }
                            SEQ_ORDER_NOBELOW => {
                                self.remove_box(
                                    &r(bb.xi - tbb.xa + sx, yminf, bb.xa - tbb.xi + sx, sy + bb.yi),
                                    tbb,
                                    tsb,
                                    org,
                                    i,
                                );
                            }
                            SEQ_ORDER_NOLEFT => {
                                self.remove_box(
                                    &r(
                                        xminf,
                                        bb.yi - tbb.ya + sy,
                                        bb.xi - tbb.xa + sx,
                                        bb.ya - tbb.yi + sy,
                                    ),
                                    tbb,
                                    tsb,
                                    org,
                                    i,
                                );
                            }
                            SEQ_ORDER_NORIGHT => {
                                self.remove_box(
                                    &r(
                                        bb.xa - tbb.xi + sx,
                                        bb.yi - tbb.ya + sy,
                                        xpinf,
                                        bb.ya - tbb.yi + sy,
                                    ),
                                    tbb,
                                    tsb,
                                    org,
                                    i,
                                );
                            }
                            _ => {}
                        }
                    }
                    if vmax < cmin - lmargin
                        || vmin > cmax + lmargin
                        || omax < otmin - lmargin
                        || omin > otmax + lmargin
                    {
                        continue;
                    }
                    let numsub = gc.num_sub_bounds(gid);
                    let ax = i as i32;
                    if numsub > 0 {
                        let mut anyhits = false;
                        for j in 0..numsub {
                            let sbb = BB::of(gc.sub_bbox(gid, j));
                            let ssb = SB::of(gc.sub_slant(gid, j));
                            let (vmin, vmax, omin, omax);
                            match i {
                                0 => {
                                    vmin = gmax(
                                        gmax(sbb.xi - tbb.xa + sx, ssb.di - tsb.da + sd + ty),
                                        ssb.si - tsb.sa + ss - ty,
                                    );
                                    vmax = gmin(
                                        gmin(sbb.xa - tbb.xi + sx, ssb.da - tsb.di + sd + ty),
                                        ssb.sa - tsb.si + ss - ty,
                                    );
                                    omin = sbb.yi + sy;
                                    omax = sbb.ya + sy;
                                }
                                1 => {
                                    vmin = gmax(
                                        gmax(sbb.yi - tbb.ya + sy, tsb.di - ssb.da - sd + tx),
                                        ssb.si - tsb.sa + ss - tx,
                                    );
                                    vmax = gmin(
                                        gmin(sbb.ya - tbb.yi + sy, tsb.da - ssb.di - sd + tx),
                                        ssb.sa - tsb.si + ss - tx,
                                    );
                                    omin = sbb.xi + sx;
                                    omax = sbb.xa + sx;
                                }
                                2 => {
                                    vmin = gmax(
                                        gmax(
                                            ssb.si - tsb.sa + ss,
                                            2.0 * (sbb.yi - tbb.ya + sy) + td,
                                        ),
                                        2.0 * (sbb.xi - tbb.xa + sx) - td,
                                    );
                                    vmax = gmin(
                                        gmin(
                                            ssb.sa - tsb.si + ss,
                                            2.0 * (sbb.ya - tbb.yi + sy) + td,
                                        ),
                                        2.0 * (sbb.xa - tbb.xi + sx) - td,
                                    );
                                    omin = ssb.di + sd;
                                    omax = ssb.da + sd;
                                }
                                _ => {
                                    vmin = gmax(
                                        gmax(
                                            ssb.di - tsb.da + sd,
                                            2.0 * (sbb.xi - tbb.xa + sx) - ts,
                                        ),
                                        -2.0 * (sbb.ya - tbb.yi + sy) + ts,
                                    );
                                    vmax = gmin(
                                        gmin(
                                            ssb.da - tsb.di + sd,
                                            2.0 * (sbb.xa - tbb.xi + sx) - ts,
                                        ),
                                        -2.0 * (sbb.yi - tbb.ya + sy) + ts,
                                    );
                                    omin = ssb.si + ss;
                                    omax = ssb.sa + ss;
                                }
                            }
                            if vmax < cmin - lmargin
                                || vmin > cmax + lmargin
                                || omax < otmin - lmargin
                                || omin > otmax + lmargin
                            {
                                continue;
                            }
                            self.weigh(ax, vmin, vmax, omin, omax, otmin, otmax, lmargin);
                            anyhits = true;
                        }
                        if anyhits {
                            is_col = true;
                        }
                    } else {
                        is_col = true;
                        self.weigh(ax, vmin, vmax, omin, omax, otmin, otmax, lmargin);
                    }
                }
            }
        }
        let mut res = true;
        let gc = &seg.face.glyphs;
        if cslot.excl_glyph > 0 && gc.check(cslot.excl_glyph) && !is_exclusion {
            let excl = seg.new_slot();
            if excl == NIL {
                return res;
            }
            seg.set_glyph(excl, cslot.excl_glyph);
            let org = seg.s(slot).position + cslot.excl_offset;
            seg.sm(excl).set_origin(org);
            let info = SlotCollision::from_slot(seg, excl).unwrap_or_default();
            res &= self.merge_slot(
                seg,
                excl,
                &info,
                curr_shift,
                is_after,
                same_cluster,
                &mut is_col,
                true,
            );
            seg.free_slot(excl);
        }
        *has_col |= is_col;
        res
    }

    /// The weighting (or exclusion) of an overlap along axis `i`.
    #[allow(clippy::too_many_arguments)]
    fn weigh(
        &mut self,
        i: i32,
        vmin: f32,
        vmax: f32,
        omin: f32,
        omax: f32,
        otmin: f32,
        otmax: f32,
        lmargin: f32,
    ) {
        let r = &mut self.ranges[i as usize];
        if omin > otmax {
            r.weighted_axis(
                i,
                vmin - lmargin,
                vmax + lmargin,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                sqr(lmargin - omin + otmax) * self.margin_wt,
                false,
            );
        } else if omax < otmin {
            r.weighted_axis(
                i,
                vmin - lmargin,
                vmax + lmargin,
                0.0,
                0.0,
                0.0,
                0.0,
                0.0,
                sqr(lmargin - otmin + omax) * self.margin_wt,
                false,
            );
        } else {
            r.exclude_with_margins(vmin, vmax, i);
        }
    }

    /// `resolve`.
    pub(crate) fn resolve(&self, is_col: &mut bool) -> Position {
        let mut total_cost = f32::MAX / 2.0;
        let mut result = Position::default();
        *is_col = true;
        for i in 0..4 {
            let mut best_cost = -1.0f32;
            let tbase = match i {
                0 => self.curr_offset.x,
                1 => self.curr_offset.y,
                2 => self.curr_offset.x + self.curr_offset.y,
                _ => self.curr_offset.x - self.curr_offset.y,
            };
            let best_pos = self.ranges[i].closest(0.0, &mut best_cost) - tbase;
            if best_cost >= 0.0 {
                *is_col = false;
                let cs = self.curr_shift;
                let testp = match i {
                    0 => Position::new(best_pos, cs.y),
                    1 => Position::new(cs.x, best_pos),
                    2 => Position::new(
                        0.5 * (cs.x - cs.y + best_pos),
                        0.5 * (cs.y - cs.x + best_pos),
                    ),
                    _ => Position::new(
                        0.5 * (cs.x + cs.y + best_pos),
                        0.5 * (cs.x + cs.y - best_pos),
                    ),
                };
                if best_cost < total_cost - 0.01 {
                    total_cost = best_cost;
                    result = testp;
                }
            }
        }
        result
    }
}

#[inline]
fn localmax(al: f32, au: f32, bl: f32, bu: f32, x: f32) -> f32 {
    if al < bl {
        if au < bu {
            return if au < x { au } else { x };
        }
    } else if au > bu {
        return if bl < x { bl } else { x };
    }
    x
}

#[inline]
fn localmin(al: f32, au: f32, bl: f32, bu: f32, x: f32) -> f32 {
    if bl > al {
        if bu > au {
            return if bl > x { bl } else { x };
        }
    } else if au > bu {
        return if al > x { al } else { x };
    }
    x
}

/// `get_edge`: the glyph's edge at height `y`, slant boxes included.
fn get_edge(
    seg: &Segment,
    s: u32,
    shift: Position,
    y: f32,
    width: f32,
    margin: f32,
    is_right: bool,
) -> f32 {
    let gc = &seg.face.glyphs;
    let gid = seg.s(s).glyphid;
    let o = seg.s(s).position;
    let sx = o.x + shift.x;
    let sy = o.y + shift.y;
    let numsub = gc.num_sub_bounds(gid);
    let mut res = if is_right { -1e38f32 } else { 1e38f32 };
    if numsub > 0 {
        for i in 0..numsub {
            let sbb = BB::of(gc.sub_bbox(gid, i));
            let ssb = SB::of(gc.sub_slant(gid, i));
            if sy + sbb.yi - margin > y + width / 2.0 || sy + sbb.ya + margin < y - width / 2.0 {
                continue;
            }
            if is_right {
                let mut x = sx + sbb.xa + margin;
                if x > res {
                    let td = sx - sy + ssb.da + margin + y;
                    let ts = sx + sy + ssb.sa + margin - y;
                    x = localmax(
                        td - width / 2.0,
                        td + width / 2.0,
                        ts - width / 2.0,
                        ts + width / 2.0,
                        x,
                    );
                    if x > res {
                        res = x;
                    }
                }
            } else {
                let mut x = sx + sbb.xi - margin;
                if x < res {
                    let td = sx - sy + ssb.di - margin + y;
                    let ts = sx + sy + ssb.si - margin - y;
                    x = localmin(
                        td - width / 2.0,
                        td + width / 2.0,
                        ts - width / 2.0,
                        ts + width / 2.0,
                        x,
                    );
                    if x < res {
                        res = x;
                    }
                }
            }
        }
    } else {
        let bb = BB::of(gc.bbox(gid));
        let sb = SB::of(gc.slant(gid));
        if sy + bb.yi - margin > y + width / 2.0 || sy + bb.ya + margin < y - width / 2.0 {
            return res;
        }
        let td = sx - sy + y;
        let ts = sx + sy - y;
        if is_right {
            res = localmax(
                td + sb.da - width / 2.0,
                td + sb.da + width / 2.0,
                ts + sb.sa - width / 2.0,
                ts + sb.sa + width / 2.0,
                sx + bb.xa,
            ) + margin;
        } else {
            res = localmin(
                td + sb.di - width / 2.0,
                td + sb.di + width / 2.0,
                ts + sb.si - width / 2.0,
                ts + sb.si + width / 2.0,
                sx + bb.xi,
            ) - margin;
        }
    }
    res
}

/// `KernCollider`.
pub(crate) struct KernCollider {
    limit: Rect,
    margin: f32,
    offset_prev: Position,
    miny: f32,
    maxy: f32,
    edges: Vec<f32>,
    slice_width: f32,
    mingap: f32,
    xbound: f32,
    hit: bool,
}

impl KernCollider {
    pub(crate) fn new() -> KernCollider {
        KernCollider {
            limit: Rect::default(),
            margin: 0.0,
            offset_prev: Position::default(),
            miny: -1e38,
            maxy: 1e38,
            edges: Vec::new(),
            slice_width: 0.0,
            mingap: 0.0,
            xbound: 0.0,
            hit: false,
        }
    }

    /// `initSlot`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn init_slot(
        &mut self,
        seg: &Segment,
        a_slot: u32,
        limit: &Rect,
        margin: f32,
        _curr_shift: Position,
        offset_prev: Position,
        dir: i32,
        ymin: f32,
        ymax: f32,
    ) -> bool {
        let gc = &seg.face.glyphs;
        let base = seg.find_root(a_slot);
        let margin = if margin < 10.0 { 10.0 } else { margin };
        self.limit = *limit;
        self.offset_prev = offset_prev;
        let fill = if dir & 1 != 0 { 1e38f32 } else { -1e38f32 };
        if self.maxy >= 1e37 {
            self.slice_width = margin / 1.5;
            self.maxy = ymax + margin;
            self.miny = ymin - margin;
            let n = ((self.maxy - self.miny + 2.0) / (self.slice_width / 1.5) + 1.0) as i32;
            self.edges.clear();
            self.edges.resize(n.max(0) as usize, fill);
            self.xbound = fill;
        } else if self.maxy != ymax || self.miny != ymin {
            if self.miny != ymin {
                let ns = ((ymin - margin - self.miny) / self.slice_width - 1.0) as i32;
                self.miny += ns as f32 * self.slice_width;
                if ns < 0 {
                    for _ in 0..-ns {
                        self.edges.insert(0, fill);
                    }
                } else if (ns as usize) < self.edges.len() {
                    self.edges.drain(..ns as usize);
                }
            }
            if self.maxy != ymax {
                let ns = ((ymax + margin - self.miny) / self.slice_width + 1.0) as i32;
                self.maxy = ns as f32 * self.slice_width + self.miny;
                if ns > self.edges.len() as i32 {
                    self.edges.resize(ns as usize, fill);
                } else if ns < self.edges.len() as i32 {
                    self.edges.truncate(ns.max(0) as usize);
                }
            }
            return self.done(margin);
        }
        let num_slices = self.edges.len() as i32;
        let mut s = base;
        while s != NIL {
            let c = seg.collision(s).copied().unwrap_or_default();
            let gid = seg.s(s).glyphid;
            if !gc.check(gid) {
                return false;
            }
            let bs = BB::of(gc.bbox(gid));
            let o = seg.s(s).position;
            let x = o.x + c.shift.x + if dir & 1 != 0 { bs.xi } else { bs.xa };
            let toffset = c.shift.y - self.miny + 1.0 + o.y;
            let smin = 0i32.max(((bs.yi + toffset) / self.slice_width) as i32);
            let smax = (num_slices - 1).min(((bs.ya + toffset) / self.slice_width + 1.0) as i32);
            for i in smin..=smax {
                let y = self.miny - 1.0 + (i as f32 + 0.5) * self.slice_width;
                let iu = i as usize;
                if dir & 1 != 0 && x < self.edges[iu] {
                    let t = get_edge(seg, s, c.shift, y, self.slice_width, margin, false);
                    if t < self.edges[iu] {
                        self.edges[iu] = t;
                        if t < self.xbound {
                            self.xbound = t;
                        }
                    }
                } else if dir & 1 == 0 && x > self.edges[iu] {
                    let t = get_edge(seg, s, c.shift, y, self.slice_width, margin, true);
                    if t > self.edges[iu] {
                        self.edges[iu] = t;
                        if t > self.xbound {
                            self.xbound = t;
                        }
                    }
                }
            }
            s = seg.next_in_cluster(s);
        }
        self.done(margin)
    }

    fn done(&mut self, margin: f32) -> bool {
        self.mingap = 1e37;
        self.margin = margin;
        true
    }

    /// `mergeSlot`.
    pub(crate) fn merge_slot(
        &mut self,
        seg: &Segment,
        slot: u32,
        curr_shift: Position,
        curr_space: f32,
        dir: i32,
    ) -> bool {
        let rtl = (dir & 1) * 2 - 1;
        let gid = seg.s(slot).glyphid;
        if !seg.face.glyphs.check(gid) {
            return false;
        }
        let bb = seg.face.glyphs.bbox(gid);
        let o = seg.s(slot).position;
        let sx = o.x + curr_shift.x;
        let rtlf = rtl as f32;
        let x = (sx + if rtl > 0 { bb.tr.x } else { bb.bl.x }) * rtlf;
        if self.hit && x < rtlf * (self.xbound - self.mingap - curr_space) {
            return false;
        }
        let sy = o.y + curr_shift.y;
        let smin =
            1i32.max(((bb.bl.y + (1.0 - self.miny + sy)) / self.slice_width + 1.0) as i32) - 1;
        let smax = (self.edges.len() as i32 - 2)
            .min(((bb.tr.y + (1.0 - self.miny + sy)) / self.slice_width + 1.0) as i32)
            + 1;
        if smin > smax {
            return false;
        }
        let mut collides = false;
        let mut nooverlap = true;
        for i in smin..=smax {
            let here = self.edges[i as usize] * rtlf;
            if here > 9e37 {
                continue;
            }
            if !self.hit || x > here - self.mingap - curr_space {
                let y = self.miny - 1.0 + (i as f32 + 0.5) * self.slice_width;
                let m = get_edge(seg, slot, curr_shift, y, self.slice_width, 0.0, rtl > 0) * rtlf
                    + 2.0 * curr_space;
                if m < -8e37 {
                    continue;
                }
                nooverlap = false;
                let t = here - m;
                if t < self.mingap || (!self.hit && !collides) {
                    self.mingap = t;
                    collides = true;
                }
            } else {
                nooverlap = false;
            }
        }
        if nooverlap {
            self.mingap = gmax(
                self.mingap,
                self.xbound - rtlf * (curr_space + self.margin + x),
            );
        }
        if collides && !nooverlap {
            self.hit = true;
        }
        collides | nooverlap
    }

    /// `resolve`.
    pub(crate) fn resolve(&self, dir: i32) -> Position {
        let needed = (1 - 2 * (dir & 1)) as f32 * self.mingap;
        let result = gmin(
            self.limit.tr.x - self.offset_prev.x,
            gmax(needed, self.limit.bl.x - self.offset_prev.x),
        );
        Position::new(result, 0.0)
    }

    /// `shift`.
    pub(crate) fn shift(&mut self, mv: Position, dir: i32) {
        for e in &mut self.edges {
            *e += mv.x;
        }
        self.xbound += (1 - 2 * (dir & 1)) as f32 * mv.x;
    }
}
