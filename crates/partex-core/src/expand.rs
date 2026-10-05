//! Part 25: Expanding the next token (§366–§401).

use crate::host::Host;
use crate::mem::NULL;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// §1371: `end_write_token`.
pub(crate) const END_WRITE_TOKEN: i32 = CS_TOKEN_FLAG + END_WRITE;

/// The watchdog's limit (the CLI's `PARTEX_WATCHDOG=N`; 0, off): this many
/// macro calls with no command between them stop the job with a panic that
/// names the macros being expanded ([`Tex::watchdog`]).
pub static WATCHDOG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WATCH_COMMANDS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static WATCH_CALLS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §366: expand the expandable command `cur_cmd`, `cur_chr`.
    pub(crate) fn expand(&mut self) -> Result<(), Jump> {
        // (a macro called from inside another expansion is not a memo
        // candidate: that expansion may be collecting its tokens)
        let top = core::mem::replace(&mut self.memo.top, false);
        let r = self.expand_inner();
        self.memo.top = top;
        r
    }

    fn expand_inner(&mut self) -> Result<(), Jump> {
        // web2c: `expand_depth` limits the recursion.
        self.expand_depth_count += 1;
        if self.expand_depth_count >= self.params.expand_depth {
            return self.overflow(b"expansion depth", self.params.expand_depth);
        }
        let cv_backup = self.cur_val;
        let cg_backup = self.cur_glue;
        let cvl_backup = self.cur_val_level;
        let radix_backup = self.radix;
        let co_backup = self.cur_order;
        if self.cur_cmd < CALL {
            self.expand_nonmacro()?;
        } else if self.cur_cmd < END_TEMPLATE {
            self.macro_call()?;
        } else {
            // §375: insert a token containing `frozen_endv`.
            self.cur_tok = CS_TOKEN_FLAG + FROZEN_ENDV;
            self.back_input()?;
        }
        self.cur_val = cv_backup;
        self.cur_glue = cg_backup;
        self.cur_val_level = cvl_backup;
        self.radix = radix_backup;
        self.cur_order = co_backup;
        self.expand_depth_count -= 1;
        Ok(())
    }

    /// §367: expand a nonmacro.
    fn expand_nonmacro(&mut self) -> Result<(), Jump> {
        self.origin_expand();
        if self.memo.recording() && !Self::memo_expand_ok(self.cur_cmd, self.cur_chr) {
            self.memo.impure_cmd(self.cur_cmd);
        }
        if self.int_par(TRACING_COMMANDS_CODE) > 1 {
            self.show_cur_cmd_chr();
        }
        match self.cur_cmd {
            TOP_BOT_MARK => {
                // §386: insert the appropriate mark text into the scanner.
                let t = self.cur_chr % MARKS_CODE;
                let class = if self.cur_chr >= MARKS_CODE {
                    self.scan_register_num()?;
                    self.cur_val
                } else {
                    0
                };
                if let Some(m) = self.mark(class, t) {
                    // (the mark's list itself: a shared value)
                    self.begin_token_list(m, MARK_TEXT)?;
                }
            }
            EXPAND_AFTER if self.cur_chr != 0 => {
                // e-TeX: `\unless`: negate a boolean conditional.
                self.get_token()?;
                if self.cur_cmd == IF_TEST && self.cur_chr != IF_CASE_CODE {
                    self.cur_chr += UNLESS_CODE;
                    return self.expand_nonmacro();
                }
                self.print_err(b"You can't use `");
                self.print_esc(b"unless");
                self.print_str(b"' before `");
                self.print_cmd_chr(self.cur_cmd, self.cur_chr);
                self.print_char(b'\'');
                self.help(&[b"Continue, and I'll forget that it ever happened."]);
                self.back_error()?;
            }
            EXPAND_AFTER => {
                // §368: expand the token after the next token.
                self.get_token()?;
                let t = self.cur_tok;
                // (with glyph origins: `t`'s, before the input moves on)
                let t_org = self.back_org();
                self.get_token()?;
                if self.cur_cmd > MAX_COMMAND {
                    self.expand()?;
                } else {
                    self.back_input()?;
                }
                self.cur_tok = t;
                self.back_input_from(t_org)?;
            }
            NO_EXPAND if self.cur_chr == 1 => {
                // pdfTeX §394: implement `\pdfprimitive`.
                let save_scanner_status = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_token()?;
                self.scanner_status = save_scanner_status;
                let p = self.prim_of_cur_cs()?;
                if p != UNDEFINED_PRIMITIVE {
                    let (cmd, chr) = self.prim_meaning(p);
                    if cmd > MAX_COMMAND {
                        self.cur_cmd = cmd;
                        self.cur_chr = chr;
                        self.cur_tok = cmd * MAX_CHAR_VAL + chr;
                        self.cur_cs = 0;
                        return self.expand_nonmacro(); // `goto reswitch`
                    }
                    self.back_input()?; // now `loc` and `start` point to a one-item list
                    self.insert_front(CS_TOKEN_FLAG + FROZEN_PRIMITIVE);
                }
            }
            NO_EXPAND => {
                // §369: suppress expansion of the next token.
                let save_scanner_status = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_token()?;
                self.scanner_status = save_scanner_status;
                let t = self.cur_tok;
                self.back_input()?; // now `start` and `loc` point to the backed-up token `t`
                if t >= CS_TOKEN_FLAG && t != END_WRITE_TOKEN {
                    // Put `\notexpanded:` in front of the backed-up token.
                    self.insert_front(CS_TOKEN_FLAG + FROZEN_DONT_EXPAND);
                }
            }
            CS_NAME => self.manufacture_cs_name()?,
            CONVERT => self.conv_toks()?,
            THE => self.ins_the_toks()?,
            IF_TEST => self.conditional()?,
            FI_OR_ELSE => self.terminate_conditional()?,
            INPUT => {
                // §378: initiate or terminate input from a file.
                if self.cur_chr == 1 {
                    self.force_eof = true;
                } else if self.cur_chr == 2 {
                    self.pseudo_start()?; // e-TeX: `\scantokens`
                } else if self.name_in_progress {
                    self.insert_relax()?;
                } else {
                    self.start_input()?;
                }
            }
            _ => {
                // §370: complain about an undefined macro.
                self.print_err(b"Undefined control sequence");
                self.help(&[
                    b"The control sequence at the end of the top line",
                    b"of your error message was never \\def'ed. If you have",
                    b"misspelled it (e.g., `\\hobx'), type `I' and the correct",
                    b"spelling (e.g., `I\\hbox'). Otherwise just continue,",
                    b"and I'll forget about whatever was undefined.",
                ]);
                self.error()?;
            }
        }
        Ok(())
    }

    /// §372: manufacture a control sequence name.
    fn manufacture_cs_name(&mut self) -> Result<(), Jump> {
        let b = self.is_in_csname;
        self.is_in_csname = true;
        let name = self.scan_cs_name(true)?;
        self.is_in_csname = b;
        self.look_up_cs_name(&name, true)?;
        if self.eq_type(self.cur_cs) == UNDEFINED_CS {
            self.eq_define(self.cur_cs, RELAX, TOO_BIG_USV)?; // N.B.: The `save_stack` might change
        } // the control sequence will now match `\relax`
        self.cur_tok = self.cur_cs + CS_TOKEN_FLAG;
        self.back_input()
    }

    /// §372: the character tokens up to `\endcsname`, expanded;
    /// `\endcsname` must be the plain one if `plain`.
    pub(crate) fn scan_cs_name(&mut self, plain: bool) -> Result<alloc::vec::Vec<i32>, Jump> {
        // the list of characters (sized for most names: growing it from
        // empty showed in profiles, one reallocation per doubling)
        let mut name = alloc::vec::Vec::with_capacity(64);
        loop {
            self.bulk_chars(&mut name); // (a run at once, `bulk.rs`)
            self.get_x_token()?;
            if self.cur_cs == 0 {
                name.push(self.cur_tok);
            }
            if self.cur_cs != 0 {
                break;
            }
        }
        if self.cur_cmd != END_CS_NAME || (plain && self.cur_chr != 0) {
            // §373: complain about missing \endcsname.
            self.print_err(b"Missing ");
            self.print_esc(b"endcsname");
            self.print_str(b" inserted");
            self.help(&[
                b"The control sequence marked <to be read again> should",
                b"not appear between \\csname and \\endcsname.",
            ]);
            self.back_error()?;
        }
        Ok(name)
    }

    /// §374: look up the characters `name` in the hash table, and set
    /// `cur_cs` (entering a new name only if `new`).
    pub(crate) fn look_up_cs_name(&mut self, name: &[i32], new: bool) -> Result<(), Jump> {
        let first = self.first;
        let mut j = first;
        for &t in name {
            // (the size tested apart from the statistic, which a dropped
            // run leaves at its highest)
            if j >= self.max_buf_stack || j + 1 >= crate::input::ux(self.params.buf_size) {
                self.max_buf_stack = self.max_buf_stack.max(j + 1);
                if j + 1 >= crate::input::ux(self.params.buf_size) {
                    return self.overflow(b"buffer size", self.params.buf_size);
                }
            }
            self.buffer[j] = crate::input::cu(tok_chr(t));
            j += 1;
        }
        if j > first + 1 {
            self.no_new_control_sequence = !new;
            self.cur_cs = self.id_lookup(first, j - first)?;
            self.no_new_control_sequence = true;
        } else if j == first {
            self.cur_cs = NULL_CS; // the list is empty
        } else {
            self.cur_cs = SINGLE_BASE + crate::input::ci(self.buffer[first]); // the list has length one
        }
        Ok(())
    }

    /// §379
    pub(crate) fn insert_relax(&mut self) -> Result<(), Jump> {
        self.cur_tok = CS_TOKEN_FLAG + self.cur_cs;
        self.back_input()?;
        self.cur_tok = CS_TOKEN_FLAG + FROZEN_RELAX;
        self.back_input()?;
        self.cur_input.index = INSERTED; // `token_type`
        Ok(())
    }

    /// §380: set `cur_cmd`, `cur_chr`, `cur_tok`, and expand macros.
    pub(crate) fn get_x_token(&mut self) -> Result<(), Jump> {
        loop {
            self.get_next()?;
            if self.cur_cmd <= MAX_COMMAND {
                break;
            }
            if self.cur_cmd >= CALL {
                if self.cur_cmd < END_TEMPLATE {
                    self.macro_call()?;
                } else {
                    self.cur_cs = FROZEN_ENDV;
                    self.cur_cmd = ENDV;
                    break; // `cur_chr=null_list`
                }
            } else {
                self.expand()?;
            }
        }
        self.set_cur_tok();
        Ok(())
    }

    /// §380 for `big_switch` (§1030): [`Self::get_x_token`], and where
    /// its loop returns to `restart` after an expansion that left only
    /// exhausted lists above a file, a candidate (DESIGN §7.16.1, "A
    /// candidate is also taken inside `big_switch`'s fetch"): LaTeX's
    /// `\par` ends in a macro that expands to nothing, and the next
    /// paragraph's first letter is fetched from the file right after it.
    /// A stop there resumes at `big_switch`, the command counted.
    pub(crate) fn get_command(&mut self) -> Result<(), Jump> {
        if !self.stop_at_candidate || !crate::machine::clean_cuts() {
            return self.get_x_token();
        }
        loop {
            self.get_next()?;
            if self.cur_cmd <= MAX_COMMAND {
                break;
            }
            if self.cur_cmd >= CALL {
                if self.cur_cmd < END_TEMPLATE {
                    self.macro_call()?;
                } else {
                    self.cur_cs = FROZEN_ENDV;
                    self.cur_cmd = ENDV;
                    break; // `cur_chr=null_list`
                }
            } else {
                self.expand()?;
            }
            if self.cur_input.state == crate::web::TOKEN_LIST && self.cur_input.loc == NULL {
                self.pop_exhausted_lists()?;
                if self.cur_input.state != crate::web::TOKEN_LIST {
                    self.at_checkpoint = true;
                    return Err(Jump::Checkpoint);
                }
            }
        }
        self.set_cur_tok();
        Ok(())
    }

    /// e-TeX: `get_x_or_protected`: `get_x_token`, except that protected
    /// macros are not expanded.
    pub(crate) fn get_x_or_protected(&mut self) -> Result<(), Jump> {
        loop {
            self.get_token()?;
            if self.cur_cmd <= MAX_COMMAND {
                return Ok(());
            }
            if (CALL..END_TEMPLATE).contains(&self.cur_cmd) && self.is_protected(self.cur_cs) {
                return Ok(());
            }
            self.expand()?;
        }
    }

    /// Is the macro that control sequence `p` means `\protected`?
    pub(crate) fn is_protected(&self, p: i32) -> bool {
        self.peek_obj(p)
            .and_then(crate::objs::Obj::toks)
            .is_some_and(|t| t.protected())
    }

    /// §381: `get_x_token` without the initial `get_next`.
    pub(crate) fn x_token(&mut self) -> Result<(), Jump> {
        while self.cur_cmd > MAX_COMMAND {
            self.expand()?;
            self.get_next()?;
        }
        self.set_cur_tok();
        Ok(())
    }

    fn set_cur_tok(&mut self) {
        self.cur_tok = if self.cur_cs == 0 {
            self.cur_cmd * MAX_CHAR_VAL + self.cur_chr
        } else {
            CS_TOKEN_FLAG + self.cur_cs
        };
    }

    /// §389: invoke a user-defined control sequence.
    /// [`WATCHDOG`]: the macro calls since the last command counted; past
    /// the limit, a panic naming the macro and the input stack's macros
    /// (an expansion that never ends, which a tracker cannot show: its
    /// terminal is the build's, written at the end).
    #[cold]
    fn watchdog(&self) {
        use core::sync::atomic::Ordering::Relaxed;
        let c = self.commands();
        if WATCH_COMMANDS.swap(c, Relaxed) != c {
            WATCH_CALLS.store(0, Relaxed);
            return;
        }
        if WATCH_CALLS.fetch_add(1, Relaxed) < WATCHDOG.load(Relaxed) {
            return;
        }
        let name = |t: &Self, cs: i32| -> alloc::string::String {
            let n = if cs > 0 && cs <= t.eqtb_top {
                t.peek_text(cs)
            } else {
                0
            };
            match usize::try_from(n) {
                Ok(n) if n > 0 && n < t.str_ptr => {
                    alloc::string::String::from_utf8_lossy(t.str_bytes(n)).into_owned()
                }
                _ => alloc::format!("#{cs}"),
            }
        };
        let mut levels = alloc::vec::Vec::new();
        for i in 0..=self.input_ptr {
            let r = if i < self.input_ptr {
                &self.input_stack[i]
            } else {
                &self.cur_input
            };
            levels.push(if r.state != TOKEN_LIST {
                alloc::format!("file:{}", r.name)
            } else if r.index == MACRO {
                name(self, r.name)
            } else {
                alloc::format!("list{}", r.index)
            });
        }
        panic!(
            "watchdog: {} macro calls with no command: \\{} at line {}; the input: {}\n{}",
            WATCHDOG.load(Relaxed),
            name(self, self.cur_cs),
            self.line,
            levels.join(" > "),
            self.tracker.diagnose()
        );
    }

    pub(crate) fn macro_call(&mut self) -> Result<(), Jump> {
        if WATCHDOG.load(core::sync::atomic::Ordering::Relaxed) > 0 {
            self.watchdog();
        }
        if self.tracker.stop_expanding(self.commands()) {
            self.at_checkpoint = true;
            return Err(Jump::Checkpoint);
        }
        self.origin_expand();
        let save_scanner_status = self.scanner_status;
        let save_warning_index = self.warning_index;
        self.warning_index = self.cur_cs;
        // (the arguments, in a vector the engine keeps: argument scanning
        // never expands, so no other macro call uses it meanwhile)
        let mut pstack = core::mem::take(&mut self.pstack_buf);
        pstack.clear();
        // (argument scanning stores the tokens: a control sequence's
        // meaning matters by its class alone)
        let r = self.tokens_only(|t| t.macro_call_body(&mut pstack));
        self.pstack_buf = pstack;
        // exit:
        self.scanner_status = save_scanner_status;
        self.warning_index = save_warning_index;
        r
    }

    fn macro_call_body(
        &mut self,
        pstack: &mut alloc::vec::Vec<Option<crate::tok::Tokens>>,
    ) -> Result<(), Jump> {
        // the macro's token list: the meaning of `cur_cs`, a shared value
        let ref_count = self.equiv_toks(self.cur_cs).cloned().unwrap_or_default();
        // `r` indexes the macro's parameter part (tex.web's pointer `r`).
        let mut r: i32 = 0;
        let mut n = 0usize;
        if self.int_par(TRACING_MACROS_CODE) > 0 {
            // §401: show the text of the macro being expanded.
            self.begin_diagnostic();
            self.print_ln();
            self.print_cs(self.warning_index);
            self.token_show(&ref_count);
            self.end_diagnostic(false);
        }
        // (the body read in place: the list is held once, by `ref_count`)
        let body: &[i32] = &ref_count;
        let info = |_: &Self, i: i32| {
            body.get(crate::input::ux(i))
                .copied()
                .unwrap_or(END_MATCH_TOKEN)
        };
        let is_match = |t: i32| (MATCH_TOKEN..=END_MATCH_TOKEN).contains(&t);
        // (glyph origins: each argument token's, kept with the argument)
        let on = self.origins_on();
        if info(self, r) != END_MATCH_TOKEN {
            // §391: scan the parameters and make `r` index the end of the
            // parameter part; but return if an illegal \par is detected.
            self.scanner_status = MATCHING;
            let mut unbalance = 0;
            self.long_state = self.eq_type(self.cur_cs);
            if self.long_state >= OUTER_CALL {
                self.long_state -= 2;
            }
            let mut m = 0;
            // `s` indexes the delimiter of the current parameter; `NULL`
            // when only a delimiter string is being matched.
            let mut s;
            let mut match_chr = 0;
            loop {
                // The argument list (tex.web's `link(temp_head):=null`).
                self.arg_list.clear();
                if on {
                    self.arg_orgs_clear();
                }
                self.arg_active = true;
                let arg = crate::bulk::Dst::Arg;
                if info(self, r) > MATCH_TOKEN + 255 || info(self, r) < MATCH_TOKEN {
                    s = NULL;
                } else {
                    match_chr = info(self, r) - MATCH_TOKEN;
                    s = r + 1;
                    r = s;
                    m = 0;
                }
                // §392: scan a parameter until its delimiter string has
                // been found; or, if `s=null`, simply scan the delimiter
                // string.
                'continue_: loop {
                    // (a run up to the delimiter at once, `bulk.rs`)
                    if s == r && s != NULL && self.bulk_on() && !is_match(info(self, r)) {
                        let par_ends = self.long_state != LONG_CALL;
                        m += self.bulk_level0(arg, info(self, r), par_ends);
                    }
                    self.get_token()?; // set `cur_tok` to the next token of input
                    if self.cur_tok == info(self, r) {
                        // §394: advance `r`; `goto found` if the parameter
                        // delimiter has been fully matched, otherwise
                        // `goto continue`.
                        r += 1;
                        if info(self, r) >= MATCH_TOKEN && info(self, r) <= END_MATCH_TOKEN {
                            if self.cur_tok < LEFT_BRACE_LIMIT {
                                self.set_align_state(self.align_state() - 1);
                            }
                            break 'continue_; // found
                        }
                        continue 'continue_;
                    }
                    // §397: contribute the recently matched tokens to the
                    // current parameter, and `goto continue` if a partial
                    // match is still in effect; but abort if `s=null`.
                    if s != r {
                        if s == NULL {
                            // §398: report an improper use of the macro and abort.
                            self.print_err(b"Use of ");
                            self.sprint_cs(self.warning_index);
                            self.print_str(b" doesn't match its definition");
                            self.help(&[
                                b"If you say, e.g., `\\def\\a1{...}', then you must always",
                                b"put `1' after `\\a', since control sequence names are",
                                b"made up of letters only. The macro here has not been",
                                b"followed by the required stuff, so I'm ignoring it.",
                            ]);
                            return self.error();
                        }
                        let mut t = s;
                        loop {
                            let x = info(self, t);
                            self.arg_list.push(x);
                            m += 1;
                            let mut u = t + 1;
                            let mut v = s;
                            loop {
                                if u == r {
                                    if self.cur_tok != info(self, v) {
                                        break; // done
                                    }
                                    r = v + 1;
                                    continue 'continue_;
                                }
                                if info(self, u) != info(self, v) {
                                    break; // done
                                }
                                u += 1;
                                v += 1;
                            }
                            t += 1;
                            if t == r {
                                break;
                            }
                        }
                        r = s; // at this point, no tokens are recently matched
                    }
                    if self.cur_tok == self.par_token && self.long_state != LONG_CALL {
                        return self.runaway_argument(pstack, n, unbalance);
                    }
                    if self.cur_tok < RIGHT_BRACE_LIMIT {
                        if self.cur_tok < LEFT_BRACE_LIMIT {
                            // §399: contribute an entire group to the
                            // current parameter.
                            unbalance = 1;
                            let how = crate::bulk::Absorb::Arg {
                                par_ends: self.long_state != LONG_CALL,
                            };
                            loop {
                                if on {
                                    self.arg_org();
                                }
                                self.arg_list.push(self.cur_tok);
                                // (the group's inside at once, `bulk.rs`)
                                self.bulk_group(arg, &mut unbalance, how);
                                self.get_token()?;
                                if self.cur_tok == self.par_token && self.long_state != LONG_CALL {
                                    return self.runaway_argument(pstack, n, unbalance);
                                }
                                if self.cur_tok < RIGHT_BRACE_LIMIT {
                                    if self.cur_tok < LEFT_BRACE_LIMIT {
                                        unbalance += 1;
                                    } else {
                                        unbalance -= 1;
                                        if unbalance == 0 {
                                            break;
                                        }
                                    }
                                }
                            }
                            if on {
                                self.arg_org();
                            }
                            self.arg_list.push(self.cur_tok);
                        } else {
                            // §395: report an extra right brace and
                            // `goto continue`.
                            self.back_input()?;
                            self.print_err(b"Argument of ");
                            self.sprint_cs(self.warning_index);
                            self.print_str(b" has an extra }");
                            self.help(&[
                                b"I've run across a `}' that doesn't seem to match anything.",
                                b"For example, `\\def\\a#1{...}' and `\\a}' would produce",
                                b"this error. If you simply proceed now, the `\\par' that",
                                b"I've just inserted will cause me to report a runaway",
                                b"argument that might be the root of the problem. But if",
                                b"your `}' was spurious, just type `2' and it will go away.",
                            ]);
                            self.set_align_state(self.align_state() + 1);
                            self.long_state = CALL;
                            self.cur_tok = self.par_token;
                            self.ins_error()?;
                            continue 'continue_;
                        } // a white lie; the \par won't always trigger a runaway
                    } else {
                        // §393: store the current token, but `goto continue`
                        // if it is a blank space that would become an
                        // undelimited parameter.
                        if self.cur_tok == SPACE_TOKEN
                            && info(self, r) <= END_MATCH_TOKEN
                            && info(self, r) >= MATCH_TOKEN
                        {
                            continue 'continue_;
                        }
                        if on {
                            self.arg_org();
                        }
                        self.arg_list.push(self.cur_tok);
                    }
                    m += 1;
                    if info(self, r) > END_MATCH_TOKEN || info(self, r) < MATCH_TOKEN {
                        continue 'continue_;
                    }
                    break 'continue_;
                }
                // found:
                if s != NULL {
                    // §400: tidy up the parameter just scanned, and tuck it
                    // away: a single group loses its braces.
                    let last = self.arg_list.last().copied();
                    let braced = m == 1 && last.is_some_and(|t| t < RIGHT_BRACE_LIMIT);
                    let a = core::mem::take(&mut self.arg_list);
                    // (the argument made a value, in a pooled allocation)
                    let mut v = self.pooled_list(|b| {
                        if braced {
                            b.extend_from_slice(&a[1..a.len() - 1]);
                        } else {
                            b.extend_from_slice(&a);
                        }
                    });
                    if on {
                        // (its tokens' origins, with it)
                        self.arg_orgs_give(&mut v, a.len(), braced);
                    }
                    self.arg_list = a;
                    self.arg_list.clear();
                    self.arg_active = false; // the argument is taken
                    pstack.push(Some(v));
                    n += 1;
                    if self.int_par(TRACING_MACROS_CODE) > 0 {
                        self.begin_diagnostic();
                        self.print_nl(&[u8::try_from(match_chr).unwrap_or(0)]);
                        self.print_int(i32::try_from(n).unwrap_or(0));
                        self.print_str(b"<-");
                        let shown = pstack[n - 1].clone().unwrap_or_default();
                        self.show_token_list(&shown, NULL, 1000);
                        self.end_diagnostic(false);
                    }
                }
                // now `info(r)` is a token whose command code is either
                // `match` or `end_match`
                if info(self, r) == END_MATCH_TOKEN {
                    break;
                }
            }
            self.arg_list.clear();
            self.arg_active = false;
        }
        if T::PROFILE {
            // (FNV-1a over the argument tokens, for the profiler)
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for a in &pstack[..n] {
                let toks: &[i32] = a.as_deref().map_or(&[][..], |l| l.tokens());
                for &t in toks.iter().chain(core::iter::once(&-1)) {
                    h = (h ^ u64::from(t.cast_unsigned())).wrapping_mul(0x100_0000_01b3);
                }
            }
            self.tracker.macro_args(self.warning_index, h);
        }
        // §390: feed the macro body and its parameters to the scanner.
        while self.state() == TOKEN_LIST
            && self.cur_input.loc == NULL
            && self.token_type() != V_TEMPLATE
        {
            self.end_token_list()?; // conserve stack space
        }
        if self.memo.enabled && self.memo.top && self.memo_call(&ref_count, &pstack[..n])? {
            return Ok(()); // a recorded call replayed
        }
        let loc = if crate::input::ux(r + 1) < ref_count.len() {
            r + 1
        } else {
            NULL
        };
        self.begin_token_list(ref_count, MACRO)?;
        self.cur_input.name = self.warning_index;
        self.cur_input.loc = loc;
        if n > 0 {
            let n = i32::try_from(n).unwrap_or(0);
            // (the size tested apart from the statistic, which a dropped
            // run leaves at its deepest)
            if self.param_ptr + n > self.max_param_stack
                || self.param_ptr + n > self.params.param_size
            {
                self.max_param_stack = self.max_param_stack.max(self.param_ptr + n);
                if self.param_ptr + n > self.params.param_size {
                    return self.overflow(b"parameter stack size", self.params.param_size);
                }
            }
            for m in 0..n {
                let i = crate::input::ux(self.param_ptr + m);
                self.param_stack[i] = pstack[crate::input::ux(m)].take();
                if T::VALUES {
                    self.input_values.params.forget(i);
                }
            }
            self.param_ptr += n;
        }
        Ok(())
    }

    /// §396: report a runaway argument and abort.
    fn runaway_argument(
        &mut self,
        pstack: &mut [Option<crate::tok::Tokens>],
        n: usize,
        unbalance: i32,
    ) -> Result<(), Jump> {
        if self.long_state == CALL {
            self.runaway();
            self.print_err(b"Paragraph ended before ");
            self.sprint_cs(self.warning_index);
            self.print_str(b" was complete");
            self.help(&[
                b"I suspect you've forgotten a `}', causing me to apply this",
                b"control sequence to too much text. How can we recover?",
                b"My plan is to forget the whole thing and hope for the best.",
            ]);
            self.back_error()?;
        }
        self.arg_list.clear();
        self.arg_active = false;
        self.set_align_state(self.align_state() - unbalance);
        for q in pstack.iter_mut().take(n + 1) {
            if let Some(l) = q.take() {
                self.release_list(l);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::input::AlphaFile;
    use crate::mem::NULL;
    use crate::testing::{engine, term_output};
    use crate::web::*;

    /// Oracle: `\def\a#1#2.{<#2|#1>}\tracingmacros=2 \tracingonline=1
    /// \message{\a x{yz} w.q\a{{p}}{r}.}` in INITEX.
    #[test]
    fn macro_call_matches_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        for (c, cat) in [(b'{', LEFT_BRACE), (b'}', RIGHT_BRACE), (b'#', MAC_PARAM)] {
            t.set_equiv(CAT_CODE_BASE + i32::from(c), cat);
        }
        // \a: `#1#2.-><#2|#1>`
        let mut rc = alloc::vec::Vec::new();
        let other = |c: u8| OTHER_CHAR * MAX_CHAR_VAL + i32::from(c);
        for tok in [
            MATCH * MAX_CHAR_VAL + 35,
            MATCH * MAX_CHAR_VAL + 35,
            other(b'.'),
            END_MATCH * MAX_CHAR_VAL,
            other(b'<'),
            OUT_PARAM * MAX_CHAR_VAL + 2,
            other(b'|'),
            OUT_PARAM * MAX_CHAR_VAL + 1,
            other(b'>'),
        ] {
            rc.push(tok);
        }
        // One-letter names live at `single_base`, not in the hash.
        let a = SINGLE_BASE + i32::from(b'a');
        let rc = t.make_list(rc);
        let mut w = t.peek_eqtb(a);
        w.set_b0(CALL);
        w.set_rh(0);
        t.set_eqtb_entry(a, w, Some(crate::objs::Obj::Toks(rc)));
        t.set_int_par(TRACING_MACROS_CODE, 2);
        t.set_int_par(TRACING_ONLINE_CODE, 1);

        t.first = 1;
        t.begin_file_reading().unwrap();
        t.input_file[t.in_open] = Some(AlphaFile {
            data: alloc::sync::Arc::from(&b"\\a x{yz} w.q\\a{{p}}{r}.}\n"[..]),
            ..Default::default()
        });
        t.cur_input.name = 20;
        t.cur_input.state = NEW_LINE;
        t.cur_input.loc = 1;
        t.cur_input.limit = 0;

        let out = term_output(&mut t, |t| {
            let mut head = alloc::vec::Vec::new();
            let mut depth = 0;
            loop {
                t.get_x_token().unwrap();
                if t.cur_cmd == LEFT_BRACE {
                    depth += 1;
                } else if t.cur_cmd == RIGHT_BRACE {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                head.push(t.cur_tok);
            }
            t.show_token_list(&head, NULL, 1000);
        });
        assert_eq!(
            core::str::from_utf8(&out).unwrap(),
            "\n\\a #1#2.-><#2|#1>\n#1<-x\n#2<-{yz} w\n\n\
             \\a #1#2.-><#2|#1>\n#1<-{p}\n#2<-r\n\
             <{yz} w|x>q<r|{p}>"
        );
    }
}
