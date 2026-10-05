//! Part 26: Basic scanning subroutines (§402–§463), with the font-related
//! scanners of part 30 (§577–§579).

use crate::arith::Scaled;
use crate::fonts::fx;
use crate::host::Host;
use crate::input::ux;
use crate::nodes::{MU_GLUE, NORMAL};
use crate::tex::{Jump, Tex};
use crate::track::Tracker;
use crate::web::*;
use partex_engine::builder::Contents;
use partex_engine::node::{GlueSpec, Node, Order, RUNNING};

/// §421: $2^{30}-1$.
pub const MAX_DIMEN: Scaled = 0o7777777777;
/// §463: 0.4pt.
pub const DEFAULT_RULE: Scaled = 26214;
/// §101
const UNITY: Scaled = 0o200000;
/// §438
const OCTAL_TOKEN: i32 = OTHER_TOKEN + b'\'' as i32;
const HEX_TOKEN: i32 = OTHER_TOKEN + b'"' as i32;
const ALPHA_TOKEN: i32 = OTHER_TOKEN + b'`' as i32;
const POINT_TOKEN: i32 = OTHER_TOKEN + b'.' as i32;
const CONTINENTAL_POINT_TOKEN: i32 = OTHER_TOKEN + b',' as i32;
/// §445
const ZERO_TOKEN: i32 = OTHER_TOKEN + b'0' as i32;
const A_TOKEN: i32 = LETTER_TOKEN + b'A' as i32;
const OTHER_A_TOKEN: i32 = OTHER_TOKEN + b'A' as i32;

/// Where `scan_dimen` goes after scanning the units (tex.web's labels).
enum Units {
    AttachFraction,
    AttachSign,
    Done,
}

/// §150: the glue order of `glue_ord` value `o`.
pub(crate) fn order(o: i32) -> Order {
    Order::ALL[usize::try_from(o).unwrap_or(0).min(3)]
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// `XeTeX` §448: fetch a character code from some table (a math code
    /// as TeX's 15 bits, or an error).
    fn fetch_code_xetex(&mut self, m: i32) -> Result<(), Jump> {
        use crate::mathcodes::{
            is_active_math_char, math_char_field, math_class_field, math_fam_field,
        };
        self.scan_usv_num()?;
        let c = self.cur_val;
        self.cur_val = if m == MATH_CODE_BASE {
            let mut v = self.math_code(c);
            if is_active_math_char(v) {
                v = 0x8000;
            } else if math_class_field(v) > 7 || math_fam_field(v) > 15 || math_char_field(v) > 255
            {
                self.print_err(b"Extended mathchar used as mathchar");
                self.help(&[
                    b"A mathchar number must be between 0 and \"7FFF.",
                    b"I changed this one to zero.",
                ]);
                self.int_error(v)?;
                v = 0;
            }
            if v == 0x8000 {
                v
            } else {
                math_class_field(v) * 0x1000 + math_fam_field(v) * 0x100 + math_char_field(v)
            }
        } else if m == DEL_CODE_BASE {
            let v = self.del_code(c);
            if v >= 0x4000_0000 {
                self.print_err(b"Extended delcode used as delcode");
                self.help(&[
                    b"A delimiter code must be between 0 and \"7FFFFFF.",
                    b"I changed this one to zero.",
                ]);
                self.error()?;
                0
            } else {
                v
            }
        } else if m < SF_CODE_BASE {
            self.equiv(crate::wide::code_loc(m, c))
        } else if m < MATH_CODE_BASE {
            self.equiv(crate::wide::code_loc(m, c)) % 0x1_0000
        } else {
            self.eqtb_int(crate::wide::code_loc(m, c))
        };
        self.cur_val_level = INT_VAL;
        Ok(())
    }

    /// `XeTeX` §447: `\the\XeTeXcharclass`, `\Umathcodenum`,
    /// `\Udelcodenum` (and the errors of `\Umathcode`, `\Udelcode`).
    fn fetch_xetex_def_code(&mut self, m: i32) -> Result<(), Jump> {
        self.scan_usv_num()?;
        let c = self.cur_val;
        self.cur_val = if m == SF_CODE_BASE {
            self.sf_code(c) / 0x1_0000
        } else if m == MATH_CODE_BASE {
            self.math_code(c)
        } else if m == MATH_CODE_BASE + 1 {
            self.print_err(b"Can't use \\Umathcode as a number (try \\Umathcodenum)");
            self.help(&[
                b"\\Umathcode is for setting a mathcode from separate values;",
                b"use \\Umathcodenum to access them as single values.",
            ]);
            self.error()?;
            0
        } else if m == DEL_CODE_BASE {
            self.del_code(c)
        } else {
            self.print_err(b"Can't use \\Udelcode as a number (try \\Udelcodenum)");
            self.help(&[
                b"\\Udelcode is for setting a delcode from separate values;",
                b"use \\Udelcodenum to access them as single values.",
            ]);
            self.error()?;
            0
        };
        self.cur_val_level = INT_VAL;
        Ok(())
    }

    /// `XeTeX`: the character of a control sequence whose name is one
    /// character past U+FFFF (its surrogate pair).
    pub(crate) fn surrogate_name(&self, p: i32) -> Option<i32> {
        if p < crate::web::HASH_BASE || p >= crate::xregs::EXT_BASE {
            return None;
        }
        let m = usize::try_from(self.text(p)).ok()?;
        if m >= self.str_ptr {
            return None;
        }
        let mut u =
            crate::strings::decode_units(&self.str_pool[self.str_start[m]..self.str_start[m + 1]]);
        let (Some(hi), Some(lo), None) = (u.next(), u.next(), u.next()) else {
            return None;
        };
        ((0xD800..=0xDBFF).contains(&hi) && (0xDC00..=0xDFFF).contains(&lo))
            .then(|| i32::try_from(0x1_0000 + (hi - 0xD800) * 0x400 + lo - 0xDC00).unwrap_or(0))
    }

    /// §404: get the next non-blank non-relax non-call token.
    pub(crate) fn get_nonblank_nonrelax_noncall(&mut self) -> Result<(), Jump> {
        loop {
            self.get_x_token()?;
            if self.cur_cmd != SPACER && self.cur_cmd != RELAX {
                return Ok(());
            }
        }
    }

    /// §406: get the next non-blank non-call token.
    pub(crate) fn get_nonblank_noncall(&mut self) -> Result<(), Jump> {
        loop {
            self.get_x_token()?;
            if self.cur_cmd != SPACER {
                return Ok(());
            }
        }
    }

    /// §443: scan an optional space.
    pub(crate) fn scan_optional_space(&mut self) -> Result<(), Jump> {
        self.get_x_token()?;
        if self.cur_cmd != SPACER {
            self.back_input()?;
        }
        Ok(())
    }

    /// §403: read a mandatory `left_brace`.
    pub(crate) fn scan_left_brace(&mut self) -> Result<(), Jump> {
        self.get_nonblank_nonrelax_noncall()?;
        if self.cur_cmd != LEFT_BRACE {
            self.print_err(b"Missing { inserted");
            self.help(&[
                b"A left brace was mandatory here, so I've put one in.",
                b"You might want to delete and/or insert some corrections",
                b"so that I will find a matching right brace soon.",
                b"(If you're confused by all this, try typing `I}' now.)",
            ]);
            self.back_error()?;
            self.cur_tok = LEFT_BRACE_TOKEN + i32::from(b'{');
            self.cur_cmd = LEFT_BRACE;
            self.cur_chr = i32::from(b'{');
            self.set_align_state(self.align_state() + 1);
        }
        Ok(())
    }

    /// §405
    pub(crate) fn scan_optional_equals(&mut self) -> Result<(), Jump> {
        self.get_nonblank_noncall()?;
        if self.cur_tok != OTHER_TOKEN + i32::from(b'=') {
            self.back_input()?;
        }
        Ok(())
    }

    /// §407: look for a given (lowercase) keyword.
    pub(crate) fn scan_keyword(&mut self, s: &[u8]) -> Result<bool, Jump> {
        // The matched tokens (tex.web keeps them at `backup_head`, saved
        // around `expand` because of recursion; a local buffer needs
        // neither). Keywords are short: no allocation.
        let mut matched = [0; 32];
        assert!(s.len() <= matched.len(), "keyword too long");
        let mut k = 0;
        while k < s.len() {
            self.get_x_token()?; // recursion is possible here
            let c = i32::from(s[k]);
            if self.cur_cs == 0
                && (self.cur_chr == c || self.cur_chr == c - i32::from(b'a') + i32::from(b'A'))
            {
                matched[k] = self.cur_tok;
                k += 1;
            } else if self.cur_cmd != SPACER || k > 0 {
                self.back_input()?;
                if k > 0 {
                    let p = self.tok_from(&matched[..k]);
                    self.back_list(p)?;
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// §408
    pub(crate) fn mu_error(&mut self) -> Result<(), Jump> {
        self.print_err(b"Incompatible glue units");
        self.help(&[b"I'm going to assume that 1mu=1pt when they're mixed."]);
        self.error()
    }

    /// §413: fetch an internal parameter.
    /// Fetch the glue of eqtb location `loc` into `cur_glue`, keeping its
    /// lineage.
    fn fetch_glue(&mut self, loc: i32) {
        self.cur_glue = self.glue_at(loc);
        self.glue_origin = Some(self.glue_value(loc).lineage);
    }

    pub(crate) fn scan_something_internal(
        &mut self,
        level: i32,
        negative: bool,
    ) -> Result<(), Jump> {
        if self.cur_cmd == IGNORE_SPACES && self.cur_chr == 1 {
            // pdfTeX §439: trap unexpandable primitives
            self.reset_primitive_tok()?;
        }
        if self.memo.recording() && !Self::memo_internal_ok(self.cur_cmd, self.cur_chr) {
            self.memo.impure_cmd(self.cur_cmd);
        }
        let mut m = self.cur_chr;
        match self.cur_cmd {
            DEF_CODE if self.unicode => self.fetch_code_xetex(m)?,
            XETEX_DEF_CODE => self.fetch_xetex_def_code(m)?,
            DEF_CODE => {
                // §414: fetch a character code from some table.
                self.scan_char_num()?;
                let v = ux(self.cur_val);
                self.cur_val = if m == XORD_CODE_BASE {
                    i32::from(self.xord[v])
                } else if m == XCHR_CODE_BASE {
                    i32::from(self.xchr[v])
                } else if m == XPRN_CODE_BASE {
                    i32::from(self.xprn[v])
                } else if m == MATH_CODE_BASE {
                    self.math_code(self.cur_val)
                } else if m < MATH_CODE_BASE {
                    self.equiv(m + self.cur_val)
                } else {
                    self.eqtb_int(m + self.cur_val)
                };
                self.cur_val_level = INT_VAL;
            }
            TOKS_REGISTER | ASSIGN_TOKS | DEF_FAMILY | SET_FONT | DEF_FONT | LETTERSPACE_FONT
            | PDF_COPY_FONT => {
                // §415: fetch a token list or font identifier, provided
                // that `level=tok_val`.
                if level != TOK_VAL {
                    self.print_err(b"Missing number, treated as zero");
                    self.help(&[
                        b"A number should have been here; I inserted `0'.",
                        b"(If you can't figure out why I needed to see a number,",
                        b"look up `weird error' in the index to The TeXbook.)",
                    ]);
                    self.back_error()?;
                    self.cur_val = 0;
                    self.cur_val_level = DIMEN_VAL;
                } else if self.cur_cmd <= ASSIGN_TOKS {
                    if self.cur_cmd < ASSIGN_TOKS {
                        // `cur_cmd=toks_register`
                        self.scan_register_num()?;
                        m = crate::xregs::reg_loc(TOK_VAL, self.cur_val);
                    } else if m == crate::web::XETEX_INTER_CHAR_LOC {
                        m = self.scan_inter_char_loc()?;
                    }
                    // (the list itself, a shared value: `cur_toks`)
                    self.cur_toks = self.equiv_toks(m).cloned();
                    self.cur_val = if self.cur_toks.is_some() { 1 } else { NULL };
                    self.cur_val_level = TOK_VAL;
                } else {
                    self.back_input()?;
                    self.scan_font_ident()?;
                    self.cur_val += FONT_ID_BASE;
                    self.cur_val_level = IDENT_VAL;
                }
            }
            ASSIGN_INT => {
                self.cur_val = self.eqtb_int(m);
                self.cur_val_level = INT_VAL;
            }
            ASSIGN_DIMEN => {
                self.cur_val = self.eqtb_int(m);
                self.cur_val_level = DIMEN_VAL;
            }
            ASSIGN_GLUE => {
                self.fetch_glue(m);
                self.cur_val_level = GLUE_VAL;
            }
            ASSIGN_MU_GLUE => {
                self.fetch_glue(m);
                self.cur_val_level = MU_VAL;
            }
            SET_AUX => {
                // §418: fetch the `space_factor` or the `prev_depth`.
                if self.mode().abs() != m {
                    self.print_err(b"Improper ");
                    self.print_cmd_chr(SET_AUX, m);
                    self.help(&[
                        b"You can refer to \\spacefactor only in horizontal mode;",
                        b"you can refer to \\prevdepth only in vertical mode; and",
                        b"neither of these is meaningful inside \\write. So",
                        b"I'm forgetting what you said and using zero instead.",
                    ]);
                    self.error()?;
                    self.cur_val = 0;
                    self.cur_val_level = if level == TOK_VAL { INT_VAL } else { DIMEN_VAL };
                } else if m == VMODE {
                    self.cur_val = self.prev_depth();
                    self.cur_val_level = DIMEN_VAL;
                } else {
                    self.cur_val = self.space_factor();
                    self.cur_val_level = INT_VAL;
                }
            }
            SET_PREV_GRAF => {
                // §422: fetch the `prev_graf`.
                if self.mode() == 0 {
                    self.cur_val = 0; // `prev_graf=0` within \write
                } else {
                    let mut p = self.nest_ptr();
                    while self.level_mode(p).abs() != VMODE {
                        p -= 1;
                    }
                    self.cur_val = self.level_pg(p);
                }
                self.cur_val_level = INT_VAL;
            }
            SET_PAGE_INT => {
                // §419: fetch the `dead_cycles` or the `insert_penalties`.
                self.cur_val = match m {
                    0 => self.dead_cycles(),
                    1 => self.insert_penalties(),
                    _ => self.interaction(),
                };
                self.cur_val_level = INT_VAL;
            }
            SET_PAGE_DIMEN => {
                // §421: fetch something on the `page_so_far`.
                self.cur_val = if self.page_contents() == Contents::Empty && !self.output_active() {
                    if m == 0 { MAX_DIMEN } else { 0 }
                } else {
                    self.page_so_far(ux(m))
                };
                self.cur_val_level = DIMEN_VAL;
            }
            SET_SHAPE if m > PAR_SHAPE_LOC => {
                // e-TeX: fetch a penalties array element (0: the count).
                self.scan_int()?;
                let k = self.cur_val;
                self.cur_val = match self.penalties(m) {
                    Some(p) if k >= 0 => {
                        let n = i32::try_from(p.len()).unwrap_or(i32::MAX);
                        match k.min(n) {
                            0 => n,
                            k => p[ux(k - 1)],
                        }
                    }
                    _ => 0,
                };
                self.cur_val_level = INT_VAL;
            }
            SET_SHAPE => {
                // §423: fetch the `par_shape` size.
                self.cur_val = self
                    .par_shape()
                    .map_or(0, |s| i32::try_from(s.len()).unwrap_or(i32::MAX));
                self.cur_val_level = INT_VAL;
            }
            SET_BOX_DIMEN => {
                // §420: fetch a box dimension.
                self.scan_register_num()?;
                self.cur_val = self.box_reg(self.cur_val).map_or(0, |b| match m {
                    WIDTH_OFFSET => b.width,
                    DEPTH_OFFSET => b.depth,
                    _ => b.height,
                });
                self.cur_val_level = DIMEN_VAL;
            }
            CHAR_GIVEN | MATH_GIVEN => {
                self.cur_val = self.cur_chr;
                self.cur_val_level = INT_VAL;
            }
            ASSIGN_FONT_DIMEN => {
                // §425: fetch a font dimension.
                self.cur_val = match self.find_font_dimen(false)? {
                    Some((f, n)) => self.fonts.get(f).param(n),
                    None => 0,
                };
                self.cur_val_level = DIMEN_VAL;
            }
            ASSIGN_FONT_INT => {
                // §426: fetch a font integer.
                self.cur_val = self.fetch_font_int(m)?;
                self.cur_val_level = INT_VAL;
            }
            REGISTER => {
                // §427: fetch a register.
                self.scan_register_num()?;
                match m {
                    INT_VAL => self.cur_val = self.count(self.cur_val),
                    DIMEN_VAL => self.cur_val = self.dimen(self.cur_val),
                    _ => self.fetch_glue(crate::xregs::reg_loc(m, self.cur_val)),
                }
                self.cur_val_level = m;
            }
            LAST_ITEM => self.fetch_last_item()?,
            _ => {
                // §428: complain that \the can't do this; give zero result.
                self.print_err(b"You can't use `");
                self.print_cmd_chr(self.cur_cmd, self.cur_chr);
                self.print_str(b"' after ");
                self.print_esc(b"the");
                self.help(&[b"I'm forgetting what you said and using zero instead."]);
                self.error()?;
                self.cur_val = 0;
                self.cur_val_level = if level == TOK_VAL { INT_VAL } else { DIMEN_VAL };
            }
        }
        while self.cur_val_level > level {
            // §429: convert `cur_val` to a lower level.
            if self.cur_val_level == GLUE_VAL {
                self.cur_val = self.cur_glue.width;
            } else if self.cur_val_level == MU_VAL {
                self.mu_error()?;
            }
            self.cur_val_level -= 1;
        }
        // §430: negate `cur_val` if `negative`.
        if negative {
            if self.cur_val_level >= GLUE_VAL {
                // §431: negate all three glue components of `cur_val`.
                self.glue_origin = None;
                let g = &mut self.cur_glue;
                *g = g.copy();
                g.width = -g.width;
                g.stretch = -g.stretch;
                g.shrink = -g.shrink;
            } else {
                self.cur_val = -self.cur_val;
            }
        }
        Ok(())
    }

    /// §424: fetch an item in the current node, if appropriate.
    fn fetch_last_item(&mut self) -> Result<(), Jump> {
        let m = self.cur_chr;
        if m >= INPUT_LINE_NO_CODE {
            if m >= XETEX_INT {
                return self.fetch_xetex_item(m);
            }
            if m >= ETEX_GLUE {
                return self.fetch_etex_glue(m);
            }
            if m >= ETEX_DIM {
                self.fetch_etex_dimen(m)?;
                self.cur_val_level = DIMEN_VAL;
                return Ok(());
            }
            self.cur_val = match m {
                INPUT_LINE_NO_CODE => {
                    if T::VALUES {
                        self.tracker.line_number(self.in_open, self.line);
                    }
                    self.line
                }
                BADNESS_CODE => self.last_badness(),
                _ => self.fetch_etex_int(m)?,
            };
            self.cur_val_level = INT_VAL;
            return Ok(());
        }
        self.cur_val = 0;
        self.cur_glue = GlueSpec::ZERO_GLUE;
        // e-TeX: the effective tail skips a final `\endM`
        let (tail, at_head) = self.effective_tail();
        if m == LAST_NODE_TYPE_CODE {
            self.cur_val_level = INT_VAL;
            if at_head || self.mode() == 0 {
                self.cur_val = -1;
            }
        } else {
            self.cur_val_level = m;
        }
        if self.mode() == 0 {
            return Ok(());
        }
        if self.mode() == VMODE && at_head {
            match m {
                INT_VAL => self.cur_val = self.page_last_penalty(),
                DIMEN_VAL => self.cur_val = self.page_last_kern(),
                GLUE_VAL => {
                    if let Some(g) = self.page_last_glue() {
                        self.cur_glue = g;
                    }
                }
                _ => self.cur_val = self.page_last_node_type(),
            }
            return Ok(());
        }
        if m == LAST_NODE_TYPE_CODE {
            if !at_head {
                self.cur_val = match &tail {
                    // a character, or a noad in math mode
                    Some(Node::Glyphs(_)) => 0,
                    Some(n) => match partex_engine::builder::type_code(n) {
                        t if t <= UNSET_NODE => t + 1,
                        _ => UNSET_NODE + 2,
                    },
                    None => UNSET_NODE + 2,
                };
            }
            return Ok(());
        }
        match (m, tail) {
            (INT_VAL, Some(Node::Penalty(p))) => self.cur_val = p,
            (DIMEN_VAL, Some(Node::Kern { width, .. })) => self.cur_val = width,
            (GLUE_VAL, Some(Node::Glue { spec, subtype, .. })) => {
                self.cur_glue = spec;
                if i32::from(subtype) == MU_GLUE {
                    self.cur_val_level = MU_VAL;
                }
            }
            (GLUE_VAL, Some(Node::Leaders(l))) => self.cur_glue = l.spec,
            _ => {}
        }
        Ok(())
    }

    /// The common shape of §433–§437: scan an integer in `0..=max`.
    fn scan_limited_int(
        &mut self,
        max: i32,
        what: &'static [u8],
        help: [&'static [u8]; 2],
    ) -> Result<(), Jump> {
        self.scan_int()?;
        if self.cur_val < 0 || self.cur_val > max {
            self.print_err(what);
            self.help(&help);
            self.int_error(self.cur_val)?;
            self.cur_val = 0;
        }
        Ok(())
    }

    /// §433
    pub(crate) fn scan_eight_bit_int(&mut self) -> Result<(), Jump> {
        let help: &[u8] = if self.unicode {
            b"A register code or char class must be between 0 and 255."
        } else {
            b"A register number must be between 0 and 255."
        };
        self.scan_limited_int(
            255,
            b"Bad register code",
            [help, b"I changed this one to zero."],
        )
    }

    /// `XeTeX` §437 `scan_usv_num`: a Unicode scalar value (TeX's
    /// `scan_char_num` in the other flavors).
    pub(crate) fn scan_usv_num(&mut self) -> Result<(), Jump> {
        if !self.unicode {
            return self.scan_char_num();
        }
        self.scan_limited_int(
            BIGGEST_USV,
            b"Bad character code",
            [
                b"A Unicode scalar value must be between 0 and \"10FFFF.",
                b"I changed this one to zero.",
            ],
        )
    }

    /// `XeTeX` §436 `scan_char_class`: 0..=4096 (4096: ignored).
    pub(crate) fn scan_char_class(&mut self) -> Result<(), Jump> {
        self.scan_limited_int(
            crate::web::CHAR_CLASS_LIMIT,
            b"Bad character class",
            [
                b"A character class must be between 0 and 4096.",
                b"I changed this one to zero.",
            ],
        )
    }

    /// `XeTeX` §436 `scan_char_class_not_ignored` (its range is the same).
    pub(crate) fn scan_char_class_not_ignored(&mut self) -> Result<(), Jump> {
        self.scan_limited_int(
            crate::web::CHAR_CLASS_LIMIT,
            b"Bad character class",
            [
                b"A class for inter-character transitions must be between 0 and 4095.",
                b"I changed this one to zero.",
            ],
        )
    }

    /// `XeTeX`: the two classes after `\XeTeXinterchartoks`, as the
    /// location of their token list.
    pub(crate) fn scan_inter_char_loc(&mut self) -> Result<i32, Jump> {
        self.scan_char_class_not_ignored()?;
        let c1 = self.cur_val;
        self.scan_char_class_not_ignored()?;
        Ok(crate::wide::inter_char_loc(c1, self.cur_val))
    }

    /// `XeTeX` §438 `scan_xetex_math_char_int`: a `\Umathchar` number.
    pub(crate) fn scan_xetex_math_char_int(&mut self) -> Result<(), Jump> {
        use crate::mathcodes::{ACTIVE_MATH_CHAR, is_active_math_char, math_char_field};
        self.scan_int()?;
        if is_active_math_char(self.cur_val) {
            if self.cur_val != ACTIVE_MATH_CHAR {
                self.print_err(b"Bad active XeTeX math code");
                self.help(&[
                    b"Since I ignore class and family for active math chars,",
                    b"I changed this one to \"1FFFFF.",
                ]);
                self.int_error(self.cur_val)?;
                self.cur_val = ACTIVE_MATH_CHAR;
            }
        } else if math_char_field(self.cur_val) > BIGGEST_USV {
            self.print_err(b"Bad XeTeX math character code");
            self.help(&[
                b"Since I expected a character number between 0 and \"10FFFF,",
                b"I changed this one to zero.",
            ]);
            self.int_error(self.cur_val)?;
            self.cur_val = 0;
        }
        Ok(())
    }

    /// `XeTeX` §438 `scan_math_class_int`: 0..=7.
    pub(crate) fn scan_math_class_int(&mut self) -> Result<(), Jump> {
        self.scan_limited_int(
            7,
            b"Bad math class",
            [
                b"Since I expected to read a number between 0 and 7,",
                b"I changed this one to zero.",
            ],
        )
    }

    /// `XeTeX` §438 `scan_math_fam_int`: a family, 0..=255 (0..=15 in the
    /// other flavors, as `scan_four_bit_int`).
    pub(crate) fn scan_math_fam_int(&mut self) -> Result<(), Jump> {
        if !self.unicode {
            return self.scan_four_bit_int();
        }
        self.scan_limited_int(
            crate::web::NUMBER_MATH_FAMILIES - 1,
            b"Bad math family",
            [
                b"Since I expected to read a number between 0 and 255,",
                b"I changed this one to zero.",
            ],
        )
    }

    /// e-TeX: a register number, at most 255 in compatibility mode and
    /// 32767 in extended mode.
    pub(crate) fn scan_register_num(&mut self) -> Result<(), Jump> {
        let help = self.max_reg_help_line;
        self.scan_limited_int(
            self.max_reg_num,
            b"Bad register code",
            [help, b"I changed this one to zero."],
        )
    }

    /// §434 (`XeTeX` §437: up to 65535)
    pub(crate) fn scan_char_num(&mut self) -> Result<(), Jump> {
        if self.unicode {
            return self.scan_limited_int(
                crate::wide::XETEX_BIGGEST_CHAR,
                b"Bad character code",
                [
                    b"A character number must be between 0 and 65535.",
                    b"I changed this one to zero.",
                ],
            );
        }
        self.scan_limited_int(
            255,
            b"Bad character code",
            [
                b"A character number must be between 0 and 255.",
                b"I changed this one to zero.",
            ],
        )
    }

    /// §435
    pub(crate) fn scan_four_bit_int(&mut self) -> Result<(), Jump> {
        self.scan_limited_int(
            15,
            b"Bad number",
            [
                b"Since I expected to read a number between 0 and 15,",
                b"I changed this one to zero.",
            ],
        )
    }

    /// §436
    pub(crate) fn scan_fifteen_bit_int(&mut self) -> Result<(), Jump> {
        self.scan_limited_int(
            0o77777,
            b"Bad mathchar",
            [
                b"A mathchar number must be between 0 and 32767.",
                b"I changed this one to zero.",
            ],
        )
    }

    /// §437
    pub(crate) fn scan_twenty_seven_bit_int(&mut self) -> Result<(), Jump> {
        self.scan_limited_int(
            0o777777777,
            b"Bad delimiter code",
            [
                b"A numeric delimiter code must be between 0 and 2^{27}-1.",
                b"I changed this one to zero.",
            ],
        )
    }

    /// web2c: `scan_four_bit_int` that also accepts 18 (`\write18`).
    pub(crate) fn scan_four_bit_int_or_18(&mut self) -> Result<(), Jump> {
        self.scan_int()?;
        if self.cur_val < 0 || (self.cur_val > 15 && self.cur_val != 18) {
            self.print_err(b"Bad number");
            self.help(&[
                b"Since I expected to read a number between 0 and 15,",
                b"I changed this one to zero.",
            ]);
            self.int_error(self.cur_val)?;
            self.cur_val = 0;
        }
        Ok(())
    }

    /// §441: get the next non-blank non-sign token; return `negative`.
    fn scan_sign(&mut self) -> Result<bool, Jump> {
        let mut negative = false;
        loop {
            self.get_nonblank_noncall()?;
            if self.cur_tok == OTHER_TOKEN + i32::from(b'-') {
                negative = !negative;
                self.cur_tok = OTHER_TOKEN + i32::from(b'+');
            }
            if self.cur_tok != OTHER_TOKEN + i32::from(b'+') {
                return Ok(negative);
            }
        }
    }

    /// §440: set `cur_val` to an integer.
    pub(crate) fn scan_int(&mut self) -> Result<(), Jump> {
        self.radix = 0;
        let mut ok_so_far = true;
        let negative = self.scan_sign()?;
        if self.cur_tok == CS_TOKEN_FLAG + FROZEN_PRIMITIVE {
            // pdfTeX §466
            self.reset_primitive_tok()?;
        }
        if self.cur_tok == ALPHA_TOKEN {
            // §442: scan an alphabetic character code into `cur_val`.
            self.get_token()?; // suppress macro expansion
            if self.cur_tok < CS_TOKEN_FLAG {
                self.cur_val = self.cur_chr;
                if self.cur_cmd <= RIGHT_BRACE {
                    if self.cur_cmd == RIGHT_BRACE {
                        self.set_align_state(self.align_state() + 1);
                    } else {
                        self.set_align_state(self.align_state() - 1);
                    }
                }
            } else if self.unicode {
                // `XeTeX` §476 (a name of one character past U+FFFF is a
                // multi-letter name: its surrogate pair)
                let p = self.cur_tok - CS_TOKEN_FLAG;
                self.cur_val = match (crate::wide::active_char(p), crate::wide::single_char(p)) {
                    (Some(c), _) | (_, Some(c)) => c,
                    _ => self.surrogate_name(p).unwrap_or(crate::web::TOO_BIG_USV),
                };
            } else if self.cur_tok < CS_TOKEN_FLAG + SINGLE_BASE {
                self.cur_val = self.cur_tok - CS_TOKEN_FLAG - ACTIVE_BASE;
            } else {
                self.cur_val = self.cur_tok - CS_TOKEN_FLAG - SINGLE_BASE;
            }
            let biggest = if self.unicode {
                crate::web::BIGGEST_USV
            } else {
                255
            };
            if self.cur_val > biggest {
                self.print_err(b"Improper alphabetic constant");
                self.help(&[
                    b"A one-character control sequence belongs after a ` mark.",
                    b"So I'm essentially inserting \\0 here.",
                ]);
                self.cur_val = i32::from(b'0');
                self.back_error()?;
            } else {
                self.scan_optional_space()?;
            }
        } else if self.cur_cmd >= MIN_INTERNAL && self.cur_cmd <= MAX_INTERNAL {
            self.scan_something_internal(INT_VAL, false)?;
        } else {
            // §444: scan a numeric constant.
            self.radix = 10;
            let mut m = 214_748_364;
            if self.cur_tok == OCTAL_TOKEN {
                self.radix = 8;
                m = 0o2000000000;
                self.get_x_token()?;
            } else if self.cur_tok == HEX_TOKEN {
                self.radix = 16;
                m = 0o1000000000;
                self.get_x_token()?;
            }
            let mut vacuous = true;
            self.cur_val = 0;
            // §445: accumulate the constant until `cur_tok` is not a
            // suitable digit.
            loop {
                let t = self.cur_tok;
                let d = if t < ZERO_TOKEN + self.radix && t >= ZERO_TOKEN && t <= ZERO_TOKEN + 9 {
                    t - ZERO_TOKEN
                } else if self.radix == 16 {
                    if t <= A_TOKEN + 5 && t >= A_TOKEN {
                        t - A_TOKEN + 10
                    } else if t <= OTHER_A_TOKEN + 5 && t >= OTHER_A_TOKEN {
                        t - OTHER_A_TOKEN + 10
                    } else {
                        break;
                    }
                } else {
                    break;
                };
                vacuous = false;
                if self.cur_val >= m && (self.cur_val > m || d > 7 || self.radix != 10) {
                    if ok_so_far {
                        self.print_err(b"Number too big");
                        self.help(&[
                            b"I can only go up to 2147483647='17777777777=\"7FFFFFFF,",
                            b"so I'm using that number instead of yours.",
                        ]);
                        self.error()?;
                        self.cur_val = INFINITY;
                        ok_so_far = false;
                    }
                } else {
                    self.cur_val = self.cur_val * self.radix + d;
                }
                self.get_x_token()?;
            }
            if vacuous {
                // §446: express astonishment that no number was here.
                self.print_err(b"Missing number, treated as zero");
                self.help(&[
                    b"A number should have been here; I inserted `0'.",
                    b"(If you can't figure out why I needed to see a number,",
                    b"look up `weird error' in the index to The TeXbook.)",
                ]);
                self.back_error()?;
            } else if self.cur_cmd != SPACER {
                self.back_input()?;
            }
        }
        if negative {
            self.cur_val = -self.cur_val;
        }
        Ok(())
    }

    /// §448: `scan_normal_dimen`.
    pub(crate) fn scan_normal_dimen(&mut self) -> Result<(), Jump> {
        self.scan_dimen(false, false, false)
    }

    /// §448: set `cur_val` to a dimension. `shortcut`: `cur_val` already
    /// holds an integer to be multiplied by the units.
    pub(crate) fn scan_dimen(&mut self, mu: bool, inf: bool, shortcut: bool) -> Result<(), Jump> {
        let mut f = 0;
        self.arith_error = false;
        self.cur_order = NORMAL;
        let mut negative = false;
        let mut attach_sign = false;
        if !shortcut {
            negative = self.scan_sign()?;
            if self.cur_cmd >= MIN_INTERNAL && self.cur_cmd <= MAX_INTERNAL {
                // §449: fetch an internal dimension and `goto attach_sign`,
                // or fetch an internal integer.
                if mu {
                    self.scan_something_internal(MU_VAL, false)?;
                    if self.cur_val_level != INT_VAL {
                        self.coerce_glue();
                        if self.cur_val_level != MU_VAL {
                            self.mu_error()?;
                        }
                        attach_sign = true;
                    }
                } else {
                    self.scan_something_internal(DIMEN_VAL, false)?;
                    if self.cur_val_level == DIMEN_VAL {
                        attach_sign = true;
                    }
                }
            } else {
                self.back_input()?;
                if self.cur_tok == CONTINENTAL_POINT_TOKEN {
                    self.cur_tok = POINT_TOKEN;
                }
                if self.cur_tok == POINT_TOKEN {
                    self.radix = 10;
                    self.cur_val = 0;
                } else {
                    self.scan_int()?;
                }
                if self.cur_tok == CONTINENTAL_POINT_TOKEN {
                    self.cur_tok = POINT_TOKEN;
                }
                if self.radix == 10 && self.cur_tok == POINT_TOKEN {
                    f = self.scan_decimal_fraction()?;
                }
            }
        }
        if !attach_sign {
            if self.cur_val < 0 {
                // in this case `f=0`
                negative = !negative;
                self.cur_val = -self.cur_val;
            }
            // §453: scan units and set `cur_val` to x*(cur_val+f/2^16).
            match self.scan_units(mu, inf, &mut f)? {
                Units::AttachSign => {}
                u => {
                    if let Units::AttachFraction = u {
                        if self.cur_val >= 0o40000 {
                            self.arith_error = true;
                        } else {
                            self.cur_val = self.cur_val * UNITY + f;
                        }
                    }
                    // done:
                    self.scan_optional_space()?;
                }
            }
        }
        // attach_sign:
        if self.arith_error || self.cur_val.abs() >= 0o10000000000 {
            // §460: report that this dimension is out of range.
            self.print_err(b"Dimension too large");
            self.help(&[
                b"I can't work with sizes bigger than about 19 feet.",
                b"Continue and I'll use the largest value I can.",
            ]);
            self.error()?;
            self.cur_val = MAX_DIMEN;
            self.arith_error = false;
        }
        if negative {
            self.cur_val = -self.cur_val;
        }
        Ok(())
    }

    /// §451: coerce glue to a dimension.
    fn coerce_glue(&mut self) {
        if self.cur_val_level >= GLUE_VAL {
            self.cur_val = self.cur_glue.width;
        }
    }

    /// §452: scan decimal fraction; returns `f`.
    fn scan_decimal_fraction(&mut self) -> Result<i32, Jump> {
        // (tex.web keeps the digits in a list, not in `dig`: expanding
        // the next token can scan another dimension)
        let mut digits = [0u8; 17];
        let mut k = 0usize;
        self.get_token()?; // `point_token` is being re-scanned
        loop {
            self.get_x_token()?;
            if self.cur_tok > ZERO_TOKEN + 9 || self.cur_tok < ZERO_TOKEN {
                break;
            }
            if k < 17 {
                // digits for `k>=17` cannot affect the result
                digits[k] = u8::try_from(self.cur_tok - ZERO_TOKEN).unwrap_or(0);
                k += 1;
            }
        }
        self.dig[..k].copy_from_slice(&digits[..k]);
        let f = self.round_decimals(k);
        if self.cur_cmd != SPACER {
            self.back_input()?;
        }
        Ok(f)
    }

    /// §453: the units part of `scan_dimen`.
    fn scan_units(&mut self, mu: bool, inf: bool, f: &mut i32) -> Result<Units, Jump> {
        if inf {
            // §454: scan for fil units; `goto attach_fraction` if found.
            if self.scan_keyword(b"fil")? {
                self.cur_order = FIL;
                while self.scan_keyword(b"l")? {
                    if self.cur_order == FILLL {
                        self.print_err(b"Illegal unit of measure (");
                        self.print_str(b"replaced by filll)");
                        self.help(&[b"I dddon't go any higher than filll."]);
                        self.error()?;
                    } else {
                        self.cur_order += 1;
                    }
                }
                return Ok(Units::AttachFraction);
            }
        }
        // §455: scan for units that are internal dimensions; `goto
        // attach_sign` with `cur_val` set if found.
        let save_cur_val = self.cur_val;
        self.get_nonblank_noncall()?;
        let v;
        if self.cur_cmd < MIN_INTERNAL || self.cur_cmd > MAX_INTERNAL {
            self.back_input()?;
            if mu {
                return self.scan_mu_units();
            }
            if self.scan_keyword(b"em")? {
                v = self.font_param(QUAD_CODE, self.cur_font()); // §558
            } else if self.scan_keyword(b"ex")? {
                v = self.font_param(X_HEIGHT_CODE, self.cur_font()); // §559
            } else {
                return self.scan_other_units(f);
            }
            self.scan_optional_space()?;
        } else {
            if mu {
                self.scan_something_internal(MU_VAL, false)?;
                self.coerce_glue();
                if self.cur_val_level != MU_VAL {
                    self.mu_error()?;
                }
            } else {
                self.scan_something_internal(DIMEN_VAL, false)?;
            }
            v = self.cur_val;
        }
        // found:
        let frac = self.xn_over_d(v, *f, 0o200000);
        self.cur_val = self.nx_plus_y(save_cur_val, v, frac);
        Ok(Units::AttachSign)
    }

    /// §456: scan for mu units and `goto attach_fraction`.
    fn scan_mu_units(&mut self) -> Result<Units, Jump> {
        if !self.scan_keyword(b"mu")? {
            self.print_err(b"Illegal unit of measure (");
            self.print_str(b"mu inserted)");
            self.help(&[
                b"The unit of measurement in math glue must be mu.",
                b"To recover gracefully from this error, it's best to",
                b"delete the erroneous units; e.g., type `2' to delete",
                b"two letters. (See Chapter 27 of The TeXbook.)",
            ]);
            self.error()?;
        }
        Ok(Units::AttachFraction)
    }

    /// §453 after `not_found`: `true`, `pt`, and all other units.
    fn scan_other_units(&mut self, f: &mut i32) -> Result<Units, Jump> {
        if self.scan_keyword(b"true")? {
            // §457: adjust for the magnification ratio.
            self.prepare_mag()?;
            let mag = self.int_par(MAG_CODE);
            if mag != 1000 {
                self.cur_val = self.xn_over_d(self.cur_val, 1000, mag);
                *f = (1000 * *f + 0o200000 * self.remainder) / mag;
                self.cur_val += *f / 0o200000;
                *f %= 0o200000;
            }
        }
        if self.scan_keyword(b"pt")? {
            return Ok(Units::AttachFraction); // the easy case
        }
        // §458: scan for all other units and adjust `cur_val` and `f`
        // accordingly; `goto done` in the case of scaled points.
        let pdftex = self.params.flavor == crate::params::Flavor::PdfTex;
        let (num, denom) = if self.scan_keyword(b"in")? {
            (7227, 100)
        } else if self.scan_keyword(b"pc")? {
            (12, 1)
        } else if self.scan_keyword(b"cm")? {
            (7227, 254)
        } else if self.scan_keyword(b"mm")? {
            (7227, 2540)
        } else if self.scan_keyword(b"bp")? {
            (7227, 7200)
        } else if self.scan_keyword(b"dd")? {
            (1238, 1157)
        } else if self.scan_keyword(b"cc")? {
            (14856, 1157)
        } else if pdftex && self.scan_keyword(b"nd")? {
            (685, 642) // pdfTeX: the new didot
        } else if pdftex && self.scan_keyword(b"nc")? {
            (1370, 107) // pdfTeX: the new cicero
        } else if self.scan_keyword(b"sp")? {
            return Ok(Units::Done);
        } else {
            // §459: complain about unknown unit and `goto done2`.
            self.print_err(b"Illegal unit of measure (");
            self.print_str(b"pt inserted)");
            self.help(&[
                b"Dimensions can be in units of em, ex, in, pt, pc,",
                if pdftex {
                    b"cm, mm, dd, cc, nd, nc, bp, or sp; but yours is a new one!"
                } else {
                    b"cm, mm, dd, cc, bp, or sp; but yours is a new one!"
                },
                b"I'll assume that you meant to say pt, for printer's points.",
                b"To recover gracefully from this error, it's best to",
                b"delete the erroneous units; e.g., type `2' to delete",
                b"two letters. (See Chapter 27 of The TeXbook.)",
            ]);
            self.error()?;
            return Ok(Units::AttachFraction);
        };
        self.cur_val = self.xn_over_d(self.cur_val, num, denom);
        *f = (num * *f + 0o200000 * self.remainder) / denom;
        self.cur_val += *f / 0o200000;
        *f %= 0o200000;
        Ok(Units::AttachFraction) // done2
    }

    /// §461: set `cur_glue` to a glue specification.
    pub(crate) fn scan_glue(&mut self, level: i32) -> Result<(), Jump> {
        self.glue_origin = None;
        let mu = level == MU_VAL;
        let negative = self.scan_sign()?;
        if self.cur_cmd >= MIN_INTERNAL && self.cur_cmd <= MAX_INTERNAL {
            self.scan_something_internal(level, negative)?;
            if self.cur_val_level >= GLUE_VAL {
                if self.cur_val_level != level {
                    self.mu_error()?;
                }
                return Ok(());
            }
            if self.cur_val_level == INT_VAL {
                self.scan_dimen(mu, false, true)?;
            } else if level == MU_VAL {
                self.mu_error()?;
            }
        } else {
            self.back_input()?;
            self.scan_dimen(mu, false, false)?;
            if negative {
                self.cur_val = -self.cur_val;
            }
        }
        // §462: create a new glue specification whose width is `cur_val`;
        // scan for its stretch and shrink components.
        let mut q = GlueSpec {
            width: self.cur_val,
            ..GlueSpec::default()
        };
        if self.scan_keyword(b"plus")? {
            self.scan_dimen(mu, true, false)?;
            q.stretch = self.cur_val;
            q.stretch_order = order(self.cur_order);
        }
        if self.scan_keyword(b"minus")? {
            self.scan_dimen(mu, true, false)?;
            q.shrink = self.cur_val;
            q.shrink_order = order(self.cur_order);
        }
        self.cur_glue = q;
        self.glue_origin = None; // (new glue, whatever it was scanned from)
        Ok(())
    }

    /// §463
    pub(crate) fn scan_rule_spec(&mut self) -> Result<Node, Jump> {
        // `width`, `depth`, and `height` all equal `null_flag` now
        let (mut width, mut height, mut depth) = (RUNNING, RUNNING, RUNNING);
        if self.cur_cmd == VRULE {
            width = DEFAULT_RULE;
        } else {
            height = DEFAULT_RULE;
            depth = 0;
        }
        loop {
            if self.scan_keyword(b"width")? {
                self.scan_normal_dimen()?;
                width = self.cur_val;
            } else if self.scan_keyword(b"height")? {
                self.scan_normal_dimen()?;
                height = self.cur_val;
            } else if self.scan_keyword(b"depth")? {
                self.scan_normal_dimen()?;
                depth = self.cur_val;
            } else {
                return Ok(Node::Rule {
                    width,
                    height,
                    depth,
                    sync: partex_engine::origin::Side(0),
                });
            }
        }
    }

    /// §558: `param(n)(f)`.
    pub(crate) fn font_param(&self, n: i32, f: i32) -> Scaled {
        self.tracker.read(crate::track::Cell::Font(f));
        self.font_read(f, crate::track::font::PARAMS);
        self.fonts.get(f).param(ux(n))
    }

    /// §577
    pub(crate) fn scan_font_ident(&mut self) -> Result<(), Jump> {
        self.get_nonblank_noncall()?;
        let f = if matches!(self.cur_cmd, DEF_FONT | LETTERSPACE_FONT | PDF_COPY_FONT) {
            self.cur_font()
        } else if self.cur_cmd == SET_FONT {
            self.cur_chr
        } else if self.cur_cmd == DEF_FAMILY {
            let m = self.cur_chr;
            self.scan_four_bit_int()?;
            self.equiv(m + self.cur_val)
        } else {
            self.print_err(b"Missing font identifier");
            self.help(&[
                b"I was looking for a control sequence whose",
                b"current meaning has been defined by \\font.",
            ]);
            self.back_error()?;
            NULL_FONT
        };
        self.cur_val = f;
        Ok(())
    }

    /// §578: find `\fontdimen n f`: `Some((f, n))` if it exists (after
    /// extending the last font's parameters if need be), `None` after the
    /// error of §579 (tex.web's `cur_val=fmem_ptr`, a scratch word).
    pub(crate) fn find_font_dimen(&mut self, writing: bool) -> Result<Option<(i32, usize)>, Jump> {
        self.scan_int()?;
        let n = self.cur_val;
        self.scan_font_ident()?;
        let f = self.cur_val;
        let fi = fx(f);
        // (whether the font is the last one loaded decides whether it can
        // get more parameters)
        self.tracker.read(crate::track::Cell::FontTable);
        self.tracker.read(crate::track::Cell::Font(f));
        if writing {
            self.tracker.write(crate::track::Cell::Font(f));
        }
        self.font_read(f, crate::track::font::PARAMS);
        let params = |t: &Self| i32::try_from(t.fonts.get(f).params.len()).unwrap_or(i32::MAX);
        let found = if n <= 0 {
            false
        } else {
            if writing && n <= SPACE_SHRINK_CODE && n >= SPACE_CODE {
                self.fonts.glue[fi] = None;
                self.font_wrote(f, crate::track::font::GLUE);
            }
            if n > params(self) {
                // (only the font loaded last has room after it, and the
                // room is the table's)
                self.font_table_read();
                if self
                    .tracker
                    .font_newest(f)
                    .unwrap_or_else(|| f == self.fonts.last_loaded())
                {
                    // §580: increase the number of parameters in the last font.
                    loop {
                        if self.fmem_ptr == self.params.font_mem_size {
                            return self.overflow(b"font memory", self.params.font_mem_size);
                        }
                        self.fonts.params_mut(f).push(0);
                        self.fmem_ptr += 1;
                        self.font_wrote(f, crate::track::font::PARAMS);
                        self.font_table_wrote();
                        if n == params(self) {
                            break;
                        }
                    }
                    true
                } else {
                    false
                }
            } else {
                true
            }
        };
        if found {
            return Ok(Some((f, ux(n))));
        }
        // §579: issue an error message if `cur_val=fmem_ptr`.
        self.print_err(b"Font ");
        self.print_esc_num(self.font_id_text(f));
        self.print_str(b" has only ");
        self.print_int(params(self));
        self.print_str(b" fontdimen parameters");
        self.help(&[
            b"To increase the number of font parameters, you must",
            b"use \\fontdimen immediately after the \\font is loaded.",
        ]);
        self.error()?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use crate::testing::{engine, feed, term_output};
    use crate::web::*;

    /// Oracle: `\mag=2000 \dimen0=<x>\relax\message{[\the\dimen0]}` (and
    /// `\skip0`, `\count0`) in INITEX.
    #[test]
    fn scanning_matches_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        t.set_int_par(MAG_CODE, 2000);
        let dims: &[(&[u8], &str)] = &[
            (b"1.5in", "108.405pt"),
            (b"-.3cm", "-8.5359pt"),
            (b"2,5mm", "7.11317pt"),
            (b"7.22726bp", "7.25436pt"),
            (b"3dd", "3.21002pt"),
            (b"1.2cc", "15.40808pt"),
            (b"100sp", "0.00153pt"),
            (b"0.99999999pt", "1.0pt"),
            (b"16383.99999pt", "16383.99998pt"),
            (b"1pc", "12.0pt"),
            (b"--1.5pt", "1.5pt"),
            (b"'17sp", "0.00023pt"),
            (b"\"1Asp", "0.0004pt"),
            (b"`Asp", "0.00099pt"),
            (b"1.5 true in", "54.2025pt"),
            (b"12.5 PT", "12.5pt"),
        ];
        for &(src, want) in dims {
            let mut line = src.to_vec();
            line.extend_from_slice(b"\\relax\n");
            feed(&mut t, &line);
            t.term_offset = 0;
            let out = term_output(&mut t, |t| {
                t.scan_normal_dimen().unwrap();
                t.print_scaled(t.cur_val);
                t.print_str(b"pt");
            });
            assert_eq!(core::str::from_utf8(&out).unwrap(), want, "{src:?}");
        }
        let glues: &[(&[u8], &str)] = &[
            (
                b"1pt plus 2fil minus 3fill",
                "1.0pt plus 2.0fil minus 3.0fill",
            ),
            (b"-2pt plus -1filll", "-2.0pt plus -1.0filll"),
            (b"0pt plus 1fil l l", "0.0pt plus 1.0filll"),
            (b"3pt minus 1pt", "3.0pt minus 1.0pt"),
            (b"-1.5pt", "-1.5pt"),
        ];
        for &(src, want) in glues {
            let mut line = src.to_vec();
            line.extend_from_slice(b"\\relax\n");
            feed(&mut t, &line);
            t.term_offset = 0;
            let out = term_output(&mut t, |t| {
                t.scan_glue(GLUE_VAL).unwrap();
                let g = t.cur_glue;
                t.print_spec(&g, b"pt");
            });
            assert_eq!(core::str::from_utf8(&out).unwrap(), want, "{src:?}");
        }
        let ints: &[(&[u8], i32)] = &[
            (b"123", 123),
            (b"-'777", -511),
            (b"\"FF", 255),
            (b"`\\a", 97),
            (b"- -+ 42", 42),
        ];
        for &(src, want) in ints {
            let mut line = src.to_vec();
            line.extend_from_slice(b"\\relax\n");
            feed(&mut t, &line);
            t.scan_int().unwrap();
            assert_eq!(t.cur_val, want, "{src:?}");
        }
    }
}
