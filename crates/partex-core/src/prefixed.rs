//! Part 49: Mode-independent processing (§1208–§1298), plus
//! `show_activities` (§218–§219).

use alloc::sync::Arc;
use alloc::vec::Vec;

use partex_engine::node::{GlueSpec, Order};

use crate::arith::Scaled;
use crate::build::{BOX_FLAG, GLOBAL_BOX_FLAG};
use crate::cmds::*;
use crate::host::Host;
use crate::input::{AlphaFile, ux};
use crate::mem::{NULL, Pointer};
use crate::nest::IGNORE_DEPTH;
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::xregs::reg_loc;

/// §1214: `global`.
fn global(a: i32) -> bool {
    a >= 4
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §1214: `define` of an entry that holds object `o` (a list, glue, a
    /// shape: the value itself, DESIGN 7.17.12).
    pub(crate) fn define_obj(
        &mut self,
        a: i32,
        p: Pointer,
        t: i32,
        o: crate::objs::Obj,
    ) -> Result<(), Jump> {
        if global(a) {
            self.geq_define_obj(p, t, NULL, Some(o));
            Ok(())
        } else {
            self.eq_define_obj(p, t, NULL, Some(o))
        }
    }

    /// §1214: `define`.
    pub(crate) fn define(&mut self, a: i32, p: Pointer, t: i32, e: i32) -> Result<(), Jump> {
        if global(a) {
            self.geq_define(p, t, e);
            Ok(())
        } else {
            self.eq_define(p, t, e)
        }
    }

    /// §1214: `word_define`.
    fn word_define(&mut self, a: i32, p: Pointer, w: i32) -> Result<(), Jump> {
        if global(a) {
            self.geq_word_define(p, w);
            Ok(())
        } else {
            self.eq_word_define(p, w)
        }
    }

    /// §404: get the next non-blank non-relax non-call token.
    pub(crate) fn get_next_nonblank_nonrelax_noncall(&mut self) -> Result<(), Jump> {
        loop {
            self.get_x_token()?;
            if self.cur_cmd != SPACER && self.cur_cmd != RELAX {
                return Ok(());
            }
        }
    }

    /// §1211
    pub(crate) fn prefixed_command(&mut self) -> Result<(), Jump> {
        let mut a = 0;
        while self.cur_cmd == PREFIX {
            if (a / self.cur_chr) % 2 == 0 {
                a += self.cur_chr;
            }
            self.get_next_nonblank_nonrelax_noncall()?;
            if self.cur_cmd <= MAX_NON_PREFIXED_COMMAND {
                // §1212: discard erroneous prefixes and return.
                self.print_err(b"You can't use a prefix with `");
                self.print_cmd_chr(self.cur_cmd, self.cur_chr);
                self.print_char(b'\'');
                if self.etex_ex() {
                    self.help(&[
                        b"I'll pretend you didn't say \\long or \\outer or \\global or \\protected.",
                    ]);
                } else {
                    self.help(&[b"I'll pretend you didn't say \\long or \\outer or \\global."]);
                }
                return self.back_error();
            }
        }
        // §1213: discard the prefixes \long and \outer (and e-TeX's
        // \protected) if they are irrelevant.
        let protected = a >= 8;
        if protected {
            a -= 8;
        }
        if self.cur_cmd != DEF && (a % 4 != 0 || protected) {
            self.print_err(b"You can't use `");
            self.print_esc(b"long");
            self.print_str(b"' or `");
            self.print_esc(b"outer");
            if self.etex_ex() {
                self.help(&[b"I'll pretend you didn't say \\long or \\outer or \\protected here."]);
                self.print_str(b"' or `");
                self.print_esc(b"protected");
            } else {
                self.help(&[b"I'll pretend you didn't say \\long or \\outer here."]);
            }
            self.print_str(b"' with `");
            self.print_cmd_chr(self.cur_cmd, self.cur_chr);
            self.print_char(b'\'');
            self.error()?;
        }
        // §1214: adjust for the setting of \globaldefs.
        let global_defs = self.int_par(GLOBAL_DEFS_CODE);
        if global_defs != 0 {
            if global_defs < 0 {
                if global(a) {
                    a -= 4;
                }
            } else if !global(a) {
                a += 4;
            }
        }
        if !self.assignment(a, protected)? {
            return Ok(());
        }
        // done: §1269: insert a token saved by \afterassignment, if any.
        if self.after_token() != 0 {
            self.cur_tok = self.after_token();
            self.back_input()?;
            self.set_after_token(0);
        }
        Ok(())
    }

    /// The `case cur_cmd of <Assignments>` of §1211. Returns `false` for
    /// tex.web's `return` (skipping the \afterassignment token), `true`
    /// for `goto done` or falling through.
    fn assignment(&mut self, mut a: i32, protected: bool) -> Result<bool, Jump> {
        match self.cur_cmd {
            // §1217
            SET_FONT => self.define(a, CUR_FONT_LOC, DATA, self.cur_chr)?,
            // §1218
            DEF => {
                if self.cur_chr % 2 == 1 && !global(a) && self.int_par(GLOBAL_DEFS_CODE) >= 0 {
                    a += 4;
                }
                let e = self.cur_chr >= 2;
                self.get_r_token()?;
                let p = self.cur_cs;
                self.scan_toks(true, e)?;
                if protected {
                    self.def_protected = true;
                }
                if self.memo.enabled {
                    self.fresh_def = true; // (see `eq_define`)
                }
                let body = self.take_def();
                let r = self.define_obj(a, p, CALL + a % 4, crate::objs::Obj::Toks(body));
                self.fresh_def = false;
                r?;
            }
            // §1221
            LET => {
                if self.cur_chr == NORMAL + 11 || self.cur_chr == NORMAL + 10 {
                    // encTeX's \noconvert and \mubyte; `tex` does not
                    // define them.
                    return self.confusion(b"mubyte").map(|()| true);
                }
                let n = self.cur_chr;
                self.get_r_token()?;
                let p = self.cur_cs;
                if n == NORMAL {
                    loop {
                        self.get_token()?;
                        if self.cur_cmd != SPACER {
                            break;
                        }
                    }
                    if self.cur_tok == OTHER_TOKEN + i32::from(b'=') {
                        self.get_token()?;
                        if self.cur_cmd == SPACER {
                            self.get_token()?;
                        }
                    }
                } else {
                    self.get_token()?;
                    let q = self.cur_tok;
                    self.get_token()?;
                    self.back_input()?;
                    self.cur_tok = q;
                    self.back_input()?; // look ahead, then back up
                } // note that `back_input` doesn't affect `cur_cmd`, `cur_chr`
                if crate::equiv::holds_object(self.cur_cmd) {
                    // (the meaning's value itself, shared)
                    let o = self.peek_obj(self.cur_cs).cloned();
                    match o {
                        Some(o) => self.define_obj(a, p, self.cur_cmd, o)?,
                        None => self.define(a, p, self.cur_cmd, NULL)?,
                    }
                } else {
                    self.define(a, p, self.cur_cmd, self.cur_chr)?;
                }
            }
            // §1224
            SHORTHAND_DEF => self.shorthand_def(a)?,
            // §1225
            READ_TO_CS => {
                let line = self.cur_chr == 1;
                self.scan_int()?;
                let n = self.cur_val;
                if !self.scan_keyword(b"to")? {
                    self.print_err(b"Missing `to' inserted");
                    self.help(&[
                        b"You should have said `\\read<number> to \\cs'.",
                        b"I'm going to look for the \\cs now.",
                    ]);
                    self.error()?;
                }
                self.get_r_token()?;
                let p = self.cur_cs;
                self.read_toks(n, p, line)?;
                let body = self.take_def();
                self.define_obj(a, p, CALL, crate::objs::Obj::Toks(body))?;
            }
            // §1226
            TOKS_REGISTER | ASSIGN_TOKS => self.assign_toks(a)?,
            // §1228
            ASSIGN_INT => {
                let p = self.cur_chr;
                self.scan_optional_equals()?;
                self.scan_int()?;
                self.word_define(a, p, self.cur_val)?;
            }
            ASSIGN_DIMEN => {
                let p = self.cur_chr;
                self.scan_optional_equals()?;
                self.scan_normal_dimen()?;
                self.word_define(a, p, self.cur_val)?;
            }
            ASSIGN_GLUE | ASSIGN_MU_GLUE => {
                let p = self.cur_chr;
                let n = self.cur_cmd;
                self.scan_optional_equals()?;
                if n == ASSIGN_MU_GLUE {
                    self.scan_glue(MU_VAL)?;
                } else {
                    self.scan_glue(GLUE_VAL)?;
                }
                self.trap_zero_glue();
                self.define_glue(a, p)?;
            }
            // §1232
            DEF_CODE => self.def_code(a)?,
            // §1234
            DEF_FAMILY => {
                let mut p = self.cur_chr;
                self.scan_four_bit_int()?;
                p += self.cur_val;
                self.scan_optional_equals()?;
                self.scan_font_ident()?;
                self.define(a, p, DATA, self.cur_val)?;
            }
            // §1235
            REGISTER | ADVANCE | MULTIPLY | DIVIDE => self.do_register_command(a)?,
            // §1241
            SET_BOX => {
                self.scan_register_num()?;
                let n = if global(a) {
                    GLOBAL_BOX_FLAG + self.cur_val
                } else {
                    BOX_FLAG + self.cur_val
                };
                self.scan_optional_equals()?;
                if self.set_box_allowed {
                    self.scan_box(n)?;
                } else {
                    self.print_err(b"Improper ");
                    self.print_esc(b"setbox");
                    self.help(&[
                        b"Sorry, \\setbox is not allowed after \\halign in a display,",
                        b"or between \\accent and an accented character.",
                    ]);
                    self.error()?;
                }
            }
            // §1242
            SET_AUX => self.alter_aux()?,
            SET_PREV_GRAF => self.alter_prev_graf()?,
            SET_PAGE_DIMEN => self.alter_page_so_far()?,
            SET_PAGE_INT => self.alter_integer()?,
            SET_BOX_DIMEN => self.alter_box_dimen()?,
            // §1248
            SET_SHAPE => {
                let q = self.cur_chr;
                self.scan_optional_equals()?;
                self.scan_int()?;
                let n = self.cur_val;
                let p = if n <= 0 {
                    None
                } else if q > PAR_SHAPE_LOC {
                    // e-TeX: a penalty array.
                    let mut pens = Vec::with_capacity(ux(n));
                    for _ in 1..=n {
                        self.scan_int()?;
                        pens.push(self.cur_val);
                    }
                    Some(crate::objs::Shaped::Penalties(Arc::from(pens)))
                } else {
                    let mut lines = Vec::with_capacity(ux(n));
                    for _ in 1..=n {
                        self.scan_normal_dimen()?;
                        let indent = self.cur_val;
                        self.scan_normal_dimen()?;
                        lines.push((indent, self.cur_val));
                    }
                    Some(crate::objs::Shaped::Lines(Arc::from(lines)))
                };
                let loc = if q > PAR_SHAPE_LOC { q } else { PAR_SHAPE_LOC };
                match p {
                    Some(sh) => {
                        self.define_obj(a, loc, SHAPE_REF, crate::objs::Obj::Shape(Arc::new(sh)))?;
                    }
                    None => self.define(a, loc, SHAPE_REF, NULL)?,
                }
            }
            // §1252
            HYPH_DATA => {
                if self.cur_chr == 1 {
                    if self.params.ini {
                        self.new_patterns()?;
                        return Ok(true);
                    }
                    self.print_err(b"Patterns can be loaded only by INITEX");
                    self.help(&[]);
                    self.error()?;
                    loop {
                        self.get_token()?;
                        if self.cur_cmd == RIGHT_BRACE {
                            break; // flush the patterns
                        }
                    }
                    return Ok(false);
                }
                self.new_hyph_exceptions()?;
            }
            // §1253
            ASSIGN_FONT_DIMEN => {
                let k = self.find_font_dimen(true)?;
                self.scan_optional_equals()?;
                self.scan_normal_dimen()?;
                if let Some((f, n)) = k {
                    self.fonts.params_mut(f)[n - 1] = self.cur_val;
                    self.font_wrote(f, crate::track::font::PARAMS);
                }
            }
            ASSIGN_FONT_INT => self.assign_font_int(self.cur_chr)?,
            // §1256
            DEF_FONT => self.new_font(a)?,
            LETTERSPACE_FONT => self.new_letterspaced_font(a)?,
            PDF_COPY_FONT => self.make_font_copy(a)?,
            // §1264
            SET_INTERACTION => self.new_interaction(),
            _ => self.confusion(b"prefix")?,
        }
        Ok(true)
    }

    /// §1215
    pub(crate) fn get_r_token(&mut self) -> Result<(), Jump> {
        loop {
            loop {
                self.get_token()?;
                if self.cur_tok != SPACE_TOKEN {
                    break;
                }
            }
            if self.cur_cs == 0
                || self.cur_cs > self.eqtb_top
                || (self.cur_cs > FROZEN_CONTROL_SEQUENCE && self.cur_cs <= EQTB_SIZE)
            {
                self.print_err(b"Missing control sequence inserted");
                self.help(&[
                    b"Please don't say `\\def cs{...}', say `\\def\\cs{...}'.",
                    b"I've inserted an inaccessible control sequence so that your",
                    b"definition will be completed without mixing me up too badly.",
                    b"You can recover graciously from this error, if you're",
                    b"careful; see exercise 27.2 in The TeXbook.",
                ]);
                if self.cur_cs == 0 {
                    self.back_input()?;
                }
                self.cur_tok = CS_TOKEN_FLAG + FROZEN_PROTECTION;
                self.ins_error()?;
                continue;
            }
            if T::VALUES {
                // (the scanner looked up the meaning this assignment
                // replaces; the assignment does not depend on it)
                self.tracker.retract(crate::track::Cell::Eqtb(self.cur_cs));
            }
            return Ok(());
        }
    }

    /// §1224
    fn shorthand_def(&mut self, a: i32) -> Result<(), Jump> {
        if self.cur_chr == CHAR_SUB_DEF_CODE {
            self.scan_char_num()?;
            let p = CHAR_SUB_CODE_BASE + self.cur_val;
            self.scan_optional_equals()?;
            self.scan_char_num()?;
            let mut n = self.cur_val; // accent character in substitution
            self.scan_char_num()?;
            if self.int_par(TRACING_CHAR_SUB_DEF_CODE) > 0 {
                self.begin_diagnostic();
                self.print_nl(b"New character substitution: ");
                self.print(p - CHAR_SUB_CODE_BASE);
                self.print_str(b" = ");
                self.print(n);
                self.print_char(b' ');
                self.print(self.cur_val);
                self.end_diagnostic(false);
            }
            n = n * 256 + self.cur_val;
            self.define(a, p, DATA, n)?;
            if p - CHAR_SUB_CODE_BASE < self.int_par(CHAR_SUB_DEF_MIN_CODE) {
                self.word_define(a, INT_BASE + CHAR_SUB_DEF_MIN_CODE, p - CHAR_SUB_CODE_BASE)?;
            }
            if p - CHAR_SUB_CODE_BASE > self.int_par(CHAR_SUB_DEF_MAX_CODE) {
                self.word_define(a, INT_BASE + CHAR_SUB_DEF_MAX_CODE, p - CHAR_SUB_CODE_BASE)?;
            }
            return Ok(());
        }
        let n = self.cur_chr;
        self.get_r_token()?;
        let p = self.cur_cs;
        self.define(a, p, RELAX, 256)?;
        self.scan_optional_equals()?;
        match n {
            CHAR_DEF_CODE => {
                self.scan_char_num()?;
                self.define(a, p, CHAR_GIVEN, self.cur_val)
            }
            MATH_CHAR_DEF_CODE => {
                self.scan_fifteen_bit_int()?;
                self.define(a, p, MATH_GIVEN, self.cur_val)
            }
            _ => {
                self.scan_register_num()?;
                let v = self.cur_val;
                match n {
                    COUNT_DEF_CODE => self.define(a, p, ASSIGN_INT, reg_loc(INT_VAL, v)),
                    DIMEN_DEF_CODE => self.define(a, p, ASSIGN_DIMEN, reg_loc(DIMEN_VAL, v)),
                    SKIP_DEF_CODE => self.define(a, p, ASSIGN_GLUE, reg_loc(GLUE_VAL, v)),
                    MU_SKIP_DEF_CODE => self.define(a, p, ASSIGN_MU_GLUE, reg_loc(MU_VAL, v)),
                    TOKS_DEF_CODE => self.define(a, p, ASSIGN_TOKS, reg_loc(TOK_VAL, v)),
                    _ => Ok(()), // there are no other cases
                }
            }
        }
    }

    /// §1226
    fn assign_toks(&mut self, a: i32) -> Result<(), Jump> {
        let q = self.cur_cs;
        let p = if self.cur_cmd == TOKS_REGISTER {
            self.scan_register_num()?;
            reg_loc(TOK_VAL, self.cur_val)
        } else {
            self.cur_chr // `p = every_par_loc` or `output_routine_loc` or …
        };
        self.scan_optional_equals()?;
        self.get_next_nonblank_nonrelax_noncall()?;
        if self.cur_cmd != LEFT_BRACE {
            // §1227: if the right-hand side is a token parameter or token
            // register, finish the assignment and `goto done`.
            if self.cur_cmd == TOKS_REGISTER {
                self.scan_register_num()?;
                self.cur_cmd = ASSIGN_TOKS;
                self.cur_chr = reg_loc(TOK_VAL, self.cur_val);
            }
            if self.cur_cmd == ASSIGN_TOKS {
                match self.equiv_toks(self.cur_chr).cloned() {
                    None => self.define(a, p, UNDEFINED_CS, NULL)?,
                    Some(q) => self.define_obj(a, p, CALL, crate::objs::Obj::Toks(q))?,
                }
                return Ok(());
            }
        }
        self.back_input()?;
        self.cur_cs = q;
        self.scan_toks(false, false)?;
        if self.def_ref.is_empty() {
            // empty list: revert to the default
            self.define(a, p, UNDEFINED_CS, NULL)?;
            self.def_ref.clear();
        } else {
            if p == OUTPUT_ROUTINE_LOC {
                // enclose in curlies
                self.def_ref.push(RIGHT_BRACE_TOKEN + i32::from(b'}'));
                self.def_ref.insert(0, LEFT_BRACE_TOKEN + i32::from(b'{'));
            }
            let body = self.take_def();
            self.define_obj(a, p, CALL, crate::objs::Obj::Toks(body))?;
        }
        Ok(())
    }

    /// §1229
    fn trap_zero_glue(&mut self) {
        if self.cur_glue.is_zero() {
            self.cur_glue = GlueSpec::ZERO_GLUE;
            self.glue_origin = None;
        }
    }

    /// `define(p, glue_ref, cur_val)` for the glue in `cur_glue`.
    fn define_glue(&mut self, a: i32, p: i32) -> Result<(), Jump> {
        let origin = self.glue_origin.take();
        let g = self.new_glue_value(self.cur_glue, origin);
        self.define_obj(a, p, GLUE_REF, crate::objs::Obj::Glue(g))
    }

    /// §1232
    fn def_code(&mut self, a: i32) -> Result<(), Jump> {
        // §1233: let `n` be the largest legal code value, based on `cur_chr`.
        let n = match self.cur_chr {
            CAT_CODE_BASE => MAX_CHAR_CODE,
            MATH_CODE_BASE => 0o100000,
            SF_CODE_BASE => 0o77777,
            DEL_CODE_BASE => 0o77777777,
            _ => 255,
        };
        let mut p = self.cur_chr;
        self.scan_char_num()?;
        p = if p == XORD_CODE_BASE {
            self.cur_val
        } else if p == XCHR_CODE_BASE {
            self.cur_val + 256
        } else if p == XPRN_CODE_BASE {
            self.cur_val + 512
        } else {
            p + self.cur_val
        };
        self.scan_optional_equals()?;
        self.scan_int()?;
        if (self.cur_val < 0 && p < DEL_CODE_BASE) || self.cur_val > n {
            self.print_err(b"Invalid code (");
            self.print_int(self.cur_val);
            if p < DEL_CODE_BASE {
                self.print_str(b"), should be in the range 0..");
            } else {
                self.print_str(b"), should be at most ");
            }
            self.print_int(n);
            self.help(&[b"I'm going to use 0 instead of that illegal code value."]);
            self.error()?;
            self.cur_val = 0;
        }
        // The first three ranges are encTeX's \xordcode, \xchrcode and
        // \xprncode (their values are at most 255 by §1233).
        let byte = u8::try_from(self.cur_val).unwrap_or(0);
        if p < 256 {
            self.xord[ux(p)] = byte;
        } else if p < 512 {
            self.xchr[ux(p - 256)] = byte;
        } else if p < 768 {
            self.xprn[ux(p - 512)] = self.cur_val != 0;
        } else if p < DEL_CODE_BASE {
            self.define(a, p, DATA, self.cur_val)?;
        } else {
            self.word_define(a, p, self.cur_val)?;
        }
        Ok(())
    }

    /// §1236
    fn do_register_command(&mut self, a: i32) -> Result<(), Jump> {
        let q = self.cur_cmd;
        // §1237: compute the register location `l` and its type `p`; but
        // `return` if invalid.
        let (l, p) = 'found: {
            if q != REGISTER {
                self.get_x_token()?;
                if self.cur_cmd >= ASSIGN_INT && self.cur_cmd <= ASSIGN_MU_GLUE {
                    break 'found (self.cur_chr, self.cur_cmd - ASSIGN_INT);
                }
                if self.cur_cmd != REGISTER {
                    self.print_err(b"You can't use `");
                    self.print_cmd_chr(self.cur_cmd, self.cur_chr);
                    self.print_str(b"' after ");
                    self.print_cmd_chr(q, 0);
                    self.help(&[b"I'm forgetting what you said and not changing anything."]);
                    return self.error();
                }
            }
            let p = self.cur_chr;
            self.scan_register_num()?;
            let l = reg_loc(p, self.cur_val);
            (l, p)
        };
        if q == REGISTER {
            self.scan_optional_equals()?;
        } else {
            self.scan_keyword(b"by")?; // optional `by`
        }
        self.arith_error = false;
        if q < MULTIPLY {
            // §1238: compute result of `register` or `advance`, put it in
            // `cur_val`.
            if p < GLUE_VAL {
                if p == INT_VAL {
                    self.scan_int()?;
                } else {
                    self.scan_normal_dimen()?;
                }
                if q == ADVANCE {
                    self.cur_val = self.cur_val.wrapping_add(self.eqtb_int(l));
                }
            } else {
                self.scan_glue(p)?;
                if q == ADVANCE {
                    // §1239: compute the sum of two glue specs.
                    let mut q = self.cur_glue.copy();
                    let r = self.glue_at(l);
                    q.width = q.width.wrapping_add(r.width);
                    if q.stretch == 0 {
                        q.stretch_order = Order::Normal;
                    }
                    if q.stretch_order == r.stretch_order {
                        q.stretch = q.stretch.wrapping_add(r.stretch);
                    } else if q.stretch_order < r.stretch_order && r.stretch != 0 {
                        q.stretch = r.stretch;
                        q.stretch_order = r.stretch_order;
                    }
                    if q.shrink == 0 {
                        q.shrink_order = Order::Normal;
                    }
                    if q.shrink_order == r.shrink_order {
                        q.shrink = q.shrink.wrapping_add(r.shrink);
                    } else if q.shrink_order < r.shrink_order && r.shrink != 0 {
                        q.shrink = r.shrink;
                        q.shrink_order = r.shrink_order;
                    }
                    self.cur_glue = q;
                    self.glue_origin = None;
                }
            }
        } else {
            // §1240: compute result of `multiply` or `divide`, put it in
            // `cur_val`.
            self.scan_int()?;
            if p < GLUE_VAL {
                let e = self.eqtb_int(l);
                self.cur_val = if q == MULTIPLY {
                    if p == INT_VAL {
                        self.mult_integers(e, self.cur_val)
                    } else {
                        self.nx_plus_y(e, self.cur_val, 0)
                    }
                } else {
                    self.x_over_n(e, self.cur_val)
                };
            } else {
                let s = self.glue_at(l);
                let n = self.cur_val;
                let (w, st, sh) = (s.width, s.stretch, s.shrink);
                let (w, st, sh) = if q == MULTIPLY {
                    (
                        self.nx_plus_y(w, n, 0),
                        self.nx_plus_y(st, n, 0),
                        self.nx_plus_y(sh, n, 0),
                    )
                } else {
                    (
                        self.x_over_n(w, n),
                        self.x_over_n(st, n),
                        self.x_over_n(sh, n),
                    )
                };
                self.cur_glue = GlueSpec {
                    width: w,
                    stretch: st,
                    shrink: sh,
                    ..s.copy()
                };
                self.glue_origin = None;
            }
        }
        if self.arith_error {
            self.print_err(b"Arithmetic overflow");
            self.help(&[
                b"I can't carry out that multiplication or division,",
                b"since the result is out of range.",
            ]);
            return self.error();
        }
        if p < GLUE_VAL {
            self.word_define(a, l, self.cur_val)
        } else {
            self.trap_zero_glue();
            self.define_glue(a, l)
        }
    }

    /// §1243
    fn alter_aux(&mut self) -> Result<(), Jump> {
        if self.cur_chr != self.mode().abs() {
            return self.report_illegal_case();
        }
        let c = self.cur_chr;
        self.scan_optional_equals()?;
        if c == VMODE {
            self.scan_normal_dimen()?;
            self.set_prev_depth(self.cur_val);
        } else {
            self.scan_int()?;
            if self.cur_val <= 0 || self.cur_val > 32767 {
                self.print_err(b"Bad space factor");
                self.help(&[b"I allow only values in the range 1..32767 here."]);
                self.int_error(self.cur_val)?;
            } else {
                self.set_space_factor(self.cur_val);
            }
        }
        Ok(())
    }

    /// §1244
    fn alter_prev_graf(&mut self) -> Result<(), Jump> {
        let mut p = self.nest_ptr();
        while self.nest_at(p).mode.abs() != VMODE {
            p -= 1;
        }
        self.scan_optional_equals()?;
        self.scan_int()?;
        if self.cur_val < 0 {
            self.print_err(b"Bad ");
            self.print_esc(b"prevgraf");
            self.help(&[b"I allow only nonnegative values here."]);
            self.int_error(self.cur_val)
        } else {
            self.nest_at_mut(p).pg = self.cur_val;
            Ok(())
        }
    }

    /// §1245
    fn alter_page_so_far(&mut self) -> Result<(), Jump> {
        let c = ux(self.cur_chr);
        self.scan_optional_equals()?;
        self.scan_normal_dimen()?;
        self.set_page_so_far(c, self.cur_val);
        Ok(())
    }

    /// §1246
    fn alter_integer(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        self.scan_optional_equals()?;
        self.scan_int()?;
        match c {
            0 => self.set_dead_cycles(self.cur_val),
            1 => self.set_insert_penalties(self.cur_val),
            // e-TeX: `\interactionmode`.
            _ if !(BATCH_MODE..=ERROR_STOP_MODE).contains(&self.cur_val) => {
                self.print_err(b"Bad interaction mode");
                self.help(&[
                    b"Modes are 0=batch, 1=nonstop, 2=scroll, and",
                    b"3=errorstop. Proceed, and I'll ignore this case.",
                ]);
                self.int_error(self.cur_val)?;
            }
            _ => {
                self.cur_chr = self.cur_val;
                self.new_interaction();
            }
        }
        Ok(())
    }

    /// §1247
    fn alter_box_dimen(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        self.scan_register_num()?;
        let b = self.cur_val;
        self.scan_optional_equals()?;
        self.scan_normal_dimen()?;
        let v = self.cur_val;
        let loc = reg_loc(BOX_VAL, b);
        let _ = self.eqtb(loc);
        if let Some(old) = self.peek_box(loc) {
            // (`mem[box(b)+c].sc:=cur_val`: a new value of the register,
            // the box made again with the dimension changed; the entry's
            // word is the same)
            let mut bx = (*old).clone();
            match c {
                WIDTH_OFFSET => bx.width = v,
                DEPTH_OFFSET => bx.depth = v,
                _ => bx.height = v,
            }
            bx.reversion();
            let w = self.peek_eqtb(loc);
            self.set_eqtb_entry(loc, w, Some(crate::objs::Obj::Box(Arc::new(bx))));
        }
        Ok(())
    }

    /// §1257
    /// §1257: get the control sequence a font is defined as, and the name
    /// for its frozen font identifier.
    pub(crate) fn font_identifier(&mut self) -> Result<(Pointer, i32), Jump> {
        self.get_r_token()?;
        let u = self.cur_cs;
        let t = if u >= HASH_BASE {
            self.text(u)
        } else if u >= SINGLE_BASE {
            if u == NULL_CS {
                i32::try_from(self.find_pool_string(b"FONT").unwrap_or(0)).unwrap_or(0)
            } else {
                u - SINGLE_BASE
            }
        } else {
            let old_setting = self.selector();
            self.set_selector(NEW_STRING);
            self.print_str(b"FONT");
            self.print(u - ACTIVE_BASE);
            self.set_selector(old_setting);
            self.str_room(1)?;
            i32::try_from(self.make_string()?).unwrap_or(0)
        };
        Ok((u, t))
    }

    fn new_font(&mut self, a: i32) -> Result<(), Jump> {
        if self.job_name() == 0 {
            self.open_log_file()?; // avoid confusing `texput` with the font name
        }
        let (u, t) = self.font_identifier()?;
        self.define(a, u, SET_FONT, NULL_FONT)?;
        self.scan_optional_equals()?;
        self.scan_file_name()?;
        // §1258: scan the font size specification.
        self.name_in_progress = true; // this keeps `cur_name` from being changed
        let mut s: Scaled;
        if self.scan_keyword(b"at")? {
            // §1259: put the (positive) `at' size into `s`.
            self.scan_normal_dimen()?;
            s = self.cur_val;
            if s <= 0 || s >= 0o1000000000 {
                self.print_err(b"Improper `at' size (");
                self.print_scaled(s);
                self.print_str(b"pt), replaced by 10pt");
                self.help(&[
                    b"I can only handle fonts at positive sizes that are",
                    b"less than 2048pt, so I've changed what you said to 10pt.",
                ]);
                self.error()?;
                s = 10 * UNITY;
            }
        } else if self.scan_keyword(b"scaled")? {
            self.scan_int()?;
            s = -self.cur_val;
            if self.cur_val <= 0 || self.cur_val > 32768 {
                self.print_err(b"Illegal magnification has been changed to 1000");
                self.help(&[b"The magnification ratio must be between 1 and 32768."]);
                self.int_error(self.cur_val)?;
                s = -1000;
            }
        } else {
            s = -1000;
        }
        self.name_in_progress = false;
        // §1260: if this font has already been loaded, set `f` to the
        // internal font number and `goto common_ending`.
        self.tracker.read(crate::track::Cell::FontTable);
        self.font_table_read();
        if self.font_cells {
            let b = alloc::sync::Arc::from(self.str_bytes(ux(self.cur_name)));
            self.font_touch(crate::fonts::FontTouch::Name(b, false));
        }
        let f = 'common_ending: {
            // (the fonts' names, areas and sizes are the table's)
            for f in self.fonts.loaded_fonts() {
                let fi = crate::fonts::fx(f);
                if self.str_eq_str(ux(self.fonts.name[fi]), ux(self.cur_name))
                    && self.str_eq_str(ux(self.fonts.area[fi]), ux(self.cur_area))
                {
                    let (size, dsize) = (self.fonts.get(f).size, self.fonts.get(f).design_size);
                    if s > 0 {
                        if s == size {
                            break 'common_ending f;
                        }
                    } else {
                        self.arith_error = false;
                        let d = self.xn_over_d(dsize, -s, 1000);
                        if size == d && !self.arith_error {
                            break 'common_ending f;
                        }
                    }
                }
            }
            self.read_font_info(u, self.cur_name, self.cur_area, s)?
        };
        if self.params.flavor == crate::params::Flavor::PdfTex {
            // (e-TeX defines, so that \tracingassigns shows it)
            self.define(a, u, SET_FONT, f)?;
        } else {
            self.set_equiv(u, f);
        }
        let w = self.eqtb(u);
        self.set_eqtb(FONT_ID_BASE + f, w);
        self.set_font_id_text(f, t);
        Ok(())
    }

    /// §1265
    fn new_interaction(&mut self) {
        self.print_ln();
        self.set_interaction(self.cur_chr);
        // §75: initialize the print `selector` based on `interaction`.
        let sel = if self.interaction() == BATCH_MODE {
            NO_PRINT
        } else {
            TERM_ONLY
        };
        self.set_selector(sel);
        if self.log_opened() {
            self.set_selector(self.selector() + 2);
        }
    }

    /// §1270
    pub(crate) fn do_assignments(&mut self) -> Result<(), Jump> {
        loop {
            self.get_next_nonblank_nonrelax_noncall()?;
            if self.cur_cmd <= MAX_NON_PREFIXED_COMMAND {
                return Ok(());
            }
            self.set_box_allowed = false;
            self.prefixed_command()?;
            self.set_box_allowed = true;
        }
    }

    /// §1275
    pub(crate) fn open_or_close_in(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        self.scan_four_bit_int()?;
        let n = ux(self.cur_val);
        if !T::VALUES {
            self.tracker.write(crate::track::Cell::Read(self.cur_val));
        }
        if self.read_open(n) != CLOSED {
            self.read_file[n] = None;
            self.set_read_open(n, CLOSED);
            self.read_file_wrote(n);
        }
        if c != 0 {
            self.scan_optional_equals()?;
            self.scan_file_name()?;
            self.pack_file_name(self.cur_name, self.cur_area, self.cur_ext);
            let name = self.name_of_file.clone();
            let found = self.read_source(&name, true);
            if let Some(f) = found {
                let name: alloc::sync::Arc<[u8]> = f.name.as_slice().into();
                if T::LINES {
                    self.tracker.lines_open(&f.contents);
                    if self.log_lines {
                        self.line_log.push((name.clone(), u32::MAX));
                    }
                }
                // (the file value: its contents' version, made at the load)
                self.read_file_loaded(n, &f.contents);
                self.read_file[n] = Some(AlphaFile {
                    data: f.contents,
                    name,
                    ..AlphaFile::default()
                });
                self.set_read_open(n, JUST_OPEN);
                self.read_file_wrote(n);
            }
        }
        Ok(())
    }

    /// §1279
    pub(crate) fn issue_message(&mut self) -> Result<(), Jump> {
        let c = self.cur_chr;
        self.scan_toks(false, true)?;
        let old_setting = self.selector();
        self.set_selector(NEW_STRING);
        self.message_printing = true;
        self.active_noconvert = true;
        let text = core::mem::take(&mut self.def_ref);
        self.token_show(&text);
        self.message_printing = false;
        self.active_noconvert = false;
        self.set_selector(old_setting);
        self.str_room(1)?;
        let s = self.make_string()?;
        if c == 0 {
            if self.notes_wanted() {
                let text = self.str_bytes(s).to_vec();
                self.diag_note(crate::diag::Severity::Note, "message", text, None);
            }
            // §1280: print string `s` on the terminal.
            let len = i32::try_from(self.length(s)).unwrap_or(i32::MAX);
            let (term_offset, file_offset) = self.offsets();
            if term_offset + len > self.params.max_print_line - 2 {
                self.print_ln();
            } else if term_offset > 0 || file_offset > 0 {
                self.print_char(b' ');
            }
            self.print(i32::try_from(s).unwrap_or(0));
            self.update_terminal();
        } else {
            // §1283: print string `s` as an error message.
            self.print_err(b"");
            self.print(i32::try_from(s).unwrap_or(0));
            if self.equiv_toks(ERR_HELP_LOC).is_some() {
                self.use_err_help = true;
            } else if self.long_help_seen() {
                self.help(&[b"(That was another \\errmessage.)"]);
            } else {
                if self.interaction() < ERROR_STOP_MODE {
                    self.set_long_help_seen(true);
                }
                self.help(&[
                    b"This error message was generated by an \\errmessage",
                    b"command, so I can't give any explicit help.",
                    b"Pretend that you're Hercule Poirot: Examine all clues,",
                    b"and deduce the truth by order and method.",
                ]);
            }
            self.error()?;
            self.use_err_help = false;
        }
        self.flush_string();
        Ok(())
    }

    /// §1288
    pub(crate) fn shift_case(&mut self) -> Result<(), Jump> {
        let b = self.cur_chr;
        self.scan_toks(false, false)?;
        let mut list = core::mem::take(&mut self.def_ref);
        for t in &mut list {
            // §1289: change the case of the token, if a change is
            // appropriate.
            if *t < CS_TOKEN_FLAG + SINGLE_BASE {
                let c = *t % 256;
                if self.equiv(b + c) != 0 {
                    *t = *t - c + self.equiv(b + c);
                }
            }
        }
        let list = self.make_list(list);
        self.back_list(list)
    }

    /// §1293
    pub(crate) fn show_whatever(&mut self) -> Result<(), Jump> {
        'common_ending: {
            match self.cur_chr {
                SHOW_LISTS_CODE => {
                    self.adjust_selector_for_show_stream();
                    self.begin_diagnostic();
                    self.show_activities();
                }
                SHOW_GROUPS => {
                    self.adjust_selector_for_show_stream();
                    self.begin_diagnostic();
                    self.show_save_groups();
                }
                SHOW_IFS => {
                    self.adjust_selector_for_show_stream();
                    self.begin_diagnostic();
                    self.show_ifs();
                }
                SHOW_BOX_CODE => {
                    // §1296: show the current contents of a box.
                    self.scan_register_num()?;
                    self.adjust_selector_for_show_stream();
                    self.begin_diagnostic();
                    self.print_nl(b"> \\box");
                    self.print_int(self.cur_val);
                    self.print_char(b'=');
                    match self.box_reg(self.cur_val).cloned() {
                        None => self.print_str(b"void"),
                        Some(b) => self.show_box_node(&b),
                    }
                }
                SHOW_CODE => {
                    // §1294: show the current meaning of a token, then
                    // `goto common_ending`.
                    self.get_token()?;
                    self.adjust_selector_for_show_stream();
                    self.print_nl(b"> ");
                    if self.cur_cs != 0 {
                        self.sprint_cs(self.cur_cs);
                        self.print_char(b'=');
                    }
                    self.print_meaning();
                    break 'common_ending;
                }
                _ => {
                    // §1297: show the current value of some parameter or
                    // register, then `goto common_ending`.
                    let p = self.the_toks()?;
                    self.adjust_selector_for_show_stream();
                    self.print_nl(b"> ");
                    self.token_show(&p);
                    self.release_list(p);
                    break 'common_ending;
                }
            }
            // §1298: complete a potentially long \show command.
            self.end_diagnostic(true);
            self.print_err(b"OK");
            if self.selector() == TERM_AND_LOG && self.tracing_online() <= 0 {
                self.set_selector(TERM_ONLY);
                self.print_str(b" (see the transcript file)");
                self.set_selector(TERM_AND_LOG);
            }
        }
        // common_ending:
        if self.selector() < NO_PRINT {
            // pdfTeX: shown into a `\showstream` file
            self.print_ln();
            let sel = if self.interaction() == BATCH_MODE {
                NO_PRINT
            } else {
                TERM_ONLY
            };
            self.set_selector(sel);
            if self.log_opened() {
                self.set_selector(self.selector() + 2);
            }
            return Ok(());
        }
        if self.interaction() < ERROR_STOP_MODE {
            self.help(&[]);
            self.set_error_count(self.error_count() - 1);
        } else if self.tracing_online() > 0 {
            self.help(&[
                b"This isn't an error message; I'm just \\showing something.",
                b"Type `I\\show...' to show more (e.g., \\show\\cs,",
                b"\\showthe\\count10, \\showbox255, \\showlists).",
            ]);
        } else {
            self.help(&[
                b"This isn't an error message; I'm just \\showing something.",
                b"Type `I\\show...' to show more (e.g., \\show\\cs,",
                b"\\showthe\\count10, \\showbox255, \\showlists).",
                b"And type `I\\tracingonline=1\\show...' to show boxes and",
                b"lists on your terminal as well as in the transcript file.",
            ]);
        }
        self.error()
    }

    /// pdfTeX: `\showstream`, if it names an open `\write` stream,
    /// receives what `\show...` shows.
    fn adjust_selector_for_show_stream(&mut self) {
        let s = self.int_par(SHOW_STREAM_CODE);
        if let Ok(n @ 0..16) = u8::try_from(s) {
            self.out_read(n);
        }
        if (0..NO_PRINT).contains(&s) && self.write_open(ux(s)) {
            self.set_selector(s);
        }
    }

    /// §218
    pub(crate) fn show_activities(&mut self) {
        self.print_nl(b"");
        self.print_ln();
        for p in (0..=self.nest_ptr()).rev() {
            let l = self.nest_at(p);
            let (m, ml, pg) = (l.mode, l.ml, l.pg);
            self.print_nl(b"### ");
            self.print_mode(m);
            self.print_str(b" entered at line ");
            self.print_int(ml.abs());
            if m == HMODE && pg != 0o40600000 {
                self.print_str(b" (language");
                self.print_int(pg % 0o200000);
                self.print_str(b":hyphenmin");
                self.print_int(pg / 0o20000000);
                self.print_char(b',');
                self.print_int((pg / 0o200000) % 0o100);
                self.print_char(b')');
            }
            if ml < 0 {
                self.print_str(b" (\\output routine)");
            }
            if p == 0 {
                // §986: show the status of the current page.
                self.show_page_status();
                if !self.nest_at(0).list.is_empty() {
                    self.print_nl(b"### recent contributions:");
                }
            }
            // The lists are cloned (cheaply: boxes are shared) so that
            // printing can borrow the engine.
            let l = self.nest_at(p);
            if m.abs() == MMODE {
                let list = l.mlist.clone();
                self.show_box_mlist(&list);
            } else {
                let list = l.list.to_vec();
                self.show_box(&list);
            }
            // §219: show the auxiliary field.
            let l = self.nest_at(p);
            match m.abs() / (MAX_COMMAND + 1) {
                0 => {
                    let pd = l.prev_depth;
                    self.print_nl(b"prevdepth ");
                    if pd <= IGNORE_DEPTH {
                        self.print_str(b"ignored");
                    } else {
                        self.print_scaled(pd);
                    }
                    if pg != 0 {
                        self.print_str(b", prevgraf ");
                        self.print_int(pg);
                        if pg == 1 {
                            self.print_str(b" line");
                        } else {
                            self.print_str(b" lines");
                        }
                    }
                }
                1 => {
                    let (sf, lang) = (l.space_factor, l.clang);
                    self.print_nl(b"spacefactor ");
                    self.print_int(sf);
                    if m > 0 && lang > 0 {
                        self.print_str(b", current language ");
                        self.print_int(lang);
                    }
                }
                2 => {
                    if let Some(n) = l.incompleat.clone() {
                        self.print_str(b"this will begin denominator of:");
                        self.show_box_mlist(&[partex_engine::math::Item::Noad(n)]);
                    }
                }
                _ => {} // there are no other cases
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::cell::RefCell;

    use crate::testing::{engine_with, feed};
    use crate::track::{Cell, Tracker};
    use crate::web::{BOX_VAL, WIDTH_OFFSET};
    use crate::xregs::reg_loc;

    /// The cells written.
    #[derive(Default)]
    struct Writes(RefCell<Vec<Cell>>);

    impl Tracker for Writes {
        fn read(&self, _: Cell) {}
        fn write(&self, c: Cell) {
            self.0.borrow_mut().push(c);
        }
    }

    /// §1247 changes the box a register holds in place: a write of the
    /// register, though its eqtb word stays (a region that sets `\wd`
    /// must say it wrote the box, or merging it with the region before
    /// keeps the box as that one left it).
    #[test]
    fn a_box_dimension_set_writes_its_register() {
        for n in [7, 300] {
            let mut t = engine_with(Writes::default());
            t.init_prim().unwrap();
            // (e-TeX's registers, as `etex.ch` sets them in extended mode)
            t.max_reg_num = 32767;
            let loc = reg_loc(BOX_VAL, n);
            t.set_box_reg(n, Some(Arc::new(partex_engine::node::BoxNode::default())));
            t.tracker.0.borrow_mut().clear();
            feed(&mut t, alloc::format!("{n}=5pt\n").as_bytes());
            t.cur_chr = WIDTH_OFFSET;
            t.alter_box_dimen().unwrap();
            assert_eq!(t.peek_box(loc).map(|b| b.width), Some(5 << 16));
            assert!(
                t.tracker.0.borrow().contains(&Cell::Eqtb(loc)),
                "\\wd{n} does not write box register {n}"
            );
        }
    }
}
