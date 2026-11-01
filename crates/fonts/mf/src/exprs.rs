//! Part 39 of `mf.web`: introduction to the parsing routines (stashing,
//! printing and recycling expressions).

use crate::arith::round_fraction;
use crate::{Mf, Pointer, R, Scaled, consts::*};

#[inline]
fn is_unknown_type(t: i32) -> bool {
    matches!(
        t,
        UNKNOWN_BOOLEAN | UNKNOWN_STRING | UNKNOWN_PEN | UNKNOWN_PICTURE | UNKNOWN_PATH
    )
}

impl Mf {
    // §799
    pub(crate) fn stash_cur_exp(&mut self) -> R<Pointer> {
        let p;
        let t = self.cur_type;
        if is_unknown_type(t)
            || matches!(
                t,
                TRANSFORM_TYPE | PAIR_TYPE | DEPENDENT | PROTO_DEPENDENT | INDEPENDENT
            )
        {
            p = self.cur_exp;
        } else {
            p = self.get_node(VALUE_NODE_SIZE)?;
            self.set_name_type(p, CAPSULE);
            self.set_type(p, self.cur_type);
            self.set_value(p, self.cur_exp);
        }
        self.cur_type = VACUOUS;
        self.set_link(p, VOID);
        Ok(p)
    }

    // §800
    pub(crate) fn unstash_cur_exp(&mut self, p: Pointer) -> R {
        self.cur_type = self.type_(p);
        let t = self.cur_type;
        if is_unknown_type(t)
            || matches!(
                t,
                TRANSFORM_TYPE | PAIR_TYPE | DEPENDENT | PROTO_DEPENDENT | INDEPENDENT
            )
        {
            self.cur_exp = p;
        } else {
            self.cur_exp = self.value(p);
            self.free_node(p, VALUE_NODE_SIZE);
        }
        Ok(())
    }

    // §801
    pub(crate) fn print_exp(&mut self, mut p: Pointer, verbosity: i32) -> R {
        let restore_cur_exp;
        if p != NULL {
            restore_cur_exp = false;
        } else {
            p = self.stash_cur_exp()?;
            restore_cur_exp = true;
        }
        let t = self.type_(p);
        let mut v = 0;
        if t < DEPENDENT {
            v = self.value(p);
        } else if t < INDEPENDENT {
            v = self.dep_list(p);
        }
        // §802
        match t {
            VACUOUS => self.prints("vacuous"),
            BOOLEAN_TYPE => {
                if v == TRUE_CODE {
                    self.prints("true");
                } else {
                    self.prints("false");
                }
            }
            UNKNOWN_BOOLEAN | UNKNOWN_STRING | UNKNOWN_PEN | UNKNOWN_PICTURE | UNKNOWN_PATH
            | NUMERIC_TYPE => {
                // §806
                self.print_type(t);
                if v != NULL {
                    self.print_char(b' ');
                    while self.name_type(v) == CAPSULE && v != p {
                        v = self.value(v);
                    }
                    self.print_variable_name(v)?;
                }
            }
            STRING_TYPE => {
                self.print_char(b'"');
                self.slow_print(v);
                self.print_char(b'"');
            }
            PEN_TYPE | FUTURE_PEN | PATH_TYPE | PICTURE_TYPE => {
                // §804
                if verbosity <= 1 {
                    self.print_type(t);
                } else {
                    if self.selector == TERM_AND_LOG && self.int(TRACING_ONLINE) <= 0 {
                        self.selector = TERM_ONLY;
                        self.print_type(t);
                        self.prints(" (see the transcript file)");
                        self.selector = TERM_AND_LOG;
                    }
                    match t {
                        PEN_TYPE => self.print_pen(v, "", false),
                        FUTURE_PEN => self.print_path(v, " (future pen)", false),
                        PATH_TYPE => self.print_path(v, "", false),
                        _ => {
                            self.cur_edges = v;
                            self.print_edges("", false, 0, 0);
                        }
                    }
                }
            }
            TRANSFORM_TYPE | PAIR_TYPE => {
                if v == NULL {
                    self.print_type(t);
                } else {
                    // §803
                    self.print_char(b'(');
                    let q = v + self.big_node_size[(t - TRANSFORM_TYPE) as usize];
                    loop {
                        if self.type_(v) == KNOWN {
                            self.print_scaled(self.value(v));
                        } else if self.type_(v) == INDEPENDENT {
                            self.print_variable_name(v)?;
                        } else {
                            self.print_dp(self.type_(v), self.dep_list(v), verbosity)?;
                        }
                        v += 2;
                        if v != q {
                            self.print_char(b',');
                        }
                        if v == q {
                            break;
                        }
                    }
                    self.print_char(b')');
                }
            }
            KNOWN => self.print_scaled(v),
            DEPENDENT | PROTO_DEPENDENT => self.print_dp(t, v, verbosity)?,
            INDEPENDENT => self.print_variable_name(p)?,
            _ => return self.confusion("exp"),
        }
        if restore_cur_exp {
            self.unstash_cur_exp(p)?;
        }
        Ok(())
    }

    // §805
    pub(crate) fn print_dp(&mut self, t: i32, p: Pointer, verbosity: i32) -> R {
        let q = self.link(p);
        if self.info(q) == NULL || verbosity > 0 {
            self.print_dependency(p, t)
        } else {
            self.prints("linearform");
            Ok(())
        }
    }

    // §807
    pub(crate) fn disp_err(&mut self, p: Pointer, s: &str) -> R {
        self.print_nl(">> ");
        self.print_exp(p, 1)?;
        if !s.is_empty() {
            self.print_nl("! ");
            self.prints(s);
        }
        Ok(())
    }
    pub(crate) fn exp_err(&mut self, s: &str) -> R {
        self.disp_err(NULL, s)
    }

    // §808
    pub(crate) fn flush_cur_exp(&mut self, v: Scaled) -> R {
        let t = self.cur_type;
        if is_unknown_type(t)
            || matches!(
                t,
                TRANSFORM_TYPE | PAIR_TYPE | DEPENDENT | PROTO_DEPENDENT | INDEPENDENT
            )
        {
            self.recycle_value(self.cur_exp)?;
            self.free_node(self.cur_exp, VALUE_NODE_SIZE);
        } else {
            match t {
                PEN_TYPE => self.delete_pen_ref(self.cur_exp),
                STRING_TYPE => self.delete_str_ref(self.cur_exp),
                FUTURE_PEN | PATH_TYPE => self.toss_knot_list(self.cur_exp),
                PICTURE_TYPE => self.toss_edges(self.cur_exp),
                _ => {}
            }
        }
        self.cur_type = KNOWN;
        self.cur_exp = v;
        Ok(())
    }

    // §809
    pub(crate) fn recycle_value(&mut self, p: Pointer) -> R {
        let mut t = self.type_(p);
        let mut v = 0;
        if t < DEPENDENT {
            v = self.value(p);
        }
        match t {
            UNDEFINED | VACUOUS | BOOLEAN_TYPE | KNOWN | NUMERIC_TYPE => {}
            UNKNOWN_BOOLEAN | UNKNOWN_STRING | UNKNOWN_PEN | UNKNOWN_PICTURE | UNKNOWN_PATH => {
                self.ring_delete(p)
            }
            STRING_TYPE => self.delete_str_ref(v),
            PEN_TYPE => self.delete_pen_ref(v),
            PATH_TYPE | FUTURE_PEN => self.toss_knot_list(v),
            PICTURE_TYPE => self.toss_edges(v),
            PAIR_TYPE | TRANSFORM_TYPE => {
                // §810
                if v != NULL {
                    let bns = self.big_node_size[(t - TRANSFORM_TYPE) as usize];
                    let mut q = v + bns;
                    loop {
                        q -= 2;
                        self.recycle_value(q)?;
                        if q == v {
                            break;
                        }
                    }
                    self.free_node(v, bns);
                }
            }
            DEPENDENT | PROTO_DEPENDENT => {
                // §811
                let mut q = self.dep_list(p);
                while self.info(q) != NULL {
                    q = self.link(q);
                }
                let pd = self.prev_dep(p);
                self.set_link(pd, self.link(q));
                let lq = self.link(q);
                self.set_prev_dep(lq, pd);
                self.set_link(q, NULL);
                self.flush_node_list(self.dep_list(p));
            }
            INDEPENDENT => {
                // §812
                self.max_c = [0, 0];
                self.max_link = [NULL, NULL];
                let mut q = self.link(DEP_HEAD);
                while q != DEP_HEAD {
                    let mut s = q + 1;
                    let r;
                    loop {
                        let rr = self.link(s);
                        if self.info(rr) == NULL {
                            r = rr;
                            break;
                        }
                        if self.info(rr) != p {
                            s = rr;
                        } else {
                            t = self.type_(q);
                            let ti = (t - DEPENDENT) as usize;
                            self.set_link(s, self.link(rr));
                            self.set_info(rr, q);
                            if self.value(rr).abs() > self.max_c[ti] {
                                // §814
                                if self.max_c[ti] > 0 {
                                    self.set_link(self.max_ptr[ti], self.max_link[ti]);
                                    self.max_link[ti] = self.max_ptr[ti];
                                }
                                self.max_c[ti] = self.value(rr).abs();
                                self.max_ptr[ti] = rr;
                            } else {
                                self.set_link(rr, self.max_link[ti]);
                                self.max_link[ti] = rr;
                            }
                        }
                    }
                    q = self.link(r);
                }
                if self.max_c[0] > 0 || self.max_c[1] > 0 {
                    // §815
                    t = if self.max_c[0] / 0o10000 >= self.max_c[1] {
                        DEPENDENT
                    } else {
                        PROTO_DEPENDENT
                    };
                    // §816
                    let ti = (t - DEPENDENT) as usize;
                    let s = self.max_ptr[ti];
                    let pp = self.info(s);
                    v = self.value(s);
                    if t == DEPENDENT {
                        self.set_value(s, -FRACTION_ONE);
                    } else {
                        self.set_value(s, -UNITY);
                    }
                    let mut r = self.dep_list(pp);
                    self.set_link(s, r);
                    while self.info(r) != NULL {
                        r = self.link(r);
                    }
                    let q = self.link(r);
                    self.set_link(r, NULL);
                    self.set_prev_dep(q, self.prev_dep(pp));
                    let pdp = self.prev_dep(pp);
                    self.set_link(pdp, q);
                    self.new_indep(pp)?;
                    if self.cur_exp == pp && self.cur_type == t {
                        self.cur_type = INDEPENDENT;
                    }
                    if self.int(TRACING_EQUATIONS) > 0 {
                        // §817
                        if self.interesting(p) {
                            self.begin_diagnostic();
                            self.print_nl("### ");
                            if v > 0 {
                                self.print_char(b'-');
                            }
                            let vv = if t == DEPENDENT {
                                round_fraction(self.max_c[0])
                            } else {
                                self.max_c[1]
                            };
                            if vv != UNITY {
                                self.print_scaled(vv);
                            }
                            self.print_variable_name(p)?;
                            while self.value(p) % S_SCALE > 0 {
                                self.prints("*4");
                                self.set_value(p, self.value(p) - 2);
                            }
                            if t == DEPENDENT {
                                self.print_char(b'=');
                            } else {
                                self.prints(" = ");
                            }
                            self.print_dependency(s, t)?;
                            self.end_diagnostic(false);
                        }
                    }
                    t = DEPENDENT + PROTO_DEPENDENT - t;
                    let ti = (t - DEPENDENT) as usize;
                    if self.max_c[ti] > 0 {
                        self.set_link(self.max_ptr[ti], self.max_link[ti]);
                        self.max_link[ti] = self.max_ptr[ti];
                    }
                    if t != DEPENDENT {
                        // §818
                        for tt in DEPENDENT..=PROTO_DEPENDENT {
                            let mut r = self.max_link[(tt - DEPENDENT) as usize];
                            while r != NULL {
                                let q = self.info(r);
                                let f = self.make_fraction(self.value(r), -v);
                                let d = self.p_plus_fq(self.dep_list(q), f, s, tt, DEPENDENT)?;
                                self.set_dep_list(q, d);
                                if self.dep_list(q) == self.dep_final {
                                    self.make_known(q, self.dep_final)?;
                                }
                                let qq = r;
                                r = self.link(r);
                                self.free_node(qq, DEP_NODE_SIZE);
                            }
                        }
                    } else {
                        // §819
                        for tt in DEPENDENT..=PROTO_DEPENDENT {
                            let mut r = self.max_link[(tt - DEPENDENT) as usize];
                            while r != NULL {
                                let q = self.info(r);
                                if tt == DEPENDENT {
                                    if self.cur_exp == q && self.cur_type == DEPENDENT {
                                        self.cur_type = PROTO_DEPENDENT;
                                    }
                                    let d = self.p_over_v(
                                        self.dep_list(q),
                                        UNITY,
                                        DEPENDENT,
                                        PROTO_DEPENDENT,
                                    );
                                    self.set_dep_list(q, d);
                                    self.set_type(q, PROTO_DEPENDENT);
                                    self.set_value(r, round_fraction(self.value(r)));
                                }
                                let f = self.make_scaled(self.value(r), -v);
                                let d = self.p_plus_fq(
                                    self.dep_list(q),
                                    f,
                                    s,
                                    PROTO_DEPENDENT,
                                    PROTO_DEPENDENT,
                                )?;
                                self.set_dep_list(q, d);
                                if self.dep_list(q) == self.dep_final {
                                    self.make_known(q, self.dep_final)?;
                                }
                                let qq = r;
                                r = self.link(r);
                                self.free_node(qq, DEP_NODE_SIZE);
                            }
                        }
                    }
                    self.flush_node_list(s);
                    if self.fix_needed {
                        self.fix_dependencies()?;
                    }
                    self.check_arith()?;
                }
            }
            TOKEN_LIST | STRUCTURED => return self.confusion("recycle"),
            UNSUFFIXED_MACRO | SUFFIXED_MACRO => self.delete_mac_ref(self.value(p))?,
            _ => {}
        }
        self.set_type(p, UNDEFINED);
        Ok(())
    }

    // §820
    pub(crate) fn flush_error(&mut self, v: Scaled) -> R {
        self.error()?;
        self.flush_cur_exp(v)
    }
    pub(crate) fn put_get_error(&mut self) -> R {
        self.back_error()?;
        self.get_x_next()
    }
    pub(crate) fn put_get_flush_error(&mut self, v: Scaled) -> R {
        self.put_get_error()?;
        self.flush_cur_exp(v)
    }
}
