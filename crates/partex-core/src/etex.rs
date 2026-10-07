//! pdfTeX part 53a: The extended features of e-TeX (pdfTeX §1648–§1866).

use crate::error::{SPOTLESS, WARNING_ISSUED};
use crate::host::Host;
use crate::input::ux;
use crate::params::Flavor;
use crate::print::NEW_STRING;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use alloc::vec::Vec;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §1336: call `primitive` for each primitive of the flavor (INITEX).
    pub(crate) fn init_prim(&mut self) -> Result<(), Jump> {
        match self.params.flavor {
            Flavor::Tex => self.init_prim_tex(),
            Flavor::PdfTex => self.init_prim_pdftex(),
            Flavor::XeTeX => self.init_prim_xetex(),
        }
    }

    /// pdfTeX §1648: enable e-TeX, if requested (`-etex` or a `*` first on
    /// the command line, in INITEX without a format); whether extended
    /// mode was just entered (then no format is loaded).
    pub(crate) fn enable_etex_if_requested(&mut self) -> Result<bool, Jump> {
        if self.params.flavor == Flavor::Tex || !self.params.ini {
            return Ok(false);
        }
        let loc = crate::input::ux(self.cur_input.loc);
        let star = self.buffer[loc] == u32::from(b'*');
        if !(self.params.etex || star) {
            return Ok(false);
        }
        if self.params.flavor == Flavor::XeTeX {
            self.generate_etex_prims_xetex()?;
        } else {
            self.generate_etex_prims()?;
        }
        if star {
            self.cur_input.loc += 1;
        }
        self.etex_mode = true; // enter extended mode
        self.init_etex_extended();
        Ok(true)
    }

    /// pdfTeX §1812: initialize variables for e-TeX compatibility mode.
    pub(crate) fn init_etex_compat(&mut self) {
        self.max_reg_num = 255;
        self.max_reg_help_line = b"A register number must be between 0 and 255.";
    }

    /// pdfTeX §1813: initialize variables for e-TeX extended mode.
    pub(crate) fn init_etex_extended(&mut self) {
        self.max_reg_num = 32767;
        self.max_reg_help_line = b"A register number must be between 0 and 32767.";
    }

    /// pdfTeX §1652: `eTeX_ex`, is this extended mode?
    #[inline]
    pub(crate) fn etex_ex(&self) -> bool {
        self.etex_mode
    }

    /// pdfTeX §1700: `TeXXeT_en`.
    pub(crate) fn texxet_en(&self) -> bool {
        self.eqtb_int(ETEX_STATE_BASE + TEXXET_CODE) > 0
    }

    /// pdfTeX §1679: `eTeX_enabled`: complain if an optional feature is
    /// off.
    pub(crate) fn etex_enabled(&mut self, b: bool, j: i32, k: i32) -> Result<bool, Jump> {
        if !b {
            self.print_err(b"Improper ");
            self.print_cmd_chr(j, k);
            self.help(&[b"Sorry, this optional e-TeX feature has been disabled."]);
            self.error()?;
        }
        Ok(b)
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// e-TeX: print the current level of grouping and its kind; `e`: the
    /// group is ending ("entered at line").
    pub(crate) fn print_group(&mut self, e: bool) {
        let g = self.cur_group();
        let name: &[u8] = match g {
            BOTTOM_LEVEL => {
                self.print_str(b"bottom level");
                return;
            }
            SIMPLE_GROUP => b"simple",
            SEMI_SIMPLE_GROUP => b"semi simple",
            HBOX_GROUP => b"hbox",
            ADJUSTED_HBOX_GROUP => b"adjusted hbox",
            VBOX_GROUP => b"vbox",
            VTOP_GROUP => b"vtop",
            ALIGN_GROUP => b"align",
            NO_ALIGN_GROUP => b"no align",
            OUTPUT_GROUP => b"output",
            DISC_GROUP => b"disc",
            INSERT_GROUP => b"insert",
            VCENTER_GROUP => b"vcenter",
            MATH_GROUP => b"math",
            MATH_CHOICE_GROUP => b"math choice",
            MATH_SHIFT_GROUP => b"math shift",
            _ => b"math left",
        };
        self.print_str(name);
        self.print_str(b" group (level ");
        self.print_int(self.cur_level());
        self.print_char(b')');
        let line = self.saved(-1);
        if line != 0 {
            self.print_str(if e {
                b" entered at line "
            } else {
                b" at line "
            });
            self.print_int(line);
        }
    }

    /// e-TeX: `\tracinggroups` output as a group begins or (`e`) ends.
    pub(crate) fn group_trace(&mut self, e: bool) {
        self.begin_diagnostic();
        self.print_char(b'{');
        self.print_str(if e { b"leaving " } else { b"entering " });
        self.print_group(e);
        self.print_char(b'}');
        self.end_diagnostic(false);
    }

    /// e-TeX: whether input file `i`'s ending anomaly is to be reported:
    /// with `\tracingnesting`, if the innermost input level reading a line
    /// of file `i` or an outer one is a real file.
    fn nesting_reported(&self, i: usize) -> bool {
        if self.int_par(TRACING_NESTING_CODE) <= 0 {
            return false;
        }
        let rec = |k: usize| {
            if k == self.input_ptr {
                self.cur_input.clone()
            } else {
                self.input_stack[k].clone()
            }
        };
        let mut k = self.input_ptr;
        while rec(k).state == TOKEN_LIST || ux(rec(k).index) > i {
            k -= 1;
        }
        rec(k).name > 17
    }

    /// e-TeX: a group ends that began in another input file.
    pub(crate) fn group_warning(&mut self) {
        let mut i = self.in_open;
        let mut w = false;
        while self.grp_stack[i] == self.cur_boundary() && i > 0 {
            w |= self.nesting_reported(i);
            self.grp_stack[i] = self.save_index(self.save_ptr());
            if T::VALUES && i < self.in_open {
                self.input_values.files = None;
            }
            i -= 1;
        }
        if w {
            self.print_nl(b"Warning: end of ");
            self.print_group(true);
            self.print_str(b" of a different file");
            self.print_ln();
            if self.int_par(TRACING_NESTING_CODE) > 1 {
                self.show_context();
            }
            if self.history() == SPOTLESS {
                self.set_history(WARNING_ISSUED);
            }
        }
    }

    /// e-TeX: " entered on line `l`" (if known).
    pub(crate) fn print_if_line(&mut self, l: i32) {
        if l != 0 {
            self.print_str(b" entered on line ");
            self.print_line_no(usize::MAX, l);
        }
    }

    /// e-TeX: a conditional ends that began in another input file.
    pub(crate) fn if_warning(&mut self) {
        self.cond_read();
        let depth = self.cond_stack.len();
        let mut i = self.in_open;
        let mut w = false;
        while self.if_stack[i] == depth {
            w |= self.nesting_reported(i);
            self.if_stack[i] = depth - 1;
            if T::VALUES && i < self.in_open {
                self.input_values.files = None;
            }
            i -= 1;
        }
        if w {
            self.print_nl(b"Warning: end of ");
            self.print_cmd_chr(IF_TEST, self.cur_if);
            self.print_if_line(self.if_line);
            self.print_str(b" of a different file");
            self.print_ln();
            if self.int_par(TRACING_NESTING_CODE) > 1 {
                self.show_context();
            }
            if self.history() == SPOTLESS {
                self.set_history(WARNING_ISSUED);
            }
        }
    }

    /// e-TeX: a file ends with groups or conditionals begun in it still
    /// incomplete.
    pub(crate) fn file_warning(&mut self) {
        let (p, l, c) = (self.save_ptr(), self.cur_level(), self.cur_group());
        self.set_save_ptr(self.cur_boundary());
        while self.grp_stack[self.in_open] != self.save_ptr() {
            self.set_cur_level(self.cur_level() - 1);
            self.print_nl(b"Warning: end of file when ");
            self.print_group(true);
            self.print_str(b" is incomplete");
            self.set_cur_group(self.save_level(self.save_ptr()));
            self.set_save_ptr(self.save_index(self.save_ptr()));
        }
        self.set_save_ptr(p);
        self.set_cur_level(l);
        self.set_cur_group(c);
        self.cond_read();
        let (cur_if, if_limit, if_line) = (self.cur_if, self.if_limit, self.if_line);
        let mut depth = self.cond_stack.len();
        while self.if_stack[self.in_open] != depth {
            self.print_nl(b"Warning: end of file when ");
            self.print_cmd_chr(IF_TEST, self.cur_if);
            if self.if_limit == FI_CODE {
                self.print_esc(b"else");
            }
            self.print_if_line(self.if_line);
            self.print_str(b" is incomplete");
            depth -= 1;
            let r = self.cond_stack[depth];
            (self.if_line, self.cur_if, self.if_limit) = (r.line, r.cur_if, r.limit);
        }
        (self.cur_if, self.if_limit, self.if_line) = (cur_if, if_limit, if_line);
        self.print_ln();
        if self.int_par(TRACING_NESTING_CODE) > 1 {
            self.show_context();
        }
        if self.history() == SPOTLESS {
            self.set_history(WARNING_ISSUED);
        }
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// e-TeX: scan a balanced text (after its compulsory left brace) as
    /// `\toks` would, without expansion; its tokens. While scanning, the
    /// tokens so far are `def_ref`'s (for a runaway message).
    pub(crate) fn scan_general_text(&mut self) -> Result<Vec<i32>, Jump> {
        let (s, w) = (self.scanner_status, self.warning_index);
        let d = core::mem::take(&mut self.def_ref);
        self.scanner_status = ABSORBING;
        self.warning_index = self.cur_cs;
        self.scan_left_brace()?; // remove the compulsory left brace
        let mut unbalance = 1;
        loop {
            self.get_token()?;
            if self.cur_tok < RIGHT_BRACE_LIMIT {
                if self.cur_cmd < RIGHT_BRACE {
                    unbalance += 1;
                } else {
                    unbalance -= 1;
                    if unbalance == 0 {
                        break;
                    }
                }
            }
            let t = self.take_raw_tok();
            self.def_ref.push(t);
        }
        let toks = core::mem::replace(&mut self.def_ref, d);
        (self.scanner_status, self.warning_index) = (s, w);
        Ok(toks)
    }

    /// The characters that showing `toks` prints (as `token_show`).
    pub(crate) fn tokens_text(&mut self, toks: &[i32]) -> Vec<u8> {
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        let start = self.str_start[self.str_ptr];
        let pool_ptr = self.pool_ptr;
        self.token_show(toks);
        self.set_selector(old_setting);
        let text = self.str_pool[pool_ptr..self.pool_ptr].to_vec();
        self.pool_ptr = pool_ptr.max(start);
        text
    }

    /// e-TeX: `\scantokens`: read a balanced text as a file (a pseudo
    /// file), its lines separated by `\newlinechar`.
    pub(crate) fn pseudo_start(&mut self) -> Result<(), Jump> {
        let toks = self.scan_general_text()?;
        let text = self.tokens_text(&toks);
        let nl = self.new_line_char();
        let mut lines = Vec::new();
        let mut rest = &text[..];
        while !rest.is_empty() {
            let end = rest
                .iter()
                .position(|&c| i32::from(c) == nl)
                .unwrap_or(rest.len());
            lines.push(rest[..end].to_vec());
            rest = rest.get(end + 1..).unwrap_or(&[]);
        }
        self.pseudo_files.push(crate::input::PseudoFile {
            lines: lines.into(),
            next: 0,
        });
        // initiate input from the new pseudo file
        self.begin_file_reading()?;
        self.line = 0;
        self.cur_input.limit = self.cur_input.start;
        self.cur_input.loc = self.cur_input.limit + 1; // force line read
        if self.int_par(TRACING_SCAN_TOKENS_CODE) > 0 {
            self.print_sep(self.params.max_print_line - 3);
            self.cur_input.name = 19;
            self.print_str(b"( ");
            self.set_open_parens(self.open_parens() + 1);
            self.update_terminal();
        } else {
            self.cur_input.name = 18;
        }
        Ok(())
    }

    /// e-TeX: read the next line of the current pseudo file into the
    /// buffer (at `first`); `false` at its end.
    pub(crate) fn pseudo_input(&mut self) -> Result<bool, Jump> {
        self.last = self.first;
        let Some(p) = self.pseudo_files.last_mut() else {
            return Ok(false);
        };
        if p.next >= p.lines.len() {
            return Ok(false);
        }
        let lines = alloc::sync::Arc::clone(&p.lines);
        let line = &lines[p.next];
        p.next += 1;
        // (e-TeX keeps lines in words of four characters)
        let words = ((line.len() + 7) / 4).max(2);
        if 4 * words - 3 >= ux(self.params.buf_size) - self.last {
            self.cur_input.loc = i32::try_from(self.first).unwrap_or(0);
            self.cur_input.limit = i32::try_from(self.last).unwrap_or(0) - 1;
            return self
                .overflow(b"buffer size", self.params.buf_size)
                .map(|()| false);
        }
        // (`XeTeX`: the line's characters, from its pool units)
        let chars: alloc::vec::Vec<u32> = if self.unicode {
            crate::strings::decode_chars(line).collect()
        } else {
            line.iter().map(|&b| u32::from(b)).collect()
        };
        let mut last = self.first + chars.len();
        self.buffer[self.first..last].copy_from_slice(&chars);
        let padded = self.first + 4 * (words - 1);
        if padded >= self.max_buf_stack {
            self.max_buf_stack = padded + 1;
        }
        while last > self.first && self.buffer[last - 1] == u32::from(b' ') {
            last -= 1;
        }
        self.last = last;
        Ok(true)
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// The mode of nest level `p` (`nest_ptr` is the current list).
    fn nest_mode(&self, p: usize) -> i32 {
        self.level_mode(p)
    }

    /// e-TeX: `\showgroups`: the groups being built, innermost first,
    /// with what began each.
    pub(crate) fn show_save_groups(&mut self) {
        /// What follows a group's description.
        enum Then {
            Context(&'static [u8]),
            Packaging(&'static [u8]),
            Brace,
            Close,
        }

        let mut p = self.nest_ptr();
        let (v, l, c) = (self.save_ptr(), self.cur_level(), self.cur_group());
        self.set_save_ptr(self.cur_boundary());
        self.set_cur_level(self.cur_level() - 1);
        let mut a: i32 = 1;
        self.print_nl(b"");
        self.print_ln();
        loop {
            self.print_nl(b"### ");
            self.print_group(true);
            if self.cur_group() == BOTTOM_LEVEL {
                break;
            }
            let mut m;
            loop {
                m = self.nest_mode(p);
                if p > 0 {
                    p -= 1;
                } else {
                    m = VMODE;
                }
                if m != HMODE {
                    break;
                }
            }
            self.print_str(b" (");
            let then = match self.cur_group() {
                SIMPLE_GROUP => {
                    p += 1;
                    Then::Brace
                }
                HBOX_GROUP | ADJUSTED_HBOX_GROUP => Then::Context(b"hbox"),
                VBOX_GROUP => Then::Context(b"vbox"),
                VTOP_GROUP => Then::Context(b"vtop"),
                ALIGN_GROUP => {
                    if a == 0 {
                        a = 1;
                        Then::Packaging(if m == -VMODE { b"halign" } else { b"valign" })
                    } else {
                        if a == 1 {
                            self.print_str(b"align entry");
                        } else {
                            self.print_esc(b"cr");
                        }
                        let pi = i32::try_from(p).unwrap_or(i32::MAX);
                        if pi >= a {
                            p = ux(pi - a);
                        }
                        a = 0;
                        Then::Close
                    }
                }
                NO_ALIGN_GROUP => {
                    p += 1;
                    a = -1;
                    self.print_esc(b"noalign");
                    Then::Brace
                }
                OUTPUT_GROUP => {
                    self.print_esc(b"output");
                    Then::Close
                }
                MATH_GROUP => Then::Brace,
                DISC_GROUP | MATH_CHOICE_GROUP => {
                    self.print_esc(if self.cur_group() == DISC_GROUP {
                        b"discretionary"
                    } else {
                        b"mathchoice"
                    });
                    for i in 1..=3 {
                        if i <= self.saved(-2) {
                            self.print_str(b"{}");
                        }
                    }
                    Then::Brace
                }
                INSERT_GROUP => {
                    if self.saved(-2) == 255 {
                        self.print_esc(b"vadjust");
                    } else {
                        self.print_esc(b"insert");
                        self.print_int(self.saved(-2));
                    }
                    Then::Brace
                }
                VCENTER_GROUP => Then::Packaging(b"vcenter"),
                SEMI_SIMPLE_GROUP => {
                    p += 1;
                    self.print_esc(b"begingroup");
                    Then::Close
                }
                MATH_SHIFT_GROUP => {
                    if m == MMODE {
                        self.print_char(b'$');
                    } else if self.nest_mode(p) == MMODE {
                        self.print_cmd_chr(EQ_NO, self.saved(-2));
                        self.print_char(b')');
                        self.close_group();
                        continue;
                    }
                    self.print_char(b'$');
                    Then::Close
                }
                _ => {
                    // math left group
                    self.print_esc(if self.nest_is_middle(p + 1) {
                        b"middle"
                    } else {
                        b"left"
                    });
                    Then::Close
                }
            };
            let then = match then {
                Then::Context(s) => {
                    // show the box context
                    let mut i = self.saved(-4);
                    if i != 0 {
                        if i < BOX_FLAG {
                            let j = if self.nest_mode(p).abs() == VMODE {
                                HMOVE
                            } else {
                                VMOVE
                            };
                            self.print_cmd_chr(j, i32::from(i <= 0));
                            self.print_scaled(i.abs());
                            self.print_str(b"pt");
                        } else if i < SHIP_OUT_FLAG {
                            if i >= GLOBAL_BOX_FLAG {
                                self.print_esc(b"global");
                                i -= GLOBAL_BOX_FLAG - BOX_FLAG;
                            }
                            self.print_esc(b"setbox");
                            self.print_int(i - BOX_FLAG);
                            self.print_char(b'=');
                        } else {
                            self.print_cmd_chr(LEADER_SHIP, i - (LEADER_FLAG - A_LEADERS));
                        }
                    }
                    Then::Packaging(s)
                }
                t => t,
            };
            if let Then::Packaging(s) = then {
                self.print_esc(s);
                // show the box packaging info
                if self.saved(-2) != 0 {
                    self.print_char(b' ');
                    self.print_str(if self.saved(-3) == EXACTLY {
                        b"to"
                    } else {
                        b"spread"
                    });
                    self.print_scaled(self.saved(-2));
                    self.print_str(b"pt");
                }
            }
            if !matches!(then, Then::Close) {
                self.print_char(b'{');
            }
            self.print_char(b')');
            self.close_group();
        }
        self.set_save_ptr(v);
        self.set_cur_level(l);
        self.set_cur_group(c);
    }

    /// `show_save_groups`: go on to the enclosing group.
    fn close_group(&mut self) {
        self.set_cur_level(self.cur_level() - 1);
        self.set_cur_group(self.save_level(self.save_ptr()));
        self.set_save_ptr(self.save_index(self.save_ptr()));
    }

    /// Whether the math list at nest level `p` was begun by `\middle`
    /// (else by `\left`).
    fn nest_is_middle(&self, p: usize) -> bool {
        self.level_middle(p)
    }

    /// e-TeX: `\showifs`: the conditionals being processed, innermost
    /// first.
    pub(crate) fn show_ifs(&mut self) {
        self.print_nl(b"");
        self.print_ln();
        self.cond_read();
        let mut n = self.cond_stack.len();
        if n == 0 {
            self.print_nl(b"### ");
            self.print_str(b"no active conditionals");
            return;
        }
        let (mut t, mut l, mut m) = (self.cur_if, self.if_line, self.if_limit);
        while n > 0 {
            self.print_nl(b"### level ");
            self.print_int(i32::try_from(n).unwrap_or(i32::MAX));
            self.print_str(b": ");
            self.print_cmd_chr(IF_TEST, t);
            if m == FI_CODE {
                self.print_esc(b"else");
            }
            self.print_if_line(l);
            n -= 1;
            let r = self.cond_stack[n];
            (t, l, m) = (r.cur_if, r.line, r.limit);
        }
    }
}
