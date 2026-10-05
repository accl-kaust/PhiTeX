//! Part 27: Building token lists (§464–§486).

use crate::host::Host;
use crate::input::ux;
use crate::mem::Pointer;
use crate::print::NEW_STRING;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;

/// §480: `read_open` states.
pub const JUST_OPEN: i32 = 1;
pub const CLOSED: i32 = 2;

/// §445
const ZERO_TOKEN: i32 = OTHER_TOKEN + b'0' as i32;

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §464: convert `str_pool[b..pool_ptr]` to a new token list.
    pub(crate) fn str_toks(&mut self, b: usize) -> Result<crate::tok::Tokens, Jump> {
        if self.unicode {
            return self.str_toks_cat(b, 0);
        }
        self.str_room(1)?;
        // (a pooled list: the result is inserted, and given back when read)
        let end = self.pool_ptr;
        let mut p = self.pooled_list(|_| {});
        if let Some(l) = alloc::sync::Arc::get_mut(&mut p) {
            let buf = l.buffer();
            for k in b..end {
                let c = i32::from(self.str_pool[k]);
                buf.push(if c == i32::from(b' ') {
                    SPACE_TOKEN
                } else {
                    OTHER_TOKEN + c
                });
            }
            l.remake(false);
        }
        self.pool_ptr = b;
        Ok(p)
    }

    /// `XeTeX` §503 `str_toks_cat`: `str_toks` of the characters of a
    /// string (surrogate pairs joined), each with category `cat`, or (0)
    /// spaces as spaces and the others as other characters.
    pub(crate) fn str_toks_cat(&mut self, b: usize, cat: i32) -> Result<crate::tok::Tokens, Jump> {
        self.str_room(1)?;
        let end = self.pool_ptr;
        let chars: alloc::vec::Vec<i32> = if self.unicode {
            crate::strings::decode_chars(&self.str_pool[b..end])
                .map(crate::input::ci)
                .collect()
        } else {
            self.str_pool[b..end]
                .iter()
                .map(|&c| i32::from(c))
                .collect()
        };
        let mut p = self.pooled_list(|_| {});
        if let Some(l) = alloc::sync::Arc::get_mut(&mut p) {
            let buf = l.buffer();
            for c in chars {
                buf.push(if c == i32::from(b' ') && cat == 0 {
                    SPACE_TOKEN
                } else if cat == 0 {
                    OTHER_TOKEN + c
                } else if cat == ACTIVE_CHAR {
                    CS_TOKEN_FLAG + crate::wide::active_cs(c)
                } else {
                    MAX_CHAR_VAL * cat + c
                });
            }
            l.remake(false);
        }
        self.pool_ptr = b;
        Ok(p)
    }

    /// A new token list of the characters `text`, as `str_toks` makes.
    pub(crate) fn text_toks(&mut self, text: &[u8]) -> crate::tok::Tokens {
        self.pooled_list(|p| {
            for &c in text {
                p.push(if c == b' ' {
                    SPACE_TOKEN
                } else {
                    OTHER_TOKEN + i32::from(c)
                });
            }
        })
    }

    /// §465: a new token list for `\the`.
    pub(crate) fn the_toks(&mut self) -> Result<crate::tok::Tokens, Jump> {
        if self.cur_chr % 2 == 1 {
            // e-TeX: `\unexpanded` (1) or `\detokenize`
            let c = self.cur_chr;
            let toks = self.scan_general_text()?;
            if c == 1 {
                return Ok(self.tok_from(&toks));
            }
            let text = self.tokens_text(&toks);
            return Ok(self.text_toks(&text));
        }
        self.get_x_token()?;
        self.scan_something_internal(TOK_VAL, false)?;
        if self.cur_val_level >= IDENT_VAL {
            // §466: copy the token list.
            if self.cur_val_level == IDENT_VAL {
                Ok(self.tok_from(&[CS_TOKEN_FLAG + self.cur_val]))
            } else {
                // (the list itself: a value is shared, never copied)
                Ok(self.cur_toks.take().unwrap_or_else(|| self.null_list()))
            }
        } else {
            let old_setting = self.selector();
            self.set_selector(NEW_STRING);
            let b = self.pool_ptr;
            match self.cur_val_level {
                INT_VAL => self.print_int(self.cur_val),
                DIMEN_VAL => {
                    self.print_scaled(self.cur_val);
                    self.print_str(b"pt");
                }
                GLUE_VAL => {
                    let g = self.cur_glue;
                    self.print_spec(&g, b"pt");
                }
                _ => {
                    let g = self.cur_glue;
                    self.print_spec(&g, b"mu");
                }
            }
            self.set_selector(old_setting);
            self.str_toks(b)
        }
    }

    /// §467
    pub(crate) fn ins_the_toks(&mut self) -> Result<(), Jump> {
        let p = self.the_toks()?;
        self.ins_list(p)
    }

    /// §470: `\number`, `\romannumeral`, `\string`, `\meaning`,
    /// `\fontname`, `\jobname`.
    pub(crate) fn conv_toks(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        if c >= XETEX_FIRST_EXPAND_CODE {
            return self.xetex_conv_toks(c);
        }
        if c >= EXPANDED_CODE && c != JOB_NAME_CODE {
            return self.pdftex_conv_toks(c);
        }
        // §471: scan the argument for command `c`.
        match c {
            NUMBER_CODE | ROMAN_NUMERAL_CODE => self.scan_int()?,
            STRING_CODE | MEANING_CODE => {
                let save_scanner_status = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_token()?;
                self.scanner_status = save_scanner_status;
            }
            FONT_NAME_CODE => self.scan_font_ident()?,
            ETEX_REVISION_CODE => {}
            _ => {
                if self.job_name() == 0 {
                    self.open_log_file()?;
                }
            }
        }
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        let b = self.pool_ptr;
        // §472: print the result of command `c`.
        match c {
            NUMBER_CODE => self.print_int(self.cur_val),
            ROMAN_NUMERAL_CODE => self.print_roman_int(self.cur_val),
            STRING_CODE => {
                if self.cur_cs != 0 {
                    self.sprint_cs(self.cur_cs);
                } else if self.unicode {
                    self.print_char_x(crate::input::cu(self.cur_chr));
                } else {
                    self.print_char(u8::try_from(self.cur_chr).unwrap_or(0));
                }
            }
            MEANING_CODE => self.print_meaning(),
            FONT_NAME_CODE => {
                self.font_read(self.cur_val, crate::track::font::METRICS);
                let f = crate::fonts::fx(self.cur_val);
                if self.is_native_font(self.cur_val) {
                    // `XeTeX` §472: a native font's name in quotes
                    let name = self.fonts.name[f];
                    let quote = if self.str_bytes(crate::input::ux(name)).contains(&b'"') {
                        b'\''
                    } else {
                        b'"'
                    };
                    self.print_char(quote);
                    self.print(name);
                    self.print_char(quote);
                } else {
                    self.print(self.fonts.name[f]);
                }
                if self.fonts.metrics[f].size != self.fonts.metrics[f].design_size {
                    self.print_str(b" at ");
                    self.print_scaled(self.fonts.metrics[f].size);
                    self.print_str(b"pt");
                }
            }
            ETEX_REVISION_CODE => self.print_str(b".6"),
            _ => self.print(self.job_name()),
        }
        self.set_selector(old_setting);
        let p = self.str_toks(b)?;
        self.ins_list(p)
    }

    /// §473: scan a macro definition (`macro_def`) or a balanced text,
    /// expanding if `xpand`, into the new list `def_ref`.
    pub(crate) fn scan_toks(&mut self, macro_def: bool, xpand: bool) -> Result<(), Jump> {
        self.scanner_status = if macro_def { DEFINING } else { ABSORBING };
        self.warning_index = self.cur_cs;
        self.def_ref.clear();
        self.def_protected = false;
        let p = crate::bulk::Dst::Def;
        let mut hash_brace = 0;
        let mut t = ZERO_TOKEN;
        let mut found = false;
        if macro_def {
            // §474: scan and build the parameter part of the macro definition.
            let mut done = false;
            loop {
                // (set `cur_cmd`, `cur_chr`, `cur_tok`; a control
                // sequence's meaning matters by its class alone)
                self.tokens_only(Self::get_token)?;
                if self.cur_tok < RIGHT_BRACE_LIMIT {
                    break; // done1
                }
                if self.cur_cmd == MAC_PARAM {
                    // §476: if the next character is a parameter number,
                    // make `cur_tok` a `match` token; but if it is a left
                    // brace, store `left_brace, end_match`, set
                    // `hash_brace`, and `goto done`.
                    let s = MATCH_TOKEN + self.cur_chr;
                    self.tokens_only(Self::get_token)?;
                    if self.cur_tok < LEFT_BRACE_LIMIT {
                        hash_brace = self.cur_tok;
                        self.def_ref.push(self.cur_tok);
                        self.def_ref.push(END_MATCH_TOKEN);
                        done = true;
                        break;
                    }
                    if t == ZERO_TOKEN + 9 {
                        self.print_err(b"You already have nine parameters");
                        self.help(&[
                            b"I'm going to ignore the # sign you just used,",
                            b"as well as the token that followed it.",
                        ]);
                        self.error()?;
                        continue;
                    }
                    t += 1;
                    if self.cur_tok != t {
                        self.print_err(b"Parameters must be numbered consecutively");
                        self.help(&[
                            b"I've inserted the digit you should have used after the #.",
                            b"Type `1' to delete what you did use.",
                        ]);
                        self.back_error()?;
                    }
                    self.cur_tok = s;
                }
                self.def_ref.push(self.cur_tok);
            }
            if !done {
                // done1:
                self.def_ref.push(END_MATCH_TOKEN);
                if self.cur_cmd == RIGHT_BRACE {
                    // §475: express shock at the missing left brace.
                    self.print_err(b"Missing { inserted");
                    self.set_align_state(self.align_state() + 1);
                    self.help(&[
                        b"Where was the left brace? You said something like `\\def\\a}',",
                        b"which I'm going to interpret as `\\def\\a{}'.",
                    ]);
                    self.error()?;
                    found = true;
                }
            }
        } else {
            self.scan_left_brace()?; // remove the compulsory left brace
        }
        if !found {
            // §477: scan and build the body of the token list.
            let mut unbalance = 1;
            let how = if xpand {
                crate::bulk::Absorb::Expand { macro_def }
            } else {
                crate::bulk::Absorb::Body { macro_def }
            };
            loop {
                // (a run at once, `bulk.rs`)
                self.bulk_group(p, &mut unbalance, how);
                if xpand {
                    // §478: expand the next part of the input.
                    loop {
                        self.get_next()?;
                        if self.cur_cmd >= CALL && self.is_protected(self.cur_cs) {
                            self.cur_cmd = RELAX;
                            self.cur_chr = NO_EXPAND_FLAG;
                        }
                        if self.cur_cmd <= MAX_COMMAND {
                            break;
                        }
                        if self.cur_cmd == THE {
                            let q = self.the_toks()?;
                            self.def_ref.extend_from_slice(&q);
                            self.release_list(q);
                        } else {
                            self.expand()?;
                        }
                    }
                    self.x_token()?;
                } else {
                    self.tokens_only(Self::get_token)?;
                }
                if self.cur_tok < RIGHT_BRACE_LIMIT {
                    if self.cur_cmd < RIGHT_BRACE {
                        unbalance += 1;
                    } else {
                        unbalance -= 1;
                        if unbalance == 0 {
                            break; // found
                        }
                    }
                } else if self.cur_cmd == MAC_PARAM && macro_def {
                    // §479: look for parameter number or ##.
                    let s = self.cur_tok;
                    if xpand {
                        self.get_x_token()?;
                    } else {
                        self.tokens_only(Self::get_token)?;
                    }
                    if self.cur_cmd != MAC_PARAM {
                        if self.cur_tok <= ZERO_TOKEN || self.cur_tok > t {
                            self.print_err(b"Illegal parameter number in definition of ");
                            self.sprint_cs(self.warning_index);
                            self.help(&[
                                b"You meant to type ## instead of #, right?",
                                b"Or maybe a } was forgotten somewhere earlier, and things",
                                b"are all screwed up? I'm going to assume that you meant ##.",
                            ]);
                            self.back_error()?;
                            self.cur_tok = s;
                        } else {
                            self.cur_tok = OUT_PARAM_TOKEN - i32::from(b'0') + self.cur_chr;
                        }
                    }
                }
                self.def_ref.push(self.cur_tok);
            }
        }
        // found:
        self.scanner_status = NORMAL;
        if hash_brace != 0 {
            self.def_ref.push(hash_brace);
        }
        Ok(())
    }

    /// §482: read a line (or more, if braces are unbalanced) of `\read n`
    /// into a token list at `def_ref`; `r` is the control sequence;
    /// `line`: e-TeX's `\readline` (the characters as they are).
    pub(crate) fn read_toks(&mut self, mut n: i32, r: Pointer, line: bool) -> Result<(), Jump> {
        self.scanner_status = DEFINING;
        self.warning_index = r;
        self.def_ref.clear();
        self.def_protected = false;
        self.def_ref.push(END_MATCH_TOKEN);
        let m = if (0..=15).contains(&n) { n } else { 16 };
        let s = self.align_state();
        self.set_align_state(1_000_000); // disable tab marks, etc.
        loop {
            // §483: input and store tokens from the next line of the file.
            self.begin_file_reading()?;
            self.cur_input.name = m + 1;
            let mu = ux(m);
            if m < 16 {
                // (the stream's file and position, which the line read
                // moves: `streams`)
                self.read_file_read(mu);
                if !T::VALUES {
                    self.tracker.write(crate::track::Cell::Read(m));
                }
            }
            if self.read_open(mu) == CLOSED {
                // §484: input for \read from the terminal.
                if self.interaction() > crate::error::NONSTOP_MODE {
                    if n < 0 {
                        self.prompt_input(b"")?;
                    } else {
                        self.print_ln();
                        self.sprint_cs(r);
                        self.prompt_input(b"=")?;
                        n = -1;
                    }
                } else {
                    self.cur_input.limit = 0;
                    return self.fatal_error(b"*** (cannot \\read from terminal in nonstop modes)");
                }
            } else if self.read_open(mu) == JUST_OPEN {
                // §485: input the first line of `read_file[m]`.
                let read = self.read_line(mu);
                match read {
                    Ok(true) => self.set_read_open(mu, NORMAL),
                    Ok(false) => {
                        self.read_file[mu] = None;
                        self.set_read_open(mu, CLOSED);
                    }
                    Err(_) => {}
                }
                self.read_file_wrote(mu);
                read?;
            } else {
                // §486: input the next line of `read_file[m]`.
                let read = self.read_line(mu);
                if matches!(read, Ok(false)) {
                    self.read_file[mu] = None;
                    self.set_read_open(mu, CLOSED);
                }
                self.read_file_wrote(mu);
                if !read? && self.align_state() != 1_000_000 {
                    self.runaway();
                    self.print_err(b"File ended within ");
                    self.print_esc(b"read");
                    self.help(&[b"This \\read has unbalanced braces."]);
                    self.set_align_state(1_000_000);
                    self.cur_input.limit = 0;
                    self.error()?;
                }
            }
            self.cur_input.limit = i32::try_from(self.last).unwrap_or(0);
            if self.end_line_char_inactive() {
                self.cur_input.limit -= 1;
            } else {
                let c = self.int_par(END_LINE_CHAR_CODE);
                self.buffer[ux(self.cur_input.limit)] = crate::input::cu(c);
            }
            self.first = ux(self.cur_input.limit + 1);
            self.cur_input.loc = self.cur_input.start;
            self.cur_input.state = NEW_LINE;
            if line {
                // e-TeX: handle \readline.
                while self.cur_input.loc <= self.cur_input.limit {
                    let c = crate::input::ci(self.buffer[ux(self.cur_input.loc)]);
                    self.cur_input.loc += 1;
                    let t = if c == i32::from(b' ') {
                        SPACE_TOKEN
                    } else {
                        OTHER_TOKEN + c
                    };
                    self.def_ref.push(t);
                }
                self.end_file_reading();
                break;
            }
            loop {
                self.get_token()?;
                if self.cur_tok == 0 {
                    break; // `cur_cmd=cur_chr=0` will occur at the end of the line
                }
                if self.align_state() < 1_000_000 {
                    // unmatched `}` aborts the line
                    loop {
                        self.get_token()?;
                        if self.cur_tok == 0 {
                            break;
                        }
                    }
                    self.set_align_state(1_000_000);
                    break;
                }
                self.def_ref.push(self.cur_tok);
            }
            self.end_file_reading();
            if self.align_state() == 1_000_000 {
                break;
            }
        }
        self.cur_val = 1; // (the list is `def_ref`)
        self.scanner_status = NORMAL;
        self.set_align_state(s);
        Ok(())
    }

    /// `input_ln(read_file[m])`.
    fn read_line(&mut self, m: usize) -> Result<bool, Jump> {
        let mut f = self.read_file[m].take().unwrap_or_default();
        let from = f.pos;
        let r = self.input_ln(&mut f);
        match r {
            Ok(true) => self.start_line(&mut f, from, false),
            Ok(false) => self.end_of_file(&f, from),
            Err(_) => {}
        }
        self.read_file[m] = Some(f);
        r
    }
}

#[cfg(test)]
mod tests {
    use crate::mem::NULL;
    use crate::testing::{engine, feed, term_output};
    use crate::web::*;

    /// Oracle: `\count5=42` and `\show` of
    /// `\def\a#1#2{x#1\the\count5 ##}`,
    /// `\edef\b#1{\number\count5 \romannumeral 14 \the\count5 x#1\string\foo\meaning\relax}`,
    /// `\def\c#1#{[#1]}`.
    #[test]
    fn scan_toks_matches_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        for (c, cat) in [(b'{', LEFT_BRACE), (b'}', RIGHT_BRACE), (b'#', MAC_PARAM)] {
            t.set_equiv(CAT_CODE_BASE + i32::from(c), cat);
        }
        t.set_eqtb_int(COUNT_BASE + 5, 42);
        let cases: &[(&[u8], bool, &str)] = &[
            (b"#1#2{x#1\\the\\count5 ##}", false, "#1#2->x#1\\the \\count 5 ##"),
            (
                b"#1{\\number\\count5 \\romannumeral 14 \\the\\count5 x#1\\string\\foo\\meaning\\relax}",
                true,
                "#1->42xiv42x#1\\foo\\relax",
            ),
            (b"#1#{[#1]}", false, "#1{->[#1]{"),
        ];
        for &(src, xpand, want) in cases {
            let mut line = src.to_vec();
            line.push(b'\n');
            feed(&mut t, &line);
            t.term_offset = 0;
            let out = term_output(&mut t, |t| {
                t.scan_toks(true, xpand).unwrap();
                t.show_token_list(&t.def_ref.clone(), NULL, 1000);
            });
            assert_eq!(core::str::from_utf8(&out).unwrap(), want);
        }
    }
}
