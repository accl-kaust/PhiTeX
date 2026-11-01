//! Parts 28 and 29 of `mf.web`: dynamic linear and nonlinear equations.

use crate::arith::{halfp, round_fraction};
use crate::{Mf, Pointer, R, Scaled, consts::*};

const COEF_BOUND: i32 = 0o4525252525;
const INDEPENDENT_BEING_FIXED: i32 = 1;

impl Mf {
    // §587
    #[inline]
    pub(crate) fn dep_list(&self, p: Pointer) -> Pointer {
        self.link(p + 1)
    }
    #[inline]
    pub(crate) fn set_dep_list(&mut self, p: Pointer, v: Pointer) {
        self.set_link(p + 1, v);
    }
    #[inline]
    pub(crate) fn prev_dep(&self, p: Pointer) -> Pointer {
        self.info(p + 1)
    }
    #[inline]
    pub(crate) fn set_prev_dep(&mut self, p: Pointer, v: Pointer) {
        self.set_info(p + 1, v);
    }
    // §586
    pub(crate) fn new_indep(&mut self, p: Pointer) -> R {
        if self.serial_no > EL_GORDO - S_SCALE {
            return self.overflow("independent variables", self.serial_no / S_SCALE);
        }
        self.set_type(p, INDEPENDENT);
        self.serial_no += S_SCALE;
        self.set_value(p, self.serial_no);
        Ok(())
    }

    // §589
    pub(crate) fn print_dependency(&mut self, mut p: Pointer, t: i32) -> R {
        let pp = p;
        loop {
            let mut v = self.value(p).abs();
            let q = self.info(p);
            if q == NULL {
                if v != 0 || p == pp {
                    if self.value(p) > 0 && p != pp {
                        self.print_char(b'+');
                    }
                    self.print_scaled(self.value(p));
                }
                return Ok(());
            }
            // §590
            if self.value(p) < 0 {
                self.print_char(b'-');
            } else if p != pp {
                self.print_char(b'+');
            }
            if t == DEPENDENT {
                v = round_fraction(v);
            }
            if v != UNITY {
                self.print_scaled(v);
            }
            if self.type_(q) != INDEPENDENT {
                return self.confusion("dep");
            }
            self.print_variable_name(q)?;
            let mut v = self.value(q) % S_SCALE;
            while v > 0 {
                self.prints("*4");
                v -= 2;
            }
            p = self.link(p);
        }
    }

    // §591
    pub(crate) fn max_coef(&self, mut p: Pointer) -> i32 {
        let mut x = 0;
        while self.info(p) != NULL {
            if self.value(p).abs() > x {
                x = self.value(p).abs();
            }
            p = self.link(p);
        }
        x
    }

    // §594
    pub(crate) fn p_plus_fq(
        &mut self,
        mut p: Pointer,
        f: i32,
        mut q: Pointer,
        t: i32,
        tt: i32,
    ) -> R<Pointer> {
        let threshold = if t == DEPENDENT {
            FRACTION_THRESHOLD
        } else {
            SCALED_THRESHOLD
        };
        let mut r = self.temp_head();
        let mut pp = self.info(p);
        let mut qq = self.info(q);
        loop {
            if pp == qq {
                if pp == NULL {
                    break;
                }
                // §595
                let v = if tt == DEPENDENT {
                    self.value(p) + self.take_fraction(f, self.value(q))
                } else {
                    self.value(p) + self.take_scaled(f, self.value(q))
                };
                self.set_value(p, v);
                let s = p;
                p = self.link(p);
                if v.abs() < threshold {
                    self.free_node(s, DEP_NODE_SIZE);
                } else {
                    if v.abs() >= COEF_BOUND && self.watch_coefs {
                        self.set_type(qq, INDEPENDENT_NEEDING_FIX);
                        self.fix_needed = true;
                    }
                    self.set_link(r, s);
                    r = s;
                }
                pp = self.info(p);
                q = self.link(q);
                qq = self.info(q);
            } else if self.value(pp) < self.value(qq) {
                // §596
                let v = if tt == DEPENDENT {
                    self.take_fraction(f, self.value(q))
                } else {
                    self.take_scaled(f, self.value(q))
                };
                if v.abs() > halfp(threshold) {
                    let s = self.get_node(DEP_NODE_SIZE)?;
                    self.set_info(s, qq);
                    self.set_value(s, v);
                    if v.abs() >= COEF_BOUND && self.watch_coefs {
                        self.set_type(qq, INDEPENDENT_NEEDING_FIX);
                        self.fix_needed = true;
                    }
                    self.set_link(r, s);
                    r = s;
                }
                q = self.link(q);
                qq = self.info(q);
            } else {
                self.set_link(r, p);
                r = p;
                p = self.link(p);
                pp = self.info(p);
            }
        }
        let v = if t == DEPENDENT {
            self.take_fraction(self.value(q), f)
        } else {
            self.take_scaled(self.value(q), f)
        };
        let s = self.slow_add(self.value(p), v);
        self.set_value(p, s);
        self.set_link(r, p);
        self.dep_final = p;
        Ok(self.link(self.temp_head()))
    }

    // §597
    pub(crate) fn p_plus_q(&mut self, mut p: Pointer, mut q: Pointer, t: i32) -> R<Pointer> {
        let threshold = if t == DEPENDENT {
            FRACTION_THRESHOLD
        } else {
            SCALED_THRESHOLD
        };
        let mut r = self.temp_head();
        let mut pp = self.info(p);
        let mut qq = self.info(q);
        loop {
            if pp == qq {
                if pp == NULL {
                    break;
                }
                // §598
                let v = self.value(p) + self.value(q);
                self.set_value(p, v);
                let s = p;
                p = self.link(p);
                pp = self.info(p);
                if v.abs() < threshold {
                    self.free_node(s, DEP_NODE_SIZE);
                } else {
                    if v.abs() >= COEF_BOUND && self.watch_coefs {
                        self.set_type(qq, INDEPENDENT_NEEDING_FIX);
                        self.fix_needed = true;
                    }
                    self.set_link(r, s);
                    r = s;
                }
                q = self.link(q);
                qq = self.info(q);
            } else if self.value(pp) < self.value(qq) {
                let s = self.get_node(DEP_NODE_SIZE)?;
                self.set_info(s, qq);
                self.set_value(s, self.value(q));
                q = self.link(q);
                qq = self.info(q);
                self.set_link(r, s);
                r = s;
            } else {
                self.set_link(r, p);
                r = p;
                p = self.link(p);
                pp = self.info(p);
            }
        }
        let s = self.slow_add(self.value(p), self.value(q));
        self.set_value(p, s);
        self.set_link(r, p);
        self.dep_final = p;
        Ok(self.link(self.temp_head()))
    }

    // §599
    pub(crate) fn p_times_v(
        &mut self,
        mut p: Pointer,
        v: i32,
        t0: i32,
        t1: i32,
        v_is_scaled: bool,
    ) -> Pointer {
        let scaling_down = if t0 != t1 { true } else { !v_is_scaled };
        let threshold = if t1 == DEPENDENT {
            HALF_FRACTION_THRESHOLD
        } else {
            HALF_SCALED_THRESHOLD
        };
        let mut r = self.temp_head();
        while self.info(p) != NULL {
            let w = if scaling_down {
                self.take_fraction(v, self.value(p))
            } else {
                self.take_scaled(v, self.value(p))
            };
            if w.abs() <= threshold {
                let s = self.link(p);
                self.free_node(p, DEP_NODE_SIZE);
                p = s;
            } else {
                if w.abs() >= COEF_BOUND {
                    self.fix_needed = true;
                    let ip = self.info(p);
                    self.set_type(ip, INDEPENDENT_NEEDING_FIX);
                }
                self.set_link(r, p);
                r = p;
                self.set_value(p, w);
                p = self.link(p);
            }
        }
        self.set_link(r, p);
        let w = if v_is_scaled {
            self.take_scaled(self.value(p), v)
        } else {
            self.take_fraction(self.value(p), v)
        };
        self.set_value(p, w);
        self.link(self.temp_head())
    }

    // §600
    pub(crate) fn p_over_v(&mut self, mut p: Pointer, v: Scaled, t0: i32, t1: i32) -> Pointer {
        let scaling_down = t0 != t1;
        let threshold = if t1 == DEPENDENT {
            HALF_FRACTION_THRESHOLD
        } else {
            HALF_SCALED_THRESHOLD
        };
        let mut r = self.temp_head();
        while self.info(p) != NULL {
            let w = if scaling_down {
                if v.abs() < 0o2000000 {
                    self.make_scaled(self.value(p), v * 0o10000)
                } else {
                    self.make_scaled(round_fraction(self.value(p)), v)
                }
            } else {
                self.make_scaled(self.value(p), v)
            };
            if w.abs() <= threshold {
                let s = self.link(p);
                self.free_node(p, DEP_NODE_SIZE);
                p = s;
            } else {
                if w.abs() >= COEF_BOUND {
                    self.fix_needed = true;
                    let ip = self.info(p);
                    self.set_type(ip, INDEPENDENT_NEEDING_FIX);
                }
                self.set_link(r, p);
                r = p;
                self.set_value(p, w);
                p = self.link(p);
            }
        }
        self.set_link(r, p);
        let w = self.make_scaled(self.value(p), v);
        self.set_value(p, w);
        self.link(self.temp_head())
    }

    // §601
    pub(crate) fn p_with_x_becoming_q(
        &mut self,
        p: Pointer,
        x: Pointer,
        q: Pointer,
        t: i32,
    ) -> R<Pointer> {
        let mut s = p;
        let mut r = self.temp_head();
        let sx = self.value(x);
        while self.value(self.info(s)) > sx {
            r = s;
            s = self.link(s);
        }
        if self.info(s) != x {
            Ok(p)
        } else {
            let th = self.temp_head();
            self.set_link(th, p);
            self.set_link(r, self.link(s));
            let v = self.value(s);
            self.free_node(s, DEP_NODE_SIZE);
            let l = self.link(th);
            self.p_plus_fq(l, v, q, t, DEPENDENT)
        }
    }

    // §602
    pub(crate) fn val_too_big(&mut self, x: Scaled) -> R {
        if self.int(WARNING_CHECK) > 0 {
            self.print_err("Value is too large (");
            self.print_scaled(x);
            self.print_char(b')');
            self.help(&[
                "The equation I just processed has given some variable",
                "a value of 4096 or more. Continue and I'll try to cope",
                "with that big value; but it might be dangerous.",
                "(Set warningcheck:=0 to suppress this message.)",
            ]);
            self.error()?;
        }
        Ok(())
    }

    // §603
    pub(crate) fn make_known(&mut self, p: Pointer, q: Pointer) -> R {
        let lq = self.link(q);
        self.set_prev_dep(lq, self.prev_dep(p));
        let pd = self.prev_dep(p);
        self.set_link(pd, lq);
        let t = self.type_(p);
        self.set_type(p, KNOWN);
        self.set_value(p, self.value(q));
        self.free_node(q, DEP_NODE_SIZE);
        if self.value(p).abs() >= FRACTION_ONE {
            self.val_too_big(self.value(p))?;
        }
        if self.int(TRACING_EQUATIONS) > 0 && self.interesting(p) {
            self.begin_diagnostic();
            self.print_nl("#### ");
            self.print_variable_name(p)?;
            self.print_char(b'=');
            self.print_scaled(self.value(p));
            self.end_diagnostic(false);
        }
        if self.cur_exp == p && self.cur_type == t {
            self.cur_type = KNOWN;
            self.cur_exp = self.value(p);
            self.free_node(p, VALUE_NODE_SIZE);
        }
        Ok(())
    }

    // §604
    pub(crate) fn fix_dependencies(&mut self) -> R {
        let mut r = self.link(DEP_HEAD);
        let mut s = NULL;
        while r != DEP_HEAD {
            let t = r;
            // §605
            r = t + 1;
            let mut q;
            loop {
                q = self.link(r);
                let x = self.info(q);
                if x == NULL {
                    break;
                }
                if self.type_(x) <= INDEPENDENT_BEING_FIXED {
                    if self.type_(x) < INDEPENDENT_BEING_FIXED {
                        let p = self.get_avail()?;
                        self.set_link(p, s);
                        s = p;
                        self.set_info(s, x);
                        self.set_type(x, INDEPENDENT_BEING_FIXED);
                    }
                    self.set_value(q, self.value(q) / 4);
                    if self.value(q) == 0 {
                        self.set_link(r, self.link(q));
                        self.free_node(q, DEP_NODE_SIZE);
                        q = r;
                    }
                }
                r = q;
            }
            r = self.link(q);
            if q == self.dep_list(t) {
                self.make_known(t, q)?;
            }
        }
        while s != NULL {
            let p = self.link(s);
            let x = self.info(s);
            self.free_avail(s);
            s = p;
            self.set_type(x, INDEPENDENT);
            self.set_value(x, self.value(x) + 2);
        }
        self.fix_needed = false;
        Ok(())
    }

    // §606
    pub(crate) fn new_dep(&mut self, q: Pointer, p: Pointer) {
        self.set_dep_list(q, p);
        self.set_prev_dep(q, DEP_HEAD);
        let r = self.link(DEP_HEAD);
        self.set_link(self.dep_final, r);
        self.set_prev_dep(r, self.dep_final);
        self.set_link(DEP_HEAD, q);
    }

    // §607
    pub(crate) fn const_dependency(&mut self, v: Scaled) -> R<Pointer> {
        self.dep_final = self.get_node(DEP_NODE_SIZE)?;
        self.set_value(self.dep_final, v);
        self.set_info(self.dep_final, NULL);
        Ok(self.dep_final)
    }

    // §608
    pub(crate) fn single_dependency(&mut self, p: Pointer) -> R<Pointer> {
        let m = self.value(p) % S_SCALE;
        if m > 28 {
            self.const_dependency(0)
        } else {
            let q = self.get_node(DEP_NODE_SIZE)?;
            self.set_value(q, self.two_to_the[(28 - m) as usize]);
            self.set_info(q, p);
            let c = self.const_dependency(0)?;
            self.set_link(q, c);
            Ok(q)
        }
    }

    // §609
    pub(crate) fn copy_dep_list(&mut self, mut p: Pointer) -> R<Pointer> {
        let q = self.get_node(DEP_NODE_SIZE)?;
        self.dep_final = q;
        loop {
            self.set_info(self.dep_final, self.info(p));
            self.set_value(self.dep_final, self.value(p));
            if self.info(self.dep_final) == NULL {
                break;
            }
            let n = self.get_node(DEP_NODE_SIZE)?;
            self.set_link(self.dep_final, n);
            self.dep_final = n;
            p = self.link(p);
        }
        Ok(q)
    }

    // §610
    pub(crate) fn linear_eq(&mut self, mut p: Pointer, t: i32) -> R {
        // §611
        let mut q = p;
        let mut r = self.link(p);
        let mut v = self.value(q);
        while self.info(r) != NULL {
            if self.value(r).abs() > v.abs() {
                q = r;
                v = self.value(r);
            }
            r = self.link(r);
        }
        let x = self.info(q);
        let mut n = self.value(x) % S_SCALE;
        // §612
        let th = self.temp_head();
        let mut s = th;
        self.set_link(s, p);
        r = p;
        loop {
            if r == q {
                self.set_link(s, self.link(r));
                self.free_node(r, DEP_NODE_SIZE);
            } else {
                let w = self.make_fraction(self.value(r), v);
                if w.abs() <= HALF_FRACTION_THRESHOLD {
                    self.set_link(s, self.link(r));
                    self.free_node(r, DEP_NODE_SIZE);
                } else {
                    self.set_value(r, -w);
                    s = r;
                }
            }
            r = self.link(s);
            if self.info(r) == NULL {
                break;
            }
        }
        if t == PROTO_DEPENDENT {
            let w = self.make_scaled(self.value(r), v);
            self.set_value(r, -w);
        } else if v != -FRACTION_ONE {
            let w = self.make_fraction(self.value(r), v);
            self.set_value(r, -w);
        }
        let final_node = r;
        p = self.link(th);
        if self.int(TRACING_EQUATIONS) > 0 {
            // §613
            if self.interesting(x) {
                self.begin_diagnostic();
                self.print_nl("## ");
                self.print_variable_name(x)?;
                let mut w = n;
                while w > 0 {
                    self.prints("*4");
                    w -= 2;
                }
                self.print_char(b'=');
                self.print_dependency(p, DEPENDENT)?;
                self.end_diagnostic(false);
            }
        }
        // §614
        let mut prev_r = DEP_HEAD;
        r = self.link(DEP_HEAD);
        while r != DEP_HEAD {
            let s = self.dep_list(r);
            let tr = self.type_(r);
            let mut q = self.p_with_x_becoming_q(s, x, p, tr)?;
            if self.info(q) == NULL {
                self.make_known(r, q)?;
            } else {
                self.set_dep_list(r, q);
                loop {
                    q = self.link(q);
                    if self.info(q) == NULL {
                        break;
                    }
                }
                prev_r = q;
            }
            r = self.link(prev_r);
        }
        // §615
        if n > 0 {
            // §616
            let mut s = th;
            self.set_link(th, p);
            r = p;
            loop {
                let w = if n > 30 {
                    0
                } else {
                    self.value(r) / self.two_to_the[n as usize]
                };
                if w.abs() <= HALF_FRACTION_THRESHOLD && self.info(r) != NULL {
                    self.set_link(s, self.link(r));
                    self.free_node(r, DEP_NODE_SIZE);
                } else {
                    self.set_value(r, w);
                    s = r;
                }
                r = self.link(s);
                if self.info(s) == NULL {
                    break;
                }
            }
            p = self.link(th);
            n = 0;
        }
        let _ = n;
        if self.info(p) == NULL {
            self.set_type(x, KNOWN);
            self.set_value(x, self.value(p));
            if self.value(x).abs() >= FRACTION_ONE {
                self.val_too_big(self.value(x))?;
            }
            self.free_node(p, DEP_NODE_SIZE);
            if self.cur_exp == x && self.cur_type == INDEPENDENT {
                self.cur_exp = self.value(x);
                self.cur_type = KNOWN;
                self.free_node(x, VALUE_NODE_SIZE);
            }
        } else {
            self.set_type(x, DEPENDENT);
            self.dep_final = final_node;
            self.new_dep(x, p);
            if self.cur_exp == x && self.cur_type == INDEPENDENT {
                self.cur_type = DEPENDENT;
            }
        }
        if self.fix_needed {
            self.fix_dependencies()?;
        }
        Ok(())
    }

    // §619
    pub(crate) fn new_ring_entry(&mut self, p: Pointer) -> R<Pointer> {
        let q = self.get_node(VALUE_NODE_SIZE)?;
        self.set_name_type(q, CAPSULE);
        self.set_type(q, self.type_(p));
        if self.value(p) == NULL {
            self.set_value(q, p);
        } else {
            self.set_value(q, self.value(p));
        }
        self.set_value(p, q);
        Ok(q)
    }

    // §620
    pub(crate) fn ring_delete(&mut self, p: Pointer) {
        let mut q = self.value(p);
        if q != NULL && q != p {
            while self.value(q) != p {
                q = self.value(q);
            }
            self.set_value(q, self.value(p));
        }
    }

    // §621
    pub(crate) fn nonlinear_eq(&mut self, v: i32, mut p: Pointer, flush_p: bool) -> R {
        let t = self.type_(p) - UNKNOWN_TAG;
        let mut q = self.value(p);
        if flush_p {
            self.set_type(p, VACUOUS);
        } else {
            p = q;
        }
        loop {
            let r = self.value(q);
            self.set_type(q, t);
            match t {
                BOOLEAN_TYPE => self.set_value(q, v),
                STRING_TYPE => {
                    self.set_value(q, v);
                    self.add_str_ref(v);
                }
                PEN_TYPE => {
                    self.set_value(q, v);
                    self.add_pen_ref(v);
                }
                PATH_TYPE => {
                    let c = self.copy_path(v)?;
                    self.set_value(q, c);
                }
                PICTURE_TYPE => {
                    let c = self.copy_edges(v)?;
                    self.set_value(q, c);
                }
                _ => {}
            }
            q = r;
            if q == p {
                break;
            }
        }
        Ok(())
    }

    // §622
    pub(crate) fn ring_merge(&mut self, p: Pointer, q: Pointer) -> R {
        let mut r = self.value(p);
        while r != p {
            if r == q {
                // §623
                self.print_err("Redundant equation");
                self.help(&[
                    "I already knew that this equation was true.",
                    "But perhaps no harm has been done; let's continue.",
                ]);
                return self.put_get_error();
            }
            r = self.value(r);
        }
        r = self.value(p);
        self.set_value(p, self.value(q));
        self.set_value(q, r);
        Ok(())
    }
}
