//! Parts 30 to 33 of `mf.web`: input stacks and states, maintaining them,
//! getting the next token.

use crate::arith::halfp;
use crate::{Mf, Pointer, R, Scaled, consts::*};

impl Mf {
    // §102
    pub(crate) fn round_decimals(&mut self, mut k: usize) -> Scaled {
        let mut a: i32 = 0;
        while k > 0 {
            k -= 1;
            a = (a + i32::from(self.dig[k]) * TWO) / 10;
        }
        halfp(a + 1)
    }

    // §631, §632
    #[inline]
    pub(crate) fn terminal_input(&self) -> bool {
        self.name() == 0
    }
    #[inline]
    pub(crate) fn token_type(&self) -> i32 {
        self.index()
    }
    #[inline]
    pub(crate) fn token_state(&self) -> bool {
        self.index() > MAX_IN_OPEN
    }
    #[inline]
    pub(crate) fn file_state(&self) -> bool {
        self.index() <= MAX_IN_OPEN
    }
    #[inline]
    pub(crate) fn param_start(&self) -> i32 {
        self.limit()
    }

    // §625
    pub(crate) fn show_cmd_mod(&mut self, c: i32, m: i32) -> R {
        self.begin_diagnostic();
        self.print_nl("{");
        self.print_cmd_mod(c, m)?;
        self.print_char(b'}');
        self.end_diagnostic(false);
        Ok(())
    }
    pub(crate) fn show_cur_cmd_mod(&mut self) -> R {
        self.show_cmd_mod(self.cur_cmd, self.cur_mod)
    }

    // §643
    pub(crate) fn set_trick_count(&mut self) {
        self.first_count = self.tally;
        self.trick_count = self.tally + 1 + self.error_line - self.half_error_line;
        if self.trick_count < self.error_line {
            self.trick_count = self.error_line;
        }
    }

    // §634
    pub(crate) fn show_context(&mut self) -> R {
        self.file_ptr = self.input_ptr;
        self.input_stack[self.file_ptr as usize] = self.cur_input;
        loop {
            self.cur_input = self.input_stack[self.file_ptr as usize];
            // §635
            if self.file_ptr == self.input_ptr
                || self.file_state()
                || self.token_type() != BACKED_UP
                || self.loc() != NULL
            {
                self.tally = 0;
                let old_setting = self.selector;
                let l;
                if self.file_state() {
                    // §636
                    if self.name() <= 1 {
                        if self.terminal_input() && self.file_ptr == 0 {
                            self.print_nl("<*>");
                        } else {
                            self.print_nl("<insert>");
                        }
                    } else if self.name() == 2 {
                        self.print_nl("<scantokens>");
                    } else {
                        self.print_nl("l.");
                        self.print_int(self.line);
                    }
                    self.print_char(b' ');
                    // §644
                    l = self.begin_pseudoprint();
                    if self.limit() > 0 {
                        for i in self.start()..self.limit() {
                            if i == self.loc() {
                                self.set_trick_count();
                            }
                            self.print(i32::from(self.buffer[i as usize]));
                        }
                    }
                } else {
                    // §637
                    match self.token_type() {
                        FOREVER_TEXT => self.print_nl("<forever> "),
                        LOOP_TEXT => {
                            // §638
                            self.print_nl("<for(");
                            let p = self.param_stack[self.param_start() as usize];
                            if p != NULL {
                                if self.link(p) == VOID {
                                    self.print_exp(p, 0)?;
                                } else {
                                    self.show_token_list(p, NULL, 20, self.tally)?;
                                }
                            }
                            self.prints(")> ");
                        }
                        PARAMETER => self.print_nl("<argument> "),
                        BACKED_UP => {
                            if self.loc() == NULL {
                                self.print_nl("<recently read> ");
                            } else {
                                self.print_nl("<to be read again> ");
                            }
                        }
                        INSERTED => self.print_nl("<inserted text> "),
                        MACRO => {
                            self.print_ln();
                            if self.name() != NULL {
                                self.slow_print(self.text(self.name()));
                            } else {
                                // §639
                                let ps = self.param_start() as usize;
                                let p = self.param_stack[ps];
                                if p == NULL {
                                    self.show_token_list(
                                        self.param_stack[ps + 1],
                                        NULL,
                                        20,
                                        self.tally,
                                    )?;
                                } else {
                                    let mut q = p;
                                    while self.link(q) != NULL {
                                        q = self.link(q);
                                    }
                                    self.set_link(q, self.param_stack[ps + 1]);
                                    self.show_token_list(p, NULL, 20, self.tally)?;
                                    self.set_link(q, NULL);
                                }
                            }
                            self.prints("->");
                        }
                        _ => self.print_nl("?"),
                    }
                    // §645
                    l = self.begin_pseudoprint();
                    if self.token_type() != MACRO {
                        self.show_token_list(self.start(), self.loc(), 100_000, 0)?;
                    } else {
                        self.show_macro(self.start(), self.loc(), 100_000)?;
                    }
                }
                self.selector = old_setting;
                // §641
                if self.trick_count == 1_000_000 {
                    self.set_trick_count();
                }
                let m = if self.tally < self.trick_count {
                    self.tally - self.first_count
                } else {
                    self.trick_count - self.first_count
                };
                let (p, n);
                if l + self.first_count <= self.half_error_line {
                    p = 0;
                    n = l + self.first_count;
                } else {
                    self.prints("...");
                    p = l + self.first_count - self.half_error_line + 3;
                    n = self.half_error_line;
                }
                for q in p..self.first_count {
                    self.print_char(self.trick_buf[(q % self.error_line) as usize]);
                }
                self.print_ln();
                for _ in 1..=n {
                    self.print_char(b' ');
                }
                let p = if m + n <= self.error_line {
                    self.first_count + m
                } else {
                    self.first_count + (self.error_line - n - 3)
                };
                for q in self.first_count..p {
                    self.print_char(self.trick_buf[(q % self.error_line) as usize]);
                }
                if m + n > self.error_line {
                    self.prints("...");
                }
            }
            if self.file_state() && (self.name() > 2 || self.file_ptr == 0) {
                break;
            }
            self.file_ptr -= 1;
        }
        self.cur_input = self.input_stack[self.input_ptr as usize];
        Ok(())
    }
    // §642
    fn begin_pseudoprint(&mut self) -> i32 {
        let l = self.tally;
        self.tally = 0;
        self.selector = PSEUDO;
        self.trick_count = 1_000_000;
        l
    }

    // §647
    pub(crate) fn push_input(&mut self) -> R {
        if self.input_ptr > self.max_in_stack {
            self.max_in_stack = self.input_ptr;
            if self.input_ptr == STACK_SIZE {
                return self.overflow("input stack size", STACK_SIZE);
            }
        }
        self.input_stack[self.input_ptr as usize] = self.cur_input;
        self.input_ptr += 1;
        Ok(())
    }
    pub(crate) fn pop_input(&mut self) {
        self.input_ptr -= 1;
        self.cur_input = self.input_stack[self.input_ptr as usize];
    }
    pub(crate) fn back_list(&mut self, p: Pointer) -> R {
        self.begin_token_list(p, BACKED_UP)
    }

    // §649
    pub(crate) fn begin_token_list(&mut self, p: Pointer, t: i32) -> R {
        self.push_input()?;
        self.set_start(p);
        self.set_index(t);
        self.set_limit(self.param_ptr);
        self.set_loc(p);
        Ok(())
    }

    // §650
    pub(crate) fn end_token_list(&mut self) -> R {
        'done: {
            if self.token_type() >= BACKED_UP {
                if self.token_type() <= INSERTED {
                    self.flush_token_list(self.start())?;
                    break 'done;
                } else {
                    self.delete_mac_ref(self.start())?;
                }
            }
            while self.param_ptr > self.param_start() {
                self.param_ptr -= 1;
                let p = self.param_stack[self.param_ptr as usize];
                if p != NULL {
                    if self.link(p) == VOID {
                        self.recycle_value(p)?;
                        self.free_node(p, VALUE_NODE_SIZE);
                    } else {
                        self.flush_token_list(p)?;
                    }
                }
            }
        }
        self.pop_input();
        self.check_interrupt()
    }

    // §651
    pub(crate) fn cur_tok(&mut self) -> R<Pointer> {
        let p;
        if self.cur_sym == 0 {
            if self.cur_cmd == CAPSULE_TOKEN {
                let save_type = self.cur_type;
                let save_exp = self.cur_exp;
                self.make_exp_copy(self.cur_mod)?;
                p = self.stash_cur_exp()?;
                self.set_link(p, NULL);
                self.cur_type = save_type;
                self.cur_exp = save_exp;
            } else {
                p = self.get_node(TOKEN_NODE_SIZE)?;
                self.set_value(p, self.cur_mod);
                self.set_name_type(p, TOKEN);
                if self.cur_cmd == NUMERIC_TOKEN {
                    self.set_type(p, KNOWN);
                } else {
                    self.set_type(p, STRING_TYPE);
                }
            }
        } else {
            p = self.fast_get_avail()?;
            self.set_info(p, self.cur_sym);
        }
        Ok(p)
    }

    // §652
    pub(crate) fn back_input(&mut self) -> R {
        let p = self.cur_tok()?;
        while self.token_state() && self.loc() == NULL {
            self.end_token_list()?;
        }
        self.back_list(p)
    }

    // §653
    pub(crate) fn back_error(&mut self) -> R {
        self.ok_to_interrupt = false;
        self.back_input()?;
        self.ok_to_interrupt = true;
        self.error()
    }
    pub(crate) fn ins_error(&mut self) -> R {
        self.ok_to_interrupt = false;
        self.back_input()?;
        self.set_index(INSERTED);
        self.ok_to_interrupt = true;
        self.error()
    }

    // §654
    pub(crate) fn begin_file_reading(&mut self) -> R {
        if self.in_open == MAX_IN_OPEN {
            return self.overflow("text input levels", MAX_IN_OPEN);
        }
        if self.first == self.buf_size {
            return self.overflow("buffer size", self.buf_size);
        }
        self.in_open += 1;
        self.push_input()?;
        self.set_index(self.in_open);
        self.line_stack[self.in_open as usize] = self.line;
        self.set_start(self.first);
        self.set_name(0);
        Ok(())
    }

    // §655
    pub(crate) fn end_file_reading(&mut self) -> R {
        self.first = self.start();
        self.line = self.line_stack[self.index() as usize];
        if self.index() != self.in_open {
            return self.confusion("endinput");
        }
        if self.name() > 2 {
            let i = self.index() as usize;
            self.input_file[i] = Default::default();
        }
        self.pop_input();
        self.in_open -= 1;
        Ok(())
    }

    // §656
    pub(crate) fn clear_for_error_prompt(&mut self) -> R {
        while self.file_state()
            && self.terminal_input()
            && self.input_ptr > 0
            && self.loc() == self.limit()
        {
            self.end_file_reading()?;
        }
        self.print_ln();
        Ok(())
    }

    // §661
    pub(crate) fn check_outer_validity(&mut self) -> R<bool> {
        if self.scanner_status == NORMAL {
            return Ok(true);
        }
        self.deletions_allowed = false;
        // §662
        if self.cur_sym != 0 {
            let p = self.get_avail()?;
            self.set_info(p, self.cur_sym);
            self.back_list(p)?;
        }
        if self.scanner_status > SKIPPING {
            // §663
            self.runaway()?;
            if self.cur_sym == 0 {
                self.print_err("File ended");
            } else {
                self.print_err("Forbidden token found");
            }
            self.prints(" while scanning ");
            self.help(&[
                "I suspect you have forgotten an `enddef',",
                "causing me to read past where you wanted me to stop.",
                "I'll try to recover; but if the error is serious,",
                "you'd better type `E' or `X' now and fix your file.",
            ]);
            // §664
            match self.scanner_status {
                FLUSHING => {
                    self.prints("to the end of the statement");
                    self.help_line[3] = "A previous error seems to have propagated,";
                    self.cur_sym = FROZEN_SEMICOLON;
                }
                ABSORBING => {
                    self.prints("a text argument");
                    self.help_line[3] = "It seems that a right delimiter was left out,";
                    if self.warning_info == 0 {
                        self.cur_sym = FROZEN_END_GROUP;
                    } else {
                        self.cur_sym = FROZEN_RIGHT_DELIMITER;
                        self.set_equiv(FROZEN_RIGHT_DELIMITER, self.warning_info);
                    }
                }
                VAR_DEFINING | OP_DEFINING => {
                    self.prints("the definition of ");
                    if self.scanner_status == OP_DEFINING {
                        self.slow_print(self.text(self.warning_info));
                    } else {
                        self.print_variable_name(self.warning_info)?;
                    }
                    self.cur_sym = FROZEN_END_DEF;
                }
                LOOP_DEFINING => {
                    self.prints("the text of a ");
                    self.slow_print(self.text(self.warning_info));
                    self.prints(" loop");
                    self.help_line[3] = "I suspect you have forgotten an `endfor',";
                    self.cur_sym = FROZEN_END_FOR;
                }
                _ => {}
            }
            self.ins_error()?;
        } else {
            self.print_err("Incomplete if; all text was ignored after line ");
            self.print_int(self.warning_info);
            self.help(&[
                "A forbidden `outer' token occurred in skipped text.",
                "This kind of error happens when you say `if...' and forget",
                "the matching `fi'. I've inserted a `fi'; this might work.",
            ]);
            if self.cur_sym == 0 {
                self.help_line[2] = "The file ended while I was skipping conditional text.";
            }
            self.cur_sym = FROZEN_FI;
            self.ins_error()?;
        }
        self.deletions_allowed = true;
        Ok(false)
    }

    // §665
    pub(crate) fn runaway(&mut self) -> R {
        if self.scanner_status > FLUSHING {
            self.print_nl("Runaway ");
            match self.scanner_status {
                ABSORBING => self.prints("text?"),
                VAR_DEFINING | OP_DEFINING => self.prints("definition?"),
                LOOP_DEFINING => self.prints("loop?"),
                _ => {}
            }
            self.print_ln();
            let hh = self.hold_head();
            self.show_token_list(self.link(hh), NULL, self.error_line - 10, 0)?;
        }
        Ok(())
    }

    #[inline]
    fn cls(&self, i: i32) -> u8 {
        self.char_class[self.buffer[i as usize] as usize]
    }

    // §667
    pub(crate) fn get_next(&mut self) -> R {
        'restart: loop {
            self.cur_sym = 0;
            if self.file_state() {
                // §669
                let k;
                'found: loop {
                    // switch:
                    let c = self.buffer[self.loc() as usize];
                    self.set_loc(self.loc() + 1);
                    let mut class = self.char_class[c as usize];
                    match class {
                        DIGIT_CLASS => {
                            return self.numeric_token(i32::from(c) - i32::from(b'0'), false);
                        }
                        PERIOD_CLASS => {
                            class = self.cls(self.loc());
                            if class > PERIOD_CLASS {
                                continue 'found;
                            } else if class < PERIOD_CLASS {
                                return self.numeric_token(0, true);
                            }
                        }
                        SPACE_CLASS => continue 'found,
                        PERCENT_CLASS => {
                            // §679
                            if self.name() > 2 {
                                // §681
                                self.line += 1;
                                self.first = self.start();
                                if !self.force_eof {
                                    if self.input_ln_file(self.index() as usize)? {
                                        self.firm_up_the_line()?;
                                    } else {
                                        self.force_eof = true;
                                    }
                                }
                                if self.force_eof {
                                    self.print_char(b')');
                                    self.open_parens -= 1;
                                    self.update_terminal();
                                    self.force_eof = false;
                                    self.end_file_reading()?;
                                    self.check_outer_validity()?;
                                    continue 'restart;
                                }
                                let lim = self.limit();
                                self.buffer[lim as usize] = b'%';
                                self.first = lim + 1;
                                self.set_loc(self.start());
                            } else {
                                if self.input_ptr > 0 {
                                    self.end_file_reading()?;
                                    continue 'restart;
                                }
                                if self.selector < LOG_ONLY {
                                    self.open_log_file()?;
                                }
                                if self.interaction > NONSTOP_MODE {
                                    if self.limit() == self.start() {
                                        self.print_nl("(Please type a command or say `end')");
                                    }
                                    self.print_ln();
                                    self.first = self.start();
                                    self.prompt_input("*")?;
                                    self.set_limit(self.last);
                                    self.buffer[self.last as usize] = b'%';
                                    self.first = self.last + 1;
                                    self.set_loc(self.start());
                                } else {
                                    return self
                                        .fatal_error("*** (job aborted, no legal end found)");
                                }
                            }
                            self.check_interrupt()?;
                            continue 'found;
                        }
                        STRING_CLASS => {
                            // §671
                            if self.buffer[self.loc() as usize] == b'"' {
                                self.cur_mod = self.pstr("");
                            } else {
                                let mut k = self.loc();
                                let lim = self.limit();
                                self.buffer[(lim + 1) as usize] = b'"';
                                loop {
                                    self.set_loc(self.loc() + 1);
                                    if self.buffer[self.loc() as usize] == b'"' {
                                        break;
                                    }
                                }
                                if self.loc() > self.limit() {
                                    // §672
                                    self.set_loc(self.limit());
                                    self.print_err("Incomplete string token has been flushed");
                                    self.help(&[
                                        "Strings should finish on the same line as they began.",
                                        "I've deleted the partial string; you might want to",
                                        "insert another by typing, e.g., `I\"new string\"'.",
                                    ]);
                                    self.deletions_allowed = false;
                                    self.error()?;
                                    self.deletions_allowed = true;
                                    continue 'restart;
                                }
                                if self.loc() == k + 1
                                    && self.length(i32::from(self.buffer[k as usize])) == 1
                                {
                                    self.cur_mod = i32::from(self.buffer[k as usize]);
                                } else {
                                    self.str_room(self.loc() - k)?;
                                    loop {
                                        self.append_char(self.buffer[k as usize]);
                                        k += 1;
                                        if k == self.loc() {
                                            break;
                                        }
                                    }
                                    self.cur_mod = self.make_string()?;
                                }
                            }
                            self.set_loc(self.loc() + 1);
                            self.cur_cmd = STRING_TOKEN;
                            return Ok(());
                        }
                        5..=8 => {
                            k = self.loc() - 1;
                            break 'found;
                        }
                        INVALID_CLASS => {
                            // §670
                            self.print_err("Text line contains an invalid character");
                            self.help(&[
                                "A funny symbol that I can't read has just been input.",
                                "Continue, and I'll forget that it ever happened.",
                            ]);
                            self.deletions_allowed = false;
                            self.error()?;
                            self.deletions_allowed = true;
                            continue 'restart;
                        }
                        _ => {}
                    }
                    k = self.loc() - 1;
                    while self.cls(self.loc()) == class {
                        self.set_loc(self.loc() + 1);
                    }
                    break 'found;
                }
                self.cur_sym = self.id_lookup(k, self.loc() - k)?;
            } else {
                // §676
                let loc = self.loc();
                if loc >= self.hi_mem_min {
                    self.cur_sym = self.info(loc);
                    self.set_loc(self.link(loc));
                    if self.cur_sym >= EXPR_BASE {
                        if self.cur_sym >= SUFFIX_BASE {
                            // §677
                            if self.cur_sym >= TEXT_BASE {
                                self.cur_sym -= PARAM_SIZE;
                            }
                            let p = self.param_stack
                                [(self.param_start() + self.cur_sym - SUFFIX_BASE) as usize];
                            self.begin_token_list(p, PARAMETER)?;
                            continue 'restart;
                        } else {
                            self.cur_cmd = CAPSULE_TOKEN;
                            self.cur_mod = self.param_stack
                                [(self.param_start() + self.cur_sym - EXPR_BASE) as usize];
                            self.cur_sym = 0;
                            return Ok(());
                        }
                    }
                } else if loc > NULL {
                    // §678
                    if self.name_type(loc) == TOKEN {
                        self.cur_mod = self.value(loc);
                        if self.type_(loc) == KNOWN {
                            self.cur_cmd = NUMERIC_TOKEN;
                        } else {
                            self.cur_cmd = STRING_TOKEN;
                            self.add_str_ref(self.cur_mod);
                        }
                    } else {
                        self.cur_mod = loc;
                        self.cur_cmd = CAPSULE_TOKEN;
                    }
                    self.set_loc(self.link(loc));
                    return Ok(());
                } else {
                    self.end_token_list()?;
                    continue 'restart;
                }
            }
            // §668
            self.cur_cmd = self.eq_type(self.cur_sym);
            self.cur_mod = self.equiv(self.cur_sym);
            if self.cur_cmd >= OUTER_TAG {
                if self.check_outer_validity()? {
                    self.cur_cmd -= OUTER_TAG;
                } else {
                    continue 'restart;
                }
            }
            return Ok(());
        }
    }

    /// §673-§675: a numeric token, from its integer part's first digit
    /// `n` (`start_numeric_token`), or at its fraction part
    /// (`start_decimal_token`, `n` zero).
    fn numeric_token(&mut self, mut n: i32, decimal: bool) -> R {
        let mut f = 0;
        let mut frac = decimal;
        if !decimal {
            // §673
            while self.cls(self.loc()) == DIGIT_CLASS {
                if n < 4096 {
                    n = 10 * n + i32::from(self.buffer[self.loc() as usize]) - i32::from(b'0');
                }
                self.set_loc(self.loc() + 1);
            }
            if self.buffer[self.loc() as usize] == b'.' && self.cls(self.loc() + 1) == DIGIT_CLASS {
                self.set_loc(self.loc() + 1);
                frac = true;
            }
        }
        if frac {
            // §674
            let mut k = 0usize;
            loop {
                if k < 17 {
                    self.dig[k] = self.buffer[self.loc() as usize] - b'0';
                    k += 1;
                }
                self.set_loc(self.loc() + 1);
                if self.cls(self.loc()) != DIGIT_CLASS {
                    break;
                }
            }
            f = self.round_decimals(k);
            if f == UNITY {
                n += 1;
                f = 0;
            }
        }
        // §675
        if n < 4096 {
            self.cur_mod = n * UNITY + f;
        } else {
            self.print_err("Enormous number has been reduced");
            self.help(&[
                "I can't handle numbers bigger than about 4095.99998;",
                "so I've changed your constant to that maximum amount.",
            ]);
            self.deletions_allowed = false;
            self.error()?;
            self.deletions_allowed = true;
            self.cur_mod = 0o1777777777;
        }
        self.cur_cmd = NUMERIC_TOKEN;
        Ok(())
    }

    // §682
    pub(crate) fn firm_up_the_line(&mut self) -> R {
        self.set_limit(self.last);
        if self.int(PAUSING) > 0 && self.interaction > NONSTOP_MODE {
            self.print_ln();
            if self.start() < self.limit() {
                for k in self.start()..self.limit() {
                    self.print(i32::from(self.buffer[k as usize]));
                }
            }
            self.first = self.limit();
            self.prompt_input("=>")?;
            if self.last > self.first {
                for k in self.first..self.last {
                    let d = (k + self.start() - self.first) as usize;
                    self.buffer[d] = self.buffer[k as usize];
                }
                self.set_limit(self.start() + self.last - self.first);
            }
        }
        Ok(())
    }
}
