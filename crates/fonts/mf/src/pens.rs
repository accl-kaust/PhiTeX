//! Parts 22 to 26 of `mf.web`: filling a contour, polygonal pens, filling
//! an envelope, elliptical pens, direction and intersection times.

use crate::arith::{ab_vs_cd, floor_scaled, floor_unscaled, half, halfp, odd};
use crate::octants::{crossing_point, octant_dir};
use crate::{Mf, Pointer, R, Scaled, consts::*};

impl Mf {
    // §393
    #[inline]
    pub(crate) fn left_octant(&self, p: Pointer) -> i32 {
        self.left_x(p)
    }
    #[inline]
    pub(crate) fn right_transition(&self, p: Pointer) -> i32 {
        self.right_y(p)
    }
    #[inline]
    pub(crate) fn left_transition(&self, p: Pointer) -> i32 {
        self.left_y(p)
    }
    // §472
    #[inline]
    pub(crate) fn max_offset(&self, p: Pointer) -> Scaled {
        self.mem[(p + 9) as usize].rh
    }
    #[inline]
    pub(crate) fn set_max_offset(&mut self, p: Pointer, v: Scaled) {
        self.mem[(p + 9) as usize].rh = v;
    }
    #[inline]
    fn t_of_the_way(&mut self, a: i32, b: i32, t: i32) -> i32 {
        a - self.take_fraction(a - b, t)
    }

    // §463
    pub(crate) fn end_round(&mut self, x: Scaled, y: Scaled) {
        let o = self.octant as usize;
        let y = y + HALF_UNIT - self.y_corr[o];
        let x = x + y - self.x_corr[o];
        self.m1 = floor_unscaled(x);
        self.n1 = floor_unscaled(y);
        if x - UNITY * self.m1 >= y - UNITY * self.n1 + self.z_corr[o] {
            self.d1 = 1;
        } else {
            self.d1 = 0;
        }
    }

    // §465
    pub(crate) fn fill_spec(&mut self, h: Pointer) -> R {
        if self.int(TRACING_EDGES) > 0 {
            self.begin_edge_tracing();
        }
        let mut p = h;
        loop {
            self.octant = self.left_octant(p);
            // §466
            let mut q = p;
            while self.right_type(q) != ENDPOINT {
                q = self.link(q);
            }
            if q != p {
                // §467
                self.end_round(self.x_coord(p), self.y_coord(p));
                self.m0 = self.m1;
                self.n0 = self.n1;
                self.d0 = self.d1;
                self.end_round(self.x_coord(q), self.y_coord(q));
                // §468
                if self.n1 - self.n0 >= MOVE_SIZE {
                    return self.overflow("move table size", MOVE_SIZE);
                }
                self.move_[0] = self.d0;
                self.move_ptr = 0;
                let mut r = p;
                loop {
                    let s = self.link(r);
                    let o = self.octant as usize;
                    self.make_moves(
                        self.x_coord(r),
                        self.right_x(r),
                        self.left_x(s),
                        self.x_coord(s),
                        self.y_coord(r) + HALF_UNIT,
                        self.right_y(r) + HALF_UNIT,
                        self.left_y(s) + HALF_UNIT,
                        self.y_coord(s) + HALF_UNIT,
                        self.xy_corr[o],
                        self.y_corr[o],
                    )?;
                    r = s;
                    if r == q {
                        break;
                    }
                }
                let mp = self.move_ptr as usize;
                self.move_[mp] -= self.d1;
                if self.int(SMOOTHING) > 0 {
                    self.smooth_moves(0, self.move_ptr);
                }
                self.move_to_edges(self.m0, self.n0, self.m1, self.n1)?;
            }
            p = self.link(q);
            if p == h {
                break;
            }
        }
        self.toss_knot_list(h);
        if self.int(TRACING_EDGES) > 0 {
            self.end_edge_tracing();
        }
        Ok(())
    }

    // §473
    pub(crate) fn print_pen(&mut self, p: Pointer, s: &str, nuline: bool) {
        self.print_diagnostic("Pen polygon", s, nuline);
        let mut nothing_printed = true;
        self.print_ln();
        for k in 1..=8 {
            self.octant = self.octant_code[k];
            let h = p + self.octant;
            let n = self.info(h);
            let mut w = self.link(h);
            if !odd(k as i32) {
                w = self.knil(w);
            }
            for _m in 1..=n + 1 {
                let ww = if odd(k as i32) {
                    self.link(w)
                } else {
                    self.knil(w)
                };
                if self.x_coord(ww) != self.x_coord(w) || self.y_coord(ww) != self.y_coord(w) {
                    // §474
                    if nothing_printed {
                        nothing_printed = false;
                    } else {
                        self.print_nl(" .. ");
                    }
                    self.print_two_true(self.x_coord(ww), self.y_coord(ww), self.octant);
                }
                w = ww;
            }
        }
        if nothing_printed {
            let w = self.link(p + FIRST_OCTANT);
            self.print_two(self.x_coord(w) + self.y_coord(w), self.y_coord(w));
        }
        self.print_nl(" .. cycle");
        self.end_diagnostic(true);
    }

    // §476
    pub(crate) fn dup_offset(&mut self, w: Pointer) -> R {
        let r = self.get_node(COORD_NODE_SIZE)?;
        self.set_x_coord(r, self.x_coord(w));
        self.set_y_coord(r, self.y_coord(w));
        let lw = self.link(w);
        self.set_link(r, lw);
        self.set_knil(lw, r);
        self.set_knil(r, w);
        self.set_link(w, r);
        Ok(())
    }

    // §477
    pub(crate) fn make_pen(&mut self, h: Pointer) -> R<Pointer> {
        let p;
        let mut hh;
        let mut mc;
        'not_found: {
            // §479
            let mut q = h;
            let mut r = self.link(q);
            mc = self.x_coord(h).abs();
            if q == r {
                hh = h;
                self.set_right_type(h, 0);
                if mc < self.y_coord(h).abs() {
                    mc = self.y_coord(h).abs();
                }
            } else {
                let mut o = 0;
                hh = NULL;
                loop {
                    let s = self.link(r);
                    if mc < self.x_coord(r).abs() {
                        mc = self.x_coord(r).abs();
                    }
                    if mc < self.y_coord(r).abs() {
                        mc = self.y_coord(r).abs();
                    }
                    let mut dx = self.x_coord(r) - self.x_coord(q);
                    let mut dy = self.y_coord(r) - self.y_coord(q);
                    if dx == 0 && dy == 0 {
                        break 'not_found;
                    }
                    if ab_vs_cd(
                        dx,
                        self.y_coord(s) - self.y_coord(r),
                        dy,
                        self.x_coord(s) - self.x_coord(r),
                    ) < 0
                    {
                        break 'not_found;
                    }
                    // §480
                    if dx > 0 {
                        self.octant = FIRST_OCTANT;
                    } else if dx == 0 {
                        if dy > 0 {
                            self.octant = FIRST_OCTANT;
                        } else {
                            self.octant = FIRST_OCTANT + NEGATE_X;
                        }
                    } else {
                        dx = -dx;
                        self.octant = FIRST_OCTANT + NEGATE_X;
                    }
                    if dy < 0 {
                        dy = -dy;
                        self.octant += NEGATE_Y;
                    } else if dy == 0 {
                        if self.octant > FIRST_OCTANT {
                            self.octant = FIRST_OCTANT + NEGATE_X + NEGATE_Y;
                        }
                    }
                    if dx < dy {
                        self.octant += SWITCH_X_AND_Y;
                    }
                    self.set_right_type(q, self.octant);
                    let oo = self.octant_number[self.octant as usize];
                    if o > oo {
                        if hh != NULL {
                            break 'not_found;
                        }
                        hh = q;
                    }
                    o = oo;
                    if q == h && hh != NULL {
                        break;
                    }
                    q = r;
                    r = s;
                }
            }
            if mc >= FRACTION_ONE - HALF_UNIT {
                break 'not_found;
            }
            p = self.get_node(PEN_NODE_SIZE)?;
            let mut q = hh;
            self.set_max_offset(p, mc);
            self.set_ref_count(p, NULL);
            if self.link(q) != q {
                self.set_link(p, NULL + 1);
            }
            for k in 1..=8i32 {
                // §481
                self.octant = self.octant_code[k as usize];
                let mut n = 0;
                let h = p + self.octant;
                let mut w = NULL;
                loop {
                    let r = self.get_node(COORD_NODE_SIZE)?;
                    self.skew(self.x_coord(q), self.y_coord(q), self.octant);
                    self.set_x_coord(r, self.cur_x);
                    self.set_y_coord(r, self.cur_y);
                    if n == 0 {
                        self.set_link(h, r);
                    } else {
                        // §482
                        if odd(k) {
                            self.set_link(w, r);
                            self.set_knil(r, w);
                        } else {
                            self.set_knil(w, r);
                            self.set_link(r, w);
                        }
                    }
                    w = r;
                    if self.right_type(q) != self.octant {
                        break;
                    }
                    q = self.link(q);
                    n += 1;
                }
                // §483
                let mut r = self.link(h);
                if odd(k) {
                    self.set_link(w, r);
                    self.set_knil(r, w);
                } else {
                    self.set_knil(w, r);
                    self.set_link(r, w);
                    self.set_link(h, w);
                    r = w;
                }
                if self.y_coord(r) != self.y_coord(self.link(r)) || n == 0 {
                    self.dup_offset(r)?;
                    n += 1;
                }
                r = self.knil(r);
                if self.x_coord(r) != self.x_coord(self.knil(r)) {
                    self.dup_offset(r)?;
                } else {
                    n -= 1;
                }
                if n >= MAX_QUARTERWORD {
                    return self.overflow("pen polygon size", MAX_QUARTERWORD);
                }
                self.set_info(h, n);
            }
            return self.make_pen_found(p);
        }
        p = NULL_PEN;
        // §478
        if mc >= FRACTION_ONE - HALF_UNIT {
            self.print_err("Pen too large");
            self.help(&[
                "The cycle you specified has a coordinate of 4095.5 or more.",
                "So I've replaced it by the trivial path `(0,0)..cycle'.",
            ]);
        } else {
            self.print_err("Pen cycle must be convex");
            self.help(&[
                "The cycle you specified either has consecutive equal points",
                "or turns right or turns through more than 360 degrees.",
                "So I've replaced it by the trivial path `(0,0)..cycle'.",
            ]);
        }
        self.put_get_error()?;
        self.make_pen_found(p)
    }
    fn make_pen_found(&mut self, p: Pointer) -> R<Pointer> {
        if self.int(TRACING_PENS) > 0 {
            self.print_pen(p, " (newly created)", true);
        }
        Ok(p)
    }

    // §484
    pub(crate) fn make_path(&mut self, pen_head: Pointer) -> R<Pointer> {
        let mut p = self.temp_head();
        for k in 1..=8i32 {
            self.octant = self.octant_code[k as usize];
            let h = pen_head + self.octant;
            let n = self.info(h);
            let mut w = self.link(h);
            if !odd(k) {
                w = self.knil(w);
            }
            for _m in 1..=n + 1 {
                let ww = if odd(k) { self.link(w) } else { self.knil(w) };
                if self.x_coord(ww) != self.x_coord(w) || self.y_coord(ww) != self.y_coord(w) {
                    // §485
                    self.unskew(self.x_coord(ww), self.y_coord(ww), self.octant);
                    let t = self.trivial_knot(self.cur_x, self.cur_y)?;
                    self.set_link(p, t);
                    p = t;
                }
                w = ww;
            }
        }
        let th = self.temp_head();
        if p == th {
            let w = self.link(pen_head + FIRST_OCTANT);
            p = self.trivial_knot(self.x_coord(w) + self.y_coord(w), self.y_coord(w))?;
            self.set_link(th, p);
        }
        self.set_link(p, self.link(th));
        Ok(self.link(th))
    }

    // §486
    pub(crate) fn trivial_knot(&mut self, x: Scaled, y: Scaled) -> R<Pointer> {
        let p = self.get_node(KNOT_NODE_SIZE)?;
        self.set_left_type(p, EXPLICIT);
        self.set_right_type(p, EXPLICIT);
        self.set_x_coord(p, x);
        self.set_left_x(p, x);
        self.set_right_x(p, x);
        self.set_y_coord(p, y);
        self.set_left_y(p, y);
        self.set_right_y(p, y);
        Ok(p)
    }

    // §487
    pub(crate) fn add_pen_ref(&mut self, p: Pointer) {
        let v = self.ref_count(p) + 1;
        self.set_ref_count(p, v);
    }
    pub(crate) fn delete_pen_ref(&mut self, p: Pointer) {
        if self.ref_count(p) == NULL {
            self.toss_pen(p);
        } else {
            let v = self.ref_count(p) - 1;
            self.set_ref_count(p, v);
        }
    }
    pub(crate) fn toss_pen(&mut self, p: Pointer) {
        if p != NULL_PEN {
            for k in 1..=8 {
                let mut w = self.link(p + k);
                loop {
                    let ww = self.link(w);
                    self.free_node(w, COORD_NODE_SIZE);
                    w = ww;
                    if w == self.link(p + k) {
                        break;
                    }
                }
            }
            self.free_node(p, PEN_NODE_SIZE);
        }
    }

    // §488
    pub(crate) fn find_offset(&mut self, mut x: Scaled, mut y: Scaled, p: Pointer) {
        // §489
        let mut octant;
        if x > 0 {
            octant = FIRST_OCTANT;
        } else if x == 0 {
            if y <= 0 {
                if y == 0 {
                    self.cur_x = 0;
                    self.cur_y = 0;
                    return;
                } else {
                    octant = FIRST_OCTANT + NEGATE_X;
                }
            } else {
                octant = FIRST_OCTANT;
            }
        } else {
            x = -x;
            if y == 0 {
                octant = FIRST_OCTANT + NEGATE_X + NEGATE_Y;
            } else {
                octant = FIRST_OCTANT + NEGATE_X;
            }
        }
        if y < 0 {
            octant += NEGATE_Y;
            y = -y;
        }
        if x >= y {
            x -= y;
        } else {
            octant += SWITCH_X_AND_Y;
            x = y - x;
            y -= x;
        }
        let s = if odd(self.octant_number[octant as usize]) {
            -1
        } else {
            1
        };
        let h = p + octant;
        let mut w = self.link(self.link(h));
        let mut ww = self.link(w);
        let mut n = self.info(h);
        while n > 1 {
            if ab_vs_cd(
                x,
                self.y_coord(ww) - self.y_coord(w),
                y,
                self.x_coord(ww) - self.x_coord(w),
            ) != s
            {
                break;
            }
            w = ww;
            ww = self.link(w);
            n -= 1;
        }
        self.unskew(self.x_coord(w), self.y_coord(w), octant);
    }

    // §491
    pub(crate) fn offset_prep(&mut self, c: Pointer, h: Pointer) -> R {
        let mut p = c;
        let n = self.info(h);
        let lh = self.link(h);
        while self.right_type(p) != ENDPOINT {
            let mut q = self.link(p);
            // §494
            if n <= 1 {
                self.set_right_type(p, 1);
            } else {
                'not_found: {
                    // §496
                    let mut x0 = self.right_x(p) - self.x_coord(p);
                    let mut x2 = self.x_coord(q) - self.left_x(q);
                    let mut x1 = self.left_x(q) - self.right_x(p);
                    let mut y0 = self.right_y(p) - self.y_coord(p);
                    let mut y2 = self.y_coord(q) - self.left_y(q);
                    let mut y1 = self.left_y(q) - self.right_y(p);
                    let mut max_coef = x0.abs();
                    for v in [x1, x2, y0, y1, y2] {
                        if v.abs() > max_coef {
                            max_coef = v.abs();
                        }
                    }
                    if max_coef == 0 {
                        break 'not_found;
                    }
                    while max_coef < FRACTION_HALF {
                        max_coef += max_coef;
                        x0 += x0;
                        x1 += x1;
                        x2 += x2;
                        y0 += y0;
                        y1 += y1;
                        y2 += y2;
                    }
                    // §501
                    let mut dx = x0;
                    let mut dy = y0;
                    if dx == 0 && dy == 0 {
                        dx = x1;
                        dy = y1;
                        if dx == 0 && dy == 0 {
                            dx = x2;
                            dy = y2;
                        }
                    }
                    if dx == 0 {
                        // §505
                        let w = self.knil(self.knil(lh));
                        self.fin_offset_prep(p, n, w, -x0, -x1, -x2, -y0, -y1, -y2, false, n)?;
                    } else {
                        // §502
                        let mut k = 1;
                        let mut w = self.link(lh);
                        loop {
                            if k == n {
                                break;
                            }
                            let ww = self.link(w);
                            if ab_vs_cd(
                                dy,
                                (self.x_coord(ww) - self.x_coord(w)).abs(),
                                dx,
                                (self.y_coord(ww) - self.y_coord(w)).abs(),
                            ) >= 0
                            {
                                k += 1;
                                w = ww;
                            } else {
                                break;
                            }
                        }
                        // §503
                        let t;
                        let mut ww = NULL;
                        let (mut t0, mut t1, mut t2) = (0, 0, 0);
                        if k == 1 {
                            t = FRACTION_ONE + 1;
                        } else {
                            ww = self.knil(w);
                            (t0, t1, t2) = self.test_coefs(w, ww, x0, x1, x2, y0, y1, y2);
                            t = crossing_point(-t0, -t1, -t2);
                        }
                        let _ = t0;
                        if t >= FRACTION_ONE {
                            self.fin_offset_prep(p, k, w, x0, x1, x2, y0, y1, y2, true, n)?;
                        } else {
                            self.split_for_offset(p, t)?;
                            let r = self.link(p);
                            let x1a = self.t_of_the_way(x0, x1, t);
                            x1 = self.t_of_the_way(x1, x2, t);
                            let x2a = self.t_of_the_way(x1a, x1, t);
                            let y1a = self.t_of_the_way(y0, y1, t);
                            y1 = self.t_of_the_way(y1, y2, t);
                            let y2a = self.t_of_the_way(y1a, y1, t);
                            self.fin_offset_prep(p, k, w, x0, x1a, x2a, y0, y1a, y2a, true, n)?;
                            x0 = x2a;
                            y0 = y2a;
                            t1 = self.t_of_the_way(t1, t2, t);
                            if t1 < 0 {
                                t1 = 0;
                            }
                            let t = crossing_point(0, t1, t2);
                            if t < FRACTION_ONE {
                                // §504
                                self.split_for_offset(r, t)?;
                                let x1a = self.t_of_the_way(x1, x2, t);
                                x1 = self.t_of_the_way(x0, x1, t);
                                let x0a = self.t_of_the_way(x1, x1a, t);
                                let y1a = self.t_of_the_way(y1, y2, t);
                                y1 = self.t_of_the_way(y0, y1, t);
                                let y0a = self.t_of_the_way(y1, y1a, t);
                                let lr = self.link(r);
                                self.fin_offset_prep(
                                    lr, k, w, x0a, x1a, x2, y0a, y1a, y2, true, n,
                                )?;
                                x2 = x0a;
                                y2 = y0a;
                            }
                            self.fin_offset_prep(
                                r,
                                k - 1,
                                ww,
                                -x0,
                                -x1,
                                -x2,
                                -y0,
                                -y1,
                                -y2,
                                false,
                                n,
                            )?;
                        }
                    }
                }
            }
            // §492
            loop {
                let mut r = self.link(p);
                if self.x_coord(p) == self.right_x(p)
                    && self.y_coord(p) == self.right_y(p)
                    && self.x_coord(p) == self.left_x(r)
                    && self.y_coord(p) == self.left_y(r)
                    && self.x_coord(p) == self.x_coord(r)
                    && self.y_coord(p) == self.y_coord(r)
                {
                    self.remove_cubic(p);
                    if r == q {
                        q = p;
                    }
                    r = p;
                }
                p = r;
                if p == q {
                    break;
                }
            }
        }
        Ok(())
    }

    // §498
    fn test_coefs(
        &mut self,
        w: Pointer,
        ww: Pointer,
        x0: i32,
        x1: i32,
        x2: i32,
        y0: i32,
        y1: i32,
        y2: i32,
    ) -> (i32, i32, i32) {
        let du = self.x_coord(ww) - self.x_coord(w);
        let dv = self.y_coord(ww) - self.y_coord(w);
        if du.abs() >= dv.abs() {
            let s = self.make_fraction(dv, du);
            (
                self.take_fraction(x0, s) - y0,
                self.take_fraction(x1, s) - y1,
                self.take_fraction(x2, s) - y2,
            )
        } else {
            let s = self.make_fraction(du, dv);
            (
                x0 - self.take_fraction(y0, s),
                x1 - self.take_fraction(y1, s),
                x2 - self.take_fraction(y2, s),
            )
        }
    }

    // §493
    pub(crate) fn split_for_offset(&mut self, p: Pointer, t: i32) -> R {
        let q = self.link(p);
        self.split_cubic(p, t, self.x_coord(q), self.y_coord(q))?;
        let r = self.link(p);
        if self.y_coord(r) < self.y_coord(p) {
            self.set_y_coord(r, self.y_coord(p));
        } else if self.y_coord(r) > self.y_coord(q) {
            self.set_y_coord(r, self.y_coord(q));
        }
        if self.x_coord(r) < self.x_coord(p) {
            self.set_x_coord(r, self.x_coord(p));
        } else if self.x_coord(r) > self.x_coord(q) {
            self.set_x_coord(r, self.x_coord(q));
        }
        Ok(())
    }

    // §497
    pub(crate) fn fin_offset_prep(
        &mut self,
        mut p: Pointer,
        mut k: i32,
        mut w: Pointer,
        mut x0: i32,
        mut x1: i32,
        mut x2: i32,
        mut y0: i32,
        mut y1: i32,
        mut y2: i32,
        rising: bool,
        n: i32,
    ) -> R {
        loop {
            self.set_right_type(p, k);
            let ww;
            if rising {
                if k == n {
                    return Ok(());
                }
                ww = self.link(w);
            } else {
                if k == 1 {
                    return Ok(());
                }
                ww = self.knil(w);
            }
            let (_t0, mut t1, t2) = {
                let (a, b, c) = self.test_coefs(w, ww, x0, x1, x2, y0, y1, y2);
                (a, b, c)
            };
            let t = crossing_point(_t0, t1, t2);
            if t >= FRACTION_ONE {
                return Ok(());
            }
            // §499
            self.split_for_offset(p, t)?;
            self.set_right_type(p, k);
            p = self.link(p);
            let v = self.t_of_the_way(x0, x1, t);
            x1 = self.t_of_the_way(x1, x2, t);
            x0 = self.t_of_the_way(v, x1, t);
            let v = self.t_of_the_way(y0, y1, t);
            y1 = self.t_of_the_way(y1, y2, t);
            y0 = self.t_of_the_way(v, y1, t);
            t1 = self.t_of_the_way(t1, t2, t);
            if t1 > 0 {
                t1 = 0;
            }
            let t = crossing_point(0, -t1, -t2);
            if t < FRACTION_ONE {
                self.split_for_offset(p, t)?;
                let lp = self.link(p);
                self.set_right_type(lp, k);
                let v = self.t_of_the_way(x1, x2, t);
                x1 = self.t_of_the_way(x0, x1, t);
                x2 = self.t_of_the_way(x1, v, t);
                let v = self.t_of_the_way(y1, y2, t);
                y1 = self.t_of_the_way(y0, y1, t);
                y2 = self.t_of_the_way(y1, v, t);
            }
            if rising {
                k += 1;
            } else {
                k -= 1;
            }
            w = ww;
        }
    }

    // §506
    pub(crate) fn fill_envelope(&mut self, spec_head: Pointer) -> R {
        if self.int(TRACING_EDGES) > 0 {
            self.begin_edge_tracing();
        }
        let mut p = spec_head;
        loop {
            self.octant = self.left_octant(p);
            let h = self.cur_pen + self.octant;
            // §466
            let mut q = p;
            while self.right_type(q) != ENDPOINT {
                q = self.link(q);
            }
            // §508
            let mut w = self.link(h);
            if self.left_transition(p) == DIAGONAL {
                w = self.knil(w);
            }
            if self.int(TRACING_EDGES) > UNITY {
                // §509
                self.print_nl("@ Octant ");
                self.prints(octant_dir(self.octant));
                self.prints(" (");
                self.print_int(self.info(h));
                if self.info(h) != 1 {
                    self.prints(" offsets");
                } else {
                    self.prints(" offset");
                }
                self.prints("), from ");
                self.print_two_true(
                    self.x_coord(p) + self.x_coord(w),
                    self.y_coord(p) + self.y_coord(w),
                    self.octant,
                );
                let mut ww = self.link(h);
                if self.right_transition(q) == DIAGONAL {
                    ww = self.knil(ww);
                }
                self.prints(" to ");
                self.print_two_true(
                    self.x_coord(q) + self.x_coord(ww),
                    self.y_coord(q) + self.y_coord(ww),
                    self.octant,
                );
            }
            let mut ww = self.link(h);
            let mut www = ww;
            if odd(self.octant_number[self.octant as usize]) {
                www = self.knil(www);
            } else {
                ww = self.knil(ww);
            }
            if w != ww {
                self.skew_line_edges(p, w, ww)?;
            }
            self.end_round(
                self.x_coord(p) + self.x_coord(ww),
                self.y_coord(p) + self.y_coord(ww),
            );
            self.m0 = self.m1;
            self.n0 = self.n1;
            self.d0 = self.d1;
            self.end_round(
                self.x_coord(q) + self.x_coord(www),
                self.y_coord(q) + self.y_coord(www),
            );
            if self.n1 - self.n0 >= MOVE_SIZE {
                return self.overflow("move table size", MOVE_SIZE);
            }
            self.offset_prep(p, h)?;
            // §466
            q = p;
            while self.right_type(q) != ENDPOINT {
                q = self.link(q);
            }
            // §512
            if odd(self.octant_number[self.octant as usize]) {
                // §513
                let o = self.octant as usize;
                let mut k = 0;
                let mut w = self.link(h);
                let ww = self.knil(w);
                let mm0 = floor_unscaled(self.x_coord(p) + self.x_coord(w) - self.xy_corr[o]);
                let mm1 = floor_unscaled(self.x_coord(q) + self.x_coord(ww) - self.xy_corr[o]);
                for n in 0..self.n1 - self.n0 {
                    self.env_move[n as usize] = mm0;
                }
                self.env_move[(self.n1 - self.n0) as usize] = mm1;
                self.move_ptr = 0;
                let mut m = mm0;
                let mut r = p;
                self.set_right_type(q, self.info(h) + 1);
                let mut smooth_bot = 0;
                let mut smooth_top = 0;
                loop {
                    if r == q {
                        smooth_top = self.move_ptr;
                    }
                    while self.right_type(r) != k {
                        // §515
                        let xx = self.x_coord(r) + self.x_coord(w);
                        let yy = self.y_coord(r) + self.y_coord(w) + HALF_UNIT;
                        if self.int(TRACING_EDGES) > UNITY {
                            self.print_nl("@ transition line ");
                            self.print_int(k);
                            self.prints(", from ");
                            self.print_two_true(xx, yy - HALF_UNIT, self.octant);
                        }
                        let (xp, yp);
                        if self.right_type(r) > k {
                            k += 1;
                            w = self.link(w);
                            xp = self.x_coord(r) + self.x_coord(w);
                            yp = self.y_coord(r) + self.y_coord(w) + HALF_UNIT;
                            if yp != yy {
                                // §516
                                let mut ty = floor_scaled(yy - self.y_corr[o]);
                                let dely = yp - yy;
                                let mut yy = yy - ty;
                                ty = yp - self.y_corr[o] - ty;
                                if ty >= UNITY {
                                    let delx = xp - xx;
                                    yy = UNITY - yy;
                                    loop {
                                        let f = self.make_fraction(yy, dely);
                                        let mut tx = self.take_fraction(delx, f);
                                        if ab_vs_cd(tx, dely, delx, yy) + self.xy_corr[o] > 0 {
                                            tx -= 1;
                                        }
                                        m = floor_unscaled(xx + tx);
                                        let mp = self.move_ptr as usize;
                                        if m > self.env_move[mp] {
                                            self.env_move[mp] = m;
                                        }
                                        ty -= UNITY;
                                        if ty < UNITY {
                                            break;
                                        }
                                        yy += UNITY;
                                        self.move_ptr += 1;
                                    }
                                }
                            }
                        } else {
                            k -= 1;
                            w = self.knil(w);
                            xp = self.x_coord(r) + self.x_coord(w);
                            yp = self.y_coord(r) + self.y_coord(w) + HALF_UNIT;
                        }
                        if self.int(TRACING_EDGES) > UNITY {
                            self.prints(" to ");
                            self.print_two_true(xp, yp - HALF_UNIT, self.octant);
                            self.print_nl("");
                        }
                        m = floor_unscaled(xp - self.xy_corr[o]);
                        self.move_ptr = floor_unscaled(yp - self.y_corr[o]) - self.n0;
                        let mp = self.move_ptr as usize;
                        if m > self.env_move[mp] {
                            self.env_move[mp] = m;
                        }
                    }
                    if r == p {
                        smooth_bot = self.move_ptr;
                    }
                    if r == q {
                        break;
                    }
                    let mp = self.move_ptr as usize;
                    self.move_[mp] = 1;
                    let mut n = self.move_ptr;
                    let s = self.link(r);
                    let xw = self.x_coord(w);
                    let yw = self.y_coord(w);
                    self.make_moves(
                        self.x_coord(r) + xw,
                        self.right_x(r) + xw,
                        self.left_x(s) + xw,
                        self.x_coord(s) + xw,
                        self.y_coord(r) + yw + HALF_UNIT,
                        self.right_y(r) + yw + HALF_UNIT,
                        self.left_y(s) + yw + HALF_UNIT,
                        self.y_coord(s) + yw + HALF_UNIT,
                        self.xy_corr[o],
                        self.y_corr[o],
                    )?;
                    // §514
                    loop {
                        m = m + self.move_[n as usize] - 1;
                        if m > self.env_move[n as usize] {
                            self.env_move[n as usize] = m;
                        }
                        n += 1;
                        if n > self.move_ptr {
                            break;
                        }
                    }
                    r = s;
                }
                // §517
                self.move_[0] = self.d0 + self.env_move[0] - mm0;
                for n in 1..=self.move_ptr as usize {
                    self.move_[n] = self.env_move[n] - self.env_move[n - 1] + 1;
                }
                let mp = self.move_ptr as usize;
                self.move_[mp] -= self.d1;
                if self.int(SMOOTHING) > 0 {
                    self.smooth_moves(smooth_bot, smooth_top);
                }
                self.move_to_edges(self.m0, self.n0, self.m1, self.n1)?;
                if self.right_transition(q) == AXIS {
                    let w = self.link(h);
                    let kw = self.knil(w);
                    self.skew_line_edges(q, kw, w)?;
                }
            } else {
                self.dual_moves(h, p, q)?;
            }
            self.set_right_type(q, ENDPOINT);
            p = self.link(q);
            if p == spec_head {
                break;
            }
        }
        if self.int(TRACING_EDGES) > 0 {
            self.end_edge_tracing();
        }
        self.toss_knot_list(spec_head);
        Ok(())
    }

    // §510
    pub(crate) fn skew_line_edges(&mut self, p: Pointer, w: Pointer, ww: Pointer) -> R {
        if self.x_coord(w) != self.x_coord(ww) || self.y_coord(w) != self.y_coord(ww) {
            let x0 = self.x_coord(p) + self.x_coord(w);
            let y0 = self.y_coord(p) + self.y_coord(w);
            let x1 = self.x_coord(p) + self.x_coord(ww);
            let y1 = self.y_coord(p) + self.y_coord(ww);
            self.unskew(x0, y0, self.octant);
            let x0 = self.cur_x;
            let y0 = self.cur_y;
            self.unskew(x1, y1, self.octant);
            if self.int(TRACING_EDGES) > UNITY {
                self.print_nl("@ retrograde line from ");
                self.print_two(x0, y0);
                self.prints(" to ");
                self.print_two(self.cur_x, self.cur_y);
                self.print_nl("");
            }
            self.line_edges(x0, y0, self.cur_x, self.cur_y)?;
        }
        Ok(())
    }

    // §518
    pub(crate) fn dual_moves(&mut self, h: Pointer, p: Pointer, q: Pointer) -> R {
        // §519
        let o = self.octant as usize;
        let mut k = self.info(h) + 1;
        let ww = self.link(h);
        let mut w = self.knil(ww);
        let mm0 = floor_unscaled(self.x_coord(p) + self.x_coord(w) - self.xy_corr[o]);
        let mm1 = floor_unscaled(self.x_coord(q) + self.x_coord(ww) - self.xy_corr[o]);
        for n in 1..=self.n1 - self.n0 + 1 {
            self.env_move[n as usize] = mm1;
        }
        self.env_move[0] = mm0;
        self.move_ptr = 0;
        let mut m = mm0;
        let mut r = p;
        let mut smooth_bot = 0;
        let mut smooth_top = 0;
        loop {
            if r == q {
                smooth_top = self.move_ptr;
            }
            while self.right_type(r) != k {
                // §521
                let xx = self.x_coord(r) + self.x_coord(w);
                let yy = self.y_coord(r) + self.y_coord(w) + HALF_UNIT;
                if self.int(TRACING_EDGES) > UNITY {
                    self.print_nl("@ transition line ");
                    self.print_int(k);
                    self.prints(", from ");
                    self.print_two_true(xx, yy - HALF_UNIT, self.octant);
                }
                let (xp, yp);
                if self.right_type(r) < k {
                    k -= 1;
                    w = self.knil(w);
                    xp = self.x_coord(r) + self.x_coord(w);
                    yp = self.y_coord(r) + self.y_coord(w) + HALF_UNIT;
                    if yp != yy {
                        // §522
                        let mut ty = floor_scaled(yy - self.y_corr[o]);
                        let dely = yp - yy;
                        let mut yy = yy - ty;
                        ty = yp - self.y_corr[o] - ty;
                        if ty >= UNITY {
                            let delx = xp - xx;
                            yy = UNITY - yy;
                            loop {
                                let mp = self.move_ptr as usize;
                                if m < self.env_move[mp] {
                                    self.env_move[mp] = m;
                                }
                                let f = self.make_fraction(yy, dely);
                                let mut tx = self.take_fraction(delx, f);
                                if ab_vs_cd(tx, dely, delx, yy) + self.xy_corr[o] > 0 {
                                    tx -= 1;
                                }
                                m = floor_unscaled(xx + tx);
                                ty -= UNITY;
                                self.move_ptr += 1;
                                if ty < UNITY {
                                    break;
                                }
                                yy += UNITY;
                            }
                            let mp = self.move_ptr as usize;
                            if m < self.env_move[mp] {
                                self.env_move[mp] = m;
                            }
                        }
                    }
                } else {
                    k += 1;
                    w = self.link(w);
                    xp = self.x_coord(r) + self.x_coord(w);
                    yp = self.y_coord(r) + self.y_coord(w) + HALF_UNIT;
                }
                if self.int(TRACING_EDGES) > UNITY {
                    self.prints(" to ");
                    self.print_two_true(xp, yp - HALF_UNIT, self.octant);
                    self.print_nl("");
                }
                m = floor_unscaled(xp - self.xy_corr[o]);
                self.move_ptr = floor_unscaled(yp - self.y_corr[o]) - self.n0;
                let mp = self.move_ptr as usize;
                if m < self.env_move[mp] {
                    self.env_move[mp] = m;
                }
            }
            if r == p {
                smooth_bot = self.move_ptr;
            }
            if r == q {
                break;
            }
            let mp = self.move_ptr as usize;
            self.move_[mp] = 1;
            let mut n = self.move_ptr;
            let s = self.link(r);
            let xw = self.x_coord(w);
            let yw = self.y_coord(w);
            self.make_moves(
                self.x_coord(r) + xw,
                self.right_x(r) + xw,
                self.left_x(s) + xw,
                self.x_coord(s) + xw,
                self.y_coord(r) + yw + HALF_UNIT,
                self.right_y(r) + yw + HALF_UNIT,
                self.left_y(s) + yw + HALF_UNIT,
                self.y_coord(s) + yw + HALF_UNIT,
                self.xy_corr[o],
                self.y_corr[o],
            )?;
            // §520
            loop {
                if m < self.env_move[n as usize] {
                    self.env_move[n as usize] = m;
                }
                m = m + self.move_[n as usize] - 1;
                n += 1;
                if n > self.move_ptr {
                    break;
                }
            }
            r = s;
        }
        // §523
        self.move_[0] = self.d0 + self.env_move[1] - mm0;
        for n in 1..=self.move_ptr as usize {
            self.move_[n] = self.env_move[n + 1] - self.env_move[n] + 1;
        }
        let mp = self.move_ptr as usize;
        self.move_[mp] -= self.d1;
        if self.int(SMOOTHING) > 0 {
            self.smooth_moves(smooth_bot, smooth_top);
        }
        self.move_to_edges(self.m0, self.n0, self.m1, self.n1)?;
        if self.right_transition(q) == DIAGONAL {
            let w = self.link(h);
            let kw = self.knil(w);
            self.skew_line_edges(q, w, kw)?;
        }
        Ok(())
    }

    // §527: right_u = right_x, left_v = left_x, right_class = right_y,
    // left_length = left_y.
    // §524
    pub(crate) fn make_ellipse(
        &mut self,
        major_axis: Scaled,
        minor_axis: Scaled,
        mut theta: i32,
    ) -> R<Pointer> {
        let symmetric;
        let (mut alpha, mut beta, mut gamma, mut delta);
        // §529
        if major_axis == minor_axis || theta % NINETY_DEG == 0 {
            symmetric = true;
            alpha = 0;
            if odd(theta / NINETY_DEG) {
                beta = major_axis;
                gamma = minor_axis;
                self.n_sin = FRACTION_ONE;
                self.n_cos = 0;
            } else {
                beta = minor_axis;
                gamma = major_axis;
                theta = 0;
            }
        } else {
            symmetric = false;
            self.n_sin_cos(theta);
            gamma = self.take_fraction(major_axis, self.n_sin);
            delta = self.take_fraction(minor_axis, self.n_cos);
            beta = self.pyth_add(gamma, delta);
            let f1 = self.make_fraction(gamma, beta);
            let a1 = self.take_fraction(major_axis, f1);
            let a1 = self.take_fraction(a1, self.n_cos);
            let f2 = self.make_fraction(delta, beta);
            let a2 = self.take_fraction(minor_axis, f2);
            let a2 = self.take_fraction(a2, self.n_sin);
            alpha = a1 - a2;
            alpha = (alpha + HALF_UNIT) / UNITY;
            let g1 = self.take_fraction(major_axis, self.n_cos);
            let g2 = self.take_fraction(minor_axis, self.n_sin);
            gamma = self.pyth_add(g1, g2);
        }
        beta = (beta + HALF_UNIT) / UNITY;
        gamma = (gamma + HALF_UNIT) / UNITY;
        // §527
        let mut p = self.get_node(KNOT_NODE_SIZE)?;
        let mut q = self.get_node(KNOT_NODE_SIZE)?;
        let mut r = self.get_node(KNOT_NODE_SIZE)?;
        let mut s = if symmetric {
            NULL
        } else {
            self.get_node(KNOT_NODE_SIZE)?
        };
        let mut h = p;
        self.set_link(p, q);
        self.set_link(q, r);
        self.set_link(r, s);
        // §528
        if beta == 0 {
            beta = 1;
        }
        if gamma == 0 {
            gamma = 1;
        }
        if gamma <= alpha.abs() {
            if alpha > 0 {
                alpha = gamma - 1;
            } else {
                alpha = 1 - gamma;
            }
        }
        self.set_x_coord(p, -alpha * HALF_UNIT);
        self.set_y_coord(p, -beta * HALF_UNIT);
        self.set_x_coord(q, gamma * HALF_UNIT);
        self.set_y_coord(q, self.y_coord(p));
        self.set_x_coord(r, self.x_coord(q));
        self.set_right_x(p, 0);
        self.set_left_x(q, -HALF_UNIT);
        self.set_right_x(q, HALF_UNIT);
        self.set_left_x(r, 0);
        self.set_right_x(r, 0);
        self.set_right_y(p, beta);
        self.set_right_y(q, gamma);
        self.set_right_y(r, beta);
        self.set_left_y(q, gamma + alpha);
        if symmetric {
            self.set_y_coord(r, 0);
            self.set_left_y(r, beta);
        } else {
            self.set_y_coord(r, -self.y_coord(p));
            self.set_left_y(r, beta + beta);
            self.set_x_coord(s, -self.x_coord(p));
            self.set_y_coord(s, self.y_coord(r));
            self.set_left_x(s, HALF_UNIT);
            self.set_left_y(s, gamma - alpha);
        }
        // §530
        'done: loop {
            let u = self.right_x(p) + self.right_x(q);
            let v = self.left_x(q) + self.left_x(r);
            let c = self.right_y(p) + self.right_y(q);
            // §533
            delta = self.pyth_add(u, v);
            let mut d;
            if major_axis == minor_axis {
                d = major_axis;
            } else {
                if theta == 0 {
                    alpha = u;
                    beta = v;
                } else {
                    alpha = self.take_fraction(u, self.n_cos) + self.take_fraction(v, self.n_sin);
                    beta = self.take_fraction(v, self.n_cos) - self.take_fraction(u, self.n_sin);
                }
                alpha = self.make_fraction(alpha, delta);
                beta = self.make_fraction(beta, delta);
                let d1 = self.take_fraction(major_axis, alpha);
                let d2 = self.take_fraction(minor_axis, beta);
                d = self.pyth_add(d1, d2);
            }
            alpha = u.abs();
            beta = v.abs();
            if alpha < beta {
                alpha = v.abs();
                beta = u.abs();
            }
            if self.int(FILLIN) != 0 {
                let f = self.make_fraction(beta + beta, delta);
                d -= self.take_fraction(self.int(FILLIN), f);
            }
            d = self.take_fraction((d + 4) / 8, delta);
            alpha /= HALF_UNIT;
            if d < alpha {
                d = alpha;
            }
            delta = c - d;
            if delta > 0 {
                if delta > self.left_y(r) {
                    delta = self.left_y(r);
                }
                if delta >= self.left_y(q) {
                    // §534
                    delta = self.left_y(q);
                    self.set_right_y(p, c - delta);
                    self.set_right_x(p, u);
                    self.set_left_x(q, v);
                    self.set_x_coord(q, self.x_coord(q) - delta * self.left_x(r));
                    self.set_y_coord(q, self.y_coord(q) + delta * self.right_x(q));
                    self.set_left_y(r, self.left_y(r) - delta);
                } else {
                    // §535
                    s = self.get_node(KNOT_NODE_SIZE)?;
                    self.set_link(p, s);
                    self.set_link(s, q);
                    self.set_x_coord(s, self.x_coord(q) + delta * self.left_x(q));
                    self.set_y_coord(s, self.y_coord(q) - delta * self.right_x(p));
                    self.set_x_coord(q, self.x_coord(q) - delta * self.left_x(r));
                    self.set_y_coord(q, self.y_coord(q) + delta * self.right_x(q));
                    self.set_left_x(s, self.left_x(q));
                    self.set_right_x(s, u);
                    self.set_left_x(q, v);
                    self.set_right_y(s, c - delta);
                    self.set_left_y(s, self.left_y(q) - delta);
                    self.set_left_y(q, delta);
                    self.set_left_y(r, self.left_y(r) - delta);
                }
            } else {
                p = q;
            }
            // §531
            loop {
                q = self.link(p);
                if q == NULL {
                    break 'done;
                }
                if self.left_y(q) == 0 {
                    self.set_link(p, self.link(q));
                    self.set_right_y(p, self.right_y(q));
                    self.set_right_x(p, self.right_x(q));
                    self.free_node(q, KNOT_NODE_SIZE);
                } else {
                    r = self.link(q);
                    if r == NULL {
                        break 'done;
                    }
                    if self.left_y(r) == 0 {
                        self.set_link(p, r);
                        self.free_node(q, KNOT_NODE_SIZE);
                        p = r;
                    } else {
                        break;
                    }
                }
            }
        }
        if symmetric {
            // §536
            s = NULL;
            q = h;
            loop {
                r = self.get_node(KNOT_NODE_SIZE)?;
                self.set_link(r, s);
                s = r;
                self.set_x_coord(s, self.x_coord(q));
                self.set_y_coord(s, -self.y_coord(q));
                if q == p {
                    break;
                }
                q = self.link(q);
                if self.y_coord(q) == 0 {
                    break;
                }
            }
            if self.link(p) != NULL {
                let lp = self.link(p);
                self.free_node(lp, KNOT_NODE_SIZE);
            }
            self.set_link(p, s);
            beta = -self.y_coord(h);
            while self.y_coord(p) != beta {
                p = self.link(p);
            }
            q = self.link(p);
        }
        // §537
        if q != NULL {
            if self.right_x(h) == 0 {
                p = h;
                h = self.link(h);
                self.free_node(p, KNOT_NODE_SIZE);
                self.set_x_coord(q, -self.x_coord(h));
            }
            p = q;
        } else {
            q = p;
        }
        r = self.link(h);
        loop {
            s = self.get_node(KNOT_NODE_SIZE)?;
            self.set_link(p, s);
            p = s;
            self.set_x_coord(p, -self.x_coord(r));
            self.set_y_coord(p, -self.y_coord(r));
            r = self.link(r);
            if r == q {
                break;
            }
        }
        self.set_link(p, h);
        Ok(h)
    }

    // §539
    pub(crate) fn find_direction_time(
        &mut self,
        mut x: Scaled,
        mut y: Scaled,
        h: Pointer,
    ) -> R<Scaled> {
        // §540
        if x.abs() < y.abs() {
            x = self.make_fraction(x, y.abs());
            y = if y > 0 { FRACTION_ONE } else { -FRACTION_ONE };
        } else if x == 0 {
            return Ok(0);
        } else {
            y = self.make_fraction(y, x.abs());
            x = if x > 0 { FRACTION_ONE } else { -FRACTION_ONE };
        }
        let mut n: Scaled = 0;
        let mut p = h;
        let mut phi = 0;
        loop {
            if self.right_type(p) == ENDPOINT {
                return Ok(-UNITY);
            }
            let q = self.link(p);
            // §541
            let mut tt = 0;
            // §543
            let mut x1 = self.right_x(p) - self.x_coord(p);
            let mut x2 = self.left_x(q) - self.right_x(p);
            let mut x3 = self.x_coord(q) - self.left_x(q);
            let mut y1 = self.right_y(p) - self.y_coord(p);
            let mut y2 = self.left_y(q) - self.right_y(p);
            let mut y3 = self.y_coord(q) - self.left_y(q);
            let mut max = x1.abs();
            for v in [x2, x3, y1, y2, y3] {
                if v.abs() > max {
                    max = v.abs();
                }
            }
            if max == 0 {
                return Ok(n + tt);
            }
            while max < FRACTION_HALF {
                max += max;
                x1 += x1;
                x2 += x2;
                x3 += x3;
                y1 += y1;
                y2 += y2;
                y3 += y3;
            }
            let mut t = x1;
            x1 = self.take_fraction(x1, x) + self.take_fraction(y1, y);
            y1 = self.take_fraction(y1, x) - self.take_fraction(t, y);
            t = x2;
            x2 = self.take_fraction(x2, x) + self.take_fraction(y2, y);
            y2 = self.take_fraction(y2, x) - self.take_fraction(t, y);
            t = x3;
            x3 = self.take_fraction(x3, x) + self.take_fraction(y3, y);
            y3 = self.take_fraction(y3, x) - self.take_fraction(t, y);
            if y1 == 0 && x1 >= 0 {
                return Ok(n + tt);
            }
            if n > 0 {
                // §544
                let theta = self.n_arg(x1, y1)?;
                if theta >= 0 && phi <= 0 && phi >= theta - ONE_EIGHTY_DEG {
                    return Ok(n + tt);
                }
                if theta <= 0 && phi >= 0 && phi <= theta + ONE_EIGHTY_DEG {
                    return Ok(n + tt);
                }
                if p == h {
                    return Ok(-UNITY);
                }
            }
            if x3 != 0 || y3 != 0 {
                phi = self.n_arg(x3, y3)?;
            }
            // §545
            'done: {
                if x1 < 0 && x2 < 0 && x3 < 0 {
                    break 'done;
                }
                if ab_vs_cd(y1, y3, y2, y2) == 0 {
                    // §547
                    if ab_vs_cd(y1, y2, 0, 0) < 0 {
                        t = self.make_fraction(y1, y1 - y2);
                        x1 = self.t_of_the_way(x1, x2, t);
                        x2 = self.t_of_the_way(x2, x3, t);
                        if self.t_of_the_way(x1, x2, t) >= 0 {
                            return Ok(n + (t + 0o4000) / 0o10000);
                        }
                    } else if y3 == 0 {
                        if y1 == 0 {
                            // §548
                            t = crossing_point(-x1, -x2, -x3);
                            if t <= FRACTION_ONE {
                                return Ok(n + (t + 0o4000) / 0o10000);
                            }
                            if ab_vs_cd(x1, x3, x2, x2) <= 0 {
                                t = self.make_fraction(x1, x1 - x2);
                                return Ok(n + (t + 0o4000) / 0o10000);
                            }
                        } else if x3 >= 0 {
                            tt = UNITY;
                            return Ok(n + tt);
                        }
                    }
                    break 'done;
                }
                if y1 <= 0 {
                    if y1 < 0 {
                        y1 = -y1;
                        y2 = -y2;
                        y3 = -y3;
                    } else if y2 > 0 {
                        y2 = -y2;
                        y3 = -y3;
                    }
                }
                // §546
                t = crossing_point(y1, y2, y3);
                if t > FRACTION_ONE {
                    break 'done;
                }
                y2 = self.t_of_the_way(y2, y3, t);
                x1 = self.t_of_the_way(x1, x2, t);
                x2 = self.t_of_the_way(x2, x3, t);
                x1 = self.t_of_the_way(x1, x2, t);
                if x1 >= 0 {
                    return Ok(n + (t + 0o4000) / 0o10000);
                }
                if y2 > 0 {
                    y2 = 0;
                }
                tt = t;
                t = crossing_point(0, -y2, -y3);
                if t > FRACTION_ONE {
                    break 'done;
                }
                x1 = self.t_of_the_way(x1, x2, t);
                x2 = self.t_of_the_way(x2, x3, t);
                if self.t_of_the_way(x1, x2, t) >= 0 {
                    t = self.t_of_the_way(tt, FRACTION_ONE, t);
                    return Ok(n + (t + 0o4000) / 0o10000);
                }
            }
            p = q;
            n += UNITY;
        }
    }

    // §553: the bisection stack
    #[inline]
    fn bs(&self, i: i32) -> i32 {
        self.bisect_stack[i as usize]
    }
    #[inline]
    fn set_bs(&mut self, i: i32, v: i32) {
        self.bisect_stack[i as usize] = v;
    }
    // §554
    fn set_min_max(&mut self, k: i32) {
        let s1 = self.bs(k);
        let s2 = self.bs(k + 1);
        let s3 = self.bs(k + 2);
        let (mn, mx);
        if s1 < 0 {
            if s3 >= 0 {
                mn = if s2 < 0 { s1 + s2 } else { s1 };
                let m = s1 + s2 + s3;
                mx = if m < 0 { 0 } else { m };
            } else {
                let m = s1 + s2 + s3;
                mn = if m > s1 { s1 } else { m };
                let m = s1 + s2;
                mx = if m < 0 { 0 } else { m };
            }
        } else if s3 <= 0 {
            mx = if s2 > 0 { s1 + s2 } else { s1 };
            let m = s1 + s2 + s3;
            mn = if m > 0 { 0 } else { m };
        } else {
            let m = s1 + s2 + s3;
            mx = if m < s1 { s1 } else { m };
            let m = s1 + s2;
            mn = if m > 0 { 0 } else { m };
        }
        self.set_bs(k + 3, mn);
        self.set_bs(k + 4, mx);
    }

    // §556
    pub(crate) fn cubic_intersection(&mut self, p: Pointer, pp: Pointer) {
        let u = |k: i32| k - 5;
        let v = |k: i32| k - 10;
        let xk = |k: i32| k - 15;
        let yk = |k: i32| k - 20;
        self.time_to_go = MAX_PATIENCE;
        self.max_t = 2;
        // §558
        let q = self.link(p);
        let qq = self.link(pp);
        self.bisect_ptr = INT_PACKETS;
        let rp = self.bisect_ptr;
        self.set_bs(u(rp), self.right_x(p) - self.x_coord(p));
        self.set_bs(u(rp) + 1, self.left_x(q) - self.right_x(p));
        self.set_bs(u(rp) + 2, self.x_coord(q) - self.left_x(q));
        self.set_min_max(u(rp));
        self.set_bs(v(rp), self.right_y(p) - self.y_coord(p));
        self.set_bs(v(rp) + 1, self.left_y(q) - self.right_y(p));
        self.set_bs(v(rp) + 2, self.y_coord(q) - self.left_y(q));
        self.set_min_max(v(rp));
        self.set_bs(xk(rp), self.right_x(pp) - self.x_coord(pp));
        self.set_bs(xk(rp) + 1, self.left_x(qq) - self.right_x(pp));
        self.set_bs(xk(rp) + 2, self.x_coord(qq) - self.left_x(qq));
        self.set_min_max(xk(rp));
        self.set_bs(yk(rp), self.right_y(pp) - self.y_coord(pp));
        self.set_bs(yk(rp) + 1, self.left_y(qq) - self.right_y(pp));
        self.set_bs(yk(rp) + 2, self.y_coord(qq) - self.left_y(qq));
        self.set_min_max(yk(rp));
        self.delx = self.x_coord(p) - self.x_coord(pp);
        self.dely = self.y_coord(p) - self.y_coord(pp);
        self.tol = 0;
        self.uv = rp;
        self.xy = rp;
        self.three_l = 0;
        self.cur_t = 1;
        self.cur_tt = 1;
        loop {
            // continue:
            if self.delx - self.tol <= self.bs(xk(self.xy) + 4) - self.bs(u(self.uv) + 3)
                && self.delx + self.tol >= self.bs(xk(self.xy) + 3) - self.bs(u(self.uv) + 4)
                && self.dely - self.tol <= self.bs(yk(self.xy) + 4) - self.bs(v(self.uv) + 3)
                && self.dely + self.tol >= self.bs(yk(self.xy) + 3) - self.bs(v(self.uv) + 4)
            {
                if self.cur_t >= self.max_t {
                    if self.max_t == TWO {
                        self.cur_t = halfp(self.cur_t + 1);
                        self.cur_tt = halfp(self.cur_tt + 1);
                        return;
                    }
                    self.max_t += self.max_t;
                    self.appr_t = self.cur_t;
                    self.appr_tt = self.cur_tt;
                }
                // §559
                let bp = self.bisect_ptr;
                self.set_bs(bp, self.delx);
                self.set_bs(bp + 1, self.dely);
                self.set_bs(bp + 2, self.tol);
                self.set_bs(bp + 3, self.uv);
                self.set_bs(bp + 4, self.xy);
                self.bisect_ptr += INT_INCREMENT;
                self.cur_t += self.cur_t;
                self.cur_tt += self.cur_tt;
                let lp = self.bisect_ptr - INT_PACKETS;
                let rp = self.bisect_ptr;
                for (base, src) in [
                    (u as fn(i32) -> i32, self.uv),
                    (v, self.uv),
                    (xk, self.xy),
                    (yk, self.xy),
                ] {
                    let l = base(lp);
                    let r = base(rp);
                    let s = base(src);
                    let a1 = self.bs(s);
                    self.set_bs(l, a1);
                    let a3 = self.bs(s + 2);
                    self.set_bs(r + 2, a3);
                    let l2 = half(a1 + self.bs(s + 1));
                    self.set_bs(l + 1, l2);
                    let r2 = half(a3 + self.bs(s + 1));
                    self.set_bs(r + 1, r2);
                    let l3 = half(l2 + r2);
                    self.set_bs(l + 2, l3);
                    self.set_bs(r, l3);
                    self.set_min_max(l);
                    self.set_min_max(r);
                }
                self.uv = lp;
                self.xy = lp;
                self.delx += self.delx;
                self.dely += self.dely;
                self.tol = self.tol - self.three_l + self.tol_step;
                self.tol += self.tol;
                self.three_l += self.tol_step;
                continue;
            }
            if self.time_to_go > 0 {
                self.time_to_go -= 1;
            } else {
                while self.appr_t < UNITY {
                    self.appr_t += self.appr_t;
                    self.appr_tt += self.appr_tt;
                }
                self.cur_t = self.appr_t;
                self.cur_tt = self.appr_tt;
                return;
            }
            // §560
            loop {
                // not_found:
                if odd(self.cur_tt) {
                    if odd(self.cur_t) {
                        // §561
                        self.cur_t = halfp(self.cur_t);
                        self.cur_tt = halfp(self.cur_tt);
                        if self.cur_t == 0 {
                            return;
                        }
                        self.bisect_ptr -= INT_INCREMENT;
                        self.three_l -= self.tol_step;
                        let bp = self.bisect_ptr;
                        self.delx = self.bs(bp);
                        self.dely = self.bs(bp + 1);
                        self.tol = self.bs(bp + 2);
                        self.uv = self.bs(bp + 3);
                        self.xy = self.bs(bp + 4);
                        continue;
                    } else {
                        self.cur_t += 1;
                        let uv = self.uv;
                        self.delx =
                            self.delx + self.bs(u(uv)) + self.bs(u(uv) + 1) + self.bs(u(uv) + 2);
                        self.dely =
                            self.dely + self.bs(v(uv)) + self.bs(v(uv) + 1) + self.bs(v(uv) + 2);
                        self.uv += INT_PACKETS;
                        self.cur_tt -= 1;
                        self.xy -= INT_PACKETS;
                        let xy = self.xy;
                        self.delx =
                            self.delx + self.bs(xk(xy)) + self.bs(xk(xy) + 1) + self.bs(xk(xy) + 2);
                        self.dely =
                            self.dely + self.bs(yk(xy)) + self.bs(yk(xy) + 1) + self.bs(yk(xy) + 2);
                    }
                } else {
                    self.cur_tt += 1;
                    self.tol += self.three_l;
                    let xy = self.xy;
                    self.delx =
                        self.delx - self.bs(xk(xy)) - self.bs(xk(xy) + 1) - self.bs(xk(xy) + 2);
                    self.dely =
                        self.dely - self.bs(yk(xy)) - self.bs(yk(xy) + 1) - self.bs(yk(xy) + 2);
                    self.xy += INT_PACKETS;
                }
                break;
            }
        }
    }

    // §562
    pub(crate) fn path_intersection(&mut self, h: Pointer, hh: Pointer) {
        // §563
        if self.right_type(h) == ENDPOINT {
            self.set_right_x(h, self.x_coord(h));
            self.set_left_x(h, self.x_coord(h));
            self.set_right_y(h, self.y_coord(h));
            self.set_left_y(h, self.y_coord(h));
            self.set_right_type(h, EXPLICIT);
        }
        if self.right_type(hh) == ENDPOINT {
            self.set_right_x(hh, self.x_coord(hh));
            self.set_left_x(hh, self.x_coord(hh));
            self.set_right_y(hh, self.y_coord(hh));
            self.set_left_y(hh, self.y_coord(hh));
            self.set_right_type(hh, EXPLICIT);
        }
        self.tol_step = 0;
        loop {
            let mut n = -UNITY;
            let mut p = h;
            loop {
                if self.right_type(p) != ENDPOINT {
                    let mut nn = -UNITY;
                    let mut pp = hh;
                    loop {
                        if self.right_type(pp) != ENDPOINT {
                            self.cubic_intersection(p, pp);
                            if self.cur_t > 0 {
                                self.cur_t += n;
                                self.cur_tt += nn;
                                return;
                            }
                        }
                        nn += UNITY;
                        pp = self.link(pp);
                        if pp == hh {
                            break;
                        }
                    }
                }
                n += UNITY;
                p = self.link(p);
                if p == h {
                    break;
                }
            }
            self.tol_step += 3;
            if self.tol_step > 3 {
                break;
            }
        }
        self.cur_t = -UNITY;
        self.cur_tt = -UNITY;
    }
}
