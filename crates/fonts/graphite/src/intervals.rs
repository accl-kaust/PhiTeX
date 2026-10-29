// Copyright 2010, SIL International, All rights reserved.
// Ported from graphite2 1.3.15 (src/Intervals.cpp, src/inc/Intervals.h),
// LGPL-2.1-or-later.

//! `Zones`: the possible positions of a glyph along one axis, as
//! intervals each with a quadratic cost.

use alloc::vec::Vec;

/// `max` and `min` as graphite2's templates compare (`a > b ? a : b`).
#[inline]
pub(crate) fn gmax(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

#[inline]
pub(crate) fn gmin(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Exclusion {
    pub x: f32,
    pub xm: f32,
    pub c: f32,
    pub sm: f32,
    pub smx: f32,
    pub open: bool,
}

impl Exclusion {
    fn new(x: f32, xm: f32, smi: f32, smxi: f32, c: f32) -> Exclusion {
        Exclusion {
            x,
            xm,
            c,
            sm: smi,
            smx: smxi,
            open: false,
        }
    }

    /// `weighted<XY>`.
    #[allow(clippy::too_many_arguments)]
    fn weighted_xy(
        xmin: f32,
        xmax: f32,
        f: f32,
        a0: f32,
        m: f32,
        xi: f32,
        _ai: f32,
        c: f32,
        _nega: bool,
    ) -> Exclusion {
        Exclusion::new(xmin, xmax, m + f, m * xi, m * xi * xi + f * a0 * a0 + c)
    }

    /// `weighted<SD>`.
    #[allow(clippy::too_many_arguments)]
    fn weighted_sd(
        xmin: f32,
        xmax: f32,
        f: f32,
        a0: f32,
        m: f32,
        xi: f32,
        ai: f32,
        c: f32,
        nega: bool,
    ) -> Exclusion {
        let xia = if nega { xi - ai } else { xi + ai };
        Exclusion::new(
            xmin,
            xmax,
            0.25 * (m + 2.0 * f),
            0.25 * m * xia,
            0.25 * (m * xia * xia + 2.0 * f * a0 * a0) + c,
        )
    }

    fn split_at(&mut self, p: f32) -> Exclusion {
        let mut r = *self;
        self.x = p;
        r.xm = p;
        r
    }

    fn add(&mut self, rhs: &Exclusion) {
        self.c += rhs.c;
        self.sm += rhs.sm;
        self.smx += rhs.smx;
        self.open = false;
    }

    fn outcode(&self, p: f32) -> u8 {
        let d = 0.0f32;
        (u8::from(p - self.xm >= d) << 1) | u8::from(self.x - p > d)
    }

    fn cost(&self, p: f32) -> f32 {
        (self.sm * p - 2.0 * self.smx) * p + self.c
    }

    fn test_position(&self, origin: f32) -> f32 {
        if self.sm < 0.0 {
            let mut res = self.x;
            let mut cl = self.cost(self.x);
            if self.x < origin && self.xm > origin {
                let co = self.cost(origin);
                if co < cl {
                    cl = co;
                    res = origin;
                }
            }
            let cr = self.cost(self.xm);
            if cl > cr { self.xm } else { res }
        } else {
            let zerox = self.smx / self.sm + origin;
            if zerox < self.x {
                self.x
            } else if zerox > self.xm {
                self.xm
            } else {
                zerox
            }
        }
    }

    fn track_cost(&self, best_cost: &mut f32, best_pos: &mut f32, origin: f32) -> bool {
        let p = self.test_position(origin);
        let localc = self.cost(p - origin);
        if self.open && localc > *best_cost {
            return true;
        }
        if localc < *best_cost {
            *best_cost = localc;
            *best_pos = p;
        }
        false
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Zones {
    pub(crate) exclusions: Vec<Exclusion>,
    margin_len: f32,
    margin_weight: f32,
    pos: f32,
    posm: f32,
}

impl Zones {
    /// `initialise<XY>` (`sd` false) or `initialise<SD>`.
    pub(crate) fn initialise(
        &mut self,
        sd: bool,
        xmin: f32,
        xmax: f32,
        margin_len: f32,
        margin_weight: f32,
        a0: f32,
    ) {
        self.margin_len = margin_len;
        self.margin_weight = margin_weight;
        self.pos = xmin;
        self.posm = xmax;
        self.exclusions.clear();
        let mut e = if sd {
            Exclusion::weighted_sd(xmin, xmax, 1.0, a0, 0.0, 0.0, 0.0, 0.0, false)
        } else {
            Exclusion::weighted_xy(xmin, xmax, 1.0, a0, 0.0, 0.0, 0.0, 0.0, false)
        };
        e.open = true;
        self.exclusions.push(e);
    }

    /// `exclude`.
    pub(crate) fn exclude(&mut self, xmin: f32, xmax: f32) {
        self.remove(xmin, xmax);
    }

    /// `weightedAxis(axis, ...)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn weighted_axis(
        &mut self,
        axis: i32,
        xmin: f32,
        xmax: f32,
        f: f32,
        a0: f32,
        m: f32,
        xi: f32,
        ai: f32,
        c: f32,
        nega: bool,
    ) {
        if axis < 2 {
            self.insert(Exclusion::weighted_xy(
                xmin, xmax, f, a0, m, xi, ai, c, nega,
            ));
        } else {
            self.insert(Exclusion::weighted_sd(
                xmin, xmax, f, a0, m, xi, ai, c, nega,
            ));
        }
    }

    /// `weighted<XY>` (`sd` false) or `weighted<SD>`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn weighted(
        &mut self,
        sd: bool,
        xmin: f32,
        xmax: f32,
        f: f32,
        a0: f32,
        m: f32,
        xi: f32,
        ai: f32,
        c: f32,
        nega: bool,
    ) {
        if sd {
            self.insert(Exclusion::weighted_sd(
                xmin, xmax, f, a0, m, xi, ai, c, nega,
            ));
        } else {
            self.insert(Exclusion::weighted_xy(
                xmin, xmax, f, a0, m, xi, ai, c, nega,
            ));
        }
    }

    /// `exclude_with_margins`.
    pub(crate) fn exclude_with_margins(&mut self, xmin: f32, xmax: f32, axis: i32) {
        self.remove(xmin, xmax);
        let (ml, mw) = (self.margin_len, self.margin_weight);
        self.weighted_axis(
            axis,
            xmin - ml,
            xmin,
            0.0,
            0.0,
            mw,
            xmin - ml,
            0.0,
            0.0,
            false,
        );
        self.weighted_axis(
            axis,
            xmax,
            xmax + ml,
            0.0,
            0.0,
            mw,
            xmax + ml,
            0.0,
            0.0,
            false,
        );
    }

    fn insert(&mut self, mut e: Exclusion) {
        e.x = gmax(e.x, self.pos);
        e.xm = gmin(e.xm, self.posm);
        if e.x >= e.xm {
            return;
        }
        let mut i = 0usize;
        while i < self.exclusions.len() && e.x < e.xm {
            let ex = self.exclusions[i];
            let oca = e.outcode(ex.x);
            let ocb = e.outcode(ex.xm);
            if oca & ocb != 0 {
                i += 1;
                continue;
            }
            match oca ^ ocb {
                0 => {
                    self.exclusions[i].add(&e);
                    e.x = self.exclusions[i].xm;
                }
                1 => {
                    if ex.xm != e.x {
                        if ex.x != e.x {
                            let r = self.exclusions[i].split_at(e.x);
                            self.exclusions.insert(i, r);
                            i += 1;
                        }
                        self.exclusions[i].add(&e);
                        e.x = self.exclusions[i].xm;
                    }
                }
                2 => {
                    if e.xm == ex.x {
                        return;
                    }
                    if e.xm != ex.xm {
                        let r = self.exclusions[i].split_at(e.xm);
                        self.exclusions.insert(i, r);
                    }
                    self.exclusions[i].add(&e);
                    return;
                }
                _ => {
                    if e.xm != ex.xm {
                        let r = self.exclusions[i].split_at(e.xm);
                        self.exclusions.insert(i, r);
                    }
                    let r = self.exclusions[i].split_at(e.x);
                    self.exclusions.insert(i, r);
                    i += 1;
                    self.exclusions[i].add(&e);
                    return;
                }
            }
            i += 1;
        }
    }

    fn remove(&mut self, x: f32, xm: f32) {
        let x = gmax(x, self.pos);
        let xm = gmin(xm, self.posm);
        if x >= xm {
            return;
        }
        let mut i: isize = 0;
        while (i as usize) < self.exclusions.len() {
            let ex = self.exclusions[i as usize];
            let oca = ex.outcode(x);
            let ocb = ex.outcode(xm);
            if oca & ocb != 0 {
                i += 1;
                continue;
            }
            match oca ^ ocb {
                0 => {
                    if ex.x != x {
                        let r = self.exclusions[i as usize].split_at(x);
                        self.exclusions.insert(i as usize, r);
                        i += 1;
                    }
                    self.exclusions[i as usize].x = xm;
                    return;
                }
                1 => {
                    self.exclusions[i as usize].x = xm;
                    return;
                }
                2 => {
                    self.exclusions[i as usize].xm = x;
                    if self.exclusions[i as usize].x != x {
                        i += 1;
                        continue;
                    }
                    self.exclusions.remove(i as usize);
                    i -= 1;
                }
                _ => {
                    self.exclusions.remove(i as usize);
                    i -= 1;
                }
            }
            i += 1;
        }
    }

    fn find_exclusion_under(&self, x: f32) -> usize {
        let (mut l, mut h) = (0usize, self.exclusions.len());
        while l < h {
            let p = (l + h) >> 1;
            match self.exclusions[p].outcode(x) {
                0 => return p,
                1 => h = p,
                _ => l = p + 1,
            }
        }
        l
    }

    /// `closest(origin, cost)`.
    pub(crate) fn closest(&self, origin: f32, cost: &mut f32) -> f32 {
        let mut best_c = f32::MAX;
        let mut best_x = 0.0f32;
        let start = self.find_exclusion_under(origin);
        for e in &self.exclusions[start..] {
            if e.track_cost(&mut best_c, &mut best_x, origin) {
                break;
            }
        }
        for e in self.exclusions[..start].iter().rev() {
            if e.track_cost(&mut best_c, &mut best_x, origin) {
                break;
            }
        }
        *cost = if best_c == f32::MAX { -1.0 } else { best_c };
        best_x
    }
}
