//! Parts 34 and 35 of `mf.web`: scanning macro definitions, expanding the
//! next token.

use crate::{Mf, Pointer, R, StrNumber, consts::*};

impl Mf {
    pub(crate) fn missing_err_str(&mut self, s: StrNumber) {
        self.print_err("Missing `");
        self.print(s);
        self.prints("' has been inserted");
    }

    // §685
    pub(crate) fn scan_toks(
        &mut self,
        terminator: i32,
        subst_list: Pointer,
        tail_end: Pointer,
        suffix_count: i32,
    ) -> R<Pointer> {
        let hh = self.hold_head();
        let mut p = hh;
        let mut balance = 1;
        self.set_link(hh, NULL);
        loop {
            self.get_next()?;
            if self.cur_sym > 0 {
                // §686
                let mut q = subst_list;
                while q != NULL {
                    if self.info(q) == self.cur_sym {
                        self.cur_sym = self.value(q);
                        self.cur_cmd = RELAX;
                        break;
                    }
                    q = self.link(q);
                }
                if self.cur_cmd == terminator {
                    // §687
                    if self.cur_mod > 0 {
                        balance += 1;
                    } else {
                        balance -= 1;
                        if balance == 0 {
                            break;
                        }
                    }
                } else if self.cur_cmd == MACRO_SPECIAL {
                    // §690
                    if self.cur_mod == QUOTE {
                        self.get_next()?;
                    } else if self.cur_mod <= suffix_count {
                        self.cur_sym = SUFFIX_BASE - 1 + self.cur_mod;
                    }
                }
            }
            let t = self.cur_tok()?;
            self.set_link(p, t);
            p = t;
        }
        self.set_link(p, tail_end);
        self.flush_node_list(subst_list);
        Ok(self.link(hh))
    }

    // §691
    pub(crate) fn get_symbol(&mut self) -> R {
        loop {
            self.get_next()?;
            if self.cur_sym == 0 || self.cur_sym > FROZEN_INACCESSIBLE {
                self.print_err("Missing symbolic token inserted");
                self.help(&[
                    "Sorry: You can't redefine a number, string, or expr.",
                    "I've inserted an inaccessible symbol so that your",
                    "definition will be completed without mixing me up too badly.",
                ]);
                if self.cur_sym > 0 {
                    self.help_line[2] = "Sorry: You can't redefine my error-recovery tokens.";
                } else if self.cur_cmd == STRING_TOKEN {
                    self.delete_str_ref(self.cur_mod);
                }
                self.cur_sym = FROZEN_INACCESSIBLE;
                self.ins_error()?;
                continue;
            }
            return Ok(());
        }
    }

    // §692
    pub(crate) fn get_clear_symbol(&mut self) -> R {
        self.get_symbol()?;
        self.clear_symbol(self.cur_sym, false)
    }

    // §693
    pub(crate) fn check_equals(&mut self) -> R {
        if self.cur_cmd != EQUALS && self.cur_cmd != ASSIGNMENT {
            self.missing_err("=");
            self.help(&[
                "The next thing in this `def' should have been `=',",
                "because I've already looked at the definition heading.",
                "But don't worry; I'll pretend that an equals sign",
                "was present. Everything from here to `enddef'",
                "will be the replacement text of this macro.",
            ]);
            self.back_error()?;
        }
        Ok(())
    }

    // §694
    pub(crate) fn make_op_def(&mut self) -> R {
        let m = self.cur_mod;
        self.get_symbol()?;
        let q = self.get_node(TOKEN_NODE_SIZE)?;
        self.set_info(q, self.cur_sym);
        self.set_value(q, EXPR_BASE);
        self.get_clear_symbol()?;
        self.warning_info = self.cur_sym;
        self.get_symbol()?;
        let p = self.get_node(TOKEN_NODE_SIZE)?;
        self.set_info(p, self.cur_sym);
        self.set_value(p, EXPR_BASE + 1);
        self.set_link(p, q);
        self.get_next()?;
        self.check_equals()?;
        self.scanner_status = OP_DEFINING;
        let q = self.get_avail()?;
        self.set_ref_count(q, NULL);
        let r = self.get_avail()?;
        self.set_link(q, r);
        self.set_info(r, GENERAL_MACRO);
        let t = self.scan_toks(MACRO_DEF, p, NULL, 0)?;
        self.set_link(r, t);
        self.scanner_status = NORMAL;
        self.set_eq_type(self.warning_info, m);
        self.set_equiv(self.warning_info, q);
        self.get_x_next()
    }

    // §697
    pub(crate) fn scan_def(&mut self) -> R {
        let m = self.cur_mod;
        let mut c = GENERAL_MACRO;
        let hh = self.hold_head();
        self.set_link(hh, NULL);
        let mut q = self.get_avail()?;
        self.set_ref_count(q, NULL);
        let mut r = NULL;
        let n;
        // §699
        if m == START_DEF {
            self.get_clear_symbol()?;
            self.warning_info = self.cur_sym;
            self.get_next()?;
            self.scanner_status = OP_DEFINING;
            n = 0;
            self.set_eq_type(self.warning_info, DEFINED_MACRO);
            self.set_equiv(self.warning_info, q);
        } else {
            let p = self.scan_declared_variable()?;
            let e = self.equiv(self.info(p));
            self.flush_variable(e, self.link(p), true)?;
            self.warning_info = self.find_variable(p)?;
            self.flush_list(p);
            if self.warning_info == NULL {
                // §700
                self.print_err("This variable already starts with a macro");
                self.help(&[
                    "After `vardef a' you can't say `vardef a.b'.",
                    "So I'll have to discard this definition.",
                ]);
                self.error()?;
                self.warning_info = BAD_VARDEF;
            }
            self.scanner_status = VAR_DEFINING;
            let mut nn = 2;
            if self.cur_cmd == MACRO_SPECIAL && self.cur_mod == MACRO_SUFFIX {
                nn = 3;
                self.get_next()?;
            }
            n = nn;
            self.set_type(self.warning_info, UNSUFFIXED_MACRO - 2 + n);
            self.set_value(self.warning_info, q);
        }
        let mut k = n;
        if self.cur_cmd == LEFT_DELIMITER {
            // §702
            loop {
                let l_delim = self.cur_sym;
                let r_delim = self.cur_mod;
                self.get_next()?;
                let base;
                if self.cur_cmd == PARAM_TYPE && self.cur_mod >= EXPR_BASE {
                    base = self.cur_mod;
                } else {
                    self.print_err("Missing parameter type; `expr' will be assumed");
                    self.help(&["You should've had `expr' or `suffix' or `text' here."]);
                    self.back_error()?;
                    base = EXPR_BASE;
                }
                // §703
                loop {
                    let a = self.get_avail()?;
                    self.set_link(q, a);
                    q = a;
                    self.set_info(q, base + k);
                    self.get_symbol()?;
                    let p = self.get_node(TOKEN_NODE_SIZE)?;
                    self.set_value(p, base + k);
                    self.set_info(p, self.cur_sym);
                    if k == PARAM_SIZE {
                        return self.overflow("parameter stack size", PARAM_SIZE);
                    }
                    k += 1;
                    self.set_link(p, r);
                    r = p;
                    self.get_next()?;
                    if self.cur_cmd != COMMA {
                        break;
                    }
                }
                self.check_delimiter(l_delim, r_delim)?;
                self.get_next()?;
                if self.cur_cmd != LEFT_DELIMITER {
                    break;
                }
            }
        }
        if self.cur_cmd == PARAM_TYPE {
            // §704
            let mut p = self.get_node(TOKEN_NODE_SIZE)?;
            if self.cur_mod < EXPR_BASE {
                c = self.cur_mod;
                self.set_value(p, EXPR_BASE + k);
            } else {
                self.set_value(p, self.cur_mod + k);
                if self.cur_mod == EXPR_BASE {
                    c = EXPR_MACRO;
                } else if self.cur_mod == SUFFIX_BASE {
                    c = SUFFIX_MACRO;
                } else {
                    c = TEXT_MACRO;
                }
            }
            if k == PARAM_SIZE {
                return self.overflow("parameter stack size", PARAM_SIZE);
            }
            k += 1;
            self.get_symbol()?;
            self.set_info(p, self.cur_sym);
            self.set_link(p, r);
            r = p;
            self.get_next()?;
            if c == EXPR_MACRO && self.cur_cmd == OF_TOKEN {
                c = OF_MACRO;
                p = self.get_node(TOKEN_NODE_SIZE)?;
                if k == PARAM_SIZE {
                    return self.overflow("parameter stack size", PARAM_SIZE);
                }
                self.set_value(p, EXPR_BASE + k);
                self.get_symbol()?;
                self.set_info(p, self.cur_sym);
                self.set_link(p, r);
                r = p;
                self.get_next()?;
            }
        }
        self.check_equals()?;
        let mut p = self.get_avail()?;
        self.set_info(p, c);
        self.set_link(q, p);
        // §698
        if m == START_DEF {
            let t = self.scan_toks(MACRO_DEF, r, NULL, n)?;
            self.set_link(p, t);
        } else {
            let q = self.get_avail()?;
            self.set_info(q, self.bg_loc);
            self.set_link(p, q);
            p = self.get_avail()?;
            self.set_info(p, self.eg_loc);
            let t = self.scan_toks(MACRO_DEF, r, p, n)?;
            self.set_link(q, t);
        }
        if self.warning_info == BAD_VARDEF {
            self.flush_token_list(self.value(BAD_VARDEF))?;
        }
        self.scanner_status = NORMAL;
        self.get_x_next()
    }

    // §707
    pub(crate) fn expand(&mut self) -> R {
        if self.int(TRACING_COMMANDS) > UNITY && self.cur_cmd != DEFINED_MACRO {
            self.show_cur_cmd_mod()?;
        }
        match self.cur_cmd {
            IF_TEST => self.conditional()?,
            FI_OR_ELSE => {
                // §751
                if self.cur_mod > self.if_limit {
                    if self.if_limit == IF_CODE {
                        self.missing_err(":");
                        self.back_input()?;
                        self.cur_sym = FROZEN_COLON;
                        self.ins_error()?;
                    } else {
                        self.print_err("Extra ");
                        self.print_cmd_mod(FI_OR_ELSE, self.cur_mod)?;
                        self.help(&["I'm ignoring this; it doesn't match any if."]);
                        self.error()?;
                    }
                } else {
                    while self.cur_mod != FI_CODE {
                        self.pass_text()?;
                    }
                    self.pop_cond();
                }
            }
            INPUT => {
                // §711
                if self.cur_mod > 0 {
                    self.force_eof = true;
                } else {
                    self.start_input()?;
                }
            }
            ITERATION => {
                if self.cur_mod == END_FOR {
                    // §708
                    self.print_err("Extra `endfor'");
                    self.help(&[
                        "I'm not currently working on a for loop,",
                        "so I had better not try to end anything.",
                    ]);
                    self.error()?;
                } else {
                    self.begin_iteration()?;
                }
            }
            REPEAT_LOOP => {
                // §712
                while self.token_state() && self.loc() == NULL {
                    self.end_token_list()?;
                }
                if self.loop_ptr == NULL {
                    self.print_err("Lost loop");
                    self.help(&[
                        "I'm confused; after exiting from a loop, I still seem",
                        "to want to repeat it. I'll try to forget the problem.",
                    ]);
                    self.error()?;
                } else {
                    self.resume_iteration()?;
                }
            }
            EXIT_TEST => {
                // §713
                self.get_boolean()?;
                if self.int(TRACING_COMMANDS) > UNITY {
                    self.show_cmd_mod(NULLARY, self.cur_exp)?;
                }
                if self.cur_exp == TRUE_CODE {
                    if self.loop_ptr == NULL {
                        self.print_err("No loop is in progress");
                        self.help(&["Why say `exitif' when there's nothing to exit from?"]);
                        if self.cur_cmd == SEMICOLON {
                            self.error()?;
                        } else {
                            self.back_error()?;
                        }
                    } else {
                        // §714
                        let mut p = NULL;
                        loop {
                            if self.file_state() {
                                self.end_file_reading()?;
                            } else {
                                if self.token_type() <= LOOP_TEXT {
                                    p = self.start();
                                }
                                self.end_token_list()?;
                            }
                            if p != NULL {
                                break;
                            }
                        }
                        if p != self.info(self.loop_ptr) {
                            return self.fatal_error("*** (loop confusion)");
                        }
                        self.stop_iteration()?;
                    }
                } else if self.cur_cmd != SEMICOLON {
                    self.missing_err(";");
                    self.help(&[
                        "After `exitif <boolean expr>' I expect to see a semicolon.",
                        "I shall pretend that one was there.",
                    ]);
                    self.back_error()?;
                }
            }
            RELAX => {}
            EXPAND_AFTER => {
                // §715
                self.get_next()?;
                let p = self.cur_tok()?;
                self.get_next()?;
                if self.cur_cmd < MIN_COMMAND {
                    self.expand()?;
                } else {
                    self.back_input()?;
                }
                self.back_list(p)?;
            }
            SCAN_TOKENS => {
                // §716
                self.get_x_next()?;
                self.scan_primary()?;
                if self.cur_type != STRING_TYPE {
                    self.disp_err(NULL, "Not a string")?;
                    self.help(&[
                        "I'm going to flush this expression, since",
                        "scantokens should be followed by a known string.",
                    ]);
                    self.put_get_flush_error(0)?;
                } else {
                    self.back_input()?;
                    if self.length(self.cur_exp) > 0 {
                        // §717
                        self.begin_file_reading()?;
                        self.set_name(2);
                        let k = self.first + self.length(self.cur_exp);
                        if k >= self.max_buf_stack {
                            if k >= self.buf_size {
                                self.max_buf_stack = self.buf_size;
                                return self.overflow("buffer size", self.buf_size);
                            }
                            self.max_buf_stack = k + 1;
                        }
                        let mut j = self.str_start[self.cur_exp as usize];
                        self.set_limit(k);
                        while self.first < self.limit() {
                            self.buffer[self.first as usize] = self.str_pool[j as usize];
                            j += 1;
                            self.first += 1;
                        }
                        let lim = self.limit() as usize;
                        self.buffer[lim] = b'%';
                        self.first = self.limit() + 1;
                        self.set_loc(self.start());
                        self.flush_cur_exp(0)?;
                    }
                }
            }
            DEFINED_MACRO => self.macro_call(self.cur_mod, NULL, self.cur_sym)?,
            _ => {}
        }
        Ok(())
    }

    // §718
    pub(crate) fn get_x_next(&mut self) -> R {
        self.get_next()?;
        if self.cur_cmd < MIN_COMMAND {
            let save_exp = self.stash_cur_exp()?;
            loop {
                if self.cur_cmd == DEFINED_MACRO {
                    self.macro_call(self.cur_mod, NULL, self.cur_sym)?;
                } else {
                    self.expand()?;
                }
                self.get_next()?;
                if self.cur_cmd >= MIN_COMMAND {
                    break;
                }
            }
            self.unstash_cur_exp(save_exp)?;
        }
        Ok(())
    }

    // §720
    pub(crate) fn macro_call(
        &mut self,
        def_ref: Pointer,
        mut arg_list: Pointer,
        macro_name: Pointer,
    ) -> R {
        let mut r = self.link(def_ref);
        self.add_mac_ref(def_ref);
        let mut n;
        let mut tail = NULL;
        if arg_list == NULL {
            n = 0;
        } else {
            // §724
            n = 1;
            tail = arg_list;
            while self.link(tail) != NULL {
                n += 1;
                tail = self.link(tail);
            }
        }
        if self.int(TRACING_MACROS) > 0 {
            // §721
            self.begin_diagnostic();
            self.print_ln();
            self.print_macro_name(arg_list, macro_name)?;
            if n == 3 {
                self.prints("@#");
            }
            self.show_macro(def_ref, NULL, 100_000)?;
            if arg_list != NULL {
                n = 0;
                let mut p = arg_list;
                loop {
                    let q = self.info(p);
                    self.print_arg(q, n, 0)?;
                    n += 1;
                    p = self.link(p);
                    if p == NULL {
                        break;
                    }
                }
            }
            self.end_diagnostic(false);
        }
        // §725
        self.cur_cmd = COMMA + 1;
        let mut l_delim = NULL;
        let mut r_delim = NULL;
        while self.info(r) >= EXPR_BASE {
            // §726
            'found: {
                if self.cur_cmd != COMMA {
                    self.get_x_next()?;
                    if self.cur_cmd != LEFT_DELIMITER {
                        self.print_err("Missing argument to ");
                        self.print_macro_name(arg_list, macro_name)?;
                        self.help(&[
                            "That macro has more parameters than you thought.",
                            "I'll continue by pretending that each missing argument",
                            "is either zero or null.",
                        ]);
                        if self.info(r) >= SUFFIX_BASE {
                            self.cur_exp = NULL;
                            self.cur_type = TOKEN_LIST;
                        } else {
                            self.cur_exp = 0;
                            self.cur_type = KNOWN;
                        }
                        self.back_error()?;
                        self.cur_cmd = RIGHT_DELIMITER;
                        break 'found;
                    }
                    l_delim = self.cur_sym;
                    r_delim = self.cur_mod;
                }
                // §729
                if self.info(r) >= TEXT_BASE {
                    self.scan_text_arg(l_delim, r_delim)?;
                } else {
                    self.get_x_next()?;
                    if self.info(r) >= SUFFIX_BASE {
                        self.scan_suffix()?;
                    } else {
                        self.scan_expression()?;
                    }
                }
                if self.cur_cmd != COMMA {
                    // §727
                    if self.cur_cmd != RIGHT_DELIMITER || self.cur_mod != l_delim {
                        if self.info(self.link(r)) >= EXPR_BASE {
                            self.missing_err(",");
                            self.help(&[
                                "I've finished reading a macro argument and am about to",
                                "read another; the arguments weren't delimited correctly.",
                                "You might want to delete some tokens before continuing.",
                            ]);
                            self.back_error()?;
                            self.cur_cmd = COMMA;
                        } else {
                            self.missing_err_str(self.text(r_delim));
                            self.help(&[
                                "I've gotten to the end of the macro parameter list.",
                                "You might want to delete some tokens before continuing.",
                            ]);
                            self.back_error()?;
                        }
                    }
                }
            }
            // §728
            self.append_arg(&mut arg_list, &mut tail, &mut n, r)?;
            r = self.link(r);
        }
        if self.cur_cmd == COMMA {
            self.print_err("Too many arguments to ");
            self.print_macro_name(arg_list, macro_name)?;
            self.print_char(b';');
            self.print_nl("  Missing `");
            self.slow_print(self.text(r_delim));
            self.prints("' has been inserted");
            self.help(&[
                "I'm going to assume that the comma I just read was a",
                "right delimiter, and then I'll begin expanding the macro.",
                "You might want to delete some tokens before continuing.",
            ]);
            self.error()?;
        }
        if self.info(r) != GENERAL_MACRO {
            // §733
            if self.info(r) < TEXT_MACRO {
                self.get_x_next()?;
                if self.info(r) != SUFFIX_MACRO
                    && (self.cur_cmd == EQUALS || self.cur_cmd == ASSIGNMENT)
                {
                    self.get_x_next()?;
                }
            }
            match self.info(r) {
                PRIMARY_MACRO => self.scan_primary()?,
                SECONDARY_MACRO => self.scan_secondary()?,
                TERTIARY_MACRO => self.scan_tertiary()?,
                EXPR_MACRO => self.scan_expression()?,
                OF_MACRO => {
                    // §734
                    self.scan_expression()?;
                    let p = self.get_avail()?;
                    let s = self.stash_cur_exp()?;
                    self.set_info(p, s);
                    if self.int(TRACING_MACROS) > 0 {
                        self.begin_diagnostic();
                        self.print_arg(self.info(p), n, 0)?;
                        self.end_diagnostic(false);
                    }
                    if arg_list == NULL {
                        arg_list = p;
                    } else {
                        self.set_link(tail, p);
                    }
                    tail = p;
                    n += 1;
                    if self.cur_cmd != OF_TOKEN {
                        self.missing_err("of");
                        self.prints(" for ");
                        self.print_macro_name(arg_list, macro_name)?;
                        self.help(&["I've got the first argument; will look now for the other."]);
                        self.back_error()?;
                    }
                    self.get_x_next()?;
                    self.scan_primary()?;
                }
                SUFFIX_MACRO => {
                    // §735
                    if self.cur_cmd != LEFT_DELIMITER {
                        l_delim = NULL;
                    } else {
                        l_delim = self.cur_sym;
                        r_delim = self.cur_mod;
                        self.get_x_next()?;
                    }
                    self.scan_suffix()?;
                    if l_delim != NULL {
                        if self.cur_cmd != RIGHT_DELIMITER || self.cur_mod != l_delim {
                            self.missing_err_str(self.text(r_delim));
                            self.help(&[
                                "I've gotten to the end of the macro parameter list.",
                                "You might want to delete some tokens before continuing.",
                            ]);
                            self.back_error()?;
                        }
                        self.get_x_next()?;
                    }
                }
                TEXT_MACRO => self.scan_text_arg(0, 0)?,
                _ => {}
            }
            self.back_input()?;
            self.append_arg(&mut arg_list, &mut tail, &mut n, r)?;
        }
        r = self.link(r);
        // §736
        while self.token_state() && self.loc() == NULL {
            self.end_token_list()?;
        }
        if self.param_ptr + n > self.max_param_stack {
            self.max_param_stack = self.param_ptr + n;
            if self.max_param_stack > PARAM_SIZE {
                return self.overflow("parameter stack size", PARAM_SIZE);
            }
        }
        self.begin_token_list(def_ref, MACRO)?;
        self.set_name(macro_name);
        self.set_loc(r);
        if n > 0 {
            let mut p = arg_list;
            loop {
                self.param_stack[self.param_ptr as usize] = self.info(p);
                self.param_ptr += 1;
                p = self.link(p);
                if p == NULL {
                    break;
                }
            }
            self.flush_list(arg_list);
        }
        Ok(())
    }
    // §728
    fn append_arg(
        &mut self,
        arg_list: &mut Pointer,
        tail: &mut Pointer,
        n: &mut i32,
        r: Pointer,
    ) -> R {
        let p = self.get_avail()?;
        if self.cur_type == TOKEN_LIST {
            self.set_info(p, self.cur_exp);
        } else {
            let s = self.stash_cur_exp()?;
            self.set_info(p, s);
        }
        if self.int(TRACING_MACROS) > 0 {
            self.begin_diagnostic();
            self.print_arg(self.info(p), *n, self.info(r))?;
            self.end_diagnostic(false);
        }
        if *arg_list == NULL {
            *arg_list = p;
        } else {
            self.set_link(*tail, p);
        }
        *tail = p;
        *n += 1;
        Ok(())
    }

    // §722
    pub(crate) fn print_macro_name(&mut self, a: Pointer, n: Pointer) -> R {
        if n != NULL {
            self.slow_print(self.text(n));
        } else {
            let p = self.info(a);
            if p == NULL {
                let t = self.info(self.info(self.link(a)));
                self.slow_print(self.text(t));
            } else {
                let mut q = p;
                while self.link(q) != NULL {
                    q = self.link(q);
                }
                self.set_link(q, self.info(self.link(a)));
                self.show_token_list(p, NULL, 1000, 0)?;
                self.set_link(q, NULL);
            }
        }
        Ok(())
    }

    // §723
    pub(crate) fn print_arg(&mut self, q: Pointer, n: i32, b: Pointer) -> R {
        if self.link(q) == VOID {
            self.print_nl("(EXPR");
        } else if b < TEXT_BASE && b != TEXT_MACRO {
            self.print_nl("(SUFFIX");
        } else {
            self.print_nl("(TEXT");
        }
        self.print_int(n);
        self.prints(")<-");
        if self.link(q) == VOID {
            self.print_exp(q, 1)
        } else {
            self.show_token_list(q, NULL, 1000, 0)
        }
    }

    // §730
    pub(crate) fn scan_text_arg(&mut self, l_delim: Pointer, r_delim: Pointer) -> R {
        self.warning_info = l_delim;
        self.scanner_status = ABSORBING;
        let hh = self.hold_head();
        let mut p = hh;
        let mut balance = 1;
        self.set_link(hh, NULL);
        loop {
            self.get_next()?;
            if l_delim == 0 {
                // §732
                if self.cur_cmd > COMMA {
                    if balance == 1 {
                        break;
                    } else if self.cur_cmd == END_GROUP {
                        balance -= 1;
                    }
                } else if self.cur_cmd == BEGIN_GROUP {
                    balance += 1;
                }
            } else {
                // §731
                if self.cur_cmd == RIGHT_DELIMITER {
                    if self.cur_mod == l_delim {
                        balance -= 1;
                        if balance == 0 {
                            break;
                        }
                    }
                } else if self.cur_cmd == LEFT_DELIMITER && self.cur_mod == r_delim {
                    balance += 1;
                }
            }
            let t = self.cur_tok()?;
            self.set_link(p, t);
            p = t;
        }
        self.cur_exp = self.link(hh);
        self.cur_type = TOKEN_LIST;
        self.scanner_status = NORMAL;
        Ok(())
    }

    // §737
    pub(crate) fn stack_argument(&mut self, p: Pointer) -> R {
        if self.param_ptr == self.max_param_stack {
            self.max_param_stack += 1;
            if self.max_param_stack > PARAM_SIZE {
                return self.overflow("parameter stack size", PARAM_SIZE);
            }
        }
        self.param_stack[self.param_ptr as usize] = p;
        self.param_ptr += 1;
        Ok(())
    }
}
