//! Parts 43 and 44 of `mf.web`: statements and commands.

use crate::arith::{floor_unscaled, round_fraction, round_unscaled};
use crate::{Mf, Pointer, R, consts::*};

impl Mf {
    // §989
    pub(crate) fn do_statement(&mut self) -> R {
        self.cur_type = VACUOUS;
        self.get_x_next()?;
        if self.cur_cmd > MAX_PRIMARY_COMMAND {
            // §990
            if self.cur_cmd < SEMICOLON {
                self.print_err("A statement can't begin with `");
                self.print_cmd_mod(self.cur_cmd, self.cur_mod)?;
                self.print_char(b'\'');
                self.help(&[
                    "I was looking for the beginning of a new statement.",
                    "If you just proceed without changing anything, I'll ignore",
                    "everything up to the next `;'. Please insert a semicolon",
                    "now in front of anything that you don't want me to delete.",
                    "(See Chapter 27 of The METAFONTbook for an example.)",
                ]);
                self.back_error()?;
                self.get_x_next()?;
            }
        } else if self.cur_cmd > MAX_STATEMENT_COMMAND {
            // §993
            self.var_flag = ASSIGNMENT;
            self.scan_expression()?;
            if self.cur_cmd < END_GROUP {
                if self.cur_cmd == EQUALS {
                    self.do_equation()?;
                } else if self.cur_cmd == ASSIGNMENT {
                    self.do_assignment()?;
                } else if self.cur_type == STRING_TYPE {
                    // §994
                    if self.int(TRACING_TITLES) > 0 {
                        self.print_nl("");
                        self.slow_print(self.cur_exp);
                        self.update_terminal();
                    }
                    if self.int(PROOFING) > 0 {
                        self.send_title()?;
                    }
                } else if self.cur_type != VACUOUS {
                    self.exp_err("Isolated expression")?;
                    self.help(&[
                        "I couldn't find an `=' or `:=' after the",
                        "expression that is shown above this error message,",
                        "so I guess I'll just ignore it and carry on.",
                    ]);
                    self.put_get_error()?;
                }
                self.flush_cur_exp(0)?;
                self.cur_type = VACUOUS;
            }
        } else {
            // §992
            if self.int(TRACING_COMMANDS) > 0 {
                self.show_cur_cmd_mod()?;
            }
            match self.cur_cmd {
                TYPE_NAME => self.do_type_declaration()?,
                MACRO_DEF => {
                    if self.cur_mod > VAR_DEF {
                        self.make_op_def()?;
                    } else if self.cur_mod > END_DEF {
                        self.scan_def()?;
                    }
                }
                RANDOM_SEED => self.do_random_seed()?,
                MODE_COMMAND => {
                    self.print_ln();
                    self.interaction = self.cur_mod;
                    // §70
                    if self.interaction == BATCH_MODE {
                        self.selector = NO_PRINT;
                    } else {
                        self.selector = TERM_ONLY;
                    }
                    if self.log_opened {
                        self.selector += 2;
                    }
                    self.get_x_next()?;
                }
                PROTECTION_COMMAND => self.do_protection()?,
                DELIMITERS => self.def_delims()?,
                SAVE_COMMAND => loop {
                    self.get_symbol()?;
                    self.save_variable(self.cur_sym)?;
                    self.get_x_next()?;
                    if self.cur_cmd != COMMA {
                        break;
                    }
                },
                INTERIM_COMMAND => self.do_interim()?,
                LET_COMMAND => self.do_let()?,
                NEW_INTERNAL => self.do_new_internal()?,
                SHOW_COMMAND => self.do_show_whatever()?,
                ADD_TO_COMMAND => self.do_add_to()?,
                SHIP_OUT_COMMAND => self.do_ship_out()?,
                DISPLAY_COMMAND => self.do_display()?,
                OPEN_WINDOW => self.do_open_window()?,
                CULL_COMMAND => self.do_cull()?,
                EVERY_JOB_COMMAND => {
                    self.get_symbol()?;
                    self.start_sym = self.cur_sym;
                    self.get_x_next()?;
                }
                MESSAGE_COMMAND => self.do_message()?,
                TFM_COMMAND => self.do_tfm_command()?,
                SPECIAL_COMMAND => self.do_special()?,
                _ => {}
            }
            self.cur_type = VACUOUS;
        }
        if self.cur_cmd < SEMICOLON {
            // §991
            self.print_err("Extra tokens will be flushed");
            self.help(&[
                "I've just read as much of that statement as I could fathom,",
                "so a semicolon should have been next. It's very puzzling...",
                "but I'll try to get myself back together, by ignoring",
                "everything up to the next `;'. Please insert a semicolon",
                "now in front of anything that you don't want me to delete.",
                "(See Chapter 27 of The METAFONTbook for an example.)",
            ]);
            self.back_error()?;
            self.scanner_status = FLUSHING;
            loop {
                self.get_next()?;
                if self.cur_cmd == STRING_TOKEN {
                    self.delete_str_ref(self.cur_mod);
                }
                if self.cur_cmd > COMMA {
                    break;
                }
            }
            self.scanner_status = NORMAL;
        }
        self.error_count = 0;
        Ok(())
    }

    // §995
    pub(crate) fn do_equation(&mut self) -> R {
        let mut lhs = self.stash_cur_exp()?;
        self.get_x_next()?;
        self.var_flag = ASSIGNMENT;
        self.scan_expression()?;
        if self.cur_cmd == EQUALS {
            self.do_equation()?;
        } else if self.cur_cmd == ASSIGNMENT {
            self.do_assignment()?;
        }
        if self.int(TRACING_COMMANDS) > TWO {
            // §997
            self.begin_diagnostic();
            self.print_nl("{(");
            self.print_exp(lhs, 0)?;
            self.prints(")=(");
            self.print_exp(NULL, 0)?;
            self.prints(")}");
            self.end_diagnostic(false);
        }
        if self.cur_type == UNKNOWN_PATH && self.type_(lhs) == PAIR_TYPE {
            let p = self.stash_cur_exp()?;
            self.unstash_cur_exp(lhs)?;
            lhs = p;
        }
        self.make_eq(lhs)
    }

    // §996
    pub(crate) fn do_assignment(&mut self) -> R {
        if self.cur_type != TOKEN_LIST {
            self.exp_err("Improper `:=' will be changed to `='")?;
            self.help(&[
                "I didn't find a variable name at the left of the `:=',",
                "so I'm going to pretend that you said `=' instead.",
            ]);
            self.error()?;
            self.do_equation()?;
        } else {
            let lhs = self.cur_exp;
            self.cur_type = VACUOUS;
            self.get_x_next()?;
            self.var_flag = ASSIGNMENT;
            self.scan_expression()?;
            if self.cur_cmd == EQUALS {
                self.do_equation()?;
            } else if self.cur_cmd == ASSIGNMENT {
                self.do_assignment()?;
            }
            if self.int(TRACING_COMMANDS) > TWO {
                // §998
                self.begin_diagnostic();
                self.print_nl("{");
                if self.info(lhs) > HASH_END {
                    self.slow_print(self.int_name[(self.info(lhs) - HASH_END) as usize]);
                } else {
                    self.show_token_list(lhs, NULL, 1000, 0)?;
                }
                self.prints(":=");
                self.print_exp(NULL, 0)?;
                self.print_char(b'}');
                self.end_diagnostic(false);
            }
            if self.info(lhs) > HASH_END {
                // §999
                let i = (self.info(lhs) - HASH_END) as usize;
                if self.cur_type == KNOWN {
                    self.internal[i] = self.cur_exp;
                } else {
                    self.exp_err("Internal quantity `")?;
                    self.slow_print(self.int_name[i]);
                    self.prints("' must receive a known value");
                    self.help(&[
                        "I can't set an internal quantity to anything but a known",
                        "numeric value, so I'll have to ignore this assignment.",
                    ]);
                    self.put_get_error()?;
                }
            } else {
                // §1000
                let p = self.find_variable(lhs)?;
                if p != NULL {
                    let q = self.stash_cur_exp()?;
                    self.cur_type = self.und_type(p);
                    self.recycle_value(p)?;
                    self.set_type(p, self.cur_type);
                    self.set_value(p, NULL);
                    self.make_exp_copy(p)?;
                    let p = self.stash_cur_exp()?;
                    self.unstash_cur_exp(q)?;
                    self.make_eq(p)?;
                } else {
                    self.obliterated(lhs)?;
                    self.put_get_error()?;
                }
            }
            self.flush_node_list(lhs);
        }
        Ok(())
    }

    // §1001
    pub(crate) fn make_eq(&mut self, lhs: Pointer) -> R {
        'done: loop {
            // restart:
            let t = self.type_(lhs);
            let mut v = 0;
            if t <= PAIR_TYPE {
                v = self.value(lhs);
            }
            // §1003
            match t {
                BOOLEAN_TYPE | STRING_TYPE | PEN_TYPE | PATH_TYPE | PICTURE_TYPE => {
                    if self.cur_type == t + UNKNOWN_TAG {
                        self.nonlinear_eq(v, self.cur_exp, false)?;
                        self.unstash_cur_exp(self.cur_exp)?;
                        break 'done;
                    } else if self.cur_type == t {
                        // §1004
                        'not_found: {
                            if self.cur_type <= STRING_TYPE {
                                if self.cur_type == STRING_TYPE {
                                    if self.str_vs_str(v, self.cur_exp) != 0 {
                                        break 'not_found;
                                    }
                                } else if v != self.cur_exp {
                                    break 'not_found;
                                }
                                // §623
                                self.print_err("Redundant equation");
                                self.help(&[
                                    "I already knew that this equation was true.",
                                    "But perhaps no harm has been done; let's continue.",
                                ]);
                                self.put_get_error()?;
                                break 'done;
                            }
                            self.print_err("Redundant or inconsistent equation");
                            self.help(&[
                                "An equation between already-known quantities can't help.",
                                "But don't worry; continue and I'll just ignore it.",
                            ]);
                            self.put_get_error()?;
                            break 'done;
                        }
                        self.print_err("Inconsistent equation");
                        self.help(&[
                            "The equation I just read contradicts what was said before.",
                            "But don't worry; continue and I'll just ignore it.",
                        ]);
                        self.put_get_error()?;
                        break 'done;
                    }
                }
                UNKNOWN_BOOLEAN | UNKNOWN_STRING | UNKNOWN_PEN | UNKNOWN_PICTURE | UNKNOWN_PATH => {
                    if self.cur_type == t - UNKNOWN_TAG {
                        self.nonlinear_eq(self.cur_exp, lhs, true)?;
                        break 'done;
                    } else if self.cur_type == t {
                        self.ring_merge(lhs, self.cur_exp)?;
                        break 'done;
                    } else if self.cur_type == PAIR_TYPE && t == UNKNOWN_PATH {
                        self.pair_to_path()?;
                        continue 'done;
                    }
                }
                TRANSFORM_TYPE | PAIR_TYPE => {
                    if self.cur_type == t {
                        // §1005
                        let bns = self.big_node_size[(t - TRANSFORM_TYPE) as usize];
                        let mut p = v + bns;
                        let mut q = self.value(self.cur_exp) + bns;
                        loop {
                            p -= 2;
                            q -= 2;
                            self.try_eq(p, q)?;
                            if p == v {
                                break;
                            }
                        }
                        break 'done;
                    }
                }
                KNOWN | DEPENDENT | PROTO_DEPENDENT | INDEPENDENT => {
                    if self.cur_type >= KNOWN {
                        self.try_eq(lhs, NULL)?;
                        break 'done;
                    }
                }
                _ => {}
            }
            // §1002
            self.disp_err(lhs, "")?;
            self.exp_err("Equation cannot be performed (")?;
            if self.type_(lhs) <= PAIR_TYPE {
                self.print_type(self.type_(lhs));
            } else {
                self.prints("numeric");
            }
            self.print_char(b'=');
            if self.cur_type <= PAIR_TYPE {
                self.print_type(self.cur_type);
            } else {
                self.prints("numeric");
            }
            self.print_char(b')');
            self.help(&[
                "I'm sorry, but I don't know how to make such things equal.",
                "(See the two expressions just above the error message.)",
            ]);
            self.put_get_error()?;
            break 'done;
        }
        self.check_arith()?;
        self.recycle_value(lhs)?;
        self.free_node(lhs, VALUE_NODE_SIZE);
        Ok(())
    }

    // §1006
    pub(crate) fn try_eq(&mut self, l: Pointer, r: Pointer) -> R {
        // §1007
        let mut t = self.type_(l);
        let mut p;
        let mut q;
        if t == KNOWN {
            t = DEPENDENT;
            p = self.const_dependency(-self.value(l))?;
            q = p;
        } else if t == INDEPENDENT {
            t = DEPENDENT;
            p = self.single_dependency(l)?;
            self.set_value(p, -self.value(p));
            q = self.dep_final;
        } else {
            p = self.dep_list(l);
            q = p;
            loop {
                self.set_value(q, -self.value(q));
                if self.info(q) == NULL {
                    break;
                }
                q = self.link(q);
            }
            let pd = self.prev_dep(l);
            self.set_link(pd, self.link(q));
            let lq = self.link(q);
            self.set_prev_dep(lq, pd);
            self.set_type(l, KNOWN);
        }
        // §1009
        'done1: {
            let mut tt;
            let pp;
            if r == NULL {
                if self.cur_type == KNOWN {
                    self.set_value(q, self.value(q) + self.cur_exp);
                    break 'done1;
                } else {
                    tt = self.cur_type;
                    pp = if tt == INDEPENDENT {
                        self.single_dependency(self.cur_exp)?
                    } else {
                        self.dep_list(self.cur_exp)
                    };
                }
            } else if self.type_(r) == KNOWN {
                self.set_value(q, self.value(q) + self.value(r));
                break 'done1;
            } else {
                tt = self.type_(r);
                pp = if tt == INDEPENDENT {
                    self.single_dependency(r)?
                } else {
                    self.dep_list(r)
                };
            }
            let copied;
            if tt != INDEPENDENT {
                copied = false;
            } else {
                copied = true;
                tt = DEPENDENT;
            }
            // §1010
            self.watch_coefs = false;
            if t == tt {
                p = self.p_plus_q(p, pp, t)?;
            } else if t == PROTO_DEPENDENT {
                p = self.p_plus_fq(p, UNITY, pp, PROTO_DEPENDENT, DEPENDENT)?;
            } else {
                q = p;
                while self.info(q) != NULL {
                    self.set_value(q, round_fraction(self.value(q)));
                    q = self.link(q);
                }
                t = PROTO_DEPENDENT;
                p = self.p_plus_q(p, pp, t)?;
            }
            self.watch_coefs = true;
            if copied {
                self.flush_node_list(pp);
            }
        }
        if self.info(p) == NULL {
            // §1008
            if self.value(p).abs() > 64 {
                self.print_err("Inconsistent equation");
                self.prints(" (off by ");
                self.print_scaled(self.value(p));
                self.print_char(b')');
                self.help(&[
                    "The equation I just read contradicts what was said before.",
                    "But don't worry; continue and I'll just ignore it.",
                ]);
                self.put_get_error()?;
            } else if r == NULL {
                self.print_err("Redundant equation");
                self.help(&[
                    "I already knew that this equation was true.",
                    "But perhaps no harm has been done; let's continue.",
                ]);
                self.put_get_error()?;
            }
            self.free_node(p, DEP_NODE_SIZE);
        } else {
            self.linear_eq(p, t)?;
            if r == NULL && self.cur_type != KNOWN && self.type_(self.cur_exp) == KNOWN {
                let pp = self.cur_exp;
                self.cur_exp = self.value(self.cur_exp);
                self.cur_type = KNOWN;
                self.free_node(pp, VALUE_NODE_SIZE);
            }
        }
        Ok(())
    }

    // §1011
    pub(crate) fn scan_declared_variable(&mut self) -> R<Pointer> {
        self.get_symbol()?;
        let x = self.cur_sym;
        if self.cur_cmd != TAG_TOKEN {
            self.clear_symbol(x, false)?;
        }
        let h = self.get_avail()?;
        self.set_info(h, x);
        let mut t = h;
        loop {
            self.get_x_next()?;
            if self.cur_sym == 0 {
                break;
            }
            if self.cur_cmd != TAG_TOKEN && self.cur_cmd != INTERNAL_QUANTITY {
                if self.cur_cmd == LEFT_BRACKET {
                    // §1012
                    let l = self.cur_sym;
                    self.get_x_next()?;
                    if self.cur_cmd != RIGHT_BRACKET {
                        self.back_input()?;
                        self.cur_sym = l;
                        self.cur_cmd = LEFT_BRACKET;
                        break;
                    } else {
                        self.cur_sym = COLLECTIVE_SUBSCRIPT;
                    }
                } else {
                    break;
                }
            }
            let a = self.get_avail()?;
            self.set_link(t, a);
            t = a;
            self.set_info(t, self.cur_sym);
        }
        if self.eq_type(x) % OUTER_TAG != TAG_TOKEN {
            self.clear_symbol(x, false)?;
        }
        if self.equiv(x) == NULL {
            self.new_root(x)?;
        }
        Ok(h)
    }

    // §1015
    pub(crate) fn do_type_declaration(&mut self) -> R {
        let t = if self.cur_mod >= TRANSFORM_TYPE {
            self.cur_mod
        } else {
            self.cur_mod + UNKNOWN_TAG
        };
        loop {
            let p = self.scan_declared_variable()?;
            let e = self.equiv(self.info(p));
            self.flush_variable(e, self.link(p), false)?;
            let q = self.find_variable(p)?;
            if q != NULL {
                self.set_type(q, t);
                self.set_value(q, NULL);
            } else {
                self.print_err("Declared variable conflicts with previous vardef");
                self.help(&[
                    "You can't use, e.g., `numeric foo[]' after `vardef foo'.",
                    "Proceed, and I'll ignore the illegal redeclaration.",
                ]);
                self.put_get_error()?;
            }
            self.flush_list(p);
            if self.cur_cmd < COMMA {
                // §1016
                self.print_err("Illegal suffix of declared variable will be flushed");
                self.help(&[
                    "Variables in declarations must consist entirely of",
                    "names and collective subscripts, e.g., `x[]a'.",
                    "Are you trying to use a reserved word in a variable name?",
                    "I'm going to discard the junk I found here,",
                    "up to the next comma or the end of the declaration.",
                ]);
                if self.cur_cmd == NUMERIC_TOKEN {
                    self.help_line[2] = "Explicit subscripts like `x15a' aren't permitted.";
                }
                self.put_get_error()?;
                self.scanner_status = FLUSHING;
                loop {
                    self.get_next()?;
                    if self.cur_cmd == STRING_TOKEN {
                        self.delete_str_ref(self.cur_mod);
                    }
                    if self.cur_cmd >= COMMA {
                        break;
                    }
                }
                self.scanner_status = NORMAL;
            }
            if self.cur_cmd > COMMA {
                break;
            }
        }
        Ok(())
    }

    // §1017
    pub(crate) fn main_control(&mut self) -> R {
        loop {
            self.do_statement()?;
            if self.cur_cmd == END_GROUP {
                self.print_err("Extra `endgroup'");
                self.help(&[
                    "I'm not currently working on a `begingroup',",
                    "so I had better not try to end anything.",
                ]);
                self.flush_error(0)?;
            }
            if self.cur_cmd == STOP {
                break;
            }
        }
        Ok(())
    }

    // §1021
    pub(crate) fn do_random_seed(&mut self) -> R {
        self.get_x_next()?;
        if self.cur_cmd != ASSIGNMENT {
            self.missing_err(":=");
            self.help(&["Always say `randomseed:=<numeric expression>'."]);
            self.back_error()?;
        }
        self.get_x_next()?;
        self.scan_expression()?;
        if self.cur_type != KNOWN {
            self.exp_err("Unknown value will be ignored")?;
            self.help(&[
                "Your expression was too random for me to handle,",
                "so I won't change the random seed just now.",
            ]);
            self.put_get_flush_error(0)?;
        } else {
            // §1022
            self.init_randoms(self.cur_exp);
            if self.selector >= LOG_ONLY {
                self.old_setting = self.selector;
                self.selector = LOG_ONLY;
                self.print_nl("{randomseed:=");
                self.print_scaled(self.cur_exp);
                self.print_char(b'}');
                self.print_nl("");
                self.selector = self.old_setting;
            }
        }
        Ok(())
    }

    // §1029
    pub(crate) fn do_protection(&mut self) -> R {
        let m = self.cur_mod;
        loop {
            self.get_symbol()?;
            let t = self.eq_type(self.cur_sym);
            if m == 0 {
                if t >= OUTER_TAG {
                    self.set_eq_type(self.cur_sym, t - OUTER_TAG);
                }
            } else if t < OUTER_TAG {
                self.set_eq_type(self.cur_sym, t + OUTER_TAG);
            }
            self.get_x_next()?;
            if self.cur_cmd != COMMA {
                break;
            }
        }
        Ok(())
    }

    // §1031
    pub(crate) fn def_delims(&mut self) -> R {
        self.get_clear_symbol()?;
        let l_delim = self.cur_sym;
        self.get_clear_symbol()?;
        let r_delim = self.cur_sym;
        self.set_eq_type(l_delim, LEFT_DELIMITER);
        self.set_equiv(l_delim, r_delim);
        self.set_eq_type(r_delim, RIGHT_DELIMITER);
        self.set_equiv(r_delim, l_delim);
        self.get_x_next()
    }

    // §1032
    pub(crate) fn check_delimiter(&mut self, l_delim: Pointer, r_delim: Pointer) -> R {
        if self.cur_cmd == RIGHT_DELIMITER && self.cur_mod == l_delim {
            return Ok(());
        }
        if self.cur_sym != r_delim {
            self.missing_err_str(self.text(r_delim));
            self.help(&[
                "I found no right delimiter to match a left one. So I've",
                "put one in, behind the scenes; this may fix the problem.",
            ]);
            self.back_error()
        } else {
            self.print_err("The token `");
            self.slow_print(self.text(r_delim));
            self.prints("' is no longer a right delimiter");
            self.help(&[
                "Strange: This token has lost its former meaning!",
                "I'll read it as a right delimiter this time;",
                "but watch out, I'll probably miss it later.",
            ]);
            self.error()
        }
    }

    // §1034
    pub(crate) fn do_interim(&mut self) -> R {
        self.get_x_next()?;
        if self.cur_cmd != INTERNAL_QUANTITY {
            self.print_err("The token `");
            if self.cur_sym == 0 {
                self.prints("(%CAPSULE)");
            } else {
                self.slow_print(self.text(self.cur_sym));
            }
            self.prints("' isn't an internal quantity");
            self.help(&["Something like `tracingonline' should follow `interim'."]);
            self.back_error()?;
        } else {
            self.save_internal(self.cur_mod)?;
            self.back_input()?;
        }
        self.do_statement()
    }

    // §1035
    pub(crate) fn do_let(&mut self) -> R {
        self.get_symbol()?;
        let l = self.cur_sym;
        self.get_x_next()?;
        if self.cur_cmd != EQUALS && self.cur_cmd != ASSIGNMENT {
            self.missing_err("=");
            self.help(&[
                "You should have said `let symbol = something'.",
                "But don't worry; I'll pretend that an equals sign",
                "was present. The next token I read will be `something'.",
            ]);
            self.back_error()?;
        }
        self.get_symbol()?;
        match self.cur_cmd {
            DEFINED_MACRO
            | SECONDARY_PRIMARY_MACRO
            | TERTIARY_SECONDARY_MACRO
            | EXPRESSION_TERTIARY_MACRO => self.add_mac_ref(self.cur_mod),
            _ => {}
        }
        self.clear_symbol(l, false)?;
        self.set_eq_type(l, self.cur_cmd);
        if self.cur_cmd == TAG_TOKEN {
            self.set_equiv(l, NULL);
        } else {
            self.set_equiv(l, self.cur_mod);
        }
        self.get_x_next()
    }

    // §1036
    pub(crate) fn do_new_internal(&mut self) -> R {
        loop {
            if self.int_ptr == MAX_INTERNAL {
                return self.overflow("number of internals", MAX_INTERNAL);
            }
            self.get_clear_symbol()?;
            self.int_ptr += 1;
            self.set_eq_type(self.cur_sym, INTERNAL_QUANTITY);
            self.set_equiv(self.cur_sym, self.int_ptr);
            self.int_name[self.int_ptr as usize] = self.text(self.cur_sym);
            self.internal[self.int_ptr as usize] = 0;
            self.get_x_next()?;
            if self.cur_cmd != COMMA {
                break;
            }
        }
        Ok(())
    }

    // §1040
    pub(crate) fn do_show(&mut self) -> R {
        loop {
            self.get_x_next()?;
            self.scan_expression()?;
            self.print_nl(">> ");
            self.print_exp(NULL, 2)?;
            self.flush_cur_exp(0)?;
            if self.cur_cmd != COMMA {
                break;
            }
        }
        Ok(())
    }

    // §1041
    pub(crate) fn disp_token(&mut self) -> R {
        self.print_nl("> ");
        if self.cur_sym == 0 {
            // §1042
            if self.cur_cmd == NUMERIC_TOKEN {
                self.print_scaled(self.cur_mod);
            } else if self.cur_cmd == CAPSULE_TOKEN {
                self.g_pointer = self.cur_mod;
                self.print_capsule()?;
            } else {
                self.print_char(b'"');
                self.slow_print(self.cur_mod);
                self.print_char(b'"');
                self.delete_str_ref(self.cur_mod);
            }
        } else {
            self.slow_print(self.text(self.cur_sym));
            self.print_char(b'=');
            if self.eq_type(self.cur_sym) >= OUTER_TAG {
                self.prints("(outer) ");
            }
            self.print_cmd_mod(self.cur_cmd, self.cur_mod)?;
            if self.cur_cmd == DEFINED_MACRO {
                self.print_ln();
                self.show_macro(self.cur_mod, NULL, 100_000)?;
            }
        }
        Ok(())
    }

    // §1044
    pub(crate) fn do_show_token(&mut self) -> R {
        loop {
            self.get_next()?;
            self.disp_token()?;
            self.get_x_next()?;
            if self.cur_cmd != COMMA {
                break;
            }
        }
        Ok(())
    }

    // §1045
    pub(crate) fn do_show_stats(&mut self) -> R {
        self.print_nl("Memory usage ");
        self.print_int(self.var_used);
        self.print_char(b'&');
        self.print_int(self.dyn_used);
        self.prints(" (");
        self.print_int(self.hi_mem_min - self.lo_mem_max - 1);
        self.prints(" still untouched)");
        self.print_ln();
        self.print_nl("String usage ");
        self.print_int(self.str_ptr - self.init_str_ptr);
        self.print_char(b'&');
        self.print_int(self.pool_ptr - self.init_pool_ptr);
        self.prints(" (");
        self.print_int(MAX_STRINGS - self.max_str_ptr);
        self.print_char(b'&');
        self.print_int(POOL_SIZE - self.max_pool_ptr);
        self.prints(" still untouched)");
        self.print_ln();
        self.get_x_next()
    }

    // §1046
    pub(crate) fn disp_var(&mut self, p: Pointer) -> R {
        if self.type_(p) == STRUCTURED {
            // §1047
            let mut q = self.attr_head(p);
            loop {
                self.disp_var(q)?;
                q = self.link(q);
                if q == END_ATTR {
                    break;
                }
            }
            q = self.subscr_head(p);
            while self.name_type(q) == SUBSCR {
                self.disp_var(q)?;
                q = self.link(q);
            }
        } else if self.type_(p) >= UNSUFFIXED_MACRO {
            // §1048
            self.print_nl("");
            self.print_variable_name(p)?;
            if self.type_(p) > UNSUFFIXED_MACRO {
                self.prints("@#");
            }
            self.prints("=macro:");
            let n = if self.file_offset >= self.max_print_line - 20 {
                5
            } else {
                self.max_print_line - self.file_offset - 15
            };
            self.show_macro(self.value(p), NULL, n)?;
        } else if self.type_(p) != UNDEFINED {
            self.print_nl("");
            self.print_variable_name(p)?;
            self.print_char(b'=');
            self.print_exp(p, 0)?;
        }
        Ok(())
    }

    // §1049
    pub(crate) fn do_show_var(&mut self) -> R {
        loop {
            self.get_next()?;
            if self.cur_sym > 0
                && self.cur_sym <= HASH_END
                && self.cur_cmd == TAG_TOKEN
                && self.cur_mod != NULL
            {
                self.disp_var(self.cur_mod)?;
            } else {
                self.disp_token()?;
            }
            self.get_x_next()?;
            if self.cur_cmd != COMMA {
                break;
            }
        }
        Ok(())
    }

    // §1050
    pub(crate) fn do_show_dependencies(&mut self) -> R {
        let mut p = self.link(DEP_HEAD);
        while p != DEP_HEAD {
            if self.interesting(p) {
                self.print_nl("");
                self.print_variable_name(p)?;
                if self.type_(p) == DEPENDENT {
                    self.print_char(b'=');
                } else {
                    self.prints(" = ");
                }
                self.print_dependency(self.dep_list(p), self.type_(p))?;
            }
            p = self.dep_list(p);
            while self.info(p) != NULL {
                p = self.link(p);
            }
            p = self.link(p);
        }
        self.get_x_next()
    }

    // §1051
    pub(crate) fn do_show_whatever(&mut self) -> R {
        match self.cur_mod {
            SHOW_TOKEN_CODE => self.do_show_token()?,
            SHOW_STATS_CODE => self.do_show_stats()?,
            SHOW_CODE => self.do_show()?,
            SHOW_VAR_CODE => self.do_show_var()?,
            _ => self.do_show_dependencies()?,
        }
        if self.int(SHOWSTOPPING) > 0 {
            self.print_err("OK");
            if self.interaction < ERROR_STOP_MODE {
                self.help(&[]);
                self.error_count -= 1;
            } else {
                self.help(&["This isn't an error message; I'm just showing something."]);
            }
            if self.cur_cmd == SEMICOLON {
                self.error()?;
            } else {
                self.put_get_error()?;
            }
        }
        Ok(())
    }

    // §1054
    pub(crate) fn scan_with(&mut self) -> R<bool> {
        let t = self.cur_mod;
        self.cur_type = VACUOUS;
        self.get_x_next()?;
        self.scan_expression()?;
        let mut result = false;
        if self.cur_type != t {
            // §1055
            self.exp_err("Improper type")?;
            self.help(&[
                "Next time say `withweight <known numeric expression>';",
                "I'll ignore the bad `with' clause and look for another.",
            ]);
            if t == PEN_TYPE {
                self.help_line[1] = "Next time say `withpen <known pen expression>';";
            }
            self.put_get_flush_error(0)?;
        } else if self.cur_type == PEN_TYPE {
            result = true;
        } else {
            // §1056
            self.cur_exp = round_unscaled(self.cur_exp);
            if self.cur_exp.abs() < 4 && self.cur_exp != 0 {
                result = true;
            } else {
                self.print_err("Weight must be -3, -2, -1, +1, +2, or +3");
                self.help(&["I'll ignore the bad `with' clause and look for another."]);
                self.put_get_flush_error(0)?;
            }
        }
        Ok(result)
    }

    // §1057
    pub(crate) fn find_edges_var(&mut self, t: Pointer) -> R {
        let p = self.find_variable(t)?;
        self.cur_edges = NULL;
        if p == NULL {
            self.obliterated(t)?;
            self.put_get_error()?;
        } else if self.type_(p) != PICTURE_TYPE {
            self.print_err("Variable ");
            self.show_token_list(t, NULL, 1000, 0)?;
            self.prints(" is the wrong type (");
            self.print_type(self.type_(p));
            self.print_char(b')');
            self.help(&[
                "I was looking for a \"known\" picture variable.",
                "So I'll not change anything just now.",
            ]);
            self.put_get_error()?;
        } else {
            self.cur_edges = self.value(p);
        }
        self.flush_node_list(t);
        Ok(())
    }

    // §1060
    fn abandon_edges_command(&mut self) -> R {
        self.exp_err("Not a suitable variable")?;
        self.help(&[
            "At this point I needed to see the name of a picture variable.",
            "(Or perhaps you have indeed presented me with one; I might",
            "have missed it, if it wasn't followed by the proper token.)",
            "So I'll not change anything just now.",
        ]);
        self.put_get_flush_error(0)
    }

    // §1059
    pub(crate) fn do_add_to(&mut self) -> R {
        self.get_x_next()?;
        self.var_flag = THING_TO_ADD;
        self.scan_primary()?;
        if self.cur_type != TOKEN_LIST {
            return self.abandon_edges_command();
        }
        let mut lhs = self.cur_exp;
        let add_to_type = self.cur_mod;
        self.cur_type = VACUOUS;
        self.get_x_next()?;
        self.scan_expression()?;
        if add_to_type == ALSO_CODE {
            // §1061
            self.find_edges_var(lhs)?;
            if self.cur_edges == NULL {
                self.flush_cur_exp(0)?;
            } else if self.cur_type != PICTURE_TYPE {
                self.exp_err("Improper `addto'")?;
                self.help(&[
                    "This expression should have specified a known picture.",
                    "So I'll not change anything just now.",
                ]);
                self.put_get_flush_error(0)?;
            } else {
                self.merge_edges(self.cur_exp)?;
                self.flush_cur_exp(0)?;
            }
            return Ok(());
        }
        // §1062
        if self.cur_type == PAIR_TYPE {
            self.pair_to_path()?;
        }
        if self.cur_type != PATH_TYPE {
            self.exp_err("Improper `addto'")?;
            self.help(&[
                "This expression should have been a known path.",
                "So I'll not change anything just now.",
            ]);
            self.put_get_flush_error(0)?;
            self.flush_token_list(lhs)?;
            return Ok(());
        }
        let mut rhs = self.cur_exp;
        let mut w = 1;
        self.cur_pen = NULL_PEN;
        while self.cur_cmd == WITH_OPTION {
            if self.scan_with()? {
                if self.cur_type == KNOWN {
                    w = self.cur_exp;
                } else {
                    // §1063
                    self.delete_pen_ref(self.cur_pen);
                    self.cur_pen = self.cur_exp;
                }
            }
        }
        // §1064
        self.find_edges_var(lhs)?;
        'not_found: {
            if self.cur_edges == NULL {
                self.toss_knot_list(rhs);
            } else {
                lhs = NULL;
                self.cur_path_type = add_to_type;
                if self.left_type(rhs) == ENDPOINT {
                    if self.cur_path_type == DOUBLE_PATH_CODE {
                        // §1065
                        if self.link(rhs) == rhs {
                            // §1066
                            self.set_right_x(rhs, self.x_coord(rhs));
                            self.set_right_y(rhs, self.y_coord(rhs));
                            self.set_left_x(rhs, self.x_coord(rhs));
                            self.set_left_y(rhs, self.y_coord(rhs));
                            self.set_left_type(rhs, EXPLICIT);
                            self.set_right_type(rhs, EXPLICIT);
                        } else {
                            let p = self.htap_ypoc(rhs)?;
                            let q = self.link(p);
                            let pt = self.path_tail;
                            self.set_right_x(pt, self.right_x(q));
                            self.set_right_y(pt, self.right_y(q));
                            self.set_right_type(pt, self.right_type(q));
                            self.set_link(pt, self.link(q));
                            self.free_node(q, KNOT_NODE_SIZE);
                            self.set_right_x(p, self.right_x(rhs));
                            self.set_right_y(p, self.right_y(rhs));
                            self.set_right_type(p, self.right_type(rhs));
                            self.set_link(p, self.link(rhs));
                            self.free_node(rhs, KNOT_NODE_SIZE);
                            rhs = p;
                        }
                    } else {
                        // §1067
                        self.print_err("Not a cycle");
                        self.help(&[
                            "That contour should have ended with `..cycle' or `&cycle'.",
                            "So I'll not change anything just now.",
                        ]);
                        self.put_get_error()?;
                        self.toss_knot_list(rhs);
                        break 'not_found;
                    }
                } else if self.cur_path_type == DOUBLE_PATH_CODE {
                    lhs = self.htap_ypoc(rhs)?;
                }
                self.cur_wt = w;
                rhs =
                    self.make_spec(rhs, self.max_offset(self.cur_pen), self.int(TRACING_SPECS))?;
                // §1068
                if self.turning_number <= 0
                    && self.cur_path_type != DOUBLE_PATH_CODE
                    && self.int(TURNING_CHECK) > 0
                {
                    if self.turning_number < 0 && self.link(self.cur_pen) == NULL {
                        self.cur_wt = -self.cur_wt;
                    } else {
                        'done: {
                            if self.turning_number == 0 {
                                if self.int(TURNING_CHECK) <= UNITY
                                    && self.link(self.cur_pen) == NULL
                                {
                                    break 'done;
                                } else {
                                    self.print_strange("Strange path (turning number is zero)");
                                }
                            } else {
                                self.print_strange("Backwards path (turning number is negative)");
                            }
                            self.help(&[
                                "The path doesn't have a counterclockwise orientation,",
                                "so I'll probably have trouble drawing it.",
                                "(See Chapter 27 of The METAFONTbook for more help.)",
                            ]);
                            self.put_get_error()?;
                        }
                    }
                }
                if self.max_offset(self.cur_pen) == 0 {
                    self.fill_spec(rhs)?;
                } else {
                    self.fill_envelope(rhs)?;
                }
                if lhs != NULL {
                    self.rev_turns = true;
                    lhs = self.make_spec(
                        lhs,
                        self.max_offset(self.cur_pen),
                        self.int(TRACING_SPECS),
                    )?;
                    self.rev_turns = false;
                    if self.max_offset(self.cur_pen) == 0 {
                        self.fill_spec(lhs)?;
                    } else {
                        self.fill_envelope(lhs)?;
                    }
                }
            }
        }
        self.delete_pen_ref(self.cur_pen);
        Ok(())
    }

    // §1070
    pub(crate) fn do_ship_out(&mut self) -> R {
        self.get_x_next()?;
        self.var_flag = SEMICOLON;
        self.scan_expression()?;
        if self.cur_type != TOKEN_LIST {
            if self.cur_type == PICTURE_TYPE {
                self.cur_edges = self.cur_exp;
            } else {
                return self.abandon_edges_command();
            }
        } else {
            self.find_edges_var(self.cur_exp)?;
            self.cur_type = VACUOUS;
        }
        if self.cur_edges != NULL {
            let mut c = round_unscaled(self.int(CHAR_CODE)) % 256;
            if c < 0 {
                c += 256;
            }
            self.store_width_info(c)?;
            if self.int(PROOFING) >= 0 {
                self.ship_out(c)?;
            }
        }
        self.flush_cur_exp(0)
    }

    // §1071
    pub(crate) fn do_display(&mut self) -> R {
        self.get_x_next()?;
        self.var_flag = IN_WINDOW;
        self.scan_primary()?;
        if self.cur_type != TOKEN_LIST {
            return self.abandon_edges_command();
        }
        let e = self.cur_exp;
        self.cur_type = VACUOUS;
        self.get_x_next()?;
        self.scan_expression()?;
        'common_ending: {
            if self.cur_type != KNOWN {
                break 'common_ending;
            }
            self.cur_exp = round_unscaled(self.cur_exp);
            if self.cur_exp >= 0 && self.cur_exp <= 15 && self.window_open[self.cur_exp as usize] {
                self.find_edges_var(e)?;
                if self.cur_edges != NULL {
                    self.disp_edges(self.cur_exp as usize);
                }
                return Ok(());
            }
            self.cur_exp *= UNITY;
        }
        self.exp_err("Bad window number")?;
        self.help(&["It should be the number of an open window."]);
        self.put_get_flush_error(0)?;
        self.flush_token_list(e)
    }

    // §1072
    pub(crate) fn get_pair(&mut self, c: i32) -> R<bool> {
        if self.cur_cmd != c {
            return Ok(false);
        }
        self.get_x_next()?;
        self.scan_expression()?;
        let b;
        if self.nice_pair(self.cur_exp, self.cur_type) {
            let p = self.value(self.cur_exp);
            self.cur_x = self.value(p);
            self.cur_y = self.value(p + 2);
            b = true;
        } else {
            b = false;
        }
        self.flush_cur_exp(0)?;
        Ok(b)
    }

    // §1073
    pub(crate) fn do_open_window(&mut self) -> R {
        self.get_x_next()?;
        self.scan_expression()?;
        'not_found: {
            if self.cur_type != KNOWN {
                break 'not_found;
            }
            let k = round_unscaled(self.cur_exp);
            if !(0..=15).contains(&k) {
                break 'not_found;
            }
            if !self.get_pair(FROM_TOKEN)? {
                break 'not_found;
            }
            let r0 = self.cur_x;
            let c0 = self.cur_y;
            if !self.get_pair(TO_TOKEN)? {
                break 'not_found;
            }
            let r1 = self.cur_x;
            let c1 = self.cur_y;
            if !self.get_pair(AT_TOKEN)? {
                break 'not_found;
            }
            self.open_a_window(k as usize, r0, c0, r1, c1, self.cur_x, self.cur_y);
            return Ok(());
        }
        self.print_err("Improper `openwindow'");
        self.help(&[
            "Say `openwindow k from (r0,c0) to (r1,c1) at (x,y)',",
            "where all quantities are known and k is between 0 and 15.",
        ]);
        self.put_get_error()
    }

    // §1074
    pub(crate) fn do_cull(&mut self) -> R {
        let mut w = 1;
        self.get_x_next()?;
        self.var_flag = CULL_OP;
        self.scan_primary()?;
        if self.cur_type != TOKEN_LIST {
            return self.abandon_edges_command();
        }
        let e = self.cur_exp;
        self.cur_type = VACUOUS;
        let keeping = self.cur_mod;
        'not_found: {
            if !self.get_pair(CULL_OP)? {
                break 'not_found;
            }
            while self.cur_cmd == WITH_OPTION && self.cur_mod == KNOWN {
                if self.scan_with()? {
                    w = self.cur_exp;
                }
            }
            // §1075
            if self.cur_x > self.cur_y {
                break 'not_found;
            }
            let (w_out, w_in);
            if keeping == DROP_CODE {
                if self.cur_x > 0 || self.cur_y < 0 {
                    break 'not_found;
                }
                w_out = w;
                w_in = 0;
            } else {
                if self.cur_x <= 0 && self.cur_y >= 0 {
                    break 'not_found;
                }
                w_out = 0;
                w_in = w;
            }
            self.find_edges_var(e)?;
            if self.cur_edges != NULL {
                self.cull_edges(
                    floor_unscaled(self.cur_x + UNITY - 1),
                    floor_unscaled(self.cur_y),
                    w_out,
                    w_in,
                )?;
            }
            return Ok(());
        }
        self.print_err("Bad culling amounts");
        self.help(&["Always cull by known amounts that exclude 0."]);
        self.put_get_error()?;
        self.flush_token_list(e)
    }

    // §1082
    pub(crate) fn do_message(&mut self) -> R {
        let m = self.cur_mod;
        self.get_x_next()?;
        self.scan_expression()?;
        if self.cur_type != STRING_TYPE {
            self.exp_err("Not a string")?;
            self.help(&["A message should be a known string expression."]);
            self.put_get_error()?;
        } else {
            match m {
                MESSAGE_CODE => {
                    self.print_nl("");
                    self.slow_print(self.cur_exp);
                }
                ERR_MESSAGE_CODE => {
                    // §1086
                    self.print_err("");
                    self.slow_print(self.cur_exp);
                    if self.err_help != 0 {
                        self.use_err_help = true;
                    } else if self.long_help_seen {
                        self.help(&["(That was another `errmessage'.)"]);
                    } else {
                        if self.interaction < ERROR_STOP_MODE {
                            self.long_help_seen = true;
                        }
                        self.help(&[
                            "This error message was generated by an `errmessage'",
                            "command, so I can't give any explicit help.",
                            "Pretend that you're Miss Marple: Examine all clues,",
                            "and deduce the truth by inspired guesses.",
                        ]);
                    }
                    self.put_get_error()?;
                    self.use_err_help = false;
                }
                _ => {
                    // §1083
                    if self.err_help != 0 {
                        self.delete_str_ref(self.err_help);
                    }
                    if self.length(self.cur_exp) == 0 {
                        self.err_help = 0;
                    } else {
                        self.err_help = self.cur_exp;
                        self.add_str_ref(self.err_help);
                    }
                }
            }
        }
        self.flush_cur_exp(0)
    }
}
