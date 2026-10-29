//! Scanning (§402–§463, §473–§479, §577–§578): numbers, dimensions,
//! glue, keywords, token lists and internal quantities, inside one run.

use phitex_typeset::arith::{nx_plus_y, round_decimals, xn_over_d};
use phitex_typeset::glue::{Glue, Order};

use crate::fields::Par;
use crate::print;
use crate::run::{R, Run};
use crate::val::{
    COUNT_BASE, DEL_CODE_BASE, LC_CODE_BASE, MATCH_CAT, MATH_CODE_BASE, MATH_FONT_BASE,
    MU_SKIP_BASE, OUT_PARAM_CAT, SCALED_BASE, SF_CODE_BASE, SKIP_BASE, TOKS_BASE, Tok,
    UC_CODE_BASE, cmd, glue_of, toks_of,
};

pub(crate) const MAX_DIMEN: i32 = 0o7777777777;
const INFINITY: i32 = 0o17777777777;

/// An internal quantity (`cur_val` with `cur_val_level`, §410).
#[derive(Clone, Debug)]
pub(crate) enum IVal {
    Int(i32),
    Dimen(i32),
    Glue(Glue),
    Mu(Glue),
    Ident(i64),
    Toks(Vec<Tok>),
}

impl IVal {
    pub(crate) fn level(&self) -> u8 {
        match self {
            IVal::Int(_) => 0,
            IVal::Dimen(_) => 1,
            IVal::Glue(_) => 2,
            IVal::Mu(_) => 3,
            IVal::Ident(_) => 4,
            IVal::Toks(_) => 5,
        }
    }

    pub(crate) fn int(&self) -> i32 {
        match self {
            IVal::Int(x) | IVal::Dimen(x) => *x,
            IVal::Glue(g) | IVal::Mu(g) => g.width,
            IVal::Ident(f) => i32::try_from(*f).unwrap_or(0),
            IVal::Toks(_) => 0,
        }
    }

    /// What `\the` prints (§465).
    pub(crate) fn print(&self, s: &mut String) {
        match self {
            IVal::Int(x) => print::print_int(s, i64::from(*x)),
            IVal::Dimen(x) => {
                print::print_scaled(s, *x);
                s.push_str("pt");
            }
            IVal::Glue(g) => print::print_spec(s, g, "pt"),
            IVal::Mu(g) => print::print_spec(s, g, "mu"),
            IVal::Ident(_) | IVal::Toks(_) => {}
        }
    }
}

fn neg_glue(g: &Glue) -> Glue {
    Glue {
        width: -g.width,
        stretch: -g.stretch,
        shrink: -g.shrink,
        ..*g
    }
}

/// The `chr` a number scan's first token is given where it is a macro
/// whose body is a number ([`Run::num_reads`]): its command is `\relax`'s,
/// and only [`Run::scan_int_num`] and `\numexpr`'s first factor look
/// at it.
pub(crate) const NUM_CHR: i32 = i32::MIN + 7;

/// An `\ifnum` operand ([`Run::scan_int_num`]): a number, or a macro's
/// number not read (the macro's token, whether negated), which the
/// comparison reads as its outcome alone (`Run::num_order`): only a
/// page whose test's outcome changes runs again when the macro's number
/// changes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Num {
    Int(i32),
    Macro(Tok, bool),
}

impl Run<'_, '_> {
    /// §406: the next non-blank non-call token.
    pub(crate) fn skip_blanks_x(&mut self) -> R<()> {
        loop {
            self.get_x_token();
            self.ck()?;
            if self.cur.cmd != cmd::SPACER {
                return Ok(());
            }
        }
    }

    /// §404: the next non-blank non-relax non-call token.
    pub(crate) fn skip_blanks_relax_x(&mut self) -> R<()> {
        loop {
            self.get_x_token();
            self.ck()?;
            if self.cur.cmd != cmd::SPACER && self.cur.cmd != cmd::RELAX {
                return Ok(());
            }
        }
    }

    pub(crate) fn back_cur(&mut self) {
        let t = self.cur.tok;
        self.back_input(t);
    }

    /// §441: the next non-blank non-sign token; whether it is negative.
    pub(crate) fn skip_signs(&mut self) -> R<bool> {
        let mut negative = false;
        loop {
            self.skip_blanks_x()?;
            if self.is_other(b'-') {
                negative = !negative;
                continue;
            }
            if !self.is_other(b'+') {
                return Ok(negative);
            }
        }
    }

    pub(crate) fn is_other(&self, c: u8) -> bool {
        !self.cur.is_cs() && self.cur.tok == Tok::ch(12, u32::from(c))
    }

    /// `scan_keyword` (§407).
    pub(crate) fn scan_keyword(&mut self, s: &str) -> R<bool> {
        // (the tokens matched so far, at most the keyword's length: no
        // list made)
        let mut matched = [Tok::c(0); 16];
        let bytes = s.as_bytes();
        debug_assert!(bytes.len() <= matched.len());
        let mut k = 0;
        while k < bytes.len() {
            self.get_x_token();
            self.ck()?;
            let c = bytes[k];
            if !self.cur.is_cs()
                && (self.cur.chr == i32::from(c)
                    || self.cur.chr == i32::from(c) - i32::from(b'a') + i32::from(b'A'))
            {
                matched[k] = self.cur.tok;
                k += 1;
            } else if self.cur.cmd != cmd::SPACER || k > 0 {
                self.back_cur();
                if k > 0 {
                    self.push_toks(&matched[..k]);
                }
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `scan_left_brace` (§403).
    pub(crate) fn scan_left_brace(&mut self) -> R<()> {
        self.skip_blanks_relax_x()?;
        if self.cur.cmd != cmd::LEFT_BRACE {
            // "Missing { inserted"
            self.back_cur();
            self.cur.tok = Tok::ch(1, u32::from(b'{'));
            self.cur.cmd = cmd::LEFT_BRACE;
            self.cur.chr = i32::from(b'{');
        }
        Ok(())
    }

    /// `scan_optional_equals` (§405).
    pub(crate) fn scan_optional_equals(&mut self) -> R<()> {
        self.skip_blanks_x()?;
        if !self.is_other(b'=') {
            self.back_cur();
        }
        Ok(())
    }

    /// `scan_int` (§440).
    pub(crate) fn scan_int(&mut self) -> R<i32> {
        self.radix = 0;
        let negative = self.skip_signs()?;
        self.scan_int_from(negative)
    }

    /// `scan_int` (§440) for `\number` (docs/spec-deferred-reads.md §2):
    /// with no sign, the operand's one internal quantity may be deferred
    /// ([`Run::defer_ok`], taken by the outermost
    /// `scan_something_internal`; the signs' expansion before it reads
    /// as any scan does). [`Run::defer_hit`] says whether it was.
    pub(crate) fn scan_int_deferring(&mut self) -> R<i32> {
        self.radix = 0;
        let negative = self.skip_signs()?;
        self.defer_ok = !negative && self.defer_allowed();
        let r = self.scan_int_from(negative);
        self.defer_ok = false;
        r
    }

    /// `scan_int` (§440) for an `\ifnum` operand ([`Num`]): a number
    /// that is a macro's whole body, the macro not this run's own, is kept
    /// as that macro (its meaning not read), directly or as all of an
    /// `\numexpr`; any other number as it is.
    pub(crate) fn scan_int_num(&mut self) -> R<Num> {
        self.radix = 0;
        self.num_reads = true;
        let negative = self.skip_signs();
        self.num_reads = false;
        let negative = negative?;
        if self.cur.cmd == cmd::RELAX && self.cur.chr == NUM_CHR {
            let tok = self.cur.tok;
            // (the macro's digits read, the scan reads on, §444: a digit
            // after them goes on with the number, an optional space ends
            // it)
            self.radix = 10;
            self.get_x_token();
            self.ck()?;
            let (cat, c) = (self.cur.tok.cat(), self.cur.tok.chr());
            if !self.cur.is_cs() && cat == Some(12) && (48..=57).contains(&c) {
                // (the number goes on: its digits as the macro's body
                // gives them, then the scan as TeX's)
                self.back_cur();
                let n = self.num_value(tok)?;
                let digits: Vec<Tok> = n
                    .to_string()
                    .bytes()
                    .map(|d| Tok::ch(12, u32::from(d)))
                    .collect();
                self.push_toks(digits);
                self.get_x_token();
                self.ck()?;
                return Ok(Num::Int(self.scan_int_from(negative)?));
            }
            if self.cur.cmd != cmd::SPACER {
                self.back_cur();
            }
            return Ok(Num::Macro(tok, negative));
        }
        if self.cur.cmd == cmd::LAST_ITEM && self.cur.chr == crate::etex::li::NUM_EXPR {
            let n = crate::etex::scan_numexpr_num(self)?;
            return Ok(match n {
                Num::Int(x) => Num::Int(if negative { -x } else { x }),
                Num::Macro(t, neg) => Num::Macro(t, neg != negative),
            });
        }
        Ok(Num::Int(self.scan_int_from(negative)?))
    }

    /// The value of `n`, reading its macro's number where it is one.
    pub(crate) fn num_int(&mut self, n: Num) -> R<i32> {
        Ok(match n {
            Num::Int(x) => x,
            Num::Macro(t, neg) => {
                let x = self.num_value(t)?;
                if neg { -x } else { x }
            }
        })
    }

    /// `scan_int` (§440) from its first token after the signs.
    #[allow(clippy::too_many_lines)]
    fn scan_int_from(&mut self, negative: bool) -> R<i32> {
        let mut ok_so_far = true;
        let mut val: i32;
        // (a deferring scan, [`Run::scan_int_deferring`]: an internal
        // quantity only, never digits or a constant)
        let defer = std::mem::take(&mut self.defer_ok);
        if self.is_other(b'`') {
            // §442
            self.get_token();
            self.ck()?;
            let t = self.cur.tok;
            // (a brace taken as a constant does not count in
            // `align_state`: get_next counted it)
            if self.aopen != 0 && !t.is_cs_or_active() {
                if self.cur.cmd == cmd::RIGHT_BRACE {
                    let a = self.astate();
                    self.set_astate(a + 1);
                } else if self.cur.cmd == cmd::LEFT_BRACE {
                    let a = self.astate();
                    self.set_astate(a - 1);
                }
            }
            // (XeTeX's constants run to `biggest_usv`; a one-character
            // name past 0xFFFF is a multi-letter one there)
            let limit = if self.xe {
                crate::xetex::BIGGEST_USV.cast_signed()
            } else {
                255
            };
            val = match t.as_cs() {
                Some(n) => {
                    let mut cs = n.chars();
                    match (cs.next(), cs.next()) {
                        (Some(c), None)
                            if !n.starts_with('\u{10FFFE}')
                                && (!self.xe || u32::from(c) <= 0xFFFF) =>
                        {
                            i32::try_from(u32::from(c)).unwrap_or(limit + 1)
                        }
                        _ => limit + 1,
                    }
                }
                None => i32::try_from(t.chr()).unwrap_or(0),
            };
            if val > limit {
                // "Improper alphabetic constant"
                val = i32::from(b'0');
                self.back_cur();
            } else {
                // §443
                self.get_x_token();
                self.ck()?;
                if self.cur.cmd != cmd::SPACER {
                    self.back_cur();
                }
            }
        } else if self.cur.cmd >= cmd::CHAR_GIVEN && self.cur.cmd <= cmd::MAX_INTERNAL {
            self.defer_ok = defer;
            val = self.scan_something_internal(0, false)?.int();
        } else {
            // §444
            self.radix = 10;
            let mut m = 214_748_364;
            if self.is_other(b'\'') {
                self.radix = 8;
                m = 0o2000000000;
                self.get_x_token();
                self.ck()?;
            } else if self.is_other(b'"') {
                self.radix = 16;
                m = 0o1000000000;
                self.get_x_token();
                self.ck()?;
            }
            let mut vacuous = true;
            val = 0;
            loop {
                let (cat, c) = (self.cur.tok.cat(), self.cur.tok.chr());
                let cs = self.cur.is_cs();
                let d: i32 = if !cs
                    && cat == Some(12)
                    && (48..=57).contains(&c)
                    && i32::try_from(c - 48).unwrap_or(99) < self.radix
                {
                    i32::try_from(c - 48).unwrap_or(0)
                } else if self.radix == 16
                    && !cs
                    && (cat == Some(11) || cat == Some(12))
                    && (65..=70).contains(&c)
                {
                    i32::try_from(c - 55).unwrap_or(0)
                } else {
                    break;
                };
                vacuous = false;
                if val >= m && (val > m || d > 7 || self.radix != 10) {
                    if ok_so_far {
                        // "Number too big"
                        ok_so_far = false;
                    }
                    val = INFINITY;
                } else {
                    val = val * self.radix + d;
                }
                self.get_x_token();
                self.ck()?;
            }
            if vacuous {
                // "Missing number, treated as zero"
                self.back_cur();
            } else if self.cur.cmd != cmd::SPACER {
                self.back_cur();
            }
        }
        Ok(if negative { -val } else { val })
    }

    pub(crate) fn scan_normal_dimen(&mut self) -> R<i32> {
        self.scan_dimen(false, false, None)
    }

    /// `scan_dimen` (§448–§460); `shortcut` is an integer already scanned.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn scan_dimen(&mut self, mu: bool, inf: bool, shortcut: Option<i32>) -> R<i32> {
        let mut f: i32 = 0;
        let mut arith_error = false;
        self.cur_order = Order::Normal;
        let mut negative = false;
        let mut val: i32;
        if let Some(v) = shortcut {
            val = v;
        } else {
            negative = self.skip_signs()?;
            if self.cur.cmd >= cmd::CHAR_GIVEN && self.cur.cmd <= cmd::MAX_INTERNAL {
                // §449
                if mu {
                    let x = self.scan_something_internal(3, false)?;
                    let x = coerce_glue(x);
                    if let IVal::Mu(g) = x {
                        return Ok(Self::attach_sign(g.width, negative, arith_error));
                    }
                    val = x.int();
                    if matches!(x, IVal::Mu(_)) {
                        return Ok(Self::attach_sign(val, negative, arith_error));
                    }
                } else {
                    let x = self.scan_something_internal(1, false)?;
                    if let IVal::Dimen(d) = x {
                        return Ok(Self::attach_sign(d, negative, arith_error));
                    }
                    val = x.int();
                }
            } else {
                self.back_cur();
                if self.is_other(b',') {
                    self.cur.tok = Tok::ch(12, u32::from(b'.'));
                }
                if self.is_other(b'.') {
                    self.radix = 10;
                    val = 0;
                } else {
                    val = self.scan_int()?;
                }
                if self.is_other(b',') {
                    self.cur.tok = Tok::ch(12, u32::from(b'.'));
                }
                if self.radix == 10 && self.is_other(b'.') {
                    // §452
                    let mut digits: Vec<u8> = Vec::new();
                    self.get_token();
                    self.ck()?;
                    loop {
                        self.get_x_token();
                        self.ck()?;
                        let c = self.cur.tok.chr();
                        if self.cur.is_cs()
                            || self.cur.tok.cat() != Some(12)
                            || !(48..=57).contains(&c)
                        {
                            break;
                        }
                        if digits.len() < 17 {
                            digits.push(u8::try_from(c - 48).unwrap_or(0));
                        }
                    }
                    f = round_decimals(&digits);
                    if self.cur.cmd != cmd::SPACER {
                        self.back_cur();
                    }
                }
            }
        }
        if val < 0 {
            negative = !negative;
            val = -val;
        }
        // §453
        if inf && self.scan_keyword("fil")? {
            // §454
            self.cur_order = Order::Fil;
            while self.scan_keyword("l")? {
                self.cur_order = match self.cur_order {
                    Order::Fil => Order::Fill,
                    _ => Order::Filll,
                };
            }
            return self.attach_fraction(val, f, negative);
        }
        // §455
        let save_val = val;
        self.skip_blanks_x()?;
        let v: Option<i32> = if self.cur.cmd < cmd::CHAR_GIVEN || self.cur.cmd > cmd::MAX_INTERNAL {
            self.back_cur();
            if mu {
                None
            } else if self.scan_keyword("em")? {
                let q = self.font_param_cur(6)?;
                self.optional_space()?;
                Some(q)
            } else if self.scan_keyword("ex")? {
                let q = self.font_param_cur(5)?;
                self.optional_space()?;
                Some(q)
            } else {
                None
            }
        } else if mu {
            let x = coerce_glue(self.scan_something_internal(3, false)?);
            Some(x.int())
        } else {
            Some(self.scan_something_internal(1, false)?.int())
        };
        if let Some(v) = v {
            let r = nx_plus_y(save_val, v, xn_over_d(v, f, 0o200000).value);
            let (r, err) = match r {
                Some(r) => (r, false),
                None => (0, true),
            };
            return Ok(Self::attach_sign(r, negative, arith_error || err));
        }
        if mu {
            // §456
            let _ = self.scan_keyword("mu")?;
            return self.attach_fraction(val, f, negative);
        }
        if self.scan_keyword("true")? {
            // §457
            let mag = self.ipar(Par::mag)?;
            if mag != 1000 && mag > 0 {
                let d = xn_over_d(val, 1000, mag);
                val = d.value;
                f = i32::try_from(
                    (1000 * i64::from(f) + 0o200000 * i64::from(d.remainder)) / i64::from(mag),
                )
                .unwrap_or(0);
                val += f / 0o200000;
                f %= 0o200000;
            }
        }
        if self.scan_keyword("pt")? {
            return self.attach_fraction(val, f, negative);
        }
        // §458
        let conv = if self.scan_keyword("in")? {
            Some((7227, 100))
        } else if self.scan_keyword("pc")? {
            Some((12, 1))
        } else if self.scan_keyword("cm")? {
            Some((7227, 254))
        } else if self.scan_keyword("mm")? {
            Some((7227, 2540))
        } else if self.scan_keyword("bp")? {
            Some((7227, 7200))
        } else if self.scan_keyword("dd")? {
            Some((1238, 1157))
        } else if self.scan_keyword("cc")? {
            Some((14856, 1157))
        } else if self.scan_keyword("sp")? {
            // goto done
            self.optional_space()?;
            return Ok(Self::attach_sign(val, negative, arith_error));
        } else {
            // "Illegal unit of measure (pt inserted)"
            None
        };
        if let Some((num, denom)) = conv {
            let d = xn_over_d(val, num, denom);
            val = d.value;
            f = i32::try_from(
                (i64::from(num) * i64::from(f) + 0o200000 * i64::from(d.remainder))
                    / i64::from(denom),
            )
            .unwrap_or(0);
            val += f / 0o200000;
            f %= 0o200000;
        }
        let _ = &mut arith_error;
        self.attach_fraction(val, f, negative)
    }

    fn attach_fraction(&mut self, val: i32, f: i32, negative: bool) -> R<i32> {
        let (v, err) = if val >= 0o40000 {
            (val, true)
        } else {
            (val * 65536 + f, false)
        };
        self.optional_space()?;
        Ok(Self::attach_sign(v, negative, err))
    }

    pub(crate) fn attach_sign(val: i32, negative: bool, arith_error: bool) -> i32 {
        let mut v = val;
        if arith_error || v.unsigned_abs() >= 0o10000000000 {
            // "Dimension too large"
            v = MAX_DIMEN;
        }
        if negative { -v } else { v }
    }

    /// §443.
    pub(crate) fn optional_space(&mut self) -> R<()> {
        self.get_x_token();
        self.ck()?;
        if self.cur.cmd != cmd::SPACER {
            self.back_cur();
        }
        Ok(())
    }

    /// The `em` and `ex` of the current font (§558).
    fn font_param_cur(&mut self, k: i32) -> R<i32> {
        let f = self.cint(crate::fields::cell::FONT)?;
        self.font_dimen(f, k)
    }

    pub(crate) fn font_dimen(&mut self, f: i64, k: i32) -> R<i32> {
        // (a font this run decoded since the table last changed: its
        // parameters as read then)
        if let (Ok(fu), Ok(ku)) = (u32::try_from(f), usize::try_from(k))
            && let Some((g, Some(e))) = self.font_memo.get(&fu)
            && *g == self.st.font_gen()
            && self.interp.is_none()
            && let Some(&d) = ku.checked_sub(1).and_then(|i| e.params.get(i))
        {
            return Ok(d);
        }
        self.font_int(f, crate::fields::FontCell::Fd(k))
    }

    /// `scan_glue` (§461–§462).
    pub(crate) fn scan_glue(&mut self, level: u8) -> R<Glue> {
        let mu = level == 3;
        let negative = self.skip_signs()?;
        let width;
        if self.cur.cmd >= cmd::CHAR_GIVEN && self.cur.cmd <= cmd::MAX_INTERNAL {
            self.glue_fresh = false;
            let x = self.scan_something_internal(level, negative)?;
            match x {
                IVal::Glue(g) | IVal::Mu(g) => {
                    // (a register's or parameter's spec, a new one when
                    // negated, §461; an expression's as `scan_expr` made it)
                    self.glue_own = negative || self.glue_fresh;
                    return Ok(g);
                }
                IVal::Int(n) => width = self.scan_dimen(mu, false, Some(n))?,
                other => width = other.int(),
            }
        } else {
            self.back_cur();
            let w = self.scan_dimen(mu, false, None)?;
            width = if negative { -w } else { w };
        }
        let mut g = Glue {
            width,
            ..Glue::ZERO
        };
        if self.scan_keyword("plus")? {
            g.stretch = self.scan_dimen(mu, true, None)?;
            g.stretch_order = self.cur_order;
        }
        if self.scan_keyword("minus")? {
            g.shrink = self.scan_dimen(mu, true, None)?;
            g.shrink_order = self.cur_order;
        }
        // (`new_spec`, §461: its own)
        self.glue_own = true;
        Ok(g)
    }

    /// A register or box number: e-TeX's `scan_register_num` (pdfTeX
    /// §1809, 0..32767) wherever tex.web reads one.
    pub(crate) fn scan_eight_bit_int(&mut self) -> R<i32> {
        self.scan_register_num()
    }

    pub(crate) fn scan_char_num(&mut self) -> R<i32> {
        // (XeTeX's `scan_char_num` is `scan_usv_num`)
        if self.xe {
            return self.scan_usv_num();
        }
        let n = self.scan_int()?;
        Ok(if (0..=255).contains(&n) { n } else { 0 })
    }

    pub(crate) fn scan_four_bit_int(&mut self) -> R<i32> {
        let n = self.scan_int()?;
        Ok(if (0..=15).contains(&n) { n } else { 0 })
    }

    pub(crate) fn scan_fifteen_bit_int(&mut self) -> R<i32> {
        let n = self.scan_int()?;
        Ok(if (0..=0o77777).contains(&n) { n } else { 0 })
    }

    pub(crate) fn scan_twenty_seven_bit_int(&mut self) -> R<i32> {
        let n = self.scan_int()?;
        Ok(if (0..=0o777777777).contains(&n) { n } else { 0 })
    }

    /// `scan_font_ident` (§577).
    pub(crate) fn scan_font_ident(&mut self) -> R<i64> {
        self.skip_blanks_x()?;
        Ok(match self.cur.cmd {
            cmd::DEF_FONT => self.cint(crate::fields::cell::FONT)?,
            cmd::SET_FONT => i64::from(self.cur.chr),
            cmd::DEF_FAMILY => {
                let m = self.cur.chr;
                let n = self.scan_fam()?;
                self.fam_font(m - MATH_FONT_BASE, n)?
            }
            _ => {
                // "Missing font identifier"
                self.back_cur();
                0
            }
        })
    }

    pub(crate) fn fam_font(&mut self, size: i32, n: i32) -> R<i64> {
        let l = self.fam_loc(size, n);
        Ok(i64::from(self.int_loc(l)?))
    }

    /// Where family `n`'s font at `size` (0, 16, 32) is: XeTeX's families
    /// past sixteen in fields of their own (`famx:<size>:<n>`).
    pub(crate) fn fam_loc(&mut self, size: i32, n: i32) -> crate::state::Loc {
        if self.xe && n >= 16 {
            let prefix =
                ["famx:0", "famx:1", "famx:2"][usize::try_from(size / 16).unwrap_or(0).min(2)];
            return self.numbered_loc(prefix, n.into());
        }
        crate::fields::fam_slot(size, n)
    }

    /// A family number: TeX's four bits, XeTeX's eight
    /// (`scan_math_fam_int`).
    pub(crate) fn scan_fam(&mut self) -> R<i32> {
        if self.xe {
            return self.scan_bounded(255, "Bad math family");
        }
        self.scan_four_bit_int()
    }

    /// `find_font_dimen` (§578): the font and parameter number, `None`
    /// where tex.web points at `fmem_ptr` (an error).
    pub(crate) fn find_font_dimen(&mut self, writing: bool) -> R<Option<(i64, i32)>> {
        let n = self.scan_int()?;
        let f = self.scan_font_ident()?;
        let _ = writing;
        if n <= 0 {
            return Ok(None);
        }
        let count = self.font_int(f, crate::fields::FontCell::Fdn)?;
        if n > count {
            // (§578: only the font loaded last, `font_ptr`, grows)
            let last = self
                .get("fontlast")?
                .map_or(0, |x| crate::val::int_of(self.v, x));
            if f != last || f == 0 {
                return Ok(None);
            }
            // §580: the parameters of the last font grow
            let v = self.v.int(i64::from(n));
            self.font_set(f, crate::fields::FontCell::Fdn, v, true);
        }
        Ok(Some((f, n)))
    }

    /// `scan_something_internal` (§413–§430).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn scan_something_internal(&mut self, level: u8, negative: bool) -> R<IVal> {
        let m = self.cur.chr;
        let c = self.cur.cmd;
        // (a deferring scan's quantity, [`Run::defer_ok`]: only this one,
        // not a register read while it is scanned)
        let defer = std::mem::take(&mut self.defer_ok);
        let mut val = match c {
            cmd::DEF_CODE if self.xe => IVal::Int(self.xe_fetch_code(m)?),
            cmd::DEF_CODE => {
                // §414
                let n = self.scan_char_num()?;
                let table = match m {
                    LC_CODE_BASE => Some(0),
                    UC_CODE_BASE => Some(1),
                    SF_CODE_BASE => Some(2),
                    MATH_CODE_BASE => Some(3),
                    DEL_CODE_BASE => Some(4),
                    _ => None,
                };
                IVal::Int(match table {
                    Some(t) => self.code(t, n)?,
                    None => i32::from(self.cat(u32::try_from(n).unwrap_or(0))?),
                })
            }
            cmd::TOKS_REGISTER
            | cmd::ASSIGN_TOKS
            | cmd::DEF_FAMILY
            | cmd::SET_FONT
            | cmd::DEF_FONT => {
                // §415
                if level != 5 {
                    // "Missing number, treated as zero"
                    self.back_cur();
                    IVal::Dimen(0)
                } else if c <= cmd::ASSIGN_TOKS {
                    let loc = if c < cmd::ASSIGN_TOKS {
                        TOKS_BASE + self.scan_eight_bit_int()?
                    } else {
                        m
                    };
                    let x = if self.xe
                        && c == cmd::ASSIGN_TOKS
                        && loc == crate::xetex::INTER_CHAR_LOC
                    {
                        let f = self.ic_field()?;
                        self.read_f(f)?
                    } else {
                        self.qget(cmd::ASSIGN_TOKS, loc)?
                    };
                    IVal::Toks(toks_of(self.v, x))
                } else {
                    self.back_cur();
                    IVal::Ident(self.scan_font_ident()?)
                }
            }
            cmd::ASSIGN_INT => {
                let l = self.quantity_loc(c, m);
                if defer && self.defer_at(l) {
                    IVal::Int(0)
                } else {
                    IVal::Int(crate::val::i32_of(self.v, self.get_loc(l)?))
                }
            }
            cmd::ASSIGN_DIMEN => IVal::Dimen(crate::val::i32_of(self.v, self.qget(c, m)?)),
            cmd::ASSIGN_GLUE => {
                let x = self.qget(c, m)?;
                IVal::Glue(glue_of(self.v, x))
            }
            cmd::ASSIGN_MU_GLUE => {
                let x = self.qget(c, m)?;
                IVal::Mu(glue_of(self.v, x))
            }
            cmd::SET_AUX => {
                // §418
                let md = self.mode()?;
                if self.mode_zero || md.m.abs() != m {
                    // "Improper \spacefactor"
                    if level == 5 {
                        IVal::Int(0)
                    } else {
                        IVal::Dimen(0)
                    }
                } else if m == 1 {
                    IVal::Dimen(self.prev_depth(&md)?)
                } else {
                    IVal::Int(self.space_factor(&md)?)
                }
            }
            cmd::SET_PREV_GRAF => {
                // §422
                if self.mode_zero {
                    IVal::Int(0)
                } else {
                    let md = self.mode()?;
                    IVal::Int(self.int32(&format!("pg:{}", md.vlid))?)
                }
            }
            cmd::SET_PAGE_INT | cmd::SET_SHAPE | cmd::ASSIGN_FONT_INT
                if crate::etex::is_internal(c, m) =>
            {
                crate::etex::internal(self)?
            }
            cmd::SET_PAGE_INT => {
                // §419
                IVal::Int(if m == 0 {
                    self.int_loc(crate::fields::cell::DEADCYCLES)?
                } else {
                    self.insert_penalties()?
                })
            }
            cmd::SET_PAGE_DIMEN => {
                // §421
                IVal::Dimen(self.page_dimen(m)?)
            }
            cmd::SET_SHAPE => {
                // §423
                let x = self.get_loc(crate::fields::cell::PARSHAPE)?;
                IVal::Int(x.map_or(0, |x| {
                    i32::try_from(crate::val::list_of(self.v, x).len() / 2).unwrap_or(0)
                }))
            }
            cmd::SET_BOX_DIMEN => {
                // §420
                let n = self.scan_eight_bit_int()?;
                let b = self.box_reg(n)?;
                IVal::Dimen(b.map_or(0, |b| crate::nodes::box_dim(self.v, b, m)))
            }
            cmd::CHAR_GIVEN | cmd::MATH_GIVEN => IVal::Int(m),
            cmd::ASSIGN_FONT_DIMEN => {
                // §425
                match self.find_font_dimen(false)? {
                    Some((f, n)) => IVal::Dimen(self.font_dimen(f, n)?),
                    None => IVal::Dimen(0),
                }
            }
            cmd::ASSIGN_FONT_INT => {
                // §426
                let f = self.scan_font_ident()?;
                let c = if m == 0 {
                    crate::fields::FontCell::Hc
                } else {
                    crate::fields::FontCell::Sc
                };
                IVal::Int(self.font_int(f, c)?)
            }
            cmd::REGISTER => {
                // §427
                let n = self.scan_eight_bit_int()?;
                match m {
                    0 => {
                        let l = self.reg_loc(0, n);
                        if defer && self.defer_at(l) {
                            IVal::Int(0)
                        } else {
                            IVal::Int(crate::val::i32_of(self.v, self.get_loc(l)?))
                        }
                    }
                    1 => IVal::Dimen(crate::val::i32_of(self.v, self.reg(1, n)?)),
                    2 => {
                        let x = self.reg(2, n)?;
                        IVal::Glue(glue_of(self.v, x))
                    }
                    _ => {
                        let x = self.reg(3, n)?;
                        IVal::Mu(glue_of(self.v, x))
                    }
                }
            }
            cmd::LAST_ITEM => self.last_item(m)?,
            _ => {
                // §428: "You can't use x after \the"
                self.back_cur();
                if level == 5 {
                    IVal::Int(0)
                } else {
                    IVal::Dimen(0)
                }
            }
        };
        // §429
        while val.level() > level {
            val = match val {
                IVal::Glue(g) => IVal::Dimen(g.width),
                IVal::Mu(g) => IVal::Glue(g),
                IVal::Dimen(d) => IVal::Int(d),
                IVal::Int(_) | IVal::Ident(_) | IVal::Toks(_) => IVal::Int(0),
            };
        }
        // §430
        if negative {
            val = match val {
                IVal::Int(x) => IVal::Int(-x),
                IVal::Dimen(x) => IVal::Dimen(-x),
                IVal::Glue(g) => IVal::Glue(neg_glue(&g)),
                IVal::Mu(g) => IVal::Mu(neg_glue(&g)),
                other => other,
            };
        }
        Ok(val)
    }

    /// `scan_toks` (§473–§479): the parameter text (for a macro) and the
    /// body. A halt is raised, never returned (spec-final §2.2: the
    /// lists are then what was scanned; the caller's check stops it).
    pub(crate) fn scan_toks(&mut self, macro_def: bool, xpand: bool) -> (Vec<Tok>, Vec<Tok>) {
        self.lift(|r| r.scan_toks_r(macro_def, xpand))
            .unwrap_or_default()
    }

    #[allow(clippy::too_many_lines)]
    fn scan_toks_r(&mut self, macro_def: bool, xpand: bool) -> R<(Vec<Tok>, Vec<Tok>)> {
        // §473: `scanner_status:=defining` or `absorbing`;
        // `warning_index:=cur_cs`
        let status = if macro_def {
            crate::run::SCAN_DEFINING
        } else {
            crate::run::SCAN_ABSORBING
        };
        let was = self.scanner.replace((status, self.cur.tok));
        let r = self.scan_toks_in(macro_def, xpand);
        self.scanner = was;
        r
    }

    fn scan_toks_in(&mut self, macro_def: bool, xpand: bool) -> R<(Vec<Tok>, Vec<Tok>)> {
        let mut params: Vec<Tok> = Vec::new();
        // (the list the last definition left, emptied: a definition copies
        // its body out and gives it back, [`Run::toks_buf`])
        let mut body: Vec<Tok> = std::mem::take(&mut self.toks_buf);
        body.clear();
        if body.capacity() == 0 {
            body.reserve(32);
        }
        let mut hash_brace: Option<Tok> = None;
        let mut t: u32 = 0; // parameters so far
        if macro_def {
            // §474
            loop {
                self.get_token_stored();
                self.ck()?;
                let tok = self.cur.tok;
                if !tok.is_cs_or_active() && matches!(tok.cat(), Some(1 | 2)) {
                    if tok.cat() == Some(2) {
                        // §475 "Missing { inserted": `\def\a}` is `\def\a{}`
                        return Ok((params, body));
                    }
                    break;
                }
                if self.cur.cmd == cmd::MAC_PARAM {
                    // §476
                    let s = Tok::ch(MATCH_CAT, tok.chr());
                    self.get_token_stored();
                    self.ck()?;
                    let nt = self.cur.tok;
                    if !nt.is_cs_or_active() && nt.cat() == Some(1) {
                        hash_brace = Some(nt);
                        let _ = s;
                        params.push(nt);
                        return self.scan_body(params, body, xpand, macro_def, t, hash_brace);
                    }
                    if t == 9 {
                        // "You already have nine parameters"
                        continue;
                    }
                    t += 1;
                    if nt != Tok::ch(12, u32::from(b'0') + t) {
                        // "Parameters must be numbered consecutively"
                        self.back_input(nt);
                    }
                    params.push(s);
                    continue;
                }
                params.push(tok);
            }
        } else {
            self.scan_left_brace()?;
        }
        body.clear();
        self.scan_body(params, body, xpand, macro_def, t, hash_brace)
    }

    #[allow(clippy::too_many_arguments)]
    fn scan_body(
        &mut self,
        params: Vec<Tok>,
        mut body: Vec<Tok>,
        xpand: bool,
        macro_def: bool,
        t: u32,
        hash_brace: Option<Tok>,
    ) -> R<(Vec<Tok>, Vec<Tok>)> {
        // §477
        let mut unbalance = 1;
        loop {
            if xpand {
                // §478 (a token stored as it is needs only its meaning's
                // class: `Run::class_lookup`; not inside an alignment,
                // whose tab marks look at the meaning, nor while a line
                // is interpreted or a load recorded, which read meanings
                // in full)
                let class = self.aopen == 0 && self.interp.is_none() && !self.rec_load;
                loop {
                    self.class_reads = class;
                    let got = self.get_next_or_end();
                    self.class_reads = false;
                    got?;
                    if self.cur.cmd <= cmd::MAX_COMMAND || crate::etex::protect(self)? {
                        break;
                    }
                    if self.cur.cmd == cmd::THE {
                        let q = self.the_toks();
                        self.ck()?;
                        body.extend(q);
                    } else if self.cur.cmd >= cmd::END_TEMPLATE
                        && self.cur.cmd != cmd::DONT_EXPAND
                        && self.cur.cmd != cmd::SYM
                    {
                        break;
                    } else {
                        self.expand();
                        self.ck()?;
                    }
                }
                self.x_token();
                self.ck()?;
                // (§120: the list fills TeX's main memory)
                if body.len() >= crate::input::MAIN_MEMORY {
                    return self.fatal(&format!(
                        "TeX capacity exceeded, sorry [main memory size={}]",
                        crate::input::MAIN_MEMORY
                    ));
                }
            } else {
                // (§473: no meaning is needed here except a control
                // sequence's in a `\def`, for §479's `#`; braces count
                // by token alone)
                let tok = self.get_token_tok();
                self.ck()?;
                // (a read that must wait stops the scan here: a raised
                // halt would leave its sentinel in the body, and the
                // definition made before the halt surfaced, so a run
                // resumed after it keeps the sentinel for the meaning)
                if macro_def && tok.is_cs_or_active() {
                    // (§479: whether it is the macro parameter character)
                    let was = std::mem::replace(&mut self.step_reads, true);
                    let r = self.set_cur(tok);
                    self.step_reads = was;
                    r?;
                }
            }
            let tok = self.cur.tok;
            if !tok.is_cs_or_active() && matches!(tok.cat(), Some(1 | 2)) {
                if tok.cat() == Some(1) {
                    unbalance += 1;
                } else {
                    unbalance -= 1;
                    if unbalance == 0 {
                        break;
                    }
                }
            } else if macro_def
                && if tok.is_cs_or_active() {
                    self.cur.cmd == cmd::MAC_PARAM
                } else {
                    tok.cat() == Some(6)
                }
            {
                // §479
                let s = tok;
                if xpand {
                    self.get_x_token();
                    self.ck()?;
                } else {
                    self.get_token_stored();
                    self.ck()?;
                }
                if self.cur.cmd != cmd::MAC_PARAM {
                    let nt = self.cur.tok;
                    let c = nt.chr();
                    if nt.is_cs_or_active() || nt.cat() != Some(12) || c <= 48 || c > 48 + t {
                        // "Illegal parameter number in definition"
                        self.back_input(nt);
                        body.push(s);
                        continue;
                    }
                    body.push(Tok::ch(OUT_PARAM_CAT, c - 48));
                    continue;
                }
                body.push(self.cur.tok);
                continue;
            }
            body.push(tok);
        }
        if let Some(h) = hash_brace {
            body.push(h);
        }
        Ok((params, body))
    }

    /// `scan_rule_spec` (§463): width, height, depth (`RUNNING` where not
    /// given).
    pub(crate) fn scan_rule_spec(&mut self, vrule: bool) -> R<(i32, i32, i32)> {
        let running = phitex_typeset::node::RUNNING;
        let (mut w, mut h, mut d) = if vrule {
            (26214, running, running)
        } else {
            (running, 26214, 0)
        };
        loop {
            if self.scan_keyword("width")? {
                w = self.scan_normal_dimen()?;
            } else if self.scan_keyword("height")? {
                h = self.scan_normal_dimen()?;
            } else if self.scan_keyword("depth")? {
                d = self.scan_normal_dimen()?;
            } else {
                return Ok((w, h, d));
            }
        }
    }

    pub(crate) fn _unused_bases() -> (i32, i32, i32) {
        (COUNT_BASE, SCALED_BASE, SKIP_BASE + MU_SKIP_BASE)
    }
}

/// §451: glue coerced to its width (the level stays mu for mu glue).
fn coerce_glue(x: IVal) -> IVal {
    match x {
        IVal::Glue(g) => IVal::Dimen(g.width),
        IVal::Mu(g) => IVal::Mu(Glue {
            width: g.width,
            ..Glue::ZERO
        }),
        other => other,
    }
}
