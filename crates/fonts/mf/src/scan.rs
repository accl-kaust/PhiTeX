//! Part 40 of `mf.web`: parsing primary expressions.

use crate::{Mf, Pointer, R, consts::*};

impl Mf {
    // §823
    pub(crate) fn scan_primary(&mut self) -> R {
        let my_var_flag = self.var_flag;
        self.var_flag = 0;
        let mut macro_ref = NULL;
        'restart: loop {
            self.check_arith()?;
            // §825
            if self.interrupt != 0 && self.ok_to_interrupt {
                self.back_input()?;
                self.check_interrupt()?;
                self.get_x_next()?;
            }
            'done: {
                match self.cur_cmd {
                    LEFT_DELIMITER => {
                        // §826
                        let l_delim = self.cur_sym;
                        let r_delim = self.cur_mod;
                        self.get_x_next()?;
                        self.scan_expression()?;
                        if self.cur_cmd == COMMA && self.cur_type >= KNOWN {
                            // §830
                            let p = self.get_node(VALUE_NODE_SIZE)?;
                            self.set_type(p, PAIR_TYPE);
                            self.set_name_type(p, CAPSULE);
                            self.init_big_node(p)?;
                            let q = self.value(p);
                            self.stash_in(q)?;
                            self.get_x_next()?;
                            self.scan_expression()?;
                            if self.cur_type < KNOWN {
                                self.exp_err("Nonnumeric ypart has been replaced by 0")?;
                                self.help(&[
                                    "I thought you were giving me a pair `(x,y)'; but",
                                    "after finding a nice xpart `x' I found a ypart `y'",
                                    "that isn't of numeric type. So I've changed y to zero.",
                                    "(The y that I didn't like appears above the error message.)",
                                ]);
                                self.put_get_flush_error(0)?;
                            }
                            self.stash_in(q + 2)?;
                            self.check_delimiter(l_delim, r_delim)?;
                            self.cur_type = PAIR_TYPE;
                            self.cur_exp = p;
                        } else {
                            self.check_delimiter(l_delim, r_delim)?;
                        }
                    }
                    BEGIN_GROUP => {
                        // §831
                        let group_line = self.line;
                        if self.int(TRACING_COMMANDS) > 0 {
                            self.show_cur_cmd_mod()?;
                        }
                        self.save_boundary_item()?;
                        loop {
                            self.do_statement()?;
                            if self.cur_cmd != SEMICOLON {
                                break;
                            }
                        }
                        if self.cur_cmd != END_GROUP {
                            self.print_err("A group begun on line ");
                            self.print_int(group_line);
                            self.prints(" never ended");
                            self.help(&[
                                "I saw a `begingroup' back there that hasn't been matched",
                                "by `endgroup'. So I've inserted `endgroup' now.",
                            ]);
                            self.back_error()?;
                            self.cur_cmd = END_GROUP;
                        }
                        self.unsave()?;
                        if self.int(TRACING_COMMANDS) > 0 {
                            self.show_cur_cmd_mod()?;
                        }
                    }
                    STRING_TOKEN => {
                        self.cur_type = STRING_TYPE;
                        self.cur_exp = self.cur_mod;
                    }
                    NUMERIC_TOKEN => {
                        // §837
                        self.cur_exp = self.cur_mod;
                        self.cur_type = KNOWN;
                        self.get_x_next()?;
                        let (num, denom);
                        if self.cur_cmd != SLASH {
                            num = 0;
                            denom = 0;
                        } else {
                            self.get_x_next()?;
                            if self.cur_cmd != NUMERIC_TOKEN {
                                self.back_input()?;
                                self.cur_cmd = SLASH;
                                self.cur_mod = OVER;
                                self.cur_sym = FROZEN_SLASH;
                                break 'done;
                            }
                            num = self.cur_exp;
                            denom = self.cur_mod;
                            if denom == 0 {
                                // §838
                                self.print_err("Division by zero");
                                self.help(&["I'll pretend that you meant to divide by 1."]);
                                self.error()?;
                            } else {
                                self.cur_exp = self.make_scaled(num, denom);
                            }
                            self.check_arith()?;
                            self.get_x_next()?;
                        }
                        if self.cur_cmd >= MIN_PRIMARY_COMMAND && self.cur_cmd < NUMERIC_TOKEN {
                            let p = self.stash_cur_exp()?;
                            self.scan_primary()?;
                            if num.abs() >= denom.abs() || self.cur_type < PAIR_TYPE {
                                self.do_binary(p, TIMES)?;
                            } else {
                                self.frac_mult(num, denom)?;
                                self.free_node(p, VALUE_NODE_SIZE);
                            }
                        }
                        break 'done;
                    }
                    NULLARY => self.do_nullary(self.cur_mod)?,
                    UNARY | TYPE_NAME | CYCLE | PLUS_OR_MINUS => {
                        // §835
                        let c = self.cur_mod;
                        self.get_x_next()?;
                        self.scan_primary()?;
                        self.do_unary(c)?;
                        break 'done;
                    }
                    PRIMARY_BINARY => {
                        // §839
                        let c = self.cur_mod;
                        self.get_x_next()?;
                        self.scan_expression()?;
                        if self.cur_cmd != OF_TOKEN {
                            self.missing_err("of");
                            self.prints(" for ");
                            self.print_cmd_mod(PRIMARY_BINARY, c)?;
                            self.help(&[
                                "I've got the first argument; will look now for the other.",
                            ]);
                            self.back_error()?;
                        }
                        let p = self.stash_cur_exp()?;
                        self.get_x_next()?;
                        self.scan_primary()?;
                        self.do_binary(p, c)?;
                        break 'done;
                    }
                    STR_OP => {
                        // §840
                        self.get_x_next()?;
                        self.scan_suffix()?;
                        self.old_setting = self.selector;
                        self.selector = NEW_STRING;
                        self.show_token_list(self.cur_exp, NULL, 100_000, 0)?;
                        self.flush_token_list(self.cur_exp)?;
                        self.cur_exp = self.make_string()?;
                        self.selector = self.old_setting;
                        self.cur_type = STRING_TYPE;
                        break 'done;
                    }
                    INTERNAL_QUANTITY => {
                        // §841
                        let q = self.cur_mod;
                        if my_var_flag == ASSIGNMENT {
                            self.get_x_next()?;
                            if self.cur_cmd == ASSIGNMENT {
                                self.cur_exp = self.get_avail()?;
                                self.set_info(self.cur_exp, q + HASH_END);
                                self.cur_type = TOKEN_LIST;
                                break 'done;
                            }
                            self.back_input()?;
                        }
                        self.cur_type = KNOWN;
                        self.cur_exp = self.int(q);
                    }
                    CAPSULE_TOKEN => self.make_exp_copy(self.cur_mod)?,
                    TAG_TOKEN => {
                        // §844
                        let pre_head = self.fast_get_avail()?;
                        let mut tail = pre_head;
                        let mut post_head = NULL;
                        let mut tt = VACUOUS;
                        loop {
                            let t = self.cur_tok()?;
                            self.set_link(tail, t);
                            if tt != UNDEFINED {
                                // §849
                                let mut p = self.link(pre_head);
                                let mut q = self.info(p);
                                tt = UNDEFINED;
                                'done2: {
                                    if self.eq_type(q) % OUTER_TAG == TAG_TOKEN {
                                        q = self.equiv(q);
                                        if q == NULL {
                                            break 'done2;
                                        }
                                        loop {
                                            p = self.link(p);
                                            if p == NULL {
                                                tt = self.type_(q);
                                                break 'done2;
                                            }
                                            if self.type_(q) != STRUCTURED {
                                                break 'done2;
                                            }
                                            q = self.link(self.attr_head(q));
                                            if p >= self.hi_mem_min {
                                                loop {
                                                    q = self.link(q);
                                                    if self.attr_loc(q) >= self.info(p) {
                                                        break;
                                                    }
                                                }
                                                if self.attr_loc(q) > self.info(p) {
                                                    break 'done2;
                                                }
                                            }
                                        }
                                    }
                                }
                                if tt >= UNSUFFIXED_MACRO {
                                    // §845
                                    self.set_link(tail, NULL);
                                    if tt > UNSUFFIXED_MACRO {
                                        post_head = self.get_avail()?;
                                        tail = post_head;
                                        self.set_link(tail, t);
                                        tt = UNDEFINED;
                                        macro_ref = self.value(q);
                                        self.add_mac_ref(macro_ref);
                                    } else {
                                        // §853
                                        let p = self.get_avail()?;
                                        self.set_info(pre_head, self.link(pre_head));
                                        self.set_link(pre_head, p);
                                        self.set_info(p, t);
                                        self.macro_call(self.value(q), pre_head, NULL)?;
                                        self.get_x_next()?;
                                        continue 'restart;
                                    }
                                }
                            }
                            self.get_x_next()?;
                            tail = t;
                            if self.cur_cmd == LEFT_BRACKET {
                                // §846
                                self.get_x_next()?;
                                self.scan_expression()?;
                                if self.cur_cmd != RIGHT_BRACKET {
                                    // §847
                                    self.back_input()?;
                                    self.back_expr()?;
                                    self.cur_cmd = LEFT_BRACKET;
                                    self.cur_mod = 0;
                                    self.cur_sym = FROZEN_LEFT_BRACKET;
                                } else {
                                    if self.cur_type != KNOWN {
                                        self.bad_subscript()?;
                                    }
                                    self.cur_cmd = NUMERIC_TOKEN;
                                    self.cur_mod = self.cur_exp;
                                    self.cur_sym = 0;
                                }
                            }
                            if self.cur_cmd > MAX_SUFFIX_TOKEN || self.cur_cmd < MIN_SUFFIX_TOKEN {
                                break;
                            }
                        }
                        // §852
                        if post_head != NULL {
                            // §854
                            self.back_input()?;
                            let p = self.get_avail()?;
                            let q = self.link(post_head);
                            self.set_info(pre_head, self.link(pre_head));
                            self.set_link(pre_head, post_head);
                            self.set_info(post_head, q);
                            self.set_link(post_head, p);
                            self.set_info(p, self.link(q));
                            self.set_link(q, NULL);
                            self.macro_call(macro_ref, pre_head, NULL)?;
                            let rc = self.ref_count(macro_ref) - 1;
                            self.set_ref_count(macro_ref, rc);
                            self.get_x_next()?;
                            continue 'restart;
                        }
                        let q = self.link(pre_head);
                        self.free_avail(pre_head);
                        if self.cur_cmd == my_var_flag {
                            self.cur_type = TOKEN_LIST;
                            self.cur_exp = q;
                            break 'done;
                        }
                        let p = self.find_variable(q)?;
                        if p != NULL {
                            self.make_exp_copy(p)?;
                        } else {
                            self.obliterated(q)?;
                            self.help_line[2] =
                                "While I was evaluating the suffix of this variable,";
                            self.help_line[1] =
                                "something was redefined, and it's no longer a variable!";
                            self.help_line[0] =
                                "In order to get back on my feet, I've inserted `0' instead.";
                            self.put_get_flush_error(0)?;
                        }
                        self.flush_node_list(q);
                        break 'done;
                    }
                    _ => {
                        self.bad_exp("A primary")?;
                        continue 'restart;
                    }
                }
                self.get_x_next()?;
            }
            // done:
            if self.cur_cmd == LEFT_BRACKET && self.cur_type >= KNOWN {
                // §859
                let p = self.stash_cur_exp()?;
                self.get_x_next()?;
                self.scan_expression()?;
                if self.cur_cmd != COMMA {
                    self.back_input()?;
                    self.back_expr()?;
                    self.cur_cmd = LEFT_BRACKET;
                    self.cur_mod = 0;
                    self.cur_sym = FROZEN_LEFT_BRACKET;
                    self.unstash_cur_exp(p)?;
                } else {
                    let q = self.stash_cur_exp()?;
                    self.get_x_next()?;
                    self.scan_expression()?;
                    if self.cur_cmd != RIGHT_BRACKET {
                        self.missing_err("]");
                        self.help(&[
                            "I've scanned an expression of the form `a[b,c',",
                            "so a right bracket should have come next.",
                            "I shall pretend that one was there.",
                        ]);
                        self.back_error()?;
                    }
                    let r = self.stash_cur_exp()?;
                    self.make_exp_copy(q)?;
                    self.do_binary(r, MINUS)?;
                    self.do_binary(p, TIMES)?;
                    self.do_binary(q, PLUS)?;
                    self.get_x_next()?;
                }
            }
            return Ok(());
        }
    }

    // §824
    pub(crate) fn bad_exp(&mut self, s: &str) -> R {
        self.print_err(s);
        self.prints(" expression can't begin with `");
        self.print_cmd_mod(self.cur_cmd, self.cur_mod)?;
        self.print_char(b'\'');
        self.help(&[
            "I'm afraid I need some sort of value in order to continue,",
            "so I've tentatively inserted `0'. You may want to",
            "delete this zero and insert something else;",
            "see Chapter 27 of The METAFONTbook for an example.",
        ]);
        self.back_input()?;
        self.cur_sym = 0;
        self.cur_cmd = NUMERIC_TOKEN;
        self.cur_mod = 0;
        self.ins_error()?;
        let save_flag = self.var_flag;
        self.var_flag = 0;
        self.get_x_next()?;
        self.var_flag = save_flag;
        Ok(())
    }

    // §827
    pub(crate) fn stash_in(&mut self, p: Pointer) -> R {
        self.set_type(p, self.cur_type);
        if self.cur_type == KNOWN {
            self.set_value(p, self.cur_exp);
        } else {
            if self.cur_type == INDEPENDENT {
                // §829
                let q = self.single_dependency(self.cur_exp)?;
                if q == self.dep_final {
                    self.set_type(p, KNOWN);
                    self.set_value(p, 0);
                    self.free_node(q, DEP_NODE_SIZE);
                } else {
                    self.set_type(p, DEPENDENT);
                    self.new_dep(p, q);
                }
                self.recycle_value(self.cur_exp)?;
            } else {
                self.mem[(p + 1) as usize] = self.mem[(self.cur_exp + 1) as usize];
                let pd = self.prev_dep(p);
                self.set_link(pd, p);
            }
            self.free_node(self.cur_exp, VALUE_NODE_SIZE);
        }
        self.cur_type = VACUOUS;
        Ok(())
    }

    // §847
    pub(crate) fn back_expr(&mut self) -> R {
        let p = self.stash_cur_exp()?;
        self.set_link(p, NULL);
        self.back_list(p)
    }

    // §848
    pub(crate) fn bad_subscript(&mut self) -> R {
        self.exp_err("Improper subscript has been replaced by zero")?;
        self.help(&[
            "A bracketed subscript must have a known numeric value;",
            "unfortunately, what I found was the value that appears just",
            "above this error message. So I'll try a zero subscript.",
        ]);
        self.flush_error(0)
    }

    // §851
    pub(crate) fn obliterated(&mut self, q: Pointer) -> R {
        self.print_err("Variable ");
        self.show_token_list(q, NULL, 1000, 0)?;
        self.prints(" has been obliterated");
        self.help(&[
            "It seems you did a nasty thing---probably by accident,",
            "but nevertheless you nearly hornswoggled me...",
            "While I was evaluating the right-hand side of this",
            "command, something happened, and the left-hand side",
            "is no longer a variable! So I won't change anything.",
        ]);
        Ok(())
    }

    // §855
    pub(crate) fn make_exp_copy(&mut self, p: Pointer) -> R {
        loop {
            self.cur_type = self.type_(p);
            match self.cur_type {
                VACUOUS | BOOLEAN_TYPE | KNOWN => self.cur_exp = self.value(p),
                UNKNOWN_BOOLEAN | UNKNOWN_STRING | UNKNOWN_PEN | UNKNOWN_PICTURE | UNKNOWN_PATH => {
                    self.cur_exp = self.new_ring_entry(p)?;
                }
                STRING_TYPE => {
                    self.cur_exp = self.value(p);
                    self.add_str_ref(self.cur_exp);
                }
                PEN_TYPE => {
                    self.cur_exp = self.value(p);
                    self.add_pen_ref(self.cur_exp);
                }
                PICTURE_TYPE => self.cur_exp = self.copy_edges(self.value(p))?,
                PATH_TYPE | FUTURE_PEN => self.cur_exp = self.copy_path(self.value(p))?,
                TRANSFORM_TYPE | PAIR_TYPE => {
                    // §857
                    if self.value(p) == NULL {
                        self.init_big_node(p)?;
                    }
                    let t = self.get_node(VALUE_NODE_SIZE)?;
                    self.set_name_type(t, CAPSULE);
                    self.set_type(t, self.cur_type);
                    self.init_big_node(t)?;
                    let bns = self.big_node_size[(self.cur_type - TRANSFORM_TYPE) as usize];
                    let mut q = self.value(p) + bns;
                    let mut r = self.value(t) + bns;
                    loop {
                        q -= 2;
                        r -= 2;
                        self.install(r, q)?;
                        if q == self.value(p) {
                            break;
                        }
                    }
                    self.cur_exp = t;
                }
                DEPENDENT | PROTO_DEPENDENT => {
                    let d = self.copy_dep_list(self.dep_list(p))?;
                    self.encapsulate(d)?;
                }
                NUMERIC_TYPE => {
                    self.new_indep(p)?;
                    continue;
                }
                INDEPENDENT => {
                    let q = self.single_dependency(p)?;
                    if q == self.dep_final {
                        self.cur_type = KNOWN;
                        self.cur_exp = 0;
                        self.free_node(q, DEP_NODE_SIZE);
                    } else {
                        self.cur_type = DEPENDENT;
                        self.encapsulate(q)?;
                    }
                }
                _ => return self.confusion("copy"),
            }
            return Ok(());
        }
    }

    // §856
    pub(crate) fn encapsulate(&mut self, p: Pointer) -> R {
        self.cur_exp = self.get_node(VALUE_NODE_SIZE)?;
        self.set_type(self.cur_exp, self.cur_type);
        self.set_name_type(self.cur_exp, CAPSULE);
        self.new_dep(self.cur_exp, p);
        Ok(())
    }

    // §858
    pub(crate) fn install(&mut self, r: Pointer, q: Pointer) -> R {
        if self.type_(q) == KNOWN {
            self.set_value(r, self.value(q));
            self.set_type(r, KNOWN);
        } else if self.type_(q) == INDEPENDENT {
            let p = self.single_dependency(q)?;
            if p == self.dep_final {
                self.set_type(r, KNOWN);
                self.set_value(r, 0);
                self.free_node(p, DEP_NODE_SIZE);
            } else {
                self.set_type(r, DEPENDENT);
                self.new_dep(r, p);
            }
        } else {
            self.set_type(r, self.type_(q));
            let d = self.copy_dep_list(self.dep_list(q))?;
            self.new_dep(r, d);
        }
        Ok(())
    }

    // §860
    pub(crate) fn scan_suffix(&mut self) -> R {
        let h = self.get_avail()?;
        let mut t = h;
        loop {
            if self.cur_cmd == LEFT_BRACKET {
                // §861
                self.get_x_next()?;
                self.scan_expression()?;
                if self.cur_type != KNOWN {
                    self.bad_subscript()?;
                }
                if self.cur_cmd != RIGHT_BRACKET {
                    self.missing_err("]");
                    self.help(&[
                        "I've seen a `[' and a subscript value, in a suffix,",
                        "so a right bracket should have come next.",
                        "I shall pretend that one was there.",
                    ]);
                    self.back_error()?;
                }
                self.cur_cmd = NUMERIC_TOKEN;
                self.cur_mod = self.cur_exp;
            }
            let p;
            if self.cur_cmd == NUMERIC_TOKEN {
                p = self.new_num_tok(self.cur_mod)?;
            } else if self.cur_cmd == TAG_TOKEN || self.cur_cmd == INTERNAL_QUANTITY {
                p = self.get_avail()?;
                self.set_info(p, self.cur_sym);
            } else {
                break;
            }
            self.set_link(t, p);
            t = p;
            self.get_x_next()?;
        }
        self.cur_exp = self.link(h);
        self.free_avail(h);
        self.cur_type = TOKEN_LIST;
        Ok(())
    }
}
