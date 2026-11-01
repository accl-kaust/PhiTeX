//! Parts 17 and 18 of `mf.web`: data structures for paths, choosing
//! control points. (`left_curl`, `left_given` are `left_x`;
//! `left_tension` is `left_y`; likewise on the right.)

use crate::arith::ab_vs_cd;
use crate::{Mf, Pointer, R, Scaled, consts::*};

fn reduce_angle(a: &mut i32) {
    if a.abs() > ONE_EIGHTY_DEG {
        if *a > 0 {
            *a -= THREE_SIXTY_DEG;
        } else {
            *a += THREE_SIXTY_DEG;
        }
    }
}

impl Mf {
    // §257
    pub(crate) fn print_path(&mut self, h: Pointer, s: &str, nuline: bool) {
        self.print_diagnostic("Path", s, nuline);
        self.print_ln();
        let mut p = h;
        'done: loop {
            let mut q = self.link(p);
            if p == NULL || q == NULL {
                self.print_nl("???");
                break 'done;
            }
            // §258
            self.print_two(self.x_coord(p), self.y_coord(p));
            'done1: {
                match self.right_type(p) {
                    ENDPOINT => {
                        if self.left_type(p) == OPEN {
                            self.prints("{open?}");
                        }
                        if self.left_type(q) != ENDPOINT || q != h {
                            q = NULL;
                        }
                        break 'done1;
                    }
                    EXPLICIT => {
                        // §261
                        self.prints("..controls ");
                        self.print_two(self.right_x(p), self.right_y(p));
                        self.prints(" and ");
                        if self.left_type(q) != EXPLICIT {
                            self.prints("??");
                        } else {
                            self.print_two(self.left_x(q), self.left_y(q));
                        }
                        break 'done1;
                    }
                    OPEN => {
                        // §262
                        if self.left_type(p) != EXPLICIT && self.left_type(p) != OPEN {
                            self.prints("{open?}");
                        }
                    }
                    CURL | GIVEN => {
                        // §263
                        if self.left_type(p) == OPEN {
                            self.prints("??");
                        }
                        if self.right_type(p) == CURL {
                            self.prints("{curl ");
                            self.print_scaled(self.right_x(p));
                        } else {
                            self.n_sin_cos(self.right_x(p));
                            self.print_char(b'{');
                            self.print_scaled(self.n_cos);
                            self.print_char(b',');
                            self.print_scaled(self.n_sin);
                        }
                        self.print_char(b'}');
                    }
                    _ => self.prints("???"),
                }
                if self.left_type(q) <= EXPLICIT {
                    self.prints("..control?");
                } else if self.right_y(p) != UNITY || self.left_y(q) != UNITY {
                    // §260
                    self.prints("..tension ");
                    if self.right_y(p) < 0 {
                        self.prints("atleast");
                    }
                    self.print_scaled(self.right_y(p).abs());
                    if self.right_y(p) != self.left_y(q) {
                        self.prints(" and ");
                        if self.left_y(q) < 0 {
                            self.prints("atleast");
                        }
                        self.print_scaled(self.left_y(q).abs());
                    }
                }
            }
            p = q;
            if p != h || self.left_type(h) != ENDPOINT {
                // §259
                self.print_nl(" ..");
                if self.left_type(p) == GIVEN {
                    self.n_sin_cos(self.left_x(p));
                    self.print_char(b'{');
                    self.print_scaled(self.n_cos);
                    self.print_char(b',');
                    self.print_scaled(self.n_sin);
                    self.print_char(b'}');
                } else if self.left_type(p) == CURL {
                    self.prints("{curl ");
                    self.print_scaled(self.left_x(p));
                    self.print_char(b'}');
                }
            }
            if p == h {
                if self.left_type(h) != ENDPOINT {
                    self.prints("cycle");
                }
                break 'done;
            }
        }
        self.end_diagnostic(true);
    }

    // §264
    pub(crate) fn copy_knot(&mut self, p: Pointer) -> R<Pointer> {
        let q = self.get_node(KNOT_NODE_SIZE)?;
        for k in 0..KNOT_NODE_SIZE {
            self.mem[(q + k) as usize] = self.mem[(p + k) as usize];
        }
        Ok(q)
    }

    // §265
    pub(crate) fn copy_path(&mut self, p: Pointer) -> R<Pointer> {
        let q = self.get_node(KNOT_NODE_SIZE)?;
        let mut qq = q;
        let mut pp = p;
        loop {
            self.set_left_type(qq, self.left_type(pp));
            self.set_right_type(qq, self.right_type(pp));
            self.set_x_coord(qq, self.x_coord(pp));
            self.set_y_coord(qq, self.y_coord(pp));
            self.set_left_x(qq, self.left_x(pp));
            self.set_left_y(qq, self.left_y(pp));
            self.set_right_x(qq, self.right_x(pp));
            self.set_right_y(qq, self.right_y(pp));
            if self.link(pp) == p {
                self.set_link(qq, q);
                return Ok(q);
            }
            let n = self.get_node(KNOT_NODE_SIZE)?;
            self.set_link(qq, n);
            qq = n;
            pp = self.link(pp);
        }
    }

    // §266
    pub(crate) fn htap_ypoc(&mut self, p: Pointer) -> R<Pointer> {
        let q = self.get_node(KNOT_NODE_SIZE)?;
        let mut qq = q;
        let mut pp = p;
        loop {
            self.set_right_type(qq, self.left_type(pp));
            self.set_left_type(qq, self.right_type(pp));
            self.set_x_coord(qq, self.x_coord(pp));
            self.set_y_coord(qq, self.y_coord(pp));
            self.set_right_x(qq, self.left_x(pp));
            self.set_right_y(qq, self.left_y(pp));
            self.set_left_x(qq, self.right_x(pp));
            self.set_left_y(qq, self.right_y(pp));
            if self.link(pp) == p {
                self.set_link(q, qq);
                self.path_tail = pp;
                return Ok(q);
            }
            let rr = self.get_node(KNOT_NODE_SIZE)?;
            self.set_link(rr, qq);
            qq = rr;
            pp = self.link(pp);
        }
    }

    // §268
    pub(crate) fn toss_knot_list(&mut self, p: Pointer) {
        let mut q = p;
        loop {
            let r = self.link(q);
            self.free_node(q, KNOT_NODE_SIZE);
            q = r;
            if q == p {
                break;
            }
        }
    }

    // §269
    pub(crate) fn make_choices(&mut self, knots: Pointer) -> R {
        self.check_arith()?;
        if self.int(TRACING_CHOICES) > 0 {
            self.print_path(knots, ", before choices", true);
        }
        // §271
        let mut p = knots;
        loop {
            let q = self.link(p);
            if self.x_coord(p) == self.x_coord(q)
                && self.y_coord(p) == self.y_coord(q)
                && self.right_type(p) > EXPLICIT
            {
                self.set_right_type(p, EXPLICIT);
                if self.left_type(p) == OPEN {
                    self.set_left_type(p, CURL);
                    self.set_left_x(p, UNITY);
                }
                self.set_left_type(q, EXPLICIT);
                if self.right_type(q) == OPEN {
                    self.set_right_type(q, CURL);
                    self.set_right_x(q, UNITY);
                }
                self.set_right_x(p, self.x_coord(p));
                self.set_left_x(q, self.x_coord(p));
                self.set_right_y(p, self.y_coord(p));
                self.set_left_y(q, self.y_coord(p));
            }
            p = q;
            if p == knots {
                break;
            }
        }
        // §272
        let mut h = knots;
        loop {
            if self.left_type(h) != OPEN {
                break;
            }
            if self.right_type(h) != OPEN {
                break;
            }
            h = self.link(h);
            if h == knots {
                self.set_left_type(h, END_CYCLE);
                break;
            }
        }
        p = h;
        loop {
            // §273
            let mut q = self.link(p);
            if self.right_type(p) >= GIVEN {
                while self.left_type(q) == OPEN && self.right_type(q) == OPEN {
                    q = self.link(q);
                }
                // §278: §281
                let mut k: usize = 0;
                let mut s = p;
                let mut n: usize = PATH_SIZE as usize;
                loop {
                    let t = self.link(s);
                    self.delta_x[k] = self.x_coord(t) - self.x_coord(s);
                    self.delta_y[k] = self.y_coord(t) - self.y_coord(s);
                    self.delta[k] = self.pyth_add(self.delta_x[k], self.delta_y[k]);
                    if k > 0 {
                        let sine = self.make_fraction(self.delta_y[k - 1], self.delta[k - 1]);
                        let cosine = self.make_fraction(self.delta_x[k - 1], self.delta[k - 1]);
                        let a = self.take_fraction(self.delta_x[k], cosine)
                            + self.take_fraction(self.delta_y[k], sine);
                        let b = self.take_fraction(self.delta_y[k], cosine)
                            - self.take_fraction(self.delta_x[k], sine);
                        self.psi[k] = self.n_arg(a, b)?;
                    }
                    k += 1;
                    s = t;
                    if k == PATH_SIZE as usize {
                        return self.overflow("path size", PATH_SIZE);
                    }
                    if s == q {
                        n = k;
                    }
                    if k >= n && self.left_type(s) != END_CYCLE {
                        break;
                    }
                }
                if k == n {
                    self.psi[n] = 0;
                } else {
                    self.psi[k] = self.psi[1];
                }
                // §282
                if self.left_type(q) == OPEN {
                    let delx = self.right_x(q) - self.x_coord(q);
                    let dely = self.right_y(q) - self.y_coord(q);
                    if delx == 0 && dely == 0 {
                        self.set_left_type(q, CURL);
                        self.set_left_x(q, UNITY);
                    } else {
                        self.set_left_type(q, GIVEN);
                        let a = self.n_arg(delx, dely)?;
                        self.set_left_x(q, a);
                    }
                }
                if self.right_type(p) == OPEN && self.left_type(p) == EXPLICIT {
                    let delx = self.x_coord(p) - self.left_x(p);
                    let dely = self.y_coord(p) - self.left_y(p);
                    if delx == 0 && dely == 0 {
                        self.set_right_type(p, CURL);
                        self.set_right_x(p, UNITY);
                    } else {
                        self.set_right_type(p, GIVEN);
                        let a = self.n_arg(delx, dely)?;
                        self.set_right_x(p, a);
                    }
                }
                self.solve_choices(p, q, n)?;
            }
            p = q;
            if p == h {
                break;
            }
        }
        if self.int(TRACING_CHOICES) > 0 {
            self.print_path(knots, ", after choices", true);
        }
        if self.arith_error {
            // §270
            self.print_err("Some number got too big");
            self.help(&[
                "The path that I just computed is out of range.",
                "So it will probably look funny. Proceed, for a laugh.",
            ]);
            self.put_get_error()?;
            self.arith_error = false;
        }
        Ok(())
    }

    // §284
    pub(crate) fn solve_choices(&mut self, p: Pointer, q: Pointer, n: usize) -> R {
        let mut k: usize = 0;
        let mut s = p;
        let mut r: Pointer = NULL;
        let (mut aa, mut bb, mut cc, mut ff, mut acc);
        let (mut dd, mut ee, mut lt, mut rt);
        loop {
            let t = self.link(s);
            if k == 0 {
                // §285
                match self.right_type(s) {
                    GIVEN => {
                        if self.left_type(t) == GIVEN {
                            // §301
                            aa = self.n_arg(self.delta_x[0], self.delta_y[0])?;
                            self.n_sin_cos(self.right_x(p) - aa);
                            self.ct = self.n_cos;
                            self.st = self.n_sin;
                            self.n_sin_cos(self.left_x(q) - aa);
                            self.cf = self.n_cos;
                            self.sf = -self.n_sin;
                            self.set_controls(p, q, 0);
                            return Ok(());
                        } else {
                            // §293
                            self.vv[0] =
                                self.right_x(s) - self.n_arg(self.delta_x[0], self.delta_y[0])?;
                            reduce_angle(&mut self.vv[0]);
                            self.uu[0] = 0;
                            self.ww[0] = 0;
                        }
                    }
                    CURL => {
                        if self.left_type(t) == CURL {
                            // §302
                            self.set_right_type(p, EXPLICIT);
                            self.set_left_type(q, EXPLICIT);
                            lt = self.left_y(q).abs();
                            rt = self.right_y(p).abs();
                            if rt == UNITY {
                                if self.delta_x[0] >= 0 {
                                    self.set_right_x(
                                        p,
                                        self.x_coord(p) + ((self.delta_x[0] + 1) / 3),
                                    );
                                } else {
                                    self.set_right_x(
                                        p,
                                        self.x_coord(p) + ((self.delta_x[0] - 1) / 3),
                                    );
                                }
                                if self.delta_y[0] >= 0 {
                                    self.set_right_y(
                                        p,
                                        self.y_coord(p) + ((self.delta_y[0] + 1) / 3),
                                    );
                                } else {
                                    self.set_right_y(
                                        p,
                                        self.y_coord(p) + ((self.delta_y[0] - 1) / 3),
                                    );
                                }
                            } else {
                                ff = self.make_fraction(UNITY, 3 * rt);
                                let a = self.x_coord(p) + self.take_fraction(self.delta_x[0], ff);
                                self.set_right_x(p, a);
                                let a = self.y_coord(p) + self.take_fraction(self.delta_y[0], ff);
                                self.set_right_y(p, a);
                            }
                            if lt == UNITY {
                                if self.delta_x[0] >= 0 {
                                    self.set_left_x(
                                        q,
                                        self.x_coord(q) - ((self.delta_x[0] + 1) / 3),
                                    );
                                } else {
                                    self.set_left_x(
                                        q,
                                        self.x_coord(q) - ((self.delta_x[0] - 1) / 3),
                                    );
                                }
                                if self.delta_y[0] >= 0 {
                                    self.set_left_y(
                                        q,
                                        self.y_coord(q) - ((self.delta_y[0] + 1) / 3),
                                    );
                                } else {
                                    self.set_left_y(
                                        q,
                                        self.y_coord(q) - ((self.delta_y[0] - 1) / 3),
                                    );
                                }
                            } else {
                                ff = self.make_fraction(UNITY, 3 * lt);
                                let a = self.x_coord(q) - self.take_fraction(self.delta_x[0], ff);
                                self.set_left_x(q, a);
                                let a = self.y_coord(q) - self.take_fraction(self.delta_y[0], ff);
                                self.set_left_y(q, a);
                            }
                            return Ok(());
                        } else {
                            // §294
                            cc = self.right_x(s);
                            lt = self.left_y(t).abs();
                            rt = self.right_y(s).abs();
                            if rt == UNITY && lt == UNITY {
                                self.uu[0] = self.make_fraction(cc + cc + UNITY, cc + TWO);
                            } else {
                                self.uu[0] = self.curl_ratio(cc, rt, lt);
                            }
                            self.vv[0] = -self.take_fraction(self.psi[1], self.uu[0]);
                            self.ww[0] = 0;
                        }
                    }
                    OPEN => {
                        self.uu[0] = 0;
                        self.vv[0] = 0;
                        self.ww[0] = FRACTION_ONE;
                    }
                    _ => {}
                }
            } else {
                match self.left_type(s) {
                    END_CYCLE | OPEN => {
                        // §287: §288
                        if self.right_y(r).abs() == UNITY {
                            aa = FRACTION_HALF;
                            dd = 2 * self.delta[k];
                        } else {
                            aa = self.make_fraction(UNITY, 3 * self.right_y(r).abs() - UNITY);
                            let m = self.make_fraction(UNITY, self.right_y(r).abs());
                            dd = self.take_fraction(self.delta[k], FRACTION_THREE - m);
                        }
                        if self.left_y(t).abs() == UNITY {
                            bb = FRACTION_HALF;
                            ee = 2 * self.delta[k - 1];
                        } else {
                            bb = self.make_fraction(UNITY, 3 * self.left_y(t).abs() - UNITY);
                            let m = self.make_fraction(UNITY, self.left_y(t).abs());
                            ee = self.take_fraction(self.delta[k - 1], FRACTION_THREE - m);
                        }
                        cc = FRACTION_ONE - self.take_fraction(self.uu[k - 1], aa);
                        // §289
                        dd = self.take_fraction(dd, cc);
                        lt = self.left_y(s).abs();
                        rt = self.right_y(s).abs();
                        if lt != rt {
                            if lt < rt {
                                ff = self.make_fraction(lt, rt);
                                ff = self.take_fraction(ff, ff);
                                dd = self.take_fraction(dd, ff);
                            } else {
                                ff = self.make_fraction(rt, lt);
                                ff = self.take_fraction(ff, ff);
                                ee = self.take_fraction(ee, ff);
                            }
                        }
                        ff = self.make_fraction(ee, ee + dd);
                        self.uu[k] = self.take_fraction(ff, bb);
                        // §290
                        acc = -self.take_fraction(self.psi[k + 1], self.uu[k]);
                        if self.right_type(r) == CURL {
                            self.ww[k] = 0;
                            self.vv[k] = acc - self.take_fraction(self.psi[1], FRACTION_ONE - ff);
                        } else {
                            ff = self.make_fraction(FRACTION_ONE - ff, cc);
                            acc -= self.take_fraction(self.psi[k], ff);
                            ff = self.take_fraction(ff, aa);
                            self.vv[k] = acc - self.take_fraction(self.vv[k - 1], ff);
                            if self.ww[k - 1] == 0 {
                                self.ww[k] = 0;
                            } else {
                                self.ww[k] = -self.take_fraction(self.ww[k - 1], ff);
                            }
                        }
                        if self.left_type(s) == END_CYCLE {
                            // §291
                            aa = 0;
                            bb = FRACTION_ONE;
                            loop {
                                k -= 1;
                                if k == 0 {
                                    k = n;
                                }
                                aa = self.vv[k] - self.take_fraction(aa, self.uu[k]);
                                bb = self.ww[k] - self.take_fraction(bb, self.uu[k]);
                                if k == n {
                                    break;
                                }
                            }
                            aa = self.make_fraction(aa, FRACTION_ONE - bb);
                            self.theta[n] = aa;
                            self.vv[0] = aa;
                            for k in 1..n {
                                self.vv[k] += self.take_fraction(aa, self.ww[k]);
                            }
                            break;
                        }
                    }
                    CURL => {
                        // §295
                        cc = self.left_x(s);
                        lt = self.left_y(s).abs();
                        rt = self.right_y(r).abs();
                        if rt == UNITY && lt == UNITY {
                            ff = self.make_fraction(cc + cc + UNITY, cc + TWO);
                        } else {
                            ff = self.curl_ratio(cc, lt, rt);
                        }
                        let a = self.take_fraction(self.vv[n - 1], ff);
                        let b = FRACTION_ONE - self.take_fraction(ff, self.uu[n - 1]);
                        self.theta[n] = -self.make_fraction(a, b);
                        break;
                    }
                    GIVEN => {
                        // §292
                        self.theta[n] = self.left_x(s)
                            - self.n_arg(self.delta_x[n - 1], self.delta_y[n - 1])?;
                        reduce_angle(&mut self.theta[n]);
                        break;
                    }
                    _ => {}
                }
            }
            r = s;
            s = t;
            k += 1;
        }
        // §297
        let mut k = n as i64 - 1;
        while k >= 0 {
            let ku = k as usize;
            self.theta[ku] = self.vv[ku] - self.take_fraction(self.theta[ku + 1], self.uu[ku]);
            k -= 1;
        }
        let mut s = p;
        let mut k = 0usize;
        loop {
            let t = self.link(s);
            self.n_sin_cos(self.theta[k]);
            self.st = self.n_sin;
            self.ct = self.n_cos;
            self.n_sin_cos(-self.psi[k + 1] - self.theta[k + 1]);
            self.sf = self.n_sin;
            self.cf = self.n_cos;
            self.set_controls(s, t, k);
            k += 1;
            s = t;
            if k == n {
                break;
            }
        }
        Ok(())
    }

    // §296
    pub(crate) fn curl_ratio(
        &mut self,
        mut gamma: Scaled,
        a_tension: Scaled,
        b_tension: Scaled,
    ) -> i32 {
        let alpha = self.make_fraction(UNITY, a_tension);
        let mut beta = self.make_fraction(UNITY, b_tension);
        let denom;
        let num;
        if alpha <= beta {
            let mut ff = self.make_fraction(alpha, beta);
            ff = self.take_fraction(ff, ff);
            gamma = self.take_fraction(gamma, ff);
            beta /= 0o10000;
            denom = self.take_fraction(gamma, alpha) + THREE - beta;
            num = self.take_fraction(gamma, FRACTION_THREE - alpha) + beta;
        } else {
            let mut ff = self.make_fraction(beta, alpha);
            ff = self.take_fraction(ff, ff);
            beta = self.take_fraction(beta, ff) / 0o10000;
            denom = self.take_fraction(gamma, alpha) + (ff / 1365) - beta;
            num = self.take_fraction(gamma, FRACTION_THREE - alpha) + beta;
        }
        if num >= denom + denom + denom + denom {
            FRACTION_FOUR
        } else {
            self.make_fraction(num, denom)
        }
    }

    // §299
    pub(crate) fn set_controls(&mut self, p: Pointer, q: Pointer, k: usize) {
        let lt = self.left_y(q).abs();
        let rt = self.right_y(p).abs();
        let mut rr = self.velocity(self.st, self.ct, self.sf, self.cf, rt);
        let mut ss = self.velocity(self.sf, self.cf, self.st, self.ct, lt);
        if self.right_y(p) < 0 || self.left_y(q) < 0 {
            // §300
            if (self.st >= 0 && self.sf >= 0) || (self.st <= 0 && self.sf <= 0) {
                let mut sine = self.take_fraction(self.st.abs(), self.cf)
                    + self.take_fraction(self.sf.abs(), self.ct);
                if sine > 0 {
                    sine = self.take_fraction(sine, FRACTION_ONE + UNITY);
                    if self.right_y(p) < 0 && ab_vs_cd(self.sf.abs(), FRACTION_ONE, rr, sine) < 0 {
                        rr = self.make_fraction(self.sf.abs(), sine);
                    }
                    if self.left_y(q) < 0 && ab_vs_cd(self.st.abs(), FRACTION_ONE, ss, sine) < 0 {
                        ss = self.make_fraction(self.st.abs(), sine);
                    }
                }
            }
        }
        let a = self.take_fraction(self.delta_x[k], self.ct)
            - self.take_fraction(self.delta_y[k], self.st);
        let v = self.x_coord(p) + self.take_fraction(a, rr);
        self.set_right_x(p, v);
        let a = self.take_fraction(self.delta_y[k], self.ct)
            + self.take_fraction(self.delta_x[k], self.st);
        let v = self.y_coord(p) + self.take_fraction(a, rr);
        self.set_right_y(p, v);
        let a = self.take_fraction(self.delta_x[k], self.cf)
            + self.take_fraction(self.delta_y[k], self.sf);
        let v = self.x_coord(q) - self.take_fraction(a, ss);
        self.set_left_x(q, v);
        let a = self.take_fraction(self.delta_y[k], self.cf)
            - self.take_fraction(self.delta_x[k], self.sf);
        let v = self.y_coord(q) - self.take_fraction(a, ss);
        self.set_left_y(q, v);
        self.set_right_type(p, EXPLICIT);
        self.set_left_type(q, EXPLICIT);
    }
}
