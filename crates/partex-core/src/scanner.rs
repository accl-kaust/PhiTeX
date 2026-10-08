//! Part 24: Getting the next token (§332–§365).
//!
//! encTeX's hooks in `get_next` (`read_buffer`'s `\mubyte` conversion and
//! `write_noexpanding`) are inactive without encTeX: `read_buffer(k)` is
//! `buffer[k]` and `mubyte_token` is zero.

use crate::host::Host;
use crate::input::ux;
use crate::mem::NULL;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// §358: a special variant of `relax` (`XeTeX`'s `special_char`, past
/// every character, DESIGN 4.7).
pub use crate::web::NO_EXPAND_FLAG;

/// §352: `is_hex(c)`.
fn is_hex(c: u32) -> bool {
    (0x30..=0x39).contains(&c) || (0x61..=0x66).contains(&c)
}

/// §352: the value of hex digit `x`.
fn hex(x: u32) -> i32 {
    crate::input::ci(if x <= 0x39 { x - 0x30 } else { x - 0x61 + 10 })
}

/// §352: `hex_to_cur_chr`.
fn hex_pair(c: u32, cc: u32) -> i32 {
    16 * hex(c) + hex(cc)
}

/// What the character scanner does next (tex.web's labels in `get_next`).
pub(crate) enum Next {
    /// `goto switch`: eat the next character.
    Switch,
    /// `goto reswitch`: digest `cur_chr` again.
    Reswitch,
    /// `goto restart`: start over.
    Restart,
    /// A token is ready (possibly after the alignment check).
    Done,
    /// `return` without the alignment check (a `\read` line ended).
    Exit,
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §336: complain about an `\outer` control sequence or end of file
    /// in the wrong place.
    pub(crate) fn check_outer_validity(&mut self) -> Result<(), Jump> {
        if self.scanner_status == NORMAL {
            return Ok(());
        }
        self.deletions_allowed = false;
        // §337: back up an outer control sequence so that it can be reread.
        if self.cur_cs != 0 {
            if self.state() == TOKEN_LIST || self.cur_input.name < 1 || self.cur_input.name > 17 {
                let p = self.tok_from(&[CS_TOKEN_FLAG + self.cur_cs]);
                self.back_list(p)?; // prepare to read the control sequence again
            }
            self.cur_cmd = SPACER;
            self.cur_chr = i32::from(b' '); // replace it by a space
        }
        if self.scanner_status > SKIPPING {
            // §338: tell the user what has run away and try to recover.
            self.runaway(); // print a definition, argument, or preamble
            if self.cur_cs == 0 {
                self.print_err(b"File ended");
            } else {
                self.cur_cs = 0;
                self.print_err(b"Forbidden control sequence found");
            }
            // §339: print either `definition` or `use` or `preamble` or
            // `text`, and insert tokens that should lead to recovery.
            let p = match self.scanner_status {
                DEFINING => {
                    self.print_str(b" while scanning definition");
                    self.tok_from(&[RIGHT_BRACE_TOKEN + i32::from(b'}')])
                }
                MATCHING => {
                    self.print_str(b" while scanning use");
                    self.long_state = OUTER_CALL;
                    self.tok_from(&[self.par_token])
                }
                ALIGNING => {
                    self.print_str(b" while scanning preamble");
                    self.set_align_state(-1_000_000);
                    self.tok_from(&[
                        CS_TOKEN_FLAG + FROZEN_CR,
                        RIGHT_BRACE_TOKEN + i32::from(b'}'),
                    ])
                }
                _ => {
                    // absorbing
                    self.print_str(b" while scanning text");
                    self.tok_from(&[RIGHT_BRACE_TOKEN + i32::from(b'}')])
                }
            };
            self.ins_list(p)?;
            self.print_str(b" of ");
            self.sprint_cs(self.warning_index);
            self.help(&[
                b"I suspect you have forgotten a `}', causing me",
                b"to read past where you wanted me to stop.",
                b"I'll try to recover; but if the error is serious,",
                b"you'd better type `E' or `X' now and fix your file.",
            ]);
            self.error()?;
        } else {
            self.cond_read();
            self.print_err(b"Incomplete ");
            self.print_cmd_chr(IF_TEST, self.cur_if);
            self.print_str(b"; all text was ignored after line ");
            self.print_line_no(usize::MAX, self.skip_line);
            self.help(&[
                b"A forbidden control sequence occurred in skipped text.",
                b"This kind of error happens when you say `\\if...' and forget",
                b"the matching `\\fi'. I've inserted a `\\fi'; this might work.",
            ]);
            if self.cur_cs != 0 {
                self.cur_cs = 0;
            } else {
                self.help_line[2] = b"The file ended while I was skipping conditional text.";
            }
            self.cur_tok = CS_TOKEN_FLAG + FROZEN_FI;
            self.ins_error()?;
        }
        self.deletions_allowed = true;
        Ok(())
    }

    /// §341: set `cur_cmd`, `cur_chr`, `cur_cs` to the next token.
    ///
    /// The common case is inlined: a token from a token list that needs
    /// none of §357's special actions (a brace, a parameter, an alignment
    /// entry's end, an `\outer` or `\noexpand`ed control sequence), with
    /// no memo recording to tell. Anything else takes the full path.
    #[inline(always)]
    #[allow(clippy::inline_always, reason = "measured: 6% of the PGF subset")]
    pub(crate) fn get_next(&mut self) -> Result<(), Jump> {
        if self.cur_raw != 0 {
            // (the tagged token read before was used as it was read: an
            // observation, unless it was stored or backed up; `reloc.rs`)
            self.cur_raw = 0;
            if self.tag_pending != 0 {
                self.flush_tag();
            }
        }
        if self.cur_input.state == TOKEN_LIST
            && self.cur_input.loc != NULL
            && !self.memo.recording()
        {
            let (t, next) = self.cur_tok_and_next();
            if t >= CS_TOKEN_FLAG {
                let cs = t - CS_TOKEN_FLAG;
                let w = self.lookup_meaning(cs);
                let cmd = w.b0();
                if cmd < OUTER_CALL && (cmd > CAR_RET || cmd < TAB_MARK || self.align_state() != 0)
                {
                    self.cur_input.loc = next;
                    self.cur_cs = cs;
                    self.cur_cmd = cmd;
                    self.cur_chr = w.rh();
                    self.pure_tok();
                    return Ok(());
                }
            } else {
                let cmd = tok_cmd(t);
                // (`CAR_RET` is `OUT_PARAM` too: a parameter, §359)
                if !matches!(cmd, TAB_MARK | CAR_RET) {
                    if cmd == LEFT_BRACE {
                        self.set_align_state(self.align_state() + 1);
                    } else if cmd == RIGHT_BRACE {
                        self.set_align_state(self.align_state() - 1);
                    }
                    self.cur_input.loc = next;
                    self.cur_cs = 0;
                    self.cur_cmd = cmd;
                    self.cur_chr = tok_chr(t);
                    if is_tagged(t) {
                        self.read_tagged(t);
                    }
                    self.pure_tok();
                    return Ok(());
                }
            }
        }
        self.get_next_slow()
    }

    /// (With [`Tracker::PURE`].) The token just read from a token list.
    #[inline]
    fn pure_tok(&self) {
        if T::PURE {
            self.tracker.pure_tok(
                self.input_ptr,
                u16::try_from(self.cur_cmd).unwrap_or(u16::MAX),
                self.cur_chr,
                self.cur_cs,
            );
        }
    }

    /// §341 in full (see [`Self::get_next`]).
    #[inline(never)]
    fn get_next_slow(&mut self) -> Result<(), Jump> {
        'restart: loop {
            self.cur_cs = 0;
            if self.state() != TOKEN_LIST {
                // §343: input from external file.
                self.memo.file_read(self.input_ptr);
                match self.next_from_file()? {
                    Next::Restart => continue 'restart,
                    Next::Exit => return Ok(()),
                    _ => {}
                }
            } else if self.cur_input.loc != NULL {
                // §357: input from token list (list not exhausted).
                // (one lookup of the list for the token and the next `loc`)
                let (t, next) = self.cur_tok_and_next();
                self.cur_input.loc = next; // move to next
                if tok_cmd(t) != OUT_PARAM {
                    self.memo.fetched(self.input_ptr, t, self.cur_level());
                }
                if t >= CS_TOKEN_FLAG {
                    // a control sequence token
                    self.cur_cs = t - CS_TOKEN_FLAG;
                    let w = self.lookup_meaning(self.cur_cs);
                    self.cur_cmd = w.b0();
                    self.cur_chr = w.rh();
                    if self.cur_cmd >= OUTER_CALL {
                        if self.cur_cmd == DONT_EXPAND {
                            // §358: get the next token, suppressing expansion.
                            let t = self.cur_tok_at(self.cur_input.loc);
                            self.memo.fetched(self.input_ptr, t, self.cur_level());
                            self.cur_cs = t - CS_TOKEN_FLAG;
                            self.cur_input.loc = NULL;
                            self.cur_cmd = self.eq_type(self.cur_cs);
                            self.cur_chr = self.equiv(self.cur_cs);
                            if self.cur_cmd > MAX_COMMAND {
                                self.cur_cmd = RELAX;
                                self.cur_chr = NO_EXPAND_FLAG;
                            }
                        } else {
                            self.check_outer_validity()?;
                        }
                    }
                } else {
                    self.cur_cmd = tok_cmd(t);
                    self.cur_chr = tok_chr(t);
                    if is_tagged(t) {
                        self.read_tagged(t);
                    }
                    match self.cur_cmd {
                        LEFT_BRACE => self.set_align_state(self.align_state() + 1),
                        RIGHT_BRACE => self.set_align_state(self.align_state() - 1),
                        OUT_PARAM => {
                            // §359: insert macro parameter and restart.
                            // (read in place: `InStateRecord::holds_param`)
                            let k = self.cur_input.limit + self.cur_chr - 1;
                            let empty = self.param_stack[ux(k)]
                                .as_ref()
                                .is_none_or(|l| l.is_empty());
                            self.push_input()?;
                            self.cur_input.state = TOKEN_LIST;
                            self.cur_input.list = None;
                            self.cur_input.start = k;
                            self.cur_input.index = PARAMETER;
                            self.cur_input.loc = if empty { NULL } else { 0 };
                            self.memo.param_pushed(self.input_ptr - 1);
                            continue 'restart;
                        }
                        _ => {}
                    }
                }
            } else {
                // we are done with this token list
                self.end_token_list()?;
                continue 'restart; // resume previous level
            }
            // §342: if an alignment entry has just ended, take appropriate
            // action.
            if self.cur_cmd <= CAR_RET && self.cur_cmd >= TAB_MARK && self.align_state() == 0 {
                // §789: insert the <v_j> template and restart.
                self.memo.impure();
                let chr = self.cur_chr;
                let aligning = self.scanner_status == ALIGNING;
                let entry = if aligning {
                    None
                } else {
                    self.edit_cur_column(|a| {
                        let cmd = a.extra_info;
                        a.extra_info = chr;
                        (cmd, a.v.clone())
                    })
                };
                let Some((cmd, v)) = entry else {
                    return self.fatal_error(b"(interwoven alignment preambles are not allowed)");
                };
                self.cur_cmd = cmd;
                if self.cur_cmd == OMIT {
                    let omit = self.omit_template();
                    self.begin_token_list(omit, V_TEMPLATE)?;
                } else {
                    self.begin_token_list(v, V_TEMPLATE)?;
                }
                self.set_align_state(1_000_000);
                continue 'restart;
            }
            if T::PURE && self.cur_input.state == TOKEN_LIST {
                self.pure_tok();
            } else if T::PURE {
                self.tracker.pure_src(
                    u16::try_from(self.cur_cmd).unwrap_or(u16::MAX),
                    self.cur_chr,
                    self.cur_cs,
                );
            }
            return Ok(());
        }
    }

    /// §343: input from external file; `Restart` if no input found.
    fn next_from_file(&mut self) -> Result<Next, Jump> {
        let mut next = Next::Switch;
        loop {
            match next {
                Next::Switch => {
                    if self.cur_input.loc <= self.cur_input.limit {
                        // current line not yet finished
                        self.cur_chr = crate::input::ci(self.buffer[ux(self.cur_input.loc)]);
                        self.cur_input.loc += 1;
                        if self.unicode {
                            // `XeTeX` §373: a surrogate pair is one character
                            let loc = ux(self.cur_input.loc);
                            if (0xD800..0xDC00).contains(&self.cur_chr)
                                && self.cur_input.loc <= self.cur_input.limit
                                && (0xDC00..0xE000).contains(&self.buffer[loc])
                            {
                                let lower = crate::input::ci(self.buffer[loc]) - 0xDC00;
                                self.cur_input.loc += 1;
                                self.cur_chr = 0x1_0000 + (self.cur_chr - 0xD800) * 1024 + lower;
                            }
                        }
                        next = Next::Reswitch;
                    } else {
                        self.cur_input.state = NEW_LINE;
                        // §360: move to next line of file, or restart if
                        // there is no next line, or return if a `\read`
                        // line has finished.
                        if let Some(n) = self.next_line()? {
                            return Ok(n);
                        }
                        self.check_interrupt()?;
                        next = Next::Switch;
                    }
                }
                Next::Reswitch => {
                    self.cur_cmd = self.cat_code(self.cur_chr);
                    next = self.change_state()?;
                }
                n => return Ok(n),
            }
        }
    }

    /// §344: change state if necessary, and `goto switch` if the current
    /// character should be ignored, or `goto reswitch` if the current
    /// character changes to another.
    fn change_state(&mut self) -> Result<Next, Jump> {
        let state = self.state();
        let cmd = self.cur_cmd;
        let any = |c: i32| cmd == c;
        // §345: cases where character is ignored.
        if any(IGNORE) || (cmd == SPACER && state != MID_LINE) {
            return Ok(Next::Switch);
        }
        match cmd {
            ESCAPE => {
                self.scan_control_sequence()?;
                return Ok(Next::Done);
            }
            ACTIVE_CHAR => {
                // §353: process an active-character control sequence.
                self.cur_cs = crate::wide::active_cs(self.cur_chr);
                let w = self.lookup_meaning(self.cur_cs);
                self.cur_cmd = w.b0();
                self.cur_chr = w.rh();
                self.cur_input.state = MID_LINE;
                if self.cur_cmd >= OUTER_CALL {
                    self.check_outer_validity()?;
                }
                return Ok(Next::Done);
            }
            SUP_MARK => {
                // §352: if this `sup_mark` starts an expanded character
                // like ^^A or ^^df, then reswitch, otherwise set
                // `state:=mid_line`.
                let loc = ux(self.cur_input.loc);
                let limit = self.cur_input.limit;
                if self.unicode {
                    if self.expanded_char_xetex() {
                        return Ok(Next::Reswitch);
                    }
                } else if self.cur_chr == crate::input::ci(self.buffer[loc])
                    && self.cur_input.loc < limit
                {
                    let c = self.buffer[loc + 1];
                    if c < 0o200 {
                        // yes we have an expanded char
                        self.cur_input.loc += 2;
                        if is_hex(c) && self.cur_input.loc <= limit {
                            let cc = self.buffer[ux(self.cur_input.loc)];
                            if is_hex(cc) {
                                self.cur_input.loc += 1;
                                self.cur_chr = hex_pair(c, cc);
                                return Ok(Next::Reswitch);
                            }
                        }
                        self.cur_chr = if c < 0o100 {
                            crate::input::ci(c) + 0o100
                        } else {
                            crate::input::ci(c) - 0o100
                        };
                        return Ok(Next::Reswitch);
                    }
                }
                self.cur_input.state = MID_LINE;
                return Ok(Next::Done);
            }
            INVALID_CHAR => {
                // §346: decry the invalid character and restart.
                self.print_err(b"Text line contains an invalid character");
                self.help(&[
                    b"A funny symbol that I can't read has just been input.",
                    b"Continue, and I'll forget that it ever happened.",
                ]);
                self.deletions_allowed = false;
                self.error()?;
                self.deletions_allowed = true;
                return Ok(Next::Restart);
            }
            _ => {}
        }
        // §347: handle situations involving spaces, braces, changes of state.
        match (state, cmd) {
            (MID_LINE, SPACER) => {
                // §349: enter `skip_blanks` state, emit a space.
                self.cur_input.state = SKIP_BLANKS;
                self.cur_chr = i32::from(b' ');
            }
            (MID_LINE, CAR_RET) => {
                // §348: finish line, emit a space.
                self.cur_input.loc = self.cur_input.limit + 1;
                self.cur_cmd = SPACER;
                self.cur_chr = i32::from(b' ');
            }
            (SKIP_BLANKS, CAR_RET) | (_, COMMENT) => {
                // §350: finish line, `goto switch`.
                self.cur_input.loc = self.cur_input.limit + 1;
                return Ok(Next::Switch);
            }
            (NEW_LINE, CAR_RET) => {
                // §351: finish line, emit a \par.
                self.cur_input.loc = self.cur_input.limit + 1;
                if T::VALUES && self.scanner_status == NORMAL {
                    // (not inside a definition, argument, preamble or
                    // skipped text; the token lists pending below, such as
                    // the rest of `\include`, are part of the boundary)
                    self.tracker.blank_line();
                }
                self.cur_cs = self.par_loc;
                let w = self.lookup_meaning(self.cur_cs);
                self.cur_cmd = w.b0();
                self.cur_chr = w.rh();
                if self.cur_cmd >= OUTER_CALL {
                    self.check_outer_validity()?;
                }
            }
            (MID_LINE, LEFT_BRACE) => self.set_align_state(self.align_state() + 1),
            (_, LEFT_BRACE) => {
                self.cur_input.state = MID_LINE;
                self.set_align_state(self.align_state() + 1);
            }
            (MID_LINE, RIGHT_BRACE) => self.set_align_state(self.align_state() - 1),
            (_, RIGHT_BRACE) => {
                self.cur_input.state = MID_LINE;
                self.set_align_state(self.align_state() - 1);
            }
            // `add_delims_to(skip_blanks)`, `add_delims_to(new_line)`
            (
                SKIP_BLANKS | NEW_LINE,
                MATH_SHIFT | TAB_MARK | MAC_PARAM | SUB_MARK | LETTER | OTHER_CHAR,
            ) => {
                self.cur_input.state = MID_LINE;
            }
            _ => {}
        }
        Ok(Next::Done)
    }

    /// §354: scan a control sequence and set `state:=skip_blanks` or
    /// `mid_line`.
    fn scan_control_sequence(&mut self) -> Result<(), Jump> {
        if self.cur_input.loc > self.cur_input.limit {
            self.cur_cs = NULL_CS; // `state` is irrelevant in this case
        } else {
            'start_cs: loop {
                let mut k = ux(self.cur_input.loc);
                self.cur_chr = crate::input::ci(self.buffer[k]);
                let mut cat = self.cat_code(self.cur_chr);
                k += 1;
                self.cur_input.state = if cat == LETTER || cat == SPACER {
                    SKIP_BLANKS
                } else {
                    MID_LINE
                };
                let limit = ux(self.cur_input.limit);
                if cat == LETTER && k <= limit {
                    // §356: scan ahead in the buffer until finding a
                    // nonletter; if an expanded code is encountered, reduce
                    // it and `goto start_cs`; otherwise if a multiletter
                    // control sequence is found, adjust `cur_cs` and `loc`,
                    // and `goto found`.
                    loop {
                        self.cur_chr = crate::input::ci(self.buffer[k]);
                        cat = self.cat_code(self.cur_chr);
                        k += 1;
                        if cat != LETTER || k > ux(self.cur_input.limit) {
                            break;
                        }
                    }
                    if self.reduce_expanded_code(k, cat) {
                        continue 'start_cs;
                    }
                    if cat != LETTER {
                        k -= 1;
                    }
                    let loc = ux(self.cur_input.loc);
                    if k > loc + 1 {
                        // multiletter control sequence has been scanned
                        self.cur_cs = self.id_lookup(loc, k - loc)?;
                        self.cur_input.loc = i32::try_from(k).unwrap_or(0);
                        break 'start_cs;
                    }
                } else if self.reduce_expanded_code(k, cat) {
                    continue 'start_cs;
                }
                let loc = ux(self.cur_input.loc);
                if self.buffer[loc] > 0xFFFF {
                    // `XeTeX` §384: a single character above 0xFFFF names a
                    // multiletter control sequence (the pool is UTF-16)
                    self.cur_cs = self.id_lookup(loc, 1)?;
                    self.cur_input.loc += 1;
                    break 'start_cs;
                }
                self.cur_cs = crate::wide::single_cs(crate::input::ci(self.buffer[loc]));
                self.cur_input.loc += 1;
                break 'start_cs;
            }
        }
        // found:
        let w = self.lookup_meaning(self.cur_cs);
        self.cur_cmd = w.b0();
        self.cur_chr = w.rh();
        if self.cur_cmd >= OUTER_CALL {
            self.check_outer_validity()?;
        }
        Ok(())
    }

    /// `XeTeX` §382: `^^` and up to four more `^` with as many hex digits
    /// (`^^^^xxxx`, `^^^^^^xxxxxx`), or `^^` and one character: whether an
    /// expanded character was read into `cur_chr` (`goto reswitch`).
    fn expanded_char_xetex(&mut self) -> bool {
        let loc = ux(self.cur_input.loc);
        let limit = ux(self.cur_input.limit);
        if self.cur_chr != crate::input::ci(self.buffer[loc]) || loc >= limit {
            return false;
        }
        // we have `^^` and another char; how many `^`s, up to 6
        let mut sup_count = 2;
        while sup_count < 6
            && loc + 2 * sup_count - 2 <= limit
            && crate::input::ci(self.buffer[loc + sup_count - 1]) == self.cur_chr
        {
            sup_count += 1;
        }
        // enough hex characters for the number of `^`s?
        for d in 1..=sup_count {
            if !is_hex(self.buffer[loc + sup_count - 2 + d]) {
                // a non-hex character: the single `^^X` form
                let c = self.buffer[loc + 1];
                if c < 0o200 {
                    self.cur_input.loc += 2;
                    self.cur_chr = if c < 0o100 {
                        crate::input::ci(c) + 0o100
                    } else {
                        crate::input::ci(c) - 0o100
                    };
                    return true;
                }
                return false;
            }
        }
        let mut v = 0;
        for d in 1..=sup_count {
            v = 16 * v + hex(self.buffer[loc + sup_count - 2 + d]);
        }
        if v > BIGGEST_USV {
            self.cur_chr = crate::input::ci(self.buffer[loc]);
            return false;
        }
        self.cur_chr = v;
        self.cur_input.loc += i32::try_from(2 * sup_count - 1).unwrap_or(0);
        true
    }

    /// `XeTeX` §385: §355 with `XeTeX`'s longer forms. `first` is not moved.
    fn reduce_expanded_code_xetex(&mut self, mut k: usize, cat: i32) -> bool {
        let limit = ux(self.cur_input.limit);
        if !(cat == SUP_MARK && crate::input::ci(self.buffer[k]) == self.cur_chr && k < limit) {
            return false;
        }
        let mut sup_count = 2;
        while sup_count < 6
            && k + 2 * sup_count - 2 <= limit
            && crate::input::ci(self.buffer[k + sup_count - 1]) == self.cur_chr
        {
            sup_count += 1;
        }
        for d in 1..=sup_count {
            if !is_hex(self.buffer[k + sup_count - 2 + d]) {
                let c = self.buffer[k + 1];
                if c < 0o200 {
                    self.buffer[k - 1] = if c < 0o100 { c + 0o100 } else { c - 0o100 };
                    let d = 2;
                    self.cur_input.limit -= 2;
                    while k <= ux(self.cur_input.limit) {
                        self.buffer[k] = self.buffer[k + d];
                        k += 1;
                    }
                    return true;
                }
                return false;
            }
        }
        let mut v = 0;
        for d in 1..=sup_count {
            v = 16 * v + hex(self.buffer[k + sup_count - 2 + d]);
        }
        if v > BIGGEST_USV {
            self.cur_chr = crate::input::ci(self.buffer[k]);
            return false;
        }
        self.cur_chr = v;
        self.buffer[k - 1] = crate::input::cu(v);
        let d = 2 * sup_count - 1;
        self.cur_input.limit -= i32::try_from(d).unwrap_or(0);
        while k <= ux(self.cur_input.limit) {
            self.buffer[k] = self.buffer[k + d];
            k += 1;
        }
        true
    }

    /// §355: if an expanded code is present at `k`, reduce it (shift the
    /// rest of the line left) and return true (`goto start_cs`).
    fn reduce_expanded_code(&mut self, mut k: usize, cat: i32) -> bool {
        if self.unicode {
            return self.reduce_expanded_code_xetex(k, cat);
        }
        let limit = ux(self.cur_input.limit);
        if crate::input::ci(self.buffer[k]) == self.cur_chr && cat == SUP_MARK && k < limit {
            let c = self.buffer[k + 1];
            if c < 0o200 {
                // yes, one is indeed present
                let mut d = 2;
                if is_hex(c) && k + 2 <= limit {
                    let cc = self.buffer[k + 2];
                    if is_hex(cc) {
                        d += 1;
                    }
                }
                self.buffer[k - 1] = if d > 2 {
                    let v = hex_pair(c, self.buffer[k + 2]);
                    self.cur_chr = v;
                    crate::input::cu(v)
                } else if c < 0o100 {
                    c + 0o100
                } else {
                    c - 0o100
                };
                self.cur_input.limit -= i32::try_from(d).unwrap_or(0);
                self.first -= d;
                while k <= ux(self.cur_input.limit) {
                    self.buffer[k] = self.buffer[k + d];
                    k += 1;
                }
                return true;
            }
        }
        false
    }

    /// §360: move to the next line of the file. `Ok(None)` continues with
    /// `goto switch`; otherwise how `get_next` goes on.
    pub(crate) fn next_line(&mut self) -> Result<Option<Next>, Jump> {
        if self.cur_input.name > 17 {
            // §362: read next line of file into `buffer`, or restart if the
            // file has ended.
            self.line += 1;
            self.first = ux(self.cur_input.start);
            if !self.force_eof {
                let index = ux(self.cur_input.index);
                let got = if self.cur_input.name <= 19 {
                    self.pseudo_input()?
                } else {
                    self.end_line_of(index);
                    let mut f = self.input_file[index].take().unwrap_or_default();
                    let from = f.pos;
                    let got = self.input_ln(&mut f);
                    match got {
                        Ok(true) => self.start_line(&mut f, from, true),
                        Ok(false) => self.end_of_file(&f, from),
                        Err(_) => {}
                    }
                    self.input_file[index] = Some(f);
                    got?
                };
                if got {
                    self.firm_up_the_line()?; // this sets `limit`
                } else if self.equiv_toks(EVERY_EOF_LOC).is_some() && !self.eof_seen[index] {
                    // e-TeX: fake one empty line, after `\everyeof`
                    self.cur_input.limit = i32::try_from(self.first).unwrap_or(0) - 1;
                    self.eof_seen[index] = true;
                    self.begin_toks_at(EVERY_EOF_LOC, EVERY_EOF_TEXT)?;
                    return Ok(Some(Next::Restart));
                } else {
                    self.force_eof = true;
                }
            }
            if self.force_eof {
                self.cond_read();
                if self.int_par(TRACING_NESTING_CODE) > 0
                    && (self.grp_stack[self.in_open] != self.cur_boundary()
                        || self.if_stack[self.in_open] != self.cond_stack.len())
                {
                    // e-TeX: groups or conditionals begun in the file are
                    // unfinished
                    self.file_warning();
                }
                if self.cur_input.name >= 19 {
                    self.print_char(b')');
                    self.set_open_parens(self.open_parens() - 1);
                    self.update_terminal(); // show user that file has been read
                }
                self.force_eof = false;
                self.end_file_reading(); // resume previous level
                self.check_outer_validity()?;
                return Ok(Some(Next::Restart));
            }
            self.end_the_line();
        } else {
            if !self.terminal_input() {
                // `\read` line has ended
                self.cur_cmd = 0;
                self.cur_chr = 0;
                return Ok(Some(Next::Exit));
            }
            if self.input_ptr > 0 {
                // text was inserted during error recovery
                self.end_file_reading();
                return Ok(Some(Next::Restart)); // resume previous level
            }
            if self.selector() < LOG_ONLY {
                self.open_log_file()?;
            }
            if self.interaction() > crate::error::NONSTOP_MODE {
                if self.end_line_char_inactive() {
                    self.cur_input.limit += 1;
                }
                if self.cur_input.limit == self.cur_input.start {
                    // previous line was empty
                    self.print_nl(b"(Please type a command or say `\\end')");
                }
                self.print_ln();
                self.first = ux(self.cur_input.start);
                self.prompt_input(b"*")?; // input on-line into `buffer`
                self.cur_input.limit = i32::try_from(self.last).unwrap_or(0);
                self.end_the_line();
            } else {
                // nonstop mode, which is intended for overnight batch
                // processing, never waits for on-line input
                return self.fatal_error(b"*** (job aborted, no legal \\end found)");
            }
        }
        Ok(None)
    }

    /// The end of §362 and §360: put `end_line_char` at the end of the
    /// line and get ready to read it.
    fn end_the_line(&mut self) {
        if self.end_line_char_inactive() {
            self.cur_input.limit -= 1;
        } else {
            let c = self.int_par(END_LINE_CHAR_CODE);
            self.buffer[ux(self.cur_input.limit)] = crate::input::cu(c);
        }
        self.first = ux(self.cur_input.limit + 1);
        self.cur_input.loc = self.cur_input.start; // ready to read
    }

    /// §360: `end_line_char_inactive`.
    pub(crate) fn end_line_char_inactive(&self) -> bool {
        let c = self.int_par(END_LINE_CHAR_CODE);
        !(0..=255).contains(&c)
    }

    /// §363: set `limit` for a new line, letting the user edit it when
    /// `\pausing` is positive.
    pub(crate) fn firm_up_the_line(&mut self) -> Result<(), Jump> {
        self.cur_input.limit = i32::try_from(self.last).unwrap_or(0);
        if self.int_par(PAUSING_CODE) > 0 && self.interaction() > crate::error::NONSTOP_MODE {
            self.print_ln();
            let mut k = ux(self.cur_input.start);
            while k < ux(self.cur_input.limit) {
                self.print_buffer(&mut k);
            }
            self.first = ux(self.cur_input.limit);
            self.prompt_input(b"=>")?; // wait for user response
            if self.last > self.first {
                let start = ux(self.cur_input.start);
                for k in self.first..self.last {
                    // move line down in buffer
                    self.buffer[k + start - self.first] = self.buffer[k];
                }
                self.cur_input.limit = i32::try_from(start + self.last - self.first).unwrap_or(0);
            }
        }
        Ok(())
    }

    /// §365: set `cur_cmd`, `cur_chr`, `cur_tok`.
    pub(crate) fn get_token(&mut self) -> Result<(), Jump> {
        self.no_new_control_sequence = false;
        let r = self.get_next();
        self.no_new_control_sequence = true;
        r?;
        self.cur_tok = if self.cur_cs == 0 {
            self.cur_cmd * MAX_CHAR_VAL + self.cur_chr
        } else {
            CS_TOKEN_FLAG + self.cur_cs
        };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::input::AlphaFile;
    use crate::mem::NULL;
    use crate::testing::{engine, term_output};
    use crate::web::*;

    /// Oracle: in INITEX with `{`, `}`, `^` given catcodes 1, 2, 7,
    /// `\def\x{<these lines>}\show\x` shows the body below: `^^` codes,
    /// skipped blanks, comments, `^^M` ending a line early, empty lines
    /// making `\par`, and `^^` at the end of a line (then `^^M` is `M`).
    #[test]
    fn tokenizes_like_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        for (c, cat) in [(b'{', LEFT_BRACE), (b'}', RIGHT_BRACE), (b'^', SUP_MARK)] {
            t.set_equiv(CAT_CODE_BASE + i32::from(c), cat);
        }
        let src = b"a ^^41^^5a  b^^zc\\relax\\foo^^6a^^6bx \\x^^5eb {!} \\  \\^^M%comment\n  \
                    q^^4d^^M  lost\n    \\par\n\n  z^^7e^^\n \\^^5c\\ab%\n y}\n";
        t.first = 1;
        t.begin_file_reading().unwrap();
        t.input_file[t.in_open] = Some(AlphaFile {
            data: alloc::sync::Arc::from(&src[..]),
            ..Default::default()
        });
        t.cur_input.name = 20;
        t.cur_input.state = NEW_LINE;
        t.cur_input.loc = 1;
        t.cur_input.limit = 0;

        // Read up to the matching closing brace into a token list.
        let mut head = alloc::vec::Vec::new();
        let mut depth = 0;
        loop {
            t.get_token().unwrap();
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
        let out = term_output(&mut t, |t| t.show_token_list(&head, NULL, 1000));
        assert_eq!(
            core::str::from_utf8(&out).unwrap(),
            "a AZ b:c\\relax \\foojkx \\x ^b {!} \\ \\^^MqM \\par \\par z~M\\\\\\ab y"
        );
        assert_eq!(t.line, 7);
    }
}
