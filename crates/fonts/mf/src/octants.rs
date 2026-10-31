//! Part 21 of `mf.web`: subdivision into octants. (`right_octant` is
//! `right_x`, `left_octant` is `left_x`, `right_transition` is `right_y`,
//! `left_transition` is `left_y`.)

use crate::arith::{half, halfp, odd};
use crate::{Mf, Pointer, R, Scaled, consts::*};

/// `octant_dir[o]` (§396), by octant code.
pub(crate) fn octant_dir(o: i32) -> &'static str {
    match o {
        FIRST_OCTANT => "ENE",
        SECOND_OCTANT => "NNE",
        THIRD_OCTANT => "NNW",
        FOURTH_OCTANT => "WNW",
        FIFTH_OCTANT => "WSW",
        SIXTH_OCTANT => "SSW",
        SEVENTH_OCTANT => "SSE",
        EIGHTH_OCTANT => "ESE",
        _ => "",
    }
}

const AXIS: i32 = 0;
const DIAGONAL: i32 = 1;

impl Mf {
    // §387
    pub(crate) fn skew(&mut self, x: Scaled, y: Scaled, octant: i32) {
        let (a, b) = match octant {
            FIRST_OCTANT => (x - y, y),
            SECOND_OCTANT => (y - x, x),
            THIRD_OCTANT => (y + x, -x),
            FOURTH_OCTANT => (-x - y, y),
            FIFTH_OCTANT => (-x + y, -y),
            SIXTH_OCTANT => (-y + x, -x),
            SEVENTH_OCTANT => (-y - x, x),
            EIGHTH_OCTANT => (x + y, -y),
            _ => return,
        };
        self.cur_x = a;
        self.cur_y = b;
    }
    // §388
    pub(crate) fn unskew(&mut self, x: Scaled, y: Scaled, octant: i32) {
        let (a, b) = match octant {
            FIRST_OCTANT => (x + y, y),
            SECOND_OCTANT => (y, x + y),
            THIRD_OCTANT => (-y, x + y),
            FOURTH_OCTANT => (-x - y, y),
            FIFTH_OCTANT => (-x - y, -y),
            SIXTH_OCTANT => (-y, -x - y),
            SEVENTH_OCTANT => (y, -x - y),
            EIGHTH_OCTANT => (x + y, -y),
            _ => return,
        };
        self.cur_x = a;
        self.cur_y = b;
    }
    // §390
    pub(crate) fn abnegate(&mut self, x: Scaled, y: Scaled, octant_before: i32, octant_after: i32) {
        self.cur_x = if odd(octant_before) == odd(octant_after) {
            x
        } else {
            -x
        };
        self.cur_y = if (octant_before > NEGATE_Y) == (octant_after > NEGATE_Y) {
            y
        } else {
            -y
        };
    }

    // §394
    pub(crate) fn print_spec(&mut self, s: &str) {
        self.print_diagnostic("Cycle spec", s, true);
        let mut p = self.cur_spec;
        let mut octant = self.left_x(p);
        self.print_ln();
        self.print_two_true(
            self.x_coord(self.cur_spec),
            self.y_coord(self.cur_spec),
            octant,
        );
        self.prints(" % beginning in octant `");
        loop {
            self.prints(octant_dir(octant));
            self.print_char(b'\'');
            let mut q;
            loop {
                q = self.link(p);
                if self.right_type(p) == ENDPOINT {
                    break;
                }
                // §397
                self.print_nl("   ..controls ");
                self.print_two_true(self.right_x(p), self.right_y(p), octant);
                self.prints(" and ");
                self.print_two_true(self.left_x(q), self.left_y(q), octant);
                self.print_nl(" ..");
                self.print_two_true(self.x_coord(q), self.y_coord(q), octant);
                self.prints(" % segment ");
                self.print_int(self.left_type(q) - 1);
                p = q;
            }
            if q == self.cur_spec {
                break;
            }
            p = q;
            octant = self.left_x(p);
            self.print_nl("% entering octant `");
        }
        self.print_nl(" & cycle");
        self.end_diagnostic(true);
    }
    pub(crate) fn print_two_true(&mut self, x: Scaled, y: Scaled, octant: i32) {
        self.unskew(x, y, octant);
        self.print_two(self.cur_x, self.cur_y);
    }

    // §398
    pub(crate) fn print_strange(&mut self, s: &str) {
        self.print_nl(">");
        // §399
        let mut p = self.cur_spec;
        let mut t = MAX_QUARTERWORD + 1;
        let mut f = NULL;
        loop {
            p = self.link(p);
            if self.left_type(p) != ENDPOINT {
                if self.left_type(p) < t {
                    f = p;
                }
                t = self.left_type(p);
            }
            if p == self.cur_spec {
                break;
            }
        }
        // §400
        p = self.cur_spec;
        let mut q = p;
        loop {
            p = self.link(p);
            if self.left_type(p) == ENDPOINT {
                q = p;
            }
            if p == f {
                break;
            }
        }
        t = 0;
        loop {
            if self.left_type(p) != ENDPOINT {
                if self.left_type(p) != t {
                    t = self.left_type(p);
                    self.print_char(b' ');
                    self.print_int(t - 1);
                }
                if q != NULL {
                    q = self.print_turns(q);
                    self.print_char(b' ');
                    self.prints(octant_dir(self.left_x(q)));
                    q = NULL;
                }
            } else if q == NULL {
                q = p;
            }
            p = self.link(p);
            if p == f {
                break;
            }
        }
        self.print_char(b' ');
        self.print_int(self.left_type(p) - 1);
        if q != NULL {
            self.print_turns(q);
        }
        self.print_err(s);
    }
    // §401
    pub(crate) fn print_turns(&mut self, mut q: Pointer) -> Pointer {
        if self.left_type(self.link(q)) == ENDPOINT {
            self.prints(" (");
            self.prints(octant_dir(self.left_x(q)));
            q = self.link(q);
            while self.left_type(self.link(q)) == ENDPOINT {
                self.print_char(b' ');
                self.prints(octant_dir(self.left_x(q)));
                q = self.link(q);
            }
            self.print_char(b')');
        }
        q
    }

    // §402
    pub(crate) fn make_spec(
        &mut self,
        h: Pointer,
        safety_margin: Scaled,
        tracing: i32,
    ) -> R<Pointer> {
        self.cur_spec = h;
        if tracing > 0 {
            self.print_path(self.cur_spec, ", before subdivision into octants", true);
        }
        self.max_allowed = FRACTION_ONE - HALF_UNIT - 1 - safety_margin;
        // §404
        let mut p = self.cur_spec;
        let mut k = 1;
        let mut chopped = 0;
        let dmax = half(self.max_allowed);
        loop {
            for off in [3, 4, 1, 2, 5, 6] {
                let v = self.mem[(p + off) as usize].rh;
                if v.abs() >= dmax {
                    if v.abs() > self.max_allowed {
                        chopped = 1;
                        self.mem[(p + off) as usize].rh = if v > 0 {
                            self.max_allowed
                        } else {
                            -self.max_allowed
                        };
                    } else if chopped == 0 {
                        chopped = -1;
                    }
                }
            }
            p = self.link(p);
            self.set_left_type(p, k);
            if k < MAX_QUARTERWORD {
                k += 1;
            } else {
                k = 1;
            }
            if p == self.cur_spec {
                break;
            }
        }
        if chopped > 0 {
            self.print_err("Curve out of range");
            self.help(&[
                "At least one of the coordinates in the path I'm about to",
                "digitize was really huge (potentially bigger than 4095).",
                "So I've cut it back to the maximum size.",
                "The results will probably be pretty wild.",
            ]);
            self.put_get_error()?;
        }
        self.quadrant_subdivide()?;
        if self.int(AUTOROUNDING) > 0 && chopped == 0 {
            self.xy_round()?;
        }
        self.octant_subdivide()?;
        if self.int(AUTOROUNDING) > UNITY && chopped == 0 {
            self.diag_round()?;
        }
        // §447
        let mut p = self.cur_spec;
        loop {
            let mut q = self.link(p);
            if p != q
                && self.x_coord(p) == self.right_x(p)
                && self.y_coord(p) == self.right_y(p)
                && self.x_coord(p) == self.left_x(q)
                && self.y_coord(p) == self.left_y(q)
            {
                self.unskew(self.x_coord(q), self.y_coord(q), self.right_type(q));
                self.skew(self.cur_x, self.cur_y, self.right_type(p));
                if self.x_coord(p) == self.cur_x && self.y_coord(p) == self.cur_y {
                    self.remove_cubic(p);
                    if q != self.cur_spec {
                        continue;
                    }
                    self.cur_spec = p;
                    q = p;
                }
            }
            p = q;
            if p == self.cur_spec {
                break;
            }
        }
        // §450
        self.turning_number = 0;
        let mut p = self.cur_spec;
        let mut q = self.link(p);
        loop {
            let mut r = self.link(q);
            if self.right_type(p) != self.right_type(q) || q == r {
                // §452
                self.new_boundary(p, self.right_type(p))?;
                let mut s = self.link(p);
                let mut o1 = self.octant_number[self.right_type(p) as usize];
                let o2 = self.octant_number[self.right_type(q) as usize];
                let clockwise;
                'done: {
                    match o2 - o1 {
                        1 | -7 | 7 | -1 => break 'done,
                        2 | -6 => clockwise = false,
                        3 | -5 | 4 | -4 | 5 | -3 => {
                            // §454: §457
                            let mut dx1 = self.x_coord(s) - self.left_x(s);
                            let mut dy1 = self.y_coord(s) - self.left_y(s);
                            if dx1 == 0 && dy1 == 0 {
                                dx1 = self.x_coord(s) - self.right_x(p);
                                dy1 = self.y_coord(s) - self.right_y(p);
                                if dx1 == 0 && dy1 == 0 {
                                    dx1 = self.x_coord(s) - self.x_coord(p);
                                    dy1 = self.y_coord(s) - self.y_coord(p);
                                }
                            }
                            let mut dmax = dx1.abs();
                            if dy1.abs() > dmax {
                                dmax = dy1.abs();
                            }
                            while dmax < FRACTION_ONE {
                                dmax += dmax;
                                dx1 += dx1;
                                dy1 += dy1;
                            }
                            let mut dx2 = self.right_x(q) - self.x_coord(q);
                            let mut dy2 = self.right_y(q) - self.y_coord(q);
                            if dx2 == 0 && dy2 == 0 {
                                dx2 = self.left_x(r) - self.x_coord(q);
                                dy2 = self.left_y(r) - self.y_coord(q);
                                if dx2 == 0 && dy2 == 0 {
                                    if self.right_type(r) == ENDPOINT {
                                        self.cur_x = self.x_coord(r);
                                        self.cur_y = self.y_coord(r);
                                    } else {
                                        self.unskew(
                                            self.x_coord(r),
                                            self.y_coord(r),
                                            self.right_type(r),
                                        );
                                        self.skew(self.cur_x, self.cur_y, self.right_type(q));
                                    }
                                    dx2 = self.cur_x - self.x_coord(q);
                                    dy2 = self.cur_y - self.y_coord(q);
                                }
                            }
                            dmax = dx2.abs();
                            if dy2.abs() > dmax {
                                dmax = dy2.abs();
                            }
                            while dmax < FRACTION_ONE {
                                dmax += dmax;
                                dx2 += dx2;
                                dy2 += dy2;
                            }
                            self.unskew(dx1, dy1, self.right_type(p));
                            let mut del = self.pyth_add(self.cur_x, self.cur_y);
                            dx1 = self.make_fraction(self.cur_x, del);
                            dy1 = self.make_fraction(self.cur_y, del);
                            self.unskew(dx2, dy2, self.right_type(q));
                            del = self.pyth_add(self.cur_x, self.cur_y);
                            dx2 = self.make_fraction(self.cur_x, del);
                            dy2 = self.make_fraction(self.cur_y, del);
                            del = self.take_fraction(dx1, dy2) - self.take_fraction(dx2, dy1);
                            clockwise = if del > 4_684_844 {
                                false
                            } else if del < -4_684_844 {
                                true
                            } else {
                                self.rev_turns
                            };
                        }
                        6 | -2 => clockwise = true,
                        0 => clockwise = self.rev_turns,
                        _ => clockwise = false,
                    }
                    // §458
                    loop {
                        if clockwise {
                            if o1 == 1 {
                                o1 = 8;
                            } else {
                                o1 -= 1;
                            }
                        } else if o1 == 8 {
                            o1 = 1;
                        } else {
                            o1 += 1;
                        }
                        if o1 == o2 {
                            break 'done;
                        }
                        self.new_boundary(s, self.octant_code[o1 as usize])?;
                        s = self.link(s);
                        self.set_left_x(s, self.right_x(s));
                    }
                }
                if q == r {
                    q = self.link(q);
                    r = q;
                    p = s;
                    self.set_link(s, q);
                    self.set_left_x(q, self.right_x(q));
                    self.set_left_type(q, ENDPOINT);
                    self.free_node(self.cur_spec, KNOT_NODE_SIZE);
                    self.cur_spec = q;
                }
                // §459
                p = self.link(p);
                loop {
                    s = self.link(p);
                    let o1 = self.octant_number[self.right_x(p) as usize];
                    let mut o2 = self.octant_number[self.left_x(s) as usize];
                    if (o1 - o2).abs() == 1 {
                        if o2 < o1 {
                            o2 = o1;
                        }
                        if odd(o2) {
                            self.set_right_y(p, AXIS);
                        } else {
                            self.set_right_y(p, DIAGONAL);
                        }
                    } else {
                        if o1 == 8 {
                            self.turning_number += 1;
                        } else {
                            self.turning_number -= 1;
                        }
                        self.set_right_y(p, AXIS);
                    }
                    self.set_left_y(s, self.right_y(p));
                    p = s;
                    if p == q {
                        break;
                    }
                }
            }
            p = q;
            q = r;
            if p == self.cur_spec {
                break;
            }
        }
        while self.left_type(self.cur_spec) != ENDPOINT {
            self.cur_spec = self.link(self.cur_spec);
        }
        if tracing > 0 {
            if self.int(AUTOROUNDING) <= 0 || chopped != 0 {
                self.print_spec(", after subdivision");
            } else if self.int(AUTOROUNDING) > UNITY {
                self.print_spec(", after subdivision and double autorounding");
            } else {
                self.print_spec(", after subdivision and autorounding");
            }
        }
        Ok(self.cur_spec)
    }

    // §405
    pub(crate) fn remove_cubic(&mut self, p: Pointer) {
        let q = self.link(p);
        self.set_right_type(p, self.right_type(q));
        self.set_link(p, self.link(q));
        self.set_x_coord(p, self.x_coord(q));
        self.set_y_coord(p, self.y_coord(q));
        self.set_right_x(p, self.right_x(q));
        self.set_right_y(p, self.right_y(q));
        self.free_node(q, KNOT_NODE_SIZE);
    }

    /// `t_of_the_way(a)(b)`.
    #[inline]
    pub(crate) fn tow(&mut self, a: i32, b: i32, t: i32) -> i32 {
        a - self.take_fraction(a - b, t)
    }

    // §410
    pub(crate) fn split_cubic(&mut self, p: Pointer, t: i32, xq: Scaled, yq: Scaled) -> R {
        let q = self.link(p);
        let r = self.get_node(KNOT_NODE_SIZE)?;
        self.set_link(p, r);
        self.set_link(r, q);
        self.set_left_type(r, self.left_type(q));
        self.set_right_type(r, self.right_type(p));
        let v = self.tow(self.right_x(p), self.left_x(q), t);
        let a = self.tow(self.x_coord(p), self.right_x(p), t);
        self.set_right_x(p, a);
        let a = self.tow(self.left_x(q), xq, t);
        self.set_left_x(q, a);
        let a = self.tow(self.right_x(p), v, t);
        self.set_left_x(r, a);
        let a = self.tow(v, self.left_x(q), t);
        self.set_right_x(r, a);
        let a = self.tow(self.left_x(r), self.right_x(r), t);
        self.set_x_coord(r, a);
        let v = self.tow(self.right_y(p), self.left_y(q), t);
        let a = self.tow(self.y_coord(p), self.right_y(p), t);
        self.set_right_y(p, a);
        let a = self.tow(self.left_y(q), yq, t);
        self.set_left_y(q, a);
        let a = self.tow(self.right_y(p), v, t);
        self.set_left_y(r, a);
        let a = self.tow(v, self.left_y(q), t);
        self.set_right_y(r, a);
        let a = self.tow(self.left_y(r), self.right_y(r), t);
        self.set_y_coord(r, a);
        Ok(())
    }

    // §406
    pub(crate) fn quadrant_subdivide(&mut self) -> R {
        let mut p = self.cur_spec;
        let first_x = self.x_coord(self.cur_spec);
        let first_y = self.y_coord(self.cur_spec);
        'repeat: loop {
            let q = self.link(p);
            // §407
            let (mut dest_x, dest_y) = if q == self.cur_spec {
                (first_x, first_y)
            } else {
                (self.x_coord(q), self.y_coord(q))
            };
            let mut del1 = self.right_x(p) - self.x_coord(p);
            let mut del2 = self.left_x(q) - self.right_x(p);
            let mut del3 = dest_x - self.left_x(q);
            let del = scale_up(&mut del1, &mut del2, &mut del3);
            let constant_x;
            if del == 0 {
                constant_x = true;
            } else {
                constant_x = false;
                if del < 0 {
                    // §409
                    self.set_x_coord(p, -self.x_coord(p));
                    self.set_right_x(p, -self.right_x(p));
                    self.set_left_x(q, -self.left_x(q));
                    del1 = -del1;
                    del2 = -del2;
                    del3 = -del3;
                    dest_x = -dest_x;
                    self.set_right_type(p, FIRST_OCTANT + NEGATE_X);
                }
                let mut t = crossing_point(del1, del2, del3);
                if t < FRACTION_ONE {
                    // §411
                    self.split_cubic(p, t, dest_x, dest_y)?;
                    let r = self.link(p);
                    if self.right_type(r) > NEGATE_X {
                        self.set_right_type(r, FIRST_OCTANT);
                    } else {
                        self.set_right_type(r, FIRST_OCTANT + NEGATE_X);
                    }
                    if self.x_coord(r) < self.x_coord(p) {
                        self.set_x_coord(r, self.x_coord(p));
                    }
                    self.set_left_x(r, self.x_coord(r));
                    if self.right_x(p) > self.x_coord(r) {
                        self.set_right_x(p, self.x_coord(r));
                    }
                    self.set_x_coord(r, -self.x_coord(r));
                    self.set_right_x(r, self.x_coord(r));
                    self.set_left_x(q, -self.left_x(q));
                    dest_x = -dest_x;
                    del2 = self.tow(del2, del3, t);
                    if del2 > 0 {
                        del2 = 0;
                    }
                    t = crossing_point(0, -del2, -del3);
                    if t < FRACTION_ONE {
                        // §412
                        self.split_cubic(r, t, dest_x, dest_y)?;
                        let s = self.link(r);
                        if self.x_coord(s) < dest_x {
                            self.set_x_coord(s, dest_x);
                        }
                        if self.x_coord(s) < self.x_coord(r) {
                            self.set_x_coord(s, self.x_coord(r));
                        }
                        self.set_right_type(s, self.right_type(p));
                        self.set_left_x(s, self.x_coord(s));
                        if self.left_x(q) < dest_x {
                            self.set_left_x(q, -dest_x);
                        } else if self.left_x(q) > self.x_coord(s) {
                            self.set_left_x(q, -self.x_coord(s));
                        } else {
                            self.set_left_x(q, -self.left_x(q));
                        }
                        self.set_x_coord(s, -self.x_coord(s));
                        self.set_right_x(s, self.x_coord(s));
                    } else {
                        if self.x_coord(r) > dest_x {
                            self.set_x_coord(r, dest_x);
                            self.set_left_x(r, -self.x_coord(r));
                            self.set_right_x(r, self.x_coord(r));
                        }
                        if self.left_x(q) > dest_x {
                            self.set_left_x(q, dest_x);
                        } else if self.left_x(q) < self.x_coord(r) {
                            self.set_left_x(q, self.x_coord(r));
                        }
                    }
                }
            }
            // §413
            let mut pp = p;
            loop {
                let qq = self.link(pp);
                self.abnegate(
                    self.x_coord(qq),
                    self.y_coord(qq),
                    self.right_type(qq),
                    self.right_type(pp),
                );
                let dest_x = self.cur_x;
                let mut dest_y = self.cur_y;
                let mut del1 = self.right_y(pp) - self.y_coord(pp);
                let mut del2 = self.left_y(qq) - self.right_y(pp);
                let mut del3 = dest_y - self.left_y(qq);
                let del = scale_up(&mut del1, &mut del2, &mut del3);
                if del != 0 {
                    if del < 0 {
                        // §414
                        self.complement_y(pp, qq, &mut del1, &mut del2, &mut del3, &mut dest_y);
                    }
                    let mut t = crossing_point(del1, del2, del3);
                    if t < FRACTION_ONE {
                        // §415
                        self.split_cubic(pp, t, dest_x, dest_y)?;
                        let r = self.link(pp);
                        if self.right_type(r) > NEGATE_Y {
                            self.set_right_type(r, self.right_type(r) - NEGATE_Y);
                        } else {
                            self.set_right_type(r, self.right_type(r) + NEGATE_Y);
                        }
                        if self.y_coord(r) < self.y_coord(pp) {
                            self.set_y_coord(r, self.y_coord(pp));
                        }
                        self.set_left_y(r, self.y_coord(r));
                        if self.right_y(pp) > self.y_coord(r) {
                            self.set_right_y(pp, self.y_coord(r));
                        }
                        self.set_y_coord(r, -self.y_coord(r));
                        self.set_right_y(r, self.y_coord(r));
                        self.set_left_y(qq, -self.left_y(qq));
                        dest_y = -dest_y;
                        if self.x_coord(r) < self.x_coord(pp) {
                            self.set_x_coord(r, self.x_coord(pp));
                        } else if self.x_coord(r) > dest_x {
                            self.set_x_coord(r, dest_x);
                        }
                        if self.left_x(r) > self.x_coord(r) {
                            self.set_left_x(r, self.x_coord(r));
                            if self.right_x(pp) > self.x_coord(r) {
                                self.set_right_x(pp, self.x_coord(r));
                            }
                        }
                        if self.right_x(r) < self.x_coord(r) {
                            self.set_right_x(r, self.x_coord(r));
                            if self.left_x(qq) < self.x_coord(r) {
                                self.set_left_x(qq, self.x_coord(r));
                            }
                        }
                        del2 = self.tow(del2, del3, t);
                        if del2 > 0 {
                            del2 = 0;
                        }
                        t = crossing_point(0, -del2, -del3);
                        if t < FRACTION_ONE {
                            // §416
                            self.split_cubic(r, t, dest_x, dest_y)?;
                            let s = self.link(r);
                            if self.y_coord(s) < dest_y {
                                self.set_y_coord(s, dest_y);
                            }
                            if self.y_coord(s) < self.y_coord(r) {
                                self.set_y_coord(s, self.y_coord(r));
                            }
                            self.set_right_type(s, self.right_type(pp));
                            self.set_left_y(s, self.y_coord(s));
                            if self.left_y(qq) < dest_y {
                                self.set_left_y(qq, -dest_y);
                            } else if self.left_y(qq) > self.y_coord(s) {
                                self.set_left_y(qq, -self.y_coord(s));
                            } else {
                                self.set_left_y(qq, -self.left_y(qq));
                            }
                            self.set_y_coord(s, -self.y_coord(s));
                            self.set_right_y(s, self.y_coord(s));
                            if self.x_coord(s) < self.x_coord(r) {
                                self.set_x_coord(s, self.x_coord(r));
                            } else if self.x_coord(s) > dest_x {
                                self.set_x_coord(s, dest_x);
                            }
                            if self.left_x(s) > self.x_coord(s) {
                                self.set_left_x(s, self.x_coord(s));
                                if self.right_x(r) > self.x_coord(s) {
                                    self.set_right_x(r, self.x_coord(s));
                                }
                            }
                            if self.right_x(s) < self.x_coord(s) {
                                self.set_right_x(s, self.x_coord(s));
                                if self.left_x(qq) < self.x_coord(s) {
                                    self.set_left_x(qq, self.x_coord(s));
                                }
                            }
                        } else {
                            if self.y_coord(r) > dest_y {
                                self.set_y_coord(r, dest_y);
                                self.set_left_y(r, -self.y_coord(r));
                                self.set_right_y(r, self.y_coord(r));
                            }
                            if self.left_y(qq) > dest_y {
                                self.set_left_y(qq, dest_y);
                            } else if self.left_y(qq) < self.y_coord(r) {
                                self.set_left_y(qq, self.y_coord(r));
                            }
                        }
                    }
                } else {
                    // §417
                    if constant_x {
                        if q != p {
                            self.remove_cubic(p);
                            if self.cur_spec != q {
                                continue 'repeat;
                            } else {
                                self.cur_spec = p;
                                return Ok(());
                            }
                        }
                    } else if !odd(self.right_type(pp)) {
                        self.complement_y(pp, qq, &mut del1, &mut del2, &mut del3, &mut dest_y);
                    }
                }
                pp = qq;
                if pp == q {
                    break;
                }
            }
            if constant_x {
                // §418
                pp = p;
                loop {
                    let qq = self.link(pp);
                    if self.right_type(pp) > NEGATE_Y {
                        self.set_right_type(pp, self.right_type(pp) + NEGATE_X);
                        self.set_x_coord(pp, -self.x_coord(pp));
                        self.set_right_x(pp, -self.right_x(pp));
                        self.set_left_x(qq, -self.left_x(qq));
                    }
                    pp = qq;
                    if pp == q {
                        break;
                    }
                }
            }
            p = q;
            if p == self.cur_spec {
                break;
            }
        }
        Ok(())
    }
    // §414
    pub(crate) fn complement_y(
        &mut self,
        pp: Pointer,
        qq: Pointer,
        del1: &mut i32,
        del2: &mut i32,
        del3: &mut i32,
        dest_y: &mut i32,
    ) {
        self.set_y_coord(pp, -self.y_coord(pp));
        self.set_right_y(pp, -self.right_y(pp));
        self.set_left_y(qq, -self.left_y(qq));
        *del1 = -*del1;
        *del2 = -*del2;
        *del3 = -*del3;
        *dest_y = -*dest_y;
        self.set_right_type(pp, self.right_type(pp) + NEGATE_Y);
    }

    // §419
    pub(crate) fn octant_subdivide(&mut self) -> R {
        let mut p = self.cur_spec;
        loop {
            let q = self.link(p);
            self.set_x_coord(p, self.x_coord(p) - self.y_coord(p));
            self.set_right_x(p, self.right_x(p) - self.right_y(p));
            self.set_left_x(q, self.left_x(q) - self.left_y(q));
            // §420: §421
            let (mut dest_x, mut dest_y);
            if q == self.cur_spec {
                self.unskew(self.x_coord(q), self.y_coord(q), self.right_type(q));
                self.skew(self.cur_x, self.cur_y, self.right_type(p));
                dest_x = self.cur_x;
                dest_y = self.cur_y;
            } else {
                self.abnegate(
                    self.x_coord(q),
                    self.y_coord(q),
                    self.right_type(q),
                    self.right_type(p),
                );
                dest_x = self.cur_x - self.cur_y;
                dest_y = self.cur_y;
            }
            let mut del1 = self.right_x(p) - self.x_coord(p);
            let mut del2 = self.left_x(q) - self.right_x(p);
            let mut del3 = dest_x - self.left_x(q);
            let del = scale_up(&mut del1, &mut del2, &mut del3);
            if del != 0 {
                if del < 0 {
                    // §423
                    self.set_y_coord(p, self.x_coord(p) + self.y_coord(p));
                    self.set_x_coord(p, -self.x_coord(p));
                    self.set_right_y(p, self.right_x(p) + self.right_y(p));
                    self.set_right_x(p, -self.right_x(p));
                    self.set_left_y(q, self.left_x(q) + self.left_y(q));
                    self.set_left_x(q, -self.left_x(q));
                    del1 = -del1;
                    del2 = -del2;
                    del3 = -del3;
                    dest_y += dest_x;
                    dest_x = -dest_x;
                    self.set_right_type(p, self.right_type(p) + SWITCH_X_AND_Y);
                }
                let mut t = crossing_point(del1, del2, del3);
                if t < FRACTION_ONE {
                    // §424
                    self.split_cubic(p, t, dest_x, dest_y)?;
                    let r = self.link(p);
                    if self.right_type(r) > SWITCH_X_AND_Y {
                        self.set_right_type(r, self.right_type(r) - SWITCH_X_AND_Y);
                    } else {
                        self.set_right_type(r, self.right_type(r) + SWITCH_X_AND_Y);
                    }
                    if self.y_coord(r) < self.y_coord(p) {
                        self.set_y_coord(r, self.y_coord(p));
                    } else if self.y_coord(r) > dest_y {
                        self.set_y_coord(r, dest_y);
                    }
                    if self.x_coord(p) + self.y_coord(r) > dest_x + dest_y {
                        self.set_y_coord(r, dest_x + dest_y - self.x_coord(p));
                    }
                    if self.left_y(r) > self.y_coord(r) {
                        self.set_left_y(r, self.y_coord(r));
                        if self.right_y(p) > self.y_coord(r) {
                            self.set_right_y(p, self.y_coord(r));
                        }
                    }
                    if self.right_y(r) < self.y_coord(r) {
                        self.set_right_y(r, self.y_coord(r));
                        if self.left_y(q) < self.y_coord(r) {
                            self.set_left_y(q, self.y_coord(r));
                        }
                    }
                    if self.x_coord(r) < self.x_coord(p) {
                        self.set_x_coord(r, self.x_coord(p));
                    } else if self.x_coord(r) + self.y_coord(r) > dest_x + dest_y {
                        self.set_x_coord(r, dest_x + dest_y - self.y_coord(r));
                    }
                    self.set_left_x(r, self.x_coord(r));
                    if self.right_x(p) > self.x_coord(r) {
                        self.set_right_x(p, self.x_coord(r));
                    }
                    self.set_y_coord(r, self.y_coord(r) + self.x_coord(r));
                    self.set_right_y(r, self.right_y(r) + self.x_coord(r));
                    self.set_x_coord(r, -self.x_coord(r));
                    self.set_right_x(r, self.x_coord(r));
                    self.set_left_y(q, self.left_y(q) + self.left_x(q));
                    self.set_left_x(q, -self.left_x(q));
                    dest_y += dest_x;
                    dest_x = -dest_x;
                    if self.right_y(r) < self.y_coord(r) {
                        self.set_right_y(r, self.y_coord(r));
                        if self.left_y(q) < self.y_coord(r) {
                            self.set_left_y(q, self.y_coord(r));
                        }
                    }
                    del2 = self.tow(del2, del3, t);
                    if del2 > 0 {
                        del2 = 0;
                    }
                    t = crossing_point(0, -del2, -del3);
                    if t < FRACTION_ONE {
                        // §425
                        self.split_cubic(r, t, dest_x, dest_y)?;
                        let s = self.link(r);
                        if self.y_coord(s) < self.y_coord(r) {
                            self.set_y_coord(s, self.y_coord(r));
                        } else if self.y_coord(s) > dest_y {
                            self.set_y_coord(s, dest_y);
                        }
                        if self.x_coord(r) + self.y_coord(s) > dest_x + dest_y {
                            self.set_y_coord(s, dest_x + dest_y - self.x_coord(r));
                        }
                        if self.left_y(s) > self.y_coord(s) {
                            self.set_left_y(s, self.y_coord(s));
                            if self.right_y(r) > self.y_coord(s) {
                                self.set_right_y(r, self.y_coord(s));
                            }
                        }
                        if self.right_y(s) < self.y_coord(s) {
                            self.set_right_y(s, self.y_coord(s));
                            if self.left_y(q) < self.y_coord(s) {
                                self.set_left_y(q, self.y_coord(s));
                            }
                        }
                        if self.x_coord(s) + self.y_coord(s) > dest_x + dest_y {
                            self.set_x_coord(s, dest_x + dest_y - self.y_coord(s));
                        } else {
                            if self.x_coord(s) < dest_x {
                                self.set_x_coord(s, dest_x);
                            }
                            if self.x_coord(s) < self.x_coord(r) {
                                self.set_x_coord(s, self.x_coord(r));
                            }
                        }
                        self.set_right_type(s, self.right_type(p));
                        self.set_left_x(s, self.x_coord(s));
                        if self.left_x(q) < dest_x {
                            self.set_left_y(q, self.left_y(q) + dest_x);
                            self.set_left_x(q, -dest_x);
                        } else if self.left_x(q) > self.x_coord(s) {
                            self.set_left_y(q, self.left_y(q) + self.x_coord(s));
                            self.set_left_x(q, -self.x_coord(s));
                        } else {
                            self.set_left_y(q, self.left_y(q) + self.left_x(q));
                            self.set_left_x(q, -self.left_x(q));
                        }
                        self.set_y_coord(s, self.y_coord(s) + self.x_coord(s));
                        self.set_right_y(s, self.right_y(s) + self.x_coord(s));
                        self.set_x_coord(s, -self.x_coord(s));
                        self.set_right_x(s, self.x_coord(s));
                        if self.right_y(s) < self.y_coord(s) {
                            self.set_right_y(s, self.y_coord(s));
                            if self.left_y(q) < self.y_coord(s) {
                                self.set_left_y(q, self.y_coord(s));
                            }
                        }
                    } else {
                        if self.x_coord(r) > dest_x {
                            self.set_x_coord(r, dest_x);
                            self.set_left_x(r, -self.x_coord(r));
                            self.set_right_x(r, self.x_coord(r));
                        }
                        if self.left_x(q) > dest_x {
                            self.set_left_x(q, dest_x);
                        } else if self.left_x(q) < self.x_coord(r) {
                            self.set_left_x(q, self.x_coord(r));
                        }
                    }
                }
            }
            p = q;
            if p == self.cur_spec {
                break;
            }
        }
        Ok(())
    }

    // §426
    pub(crate) fn make_safe(&mut self) {
        let crp = self.cur_rounding_ptr as usize;
        self.before[crp] = self.before[0];
        self.node_to_round[crp] = self.node_to_round[0];
        loop {
            self.after[crp] = self.after[0];
            let mut all_safe = true;
            let mut next_a = self.after[0];
            for k in 0..crp {
                let delta_b = self.before[k + 1] - self.before[k];
                let delta_a = if delta_b >= 0 {
                    self.after[k + 1] - next_a
                } else {
                    next_a - self.after[k + 1]
                };
                next_a = self.after[k + 1];
                if delta_a < 0 || delta_a > (delta_b + delta_b).abs() {
                    all_safe = false;
                    self.after[k] = self.before[k];
                    if k == crp - 1 {
                        self.after[0] = self.before[0];
                    } else {
                        self.after[k + 1] = self.before[k + 1];
                    }
                }
            }
            if all_safe {
                break;
            }
        }
    }

    // §429
    pub(crate) fn before_and_after(&mut self, b: Scaled, a: Scaled, p: Pointer) -> R {
        if self.cur_rounding_ptr == self.max_rounding_ptr {
            if self.max_rounding_ptr < MAX_WIGGLE {
                self.max_rounding_ptr += 1;
            } else {
                return self.overflow("rounding table size", MAX_WIGGLE);
            }
        }
        let c = self.cur_rounding_ptr as usize;
        self.after[c] = a;
        self.before[c] = b;
        self.node_to_round[c] = p;
        self.cur_rounding_ptr += 1;
        Ok(())
    }

    // §431
    pub(crate) fn good_val(&self, b: Scaled, o: Scaled) -> Scaled {
        let mut a = b + o;
        if a >= 0 {
            a = a - (a % self.cur_gran) - o;
        } else {
            a = a + ((-(a + 1)) % self.cur_gran) - self.cur_gran + 1 - o;
        }
        if b - a < a + self.cur_gran - b {
            a
        } else {
            a + self.cur_gran
        }
    }
    // §432
    pub(crate) fn compromise(&self, u: Scaled, v: Scaled) -> Scaled {
        half(self.good_val(u + u, -u - v))
    }

    // §435
    pub(crate) fn north_edge(&self, p: Pointer) -> Scaled {
        self.y_coord(self.link(p + FOURTH_OCTANT))
    }
    pub(crate) fn south_edge(&self, p: Pointer) -> Scaled {
        self.y_coord(self.link(p + FIRST_OCTANT))
    }
    pub(crate) fn east_edge(&self, p: Pointer) -> Scaled {
        self.y_coord(self.link(p + SECOND_OCTANT))
    }
    pub(crate) fn west_edge(&self, p: Pointer) -> Scaled {
        self.y_coord(self.link(p + SEVENTH_OCTANT))
    }

    // §433
    pub(crate) fn xy_round(&mut self) -> R {
        self.cur_gran = self.int(GRANULARITY).abs();
        if self.cur_gran == 0 {
            self.cur_gran = UNITY;
        }
        let mut p = self.cur_spec;
        self.cur_rounding_ptr = 0;
        loop {
            let q = self.link(p);
            // §434
            if odd(self.right_type(p)) != odd(self.right_type(q)) {
                let b = if odd(self.right_type(q)) {
                    self.x_coord(q)
                } else {
                    -self.x_coord(q)
                };
                let mut a;
                if (self.x_coord(q) - self.right_x(q)).abs() < 655
                    || (self.x_coord(q) + self.left_x(q)).abs() < 655
                {
                    // §435
                    let pen_edge = if self.cur_pen == NULL_PEN {
                        0
                    } else if self.cur_path_type == DOUBLE_PATH_CODE {
                        self.compromise(self.east_edge(self.cur_pen), self.west_edge(self.cur_pen))
                    } else if odd(self.right_type(q)) {
                        self.west_edge(self.cur_pen)
                    } else {
                        self.east_edge(self.cur_pen)
                    };
                    a = self.good_val(b, pen_edge);
                } else {
                    a = b;
                }
                if a.abs() > self.max_allowed {
                    a = if a > 0 {
                        self.max_allowed
                    } else {
                        -self.max_allowed
                    };
                }
                self.before_and_after(b, a, q)?;
            }
            p = q;
            if p == self.cur_spec {
                break;
            }
        }
        if self.cur_rounding_ptr > 0 {
            // §436
            self.make_safe();
            loop {
                self.cur_rounding_ptr -= 1;
                let c = self.cur_rounding_ptr as usize;
                if self.after[c] != self.before[c] || self.after[c + 1] != self.before[c + 1] {
                    let mut p = self.node_to_round[c];
                    let (b, a) = if odd(self.right_type(p)) {
                        (self.before[c], self.after[c])
                    } else {
                        (-self.before[c], -self.after[c])
                    };
                    let alpha = if self.before[c] == self.before[c + 1] {
                        FRACTION_ONE
                    } else {
                        self.make_fraction(
                            self.after[c + 1] - self.after[c],
                            self.before[c + 1] - self.before[c],
                        )
                    };
                    loop {
                        let v = self.take_fraction(alpha, self.x_coord(p) - b) + a;
                        self.set_x_coord(p, v);
                        let v = self.take_fraction(alpha, self.right_x(p) - b) + a;
                        self.set_right_x(p, v);
                        p = self.link(p);
                        let v = self.take_fraction(alpha, self.left_x(p) - b) + a;
                        self.set_left_x(p, v);
                        if p == self.node_to_round[c + 1] {
                            break;
                        }
                    }
                }
                if self.cur_rounding_ptr == 0 {
                    break;
                }
            }
        }
        p = self.cur_spec;
        self.cur_rounding_ptr = 0;
        loop {
            let q = self.link(p);
            // §437
            if (self.right_type(p) > NEGATE_Y) != (self.right_type(q) > NEGATE_Y) {
                let b = if self.right_type(q) <= NEGATE_Y {
                    self.y_coord(q)
                } else {
                    -self.y_coord(q)
                };
                let mut a;
                if (self.y_coord(q) - self.right_y(q)).abs() < 655
                    || (self.y_coord(q) + self.left_y(q)).abs() < 655
                {
                    // §438
                    let pen_edge = if self.cur_pen == NULL_PEN {
                        0
                    } else if self.cur_path_type == DOUBLE_PATH_CODE {
                        self.compromise(
                            self.north_edge(self.cur_pen),
                            self.south_edge(self.cur_pen),
                        )
                    } else if self.right_type(q) <= NEGATE_Y {
                        self.south_edge(self.cur_pen)
                    } else {
                        self.north_edge(self.cur_pen)
                    };
                    a = self.good_val(b, pen_edge);
                } else {
                    a = b;
                }
                if a.abs() > self.max_allowed {
                    a = if a > 0 {
                        self.max_allowed
                    } else {
                        -self.max_allowed
                    };
                }
                self.before_and_after(b, a, q)?;
            }
            p = q;
            if p == self.cur_spec {
                break;
            }
        }
        if self.cur_rounding_ptr > 0 {
            // §439
            self.make_safe();
            loop {
                self.cur_rounding_ptr -= 1;
                let c = self.cur_rounding_ptr as usize;
                if self.after[c] != self.before[c] || self.after[c + 1] != self.before[c + 1] {
                    let mut p = self.node_to_round[c];
                    let (b, a) = if self.right_type(p) <= NEGATE_Y {
                        (self.before[c], self.after[c])
                    } else {
                        (-self.before[c], -self.after[c])
                    };
                    let alpha = if self.before[c] == self.before[c + 1] {
                        FRACTION_ONE
                    } else {
                        self.make_fraction(
                            self.after[c + 1] - self.after[c],
                            self.before[c + 1] - self.before[c],
                        )
                    };
                    loop {
                        let v = self.take_fraction(alpha, self.y_coord(p) - b) + a;
                        self.set_y_coord(p, v);
                        let v = self.take_fraction(alpha, self.right_y(p) - b) + a;
                        self.set_right_y(p, v);
                        p = self.link(p);
                        let v = self.take_fraction(alpha, self.left_y(p) - b) + a;
                        self.set_left_y(p, v);
                        if p == self.node_to_round[c + 1] {
                            break;
                        }
                    }
                }
                if self.cur_rounding_ptr == 0 {
                    break;
                }
            }
        }
        Ok(())
    }

    // §442
    pub(crate) fn diag_offset(&self, o: i32) -> Scaled {
        self.x_coord(self.knil(self.link(self.cur_pen + o)))
    }

    // §440
    pub(crate) fn diag_round(&mut self) -> R {
        let mut p = self.cur_spec;
        self.cur_rounding_ptr = 0;
        loop {
            let q = self.link(p);
            // §441
            if self.right_type(p) != self.right_type(q) {
                let b = if self.right_type(q) > SWITCH_X_AND_Y {
                    -self.x_coord(q)
                } else {
                    self.x_coord(q)
                };
                let a;
                if (self.right_type(q) - self.right_type(p)).abs() == SWITCH_X_AND_Y {
                    if (self.x_coord(q) - self.right_x(q)).abs() < 655
                        || (self.x_coord(q) + self.left_x(q)).abs() < 655
                    {
                        // §442
                        let pen_edge = if self.cur_pen == NULL_PEN {
                            0
                        } else if self.cur_path_type == DOUBLE_PATH_CODE {
                            // §443
                            match self.right_type(q) {
                                FIRST_OCTANT | SECOND_OCTANT => self.compromise(
                                    self.diag_offset(FIRST_OCTANT),
                                    -self.diag_offset(FIFTH_OCTANT),
                                ),
                                FIFTH_OCTANT | SIXTH_OCTANT => -self.compromise(
                                    self.diag_offset(FIRST_OCTANT),
                                    -self.diag_offset(FIFTH_OCTANT),
                                ),
                                THIRD_OCTANT | FOURTH_OCTANT => self.compromise(
                                    self.diag_offset(FOURTH_OCTANT),
                                    -self.diag_offset(EIGHTH_OCTANT),
                                ),
                                _ => -self.compromise(
                                    self.diag_offset(FOURTH_OCTANT),
                                    -self.diag_offset(EIGHTH_OCTANT),
                                ),
                            }
                        } else if self.right_type(q) <= SWITCH_X_AND_Y {
                            self.diag_offset(self.right_type(q))
                        } else {
                            -self.diag_offset(self.right_type(q))
                        };
                        if odd(self.right_type(q)) {
                            a = self.good_val(b, pen_edge + halfp(self.cur_gran));
                        } else {
                            a = self.good_val(b - 1, pen_edge + halfp(self.cur_gran));
                        }
                    } else {
                        a = b;
                    }
                } else {
                    a = b;
                }
                self.before_and_after(b, a, q)?;
            }
            p = q;
            if p == self.cur_spec {
                break;
            }
        }
        if self.cur_rounding_ptr > 0 {
            // §444
            let p0 = self.node_to_round[0];
            let first_x = self.x_coord(p0);
            let first_y = self.y_coord(p0);
            // §446
            let crp = self.cur_rounding_ptr as usize;
            self.before[crp] = self.before[0];
            self.node_to_round[crp] = self.node_to_round[0];
            loop {
                self.after[crp] = self.after[0];
                let mut all_safe = true;
                let mut next_a = self.after[0];
                for k in 0..crp {
                    let mut a = next_a;
                    let mut b = self.before[k];
                    next_a = self.after[k + 1];
                    let mut aa = next_a;
                    let mut bb = self.before[k + 1];
                    if a != b || aa != bb {
                        let p = self.node_to_round[k];
                        let pp = self.node_to_round[k + 1];
                        let (c, d, cc, dd) = self.before_after_both(
                            p, pp, &mut a, &mut b, &mut aa, &mut bb, first_x, first_y,
                        );
                        if aa < a || cc < c || aa - a > 2 * (bb - b) || cc - c > 2 * (dd - d) {
                            all_safe = false;
                            self.after[k] = self.before[k];
                            if k == crp - 1 {
                                self.after[0] = self.before[0];
                            } else {
                                self.after[k + 1] = self.before[k + 1];
                            }
                        }
                    }
                }
                if all_safe {
                    break;
                }
            }
            for k in 0..crp {
                let mut a = self.after[k];
                let mut b = self.before[k];
                let mut aa = self.after[k + 1];
                let mut bb = self.before[k + 1];
                if a != b || aa != bb {
                    let mut p = self.node_to_round[k];
                    let pp = self.node_to_round[k + 1];
                    let (c, d, cc, dd) = self.before_after_both(
                        p, pp, &mut a, &mut b, &mut aa, &mut bb, first_x, first_y,
                    );
                    let alpha = if b == bb {
                        FRACTION_ONE
                    } else {
                        self.make_fraction(aa - a, bb - b)
                    };
                    let beta = if d == dd {
                        FRACTION_ONE
                    } else {
                        self.make_fraction(cc - c, dd - d)
                    };
                    loop {
                        let v = self.take_fraction(alpha, self.x_coord(p) - b) + a;
                        self.set_x_coord(p, v);
                        let v = self.take_fraction(beta, self.y_coord(p) - d) + c;
                        self.set_y_coord(p, v);
                        let v = self.take_fraction(alpha, self.right_x(p) - b) + a;
                        self.set_right_x(p, v);
                        let v = self.take_fraction(beta, self.right_y(p) - d) + c;
                        self.set_right_y(p, v);
                        p = self.link(p);
                        let v = self.take_fraction(alpha, self.left_x(p) - b) + a;
                        self.set_left_x(p, v);
                        let v = self.take_fraction(beta, self.left_y(p) - d) + c;
                        self.set_left_y(p, v);
                        if p == pp {
                            break;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    // §445: (c, d, cc, dd)
    pub(crate) fn before_after_both(
        &mut self,
        p: Pointer,
        pp: Pointer,
        a: &mut i32,
        b: &mut i32,
        aa: &mut i32,
        bb: &mut i32,
        first_x: Scaled,
        first_y: Scaled,
    ) -> (i32, i32, i32, i32) {
        let cc;
        let dd;
        if *aa == *bb {
            if pp == self.node_to_round[0] {
                self.unskew(first_x, first_y, self.right_type(pp));
            } else {
                self.unskew(self.x_coord(pp), self.y_coord(pp), self.right_type(pp));
            }
            self.skew(self.cur_x, self.cur_y, self.right_type(p));
            *bb = self.cur_x;
            *aa = *bb;
            dd = self.cur_y;
            cc = dd;
            if self.right_type(p) > SWITCH_X_AND_Y {
                *b = -*b;
                *a = -*a;
            }
        } else {
            if self.right_type(p) > SWITCH_X_AND_Y {
                *bb = -*bb;
                *aa = -*aa;
                *b = -*b;
                *a = -*a;
            }
            dd = if pp == self.node_to_round[0] {
                first_y - *bb
            } else {
                self.y_coord(pp) - *bb
            };
            if odd(*aa - *bb) {
                if self.right_type(p) > SWITCH_X_AND_Y {
                    cc = dd - half(*aa - *bb + 1);
                } else {
                    cc = dd - half(*aa - *bb - 1);
                }
            } else {
                cc = dd - half(*aa - *bb);
            }
        }
        let d = self.y_coord(p);
        let c = if odd(*a - *b) {
            if self.right_type(p) > SWITCH_X_AND_Y {
                d - half(*a - *b - 1)
            } else {
                d - half(*a - *b + 1)
            }
        } else {
            d - half(*a - *b)
        };
        (c, d, cc, dd)
    }

    // §451
    pub(crate) fn new_boundary(&mut self, p: Pointer, octant: i32) -> R {
        let q = self.link(p);
        let r = self.get_node(KNOT_NODE_SIZE)?;
        self.set_link(r, q);
        self.set_link(p, r);
        self.set_left_type(r, self.left_type(q));
        self.set_left_x(r, self.left_x(q));
        self.set_left_y(r, self.left_y(q));
        self.set_right_type(r, ENDPOINT);
        self.set_left_type(q, ENDPOINT);
        self.set_right_x(r, octant);
        self.set_left_x(q, self.right_type(q));
        self.unskew(self.x_coord(q), self.y_coord(q), self.right_type(q));
        self.skew(self.cur_x, self.cur_y, octant);
        self.set_x_coord(r, self.cur_x);
        self.set_y_coord(r, self.cur_y);
        Ok(())
    }
}

/// §408: scale up `del1`, `del2`, `del3`; the first of them not zero.
fn scale_up(del1: &mut i32, del2: &mut i32, del3: &mut i32) -> i32 {
    let del = if *del1 != 0 {
        *del1
    } else if *del2 != 0 {
        *del2
    } else {
        *del3
    };
    if del != 0 {
        let mut dmax = del1.abs();
        if del2.abs() > dmax {
            dmax = del2.abs();
        }
        if del3.abs() > dmax {
            dmax = del3.abs();
        }
        while dmax < FRACTION_HALF {
            dmax += dmax;
            *del1 += *del1;
            *del2 += *del2;
            *del3 += *del3;
        }
    }
    del
}

// §391
pub(crate) fn crossing_point(a: i32, b: i32, c: i32) -> i32 {
    const NO: i32 = FRACTION_ONE + 1;
    if a < 0 {
        return 0;
    }
    if c >= 0 {
        if b >= 0 {
            if c > 0 {
                return NO;
            } else if a == 0 && b == 0 {
                return NO;
            } else {
                return FRACTION_ONE;
            }
        }
        if a == 0 {
            return 0;
        }
    } else if a == 0 && b <= 0 {
        return 0;
    }
    // §392
    let mut d = 1;
    let mut x0 = a;
    let mut x1 = a - b;
    let mut x2 = b - c;
    loop {
        let x = half(x1 + x2);
        if x1 - x0 > x0 {
            x2 = x;
            x0 += x0;
            d += d;
        } else {
            let xx = x1 + x - x0;
            if xx > x0 {
                x2 = x;
                x0 += x0;
                d += d;
            } else {
                x0 -= xx;
                if x <= x0 && x + x2 <= x0 {
                    return NO;
                }
                x1 = x;
                d = d + d + 1;
            }
        }
        if d >= FRACTION_ONE {
            break;
        }
    }
    d - FRACTION_ONE
}
