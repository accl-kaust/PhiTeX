//! Parts 36 and 37 of `mf.web`: conditional processing, iterations.

use crate::{Mf, Pointer, R, consts::*};

impl Mf {
    // §738
    #[inline]
    fn if_line_field(&self, p: Pointer) -> i32 {
        self.mem[(p + 1) as usize].rh
    }
    #[inline]
    fn set_if_line_field(&mut self, p: Pointer, v: i32) {
        self.mem[(p + 1) as usize].rh = v;
    }
    // §752
    #[inline]
    pub(crate) fn loop_type(&self, p: Pointer) -> Pointer {
        self.info(p + 1)
    }
    #[inline]
    fn set_loop_type(&mut self, p: Pointer, v: Pointer) {
        self.set_info(p + 1, v);
    }
    #[inline]
    fn loop_list(&self, p: Pointer) -> Pointer {
        self.link(p + 1)
    }
    #[inline]
    fn set_loop_list(&mut self, p: Pointer, v: Pointer) {
        self.set_link(p + 1, v);
    }
    #[inline]
    fn step_size(&self, p: Pointer) -> i32 {
        self.mem[(p + 2) as usize].rh
    }
    #[inline]
    fn final_value(&self, p: Pointer) -> i32 {
        self.mem[(p + 3) as usize].rh
    }

    // §742
    pub(crate) fn pass_text(&mut self) -> R {
        self.scanner_status = SKIPPING;
        let mut l = 0;
        self.warning_info = self.line;
        loop {
            self.get_next()?;
            if self.cur_cmd <= FI_OR_ELSE {
                if self.cur_cmd < FI_OR_ELSE {
                    l += 1;
                } else {
                    if l == 0 {
                        break;
                    }
                    if self.cur_mod == FI_CODE {
                        l -= 1;
                    }
                }
            } else if self.cur_cmd == STRING_TOKEN {
                self.delete_str_ref(self.cur_mod);
            }
        }
        self.scanner_status = NORMAL;
        Ok(())
    }

    // §745
    fn push_cond(&mut self) -> R {
        let p = self.get_node(IF_NODE_SIZE)?;
        self.set_link(p, self.cond_ptr);
        self.set_type(p, self.if_limit);
        self.set_name_type(p, self.cur_if);
        self.set_if_line_field(p, self.if_line);
        self.cond_ptr = p;
        self.if_limit = IF_CODE;
        self.if_line = self.line;
        self.cur_if = IF_CODE;
        Ok(())
    }
    // §746
    pub(crate) fn pop_cond(&mut self) {
        let p = self.cond_ptr;
        self.if_line = self.if_line_field(p);
        self.cur_if = self.name_type(p);
        self.if_limit = self.type_(p);
        self.cond_ptr = self.link(p);
        self.free_node(p, IF_NODE_SIZE);
    }

    // §747
    pub(crate) fn change_if_limit(&mut self, l: i32, p: Pointer) -> R {
        if p == self.cond_ptr {
            self.if_limit = l;
        } else {
            let mut q = self.cond_ptr;
            loop {
                if q == NULL {
                    return self.confusion("if");
                }
                if self.link(q) == p {
                    self.set_type(q, l);
                    return Ok(());
                }
                q = self.link(q);
            }
        }
        Ok(())
    }

    // §747
    pub(crate) fn check_colon(&mut self) -> R {
        if self.cur_cmd != COLON {
            self.missing_err(":");
            self.help(&[
                "There should've been a colon after the condition.",
                "I shall pretend that one was there.",
            ]);
            self.back_error()?;
        }
        Ok(())
    }

    // §748
    pub(crate) fn conditional(&mut self) -> R {
        self.push_cond()?;
        let save_cond_ptr = self.cond_ptr;
        let mut new_if_limit;
        let mut reswitch = true;
        loop {
            if reswitch {
                self.get_boolean()?;
                new_if_limit = ELSE_IF_CODE;
                if self.int(TRACING_COMMANDS) > UNITY {
                    // §750
                    self.begin_diagnostic();
                    if self.cur_exp == TRUE_CODE {
                        self.prints("{true}");
                    } else {
                        self.prints("{false}");
                    }
                    self.end_diagnostic(false);
                }
            } else {
                new_if_limit = FI_CODE;
            }
            // found:
            self.check_colon()?;
            if self.cur_exp == TRUE_CODE {
                self.change_if_limit(new_if_limit, save_cond_ptr)?;
                return Ok(());
            }
            // §749
            loop {
                self.pass_text()?;
                if self.cond_ptr == save_cond_ptr {
                    break;
                } else if self.cur_mod == FI_CODE {
                    self.pop_cond();
                }
            }
            // done:
            self.cur_if = self.cur_mod;
            self.if_line = self.line;
            if self.cur_mod == FI_CODE {
                self.pop_cond();
                return Ok(());
            } else if self.cur_mod == ELSE_IF_CODE {
                reswitch = true;
            } else {
                self.cur_exp = TRUE_CODE;
                reswitch = false;
                self.get_x_next()?;
            }
        }
    }

    // §754
    pub(crate) fn bad_for(&mut self, s: &str) -> R {
        self.disp_err(NULL, "Improper ")?;
        self.prints(s);
        self.prints(" has been replaced by 0");
        self.help(&[
            "When you say `for x=a step b until c',",
            "the initial value `a' and the step size `b'",
            "and the final value `c' must have known numeric values.",
            "I'm zeroing this one. Proceed, with fingers crossed.",
        ]);
        self.put_get_flush_error(0)
    }

    // §755
    pub(crate) fn begin_iteration(&mut self) -> R {
        let m = self.cur_mod;
        let n = self.cur_sym;
        let s = self.get_node(LOOP_NODE_SIZE)?;
        let p;
        if m == START_FOREVER {
            self.set_loop_type(s, VOID);
            p = NULL;
            self.get_x_next()?;
        } else {
            self.get_symbol()?;
            p = self.get_node(TOKEN_NODE_SIZE)?;
            self.set_info(p, self.cur_sym);
            self.set_value(p, m);
            self.get_x_next()?;
            if self.cur_cmd != EQUALS && self.cur_cmd != ASSIGNMENT {
                self.missing_err("=");
                self.help(&[
                    "The next thing in this loop should have been `=' or `:='.",
                    "But don't worry; I'll pretend that an equals sign",
                    "was present, and I'll look for the values next.",
                ]);
                self.back_error()?;
            }
            // §764
            self.set_loop_type(s, NULL);
            let mut q = s + 1;
            self.set_link(q, NULL);
            'done: loop {
                self.get_x_next()?;
                'cont: {
                    if m != EXPR_BASE {
                        self.scan_suffix()?;
                    } else {
                        if self.cur_cmd >= COLON && self.cur_cmd <= COMMA {
                            break 'cont;
                        }
                        self.scan_expression()?;
                        if self.cur_cmd == STEP_TOKEN && q == s + 1 {
                            // §765
                            if self.cur_type != KNOWN {
                                self.bad_for("initial value")?;
                            }
                            let pp = self.get_node(PROGRESSION_NODE_SIZE)?;
                            self.set_value(pp, self.cur_exp);
                            self.get_x_next()?;
                            self.scan_expression()?;
                            if self.cur_type != KNOWN {
                                self.bad_for("step size")?;
                            }
                            self.mem[(pp + 2) as usize].rh = self.cur_exp;
                            if self.cur_cmd != UNTIL_TOKEN {
                                self.missing_err("until");
                                self.help(&[
                                    "I assume you meant to say `until' after `step'.",
                                    "So I'll look for the final value and colon next.",
                                ]);
                                self.back_error()?;
                            }
                            self.get_x_next()?;
                            self.scan_expression()?;
                            if self.cur_type != KNOWN {
                                self.bad_for("final value")?;
                            }
                            self.mem[(pp + 3) as usize].rh = self.cur_exp;
                            self.set_loop_type(s, pp);
                            break 'done;
                        }
                        self.cur_exp = self.stash_cur_exp()?;
                    }
                    let a = self.get_avail()?;
                    self.set_link(q, a);
                    q = a;
                    self.set_info(q, self.cur_exp);
                    self.cur_type = VACUOUS;
                }
                if self.cur_cmd != COMMA {
                    break;
                }
            }
        }
        // found: §756
        if self.cur_cmd != COLON {
            self.missing_err(":");
            self.help(&[
                "The next thing in this loop should have been a `:'.",
                "So I'll pretend that a colon was present;",
                "everything from here to `endfor' will be iterated.",
            ]);
            self.back_error()?;
        }
        // §758
        let q = self.get_avail()?;
        self.set_info(q, FROZEN_REPEAT_LOOP);
        self.scanner_status = LOOP_DEFINING;
        self.warning_info = n;
        let t = self.scan_toks(ITERATION, p, q, 0)?;
        self.set_info(s, t);
        self.scanner_status = NORMAL;
        self.set_link(s, self.loop_ptr);
        self.loop_ptr = s;
        self.resume_iteration()
    }

    // §760
    pub(crate) fn resume_iteration(&mut self) -> R {
        let mut p = self.loop_type(self.loop_ptr);
        let q;
        if p > VOID {
            self.cur_exp = self.value(p);
            if (self.step_size(p) > 0 && self.cur_exp > self.final_value(p))
                || (self.step_size(p) < 0 && self.cur_exp < self.final_value(p))
            {
                return self.stop_iteration();
            }
            self.cur_type = KNOWN;
            q = self.stash_cur_exp()?;
            self.set_value(p, self.cur_exp + self.step_size(p));
        } else if p < VOID {
            p = self.loop_list(self.loop_ptr);
            if p == NULL {
                return self.stop_iteration();
            }
            self.set_loop_list(self.loop_ptr, self.link(p));
            q = self.info(p);
            self.free_avail(p);
        } else {
            return self.begin_token_list(self.info(self.loop_ptr), FOREVER_TEXT);
        }
        self.begin_token_list(self.info(self.loop_ptr), LOOP_TEXT)?;
        self.stack_argument(q)?;
        if self.int(TRACING_COMMANDS) > UNITY {
            // §762
            self.begin_diagnostic();
            self.print_nl("{loop value=");
            if q != NULL && self.link(q) == VOID {
                self.print_exp(q, 1)?;
            } else {
                self.show_token_list(q, NULL, 50, 0)?;
            }
            self.print_char(b'}');
            self.end_diagnostic(false);
        }
        Ok(())
    }

    // §763
    pub(crate) fn stop_iteration(&mut self) -> R {
        let mut p = self.loop_type(self.loop_ptr);
        if p > VOID {
            self.free_node(p, PROGRESSION_NODE_SIZE);
        } else if p < VOID {
            let mut q = self.loop_list(self.loop_ptr);
            while q != NULL {
                p = self.info(q);
                if p != NULL {
                    if self.link(p) == VOID {
                        self.recycle_value(p)?;
                        self.free_node(p, VALUE_NODE_SIZE);
                    } else {
                        self.flush_token_list(p)?;
                    }
                }
                p = q;
                q = self.link(q);
                self.free_avail(p);
            }
        }
        p = self.loop_ptr;
        self.loop_ptr = self.link(p);
        self.flush_token_list(self.info(p))?;
        self.free_node(p, LOOP_NODE_SIZE);
        Ok(())
    }
}
