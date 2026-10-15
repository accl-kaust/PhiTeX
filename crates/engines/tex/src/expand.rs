//! Expansion (§366–§401, §465–§510): macro calls, conditionals,
//! `\expandafter`, `\noexpand`, `\csname`, `\the`, the conversions, marks
//! and `\input`, inside one run.

use std::rc::Rc;

use phitex_defuse::Value;

use crate::print;
use crate::run::{R, Run};
use crate::val::{MATCH_CAT, Macro, Mean, Tok, cmd, int_of, str_of};

// §489
pub(crate) const IF_CODE: u8 = 1;
pub(crate) const FI_CODE: i32 = 2;
pub(crate) const ELSE_CODE: i32 = 3;
pub(crate) const OR_CODE: i32 = 4;

/// A conditional begun before this run is in its taken branch (§510).
const OUTSIDE_LIMIT: u8 = 4;

/// `macro_call`'s buffers, kept by each thread between calls.
#[derive(Default)]
pub(crate) struct MacroBufs {
    /// The arguments' tokens, one after another.
    args: Vec<Tok>,
    /// Argument `#k` is `args[bounds[k-1].0..bounds[k-1].1]`.
    bounds: Vec<(usize, usize)>,
    /// The body with its arguments substituted.
    out: Vec<Tok>,
}

thread_local! {
    static NAME: std::cell::Cell<String> = std::cell::Cell::default();
}

/// A control sequence interned once a process.
macro_rules! cs_once {
    ($name:ident, $s:expr) => {
        pub(crate) fn $name() -> Tok {
            static C: std::sync::OnceLock<crate::val::CsId> = std::sync::OnceLock::new();
            Tok::of_cs(*C.get_or_init(|| crate::val::CsId::intern($s)))
        }
    };
}
cs_once!(par_tok, "par");
cs_once!(relax_tok, "\u{10FFFE}relax");
cs_once!(notexpanded_tok, "\u{10FFFE}notexpanded");
cs_once!(halt_tok, "\u{10FFFE}halt");

impl Run<'_, '_> {
    /// `expand` (§366): `cur` is expandable.
    pub(crate) fn expand(&mut self) -> R<()> {
        let saved = (self.radix, self.cur_order);
        let c = self.cur.cmd;
        if crate::etex::expand(self)? {
            (self.radix, self.cur_order) = saved;
            return Ok(());
        }
        if (cmd::CALL..cmd::END_TEMPLATE).contains(&c) {
            self.macro_call()?;
        } else {
            match c {
                cmd::TOP_BOT_MARK => {
                    // §386
                    let name = ["top", "first", "bot", "splitfirst", "splitbot"]
                        [usize::try_from(self.cur.chr).unwrap_or(0).min(4)];
                    let m = crate::etex::mark_read(self, name, 0)?;
                    let toks = crate::val::toks_rc(self.v, m);
                    self.push_toks(toks);
                }
                cmd::EXPAND_AFTER => {
                    // §368
                    self.get_token()?;
                    let t = self.cur.tok;
                    self.get_token()?;
                    if self.cur.cmd > cmd::MAX_COMMAND {
                        self.expand()?;
                    } else {
                        let u = self.cur.tok;
                        self.back_input(u);
                    }
                    self.back_input(t);
                }
                cmd::NO_EXPAND => {
                    // §369: `scanner_status:=normal` while the token is
                    // read (`\everyeof{\noexpand}`: a file's end met here
                    // is no runaway, §336)
                    let was = self.scanner.take();
                    let r = self.get_token();
                    self.scanner = was;
                    r?;
                    let t = self.cur.tok;
                    let cs = t.is_cs_or_active();
                    self.back_input(t);
                    if cs {
                        self.back_input(notexpanded_tok());
                    }
                }
                cmd::CS_NAME => self.cs_name()?,
                cmd::CONVERT => self.conv_toks()?,
                cmd::THE => {
                    let toks = self.the_toks()?;
                    self.push_toks(toks);
                }
                cmd::IF_TEST => self.conditional()?,
                cmd::FI_OR_ELSE => {
                    // §510
                    let limit = self.conds.last().copied().unwrap_or(OUTSIDE_LIMIT);
                    if self.cur.chr > i32::from(limit) {
                        if limit == IF_CODE {
                            self.insert_relax();
                        }
                        // "Extra \or/\else/\fi": ignored
                    } else {
                        while self.cur.chr != FI_CODE {
                            self.pass_text()?;
                        }
                        self.conds.pop();
                        self.if_codes.truncate(self.conds.len());
                    }
                }
                cmd::INPUT => {
                    if self.cur.chr > 0 {
                        self.end_input()?;
                    } else {
                        self.start_input()?;
                    }
                }
                _ => {
                    // §370: "Undefined control sequence" (dropped)
                    if crate::exec::env_flag!("PHITEX_TEX_TRACE")
                        || crate::exec::env_flag!("PHITEX_UNDEF_TRACE")
                    {
                        eprintln!(
                            "undefined {:?} in node {} (op {:?}, page {})",
                            self.cur.tok,
                            self.cx.node().raw(),
                            self.cx.op(),
                            self.page_index()
                        );
                    }
                    self.log("error", "Undefined control sequence")?;
                }
            }
        }
        (self.radix, self.cur_order) = saved;
        crate::etex::after_expand(self, c);
        Ok(())
    }

    /// `insert_relax` (§379).
    pub(crate) fn insert_relax(&mut self) {
        let t = self.cur.tok;
        self.back_input(t);
        self.back_input(relax_tok());
    }

    /// §372–§374.
    fn cs_name(&mut self) -> R<()> {
        let _in_csname = crate::etex::InCsname::enter();
        let mut name = NAME.with(std::cell::Cell::take);
        name.clear();
        let r = self.cs_name_chars(&mut name);
        let t = Tok::cs(&name);
        NAME.with(|c| c.set(name));
        r?;
        if self.cur.cmd != cmd::END_CS_NAME {
            // §373: back_error
            let t = self.cur.tok;
            self.back_input(t);
        }
        if self.mean_of(t)? == Mean::Undefined {
            let relax = Mean::Prim("relax", cmd::RELAX, 256).encode(self.v);
            let f = t.field_name().unwrap_or_default();
            self.set(&f, relax, false);
        }
        self.back_input(t);
        Ok(())
    }

    /// `macro_call` (§389–§400).
    /// §396: "Paragraph ended before \\x was complete".
    fn runaway(&mut self, name: Tok) -> R<()> {
        let mut s = String::from("Paragraph ended before ");
        let mut cs = String::new();
        self.show_toks_into(&mut cs, std::slice::from_ref(&name))?;
        s.push_str(cs.trim_end());
        s.push_str(" was complete");
        self.log("error", &s)
    }

    /// §372: the characters of a `\\csname` into `name`.
    fn cs_name_chars(&mut self, name: &mut String) -> R<()> {
        loop {
            self.get_x_token()?;
            if self.cur.is_cs() {
                return Ok(());
            }
            print::push_code(name, self.cur.tok.chr() & 0xFF);
        }
    }

    /// `macro_call` (§389–§400): the arguments are scanned into one
    /// per-thread buffer (ranges, no list per argument) and the body with
    /// its arguments substituted is built in another, then pushed as one
    /// shared list (one allocation a call).
    pub(crate) fn macro_call(&mut self) -> R<()> {
        let m = if let Some(m) = self.cur_macro() {
            m
        } else {
            let Mean::Macro(m) = self.cur_mean()? else {
                return Ok(());
            };
            m
        };
        if m.params.is_empty() {
            // no arguments: the body is read in place (§390); a body
            // already in the arena is pushed by its range
            let r = self.body_range(&m.body);
            self.push_range(r, crate::input::ty::MACRO);
            return Ok(());
        }
        let mut bufs = std::mem::take(&mut self.mbufs);
        // §389: `scanner_status:=matching; warning_index:=warning_index`
        let was = self
            .scanner
            .replace((crate::run::SCAN_MATCHING, self.cur.tok));
        let r = self.macro_args(&m, &mut bufs);
        self.scanner = was;
        if matches!(r, Ok(true)) {
            // §400: the body with its arguments substituted, written
            // straight into the arena as the macro's level
            let MacroBufs { args, bounds, .. } = &mut bufs;
            self.input.pop_spent();
            let start = self.input.arena.begin();
            for s in &*m.segs {
                self.input
                    .arena
                    .extend(&m.body[s.lo as usize..s.hi as usize]);
                if s.param != 0
                    && let Some(&(lo, hi)) = bounds.get(usize::from(s.param) - 1)
                {
                    self.input.arena.extend(&args[lo..hi]);
                }
            }
            let range = self.input.arena.finish(start);
            self.push_range(range, crate::input::ty::MACRO);
        }
        bufs.args.clear();
        bufs.bounds.clear();
        bufs.out.clear();
        self.mbufs = bufs;
        r.map(|_| ())
    }

    /// §391–§400: the arguments of `m` into `bufs` (`args` with their
    /// `bounds`); false when the call was abandoned.
    #[allow(clippy::too_many_lines)]
    fn macro_args(&mut self, m: &Macro, bufs: &mut MacroBufs) -> R<bool> {
        let name = self.cur.tok;
        let MacroBufs { args, bounds, .. } = bufs;
        args.clear();
        bounds.clear();
        let p: &[Tok] = &m.params;
        let len = p.len();
        let is_match = |t: &Tok| t.raw() >> 21 == u32::from(MATCH_CAT);
        let mut long_state = m.flags & 1 != 0;
        let mut r = 0usize;
        let par = par_tok();
        let space = crate::val::space_tok();
        loop {
            // §391
            let s: Option<usize> = if is_match(&p[r]) {
                r += 1;
                Some(r)
            } else {
                None
            };
            let start = args.len();
            let mut m_count = 0usize;
            let mut rbrace: Option<usize> = None;
            'cont: loop {
                let tok = self.get_token_tok()?;
                if r < len && !is_match(&p[r]) && tok == p[r] {
                    // §394
                    r += 1;
                    if r >= len || is_match(&p[r]) {
                        // §397: a delimiter ending `#{`: its brace is
                        // scanned again from the body, and counts in
                        // `align_state` once
                        if tok.raw() >> 21 == 1 {
                            let a = self.astate();
                            self.set_astate(a - 1);
                        }
                        break 'cont;
                    }
                    continue 'cont;
                }
                // §397
                if s != Some(r) {
                    let Some(s0) = s else {
                        // §398: "Use of \x doesn't match its definition"
                        self.log("error", "doesn't match its definition")?;
                        return Ok(false);
                    };
                    let mut t = s0;
                    loop {
                        args.push(p[t]);
                        m_count += 1;
                        let mut u = t + 1;
                        let mut v = s0;
                        let mut resumed = false;
                        loop {
                            if u == r {
                                if tok != p[v] {
                                    break;
                                }
                                r = v + 1;
                                resumed = true;
                                break;
                            }
                            if p[u] != p[v] {
                                break;
                            }
                            u += 1;
                            v += 1;
                        }
                        if resumed {
                            continue 'cont;
                        }
                        t += 1;
                        if t == r {
                            break;
                        }
                    }
                    r = s0;
                }
                if tok == par && (!long_state || self.outer_call) {
                    // §392: the `\par` §338 inserted ends the call quietly
                    if std::mem::take(&mut self.outer_call) {
                        return Ok(false);
                    }
                    // §396: a runaway argument; the \par is read again
                    self.runaway(name)?;
                    self.back_input(tok);
                    return Ok(false);
                }
                match tok {
                    x if x.raw() >> 21 == 1 => {
                        // §399
                        let mut unbalance = 1;
                        args.push(tok);
                        loop {
                            let t2 = self.get_token_tok()?;
                            if t2 == par && (!long_state || self.outer_call) {
                                if std::mem::take(&mut self.outer_call) {
                                    return Ok(false);
                                }
                                self.runaway(name)?;
                                self.back_input(t2);
                                return Ok(false);
                            }
                            if let Some(y) = t2.as_char() {
                                match y >> 21 {
                                    1 => unbalance += 1,
                                    2 => {
                                        unbalance -= 1;
                                        if unbalance == 0 {
                                            rbrace = Some(args.len() - start);
                                            args.push(t2);
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            args.push(t2);
                        }
                    }
                    x if x.raw() >> 21 == 2 => {
                        // §395: an extra }, and a \par inserted
                        self.back_input(tok);
                        self.back_input(par);
                        long_state = false;
                        continue 'cont;
                    }
                    _ => {
                        // §393
                        if tok == space && (r >= len || is_match(&p[r])) {
                            continue 'cont;
                        }
                        args.push(tok);
                    }
                }
                m_count += 1;
                if r < len && !is_match(&p[r]) {
                    continue 'cont;
                }
                break 'cont;
            }
            // found: §400
            if s.is_some() {
                let mut lo = start;
                let mut hi = args.len();
                if m_count == 1
                    && let Some(rb) = rbrace
                    && hi > lo
                    && args[hi - 1].raw() >> 21 == 2
                    && rb == hi - lo - 1
                {
                    lo += 1;
                    hi -= 1;
                }
                bounds.push((lo, hi));
            }
            if r >= len {
                break;
            }
        }
        Ok(true)
    }

    /// `pass_text` (§494): to the `\or`, `\else` or `\fi` at this level.
    pub(crate) fn pass_text(&mut self) -> R<()> {
        self.in_skip = true;
        let r = self.pass_text_in();
        self.in_skip = false;
        r
    }

    fn pass_text_in(&mut self) -> R<()> {
        let mut l = 0;
        loop {
            if !self.get_next()? {
                return Ok(());
            }
            if self.cur.cmd == cmd::FI_OR_ELSE {
                if l == 0 {
                    return Ok(());
                }
                if self.cur.chr == FI_CODE {
                    l -= 1;
                }
            } else if self.cur.cmd == cmd::IF_TEST {
                l += 1;
            }
        }
    }

    /// `get_x_token_or_active_char` (§506): `(cmd, chr)` for `\if`.
    fn x_char_or_active(&mut self) -> R<(u8, i32)> {
        self.get_x_token()?;
        let (mut c, mut h) = (self.cur.cmd, self.cur.chr);
        if c == cmd::RELAX && h == crate::val::NO_EXPAND_FLAG {
            if self.cur.tok.cat() == Some(13) && !self.cur.tok.is_cs() {
                c = cmd::ACTIVE_CHAR;
                h = i32::try_from(self.cur.tok.chr()).unwrap_or(0);
            } else {
                c = cmd::ACTIVE_CHAR;
                h = 256;
            }
        }
        if c > cmd::ACTIVE_CHAR || h > 255 {
            return Ok((cmd::RELAX, 256));
        }
        Ok((c, h))
    }

    /// §507: `\ifx`'s two tokens compared.
    fn if_x(&mut self) -> R<bool> {
        Ok(if self.get_next()? {
            let m1 = self.cur_mean()?;
            let t1 = self.cur.tok;
            let (p, q) = self.cur_cmd_chr_for_ifx(t1, &m1);
            if self.get_next()? {
                let m2 = self.cur_mean()?;
                let t2 = self.cur.tok;
                let (c2, h2) = self.cur_cmd_chr_for_ifx(t2, &m2);
                if c2 != p {
                    false
                } else if c2 < cmd::CALL {
                    h2 == q
                } else {
                    match (&m1, &m2) {
                        (Mean::Macro(a), Mean::Macro(b)) => {
                            Rc::ptr_eq(a, b) || (a.params == b.params && a.body == b.body)
                        }
                        _ => false,
                    }
                }
            } else {
                false
            }
        } else {
            false
        })
    }

    /// `conditional` (§498–§509).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn conditional(&mut self) -> R<()> {
        self.conds.push(IF_CODE);
        let save = self.conds.len();
        crate::etex::if_begin(self, self.cur.chr);
        let negate = self.cur.chr >= crate::etex::UNLESS_CODE;
        let this_if = self.cur.chr % crate::etex::UNLESS_CODE;
        let b = match this_if {
            0 | 1 => {
                let (m, n) = self.x_char_or_active()?;
                let (c, h) = self.x_char_or_active()?;
                if this_if == 0 { n == h } else { m == c }
            }
            2 | 3 => {
                let n = if this_if == 2 {
                    self.scan_int()?
                } else {
                    self.scan_normal_dimen()?
                };
                self.skip_blanks_x()?;
                let rel = match self.cur.tok {
                    _ if self.cur.tok.cat() == Some(12)
                        && (60..=62).contains(&self.cur.tok.chr()) =>
                    {
                        self.cur.tok.chr()
                    }
                    _ => {
                        let t = self.cur.tok;
                        self.back_input(t);
                        61
                    }
                };
                let x = if this_if == 2 {
                    self.scan_int()?
                } else {
                    self.scan_normal_dimen()?
                };
                match rel {
                    60 => n < x,
                    61 => n == x,
                    _ => n > x,
                }
            }
            4 => self.scan_int()? % 2 != 0,
            5..=8 => {
                let m = self.mode()?.m;
                match this_if {
                    5 => m.abs() == 1,
                    6 => m.abs() == 102,
                    7 => m.abs() == 203,
                    _ => m < 0,
                }
            }
            9..=11 => {
                let n = self.scan_eight_bit_int()?;
                let bx = self.box_reg(n)?;
                match bx {
                    None => this_if == 9,
                    Some(b) => {
                        let hl = crate::nodes::box_is_h(self.v, b);
                        match this_if {
                            9 => false,
                            10 => hl,
                            _ => !hl,
                        }
                    }
                }
            }
            12 => {
                // §507: `scanner_status:=normal` while the two tokens are
                // read (a file's end between them is no runaway, §336)
                let was = self.scanner.take();
                let r = self.if_x();
                self.scanner = was;
                r?
            }
            13 => {
                let n = self.scan_four_bit_int()?;
                let st = self.get(&format!("read:{n}"))?;
                st.is_none_or(|x| matches!(&*self.v.get(x), Value::Unit))
            }
            14 => true,
            15 => false,
            17.. => crate::etex::if_test(self, this_if)?,
            _ => {
                // §509
                let mut n = self.scan_int()?;
                while n != 0 {
                    self.pass_text()?;
                    if self.conds.len() == save {
                        if self.cur.chr == OR_CODE {
                            n -= 1;
                        } else {
                            self.common_ending(save);
                            return Ok(());
                        }
                    } else if self.cur.chr == FI_CODE {
                        self.conds.pop();
                        self.if_codes.truncate(self.conds.len());
                    }
                }
                self.change_if_limit(OR_CODE, save);
                return Ok(());
            }
        };
        if b != negate {
            self.change_if_limit(ELSE_CODE, save);
            return Ok(());
        }
        // §500
        loop {
            self.pass_text()?;
            if !self.has_cur_cond(save) {
                // the input ended
                break;
            }
            if self.conds.len() == save {
                if self.cur.chr != OR_CODE {
                    break;
                }
                // "Extra \or": ignored
            } else if self.cur.chr == FI_CODE {
                self.conds.pop();
                self.if_codes.truncate(self.conds.len());
            }
        }
        self.common_ending(save);
        Ok(())
    }

    fn has_cur_cond(&self, save: usize) -> bool {
        self.conds.len() >= save
    }

    fn cur_cmd_chr_for_ifx(&self, _t: Tok, m: &Mean) -> (u8, i32) {
        (
            self.cur.cmd,
            if matches!(m, Mean::Macro(_)) {
                0
            } else {
                self.cur.chr
            },
        )
    }

    fn common_ending(&mut self, save: usize) {
        if self.cur.chr == FI_CODE {
            self.conds.truncate(save - 1);
            self.if_codes.truncate(self.conds.len());
        } else {
            self.change_if_limit(FI_CODE, save);
        }
    }

    fn change_if_limit(&mut self, l: i32, save: usize) {
        if let Some(x) = self.conds.get_mut(save - 1) {
            *x = u8::try_from(l).unwrap_or(4);
        }
    }

    /// `the_toks` (§465–§466).
    pub(crate) fn the_toks(&mut self) -> R<Vec<Tok>> {
        if self.cur.cmd == cmd::THE && self.cur.chr != 0 {
            return crate::etex::the_toks(self);
        }
        self.get_x_token()?;
        let val = self.scan_something_internal(5, false)?;
        Ok(match val {
            crate::scan::IVal::Toks(t) => t,
            crate::scan::IVal::Ident(f) => vec![self.font_id_tok(f)?],
            other => {
                let mut s = String::new();
                other.print(&mut s);
                str_toks(&s)
            }
        })
    }

    /// The font identifier token of font `f` (§1257: `font_id_text`).
    pub(crate) fn font_id_tok(&mut self, f: i64) -> R<Tok> {
        let x = self.get(&format!("fontid:{f}"))?;
        let name = x.map_or_else(|| "nullfont".to_owned(), |x| str_of(self.v, x));
        Ok(Tok::cs(&format!("\u{10FFFE}F{f}\u{2}{name}")))
    }

    /// `conv_toks` (§470–§472).
    fn conv_toks(&mut self) -> R<()> {
        let c = self.cur.chr;
        let mut s = String::new();
        match c {
            0 => {
                let n = self.scan_int()?;
                print::print_int(&mut s, i64::from(n));
            }
            1 => {
                let n = self.scan_int()?;
                print::print_roman_int(&mut s, i64::from(n));
            }
            2 => {
                self.get_token()?;
                let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
                if self.cur.is_cs() {
                    print::sprint_cs(&mut s, self.cur.tok, esc);
                } else {
                    print::push_code(&mut s, self.cur.tok.chr());
                }
            }
            3 => {
                self.get_token()?;
                let (t, m) = (self.cur.tok, self.cur_mean()?);
                let (c, h) = (self.cur.cmd, self.cur.chr);
                let _ = t;
                self.print_meaning(&mut s, &m, c, h)?;
            }
            4 => {
                let f = self.scan_font_ident()?;
                self.print_font_name(&mut s, f)?;
            }
            _ => {
                let j = self.get_loc(crate::fields::cell::JOBNAME)?;
                s.push_str(&j.map_or_else(|| "texput".to_owned(), |x| str_of(self.v, x)));
            }
        }
        let toks: Rc<[Tok]> = s.chars().map(char_tok).collect();
        self.push_toks(toks);
        Ok(())
    }

    pub(crate) fn print_font_name(&mut self, s: &mut String, f: i64) -> R<()> {
        let info = self.get(&format!("fontinfo:{f}"))?;
        if let Some(info) = info {
            let xs = crate::val::list_of(self.v, info);
            let name = xs.first().map(|&x| str_of(self.v, x)).unwrap_or_default();
            s.push_str(&name);
            let size = xs.get(1).map_or(0, |&x| int_of(self.v, x));
            let dsize = xs.get(2).map_or(0, |&x| int_of(self.v, x));
            if size != dsize {
                s.push_str(" at ");
                print::print_scaled(s, i32::try_from(size).unwrap_or(0));
                s.push_str("pt");
            }
        } else {
            s.push_str("nullfont");
        }
        Ok(())
    }

    /// `print_meaning` (§296) with `print_cmd_chr` (§298 and the others).
    pub(crate) fn print_meaning(&mut self, s: &mut String, m: &Mean, c: u8, h: i32) -> R<()> {
        let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
        match m {
            Mean::Macro(mac) => {
                if mac.flags & crate::etex::PROTECTED != 0 {
                    print::print_esc(s, "protected", esc);
                    if mac.flags.trailing_zeros() >= 2 {
                        s.push(' ');
                    }
                }
                match mac.flags & 3 {
                    0 => s.push_str("macro"),
                    1 => print::print_esc(s, "long macro", esc),
                    2 => print::print_esc(s, "outer macro", esc),
                    _ => {
                        print::print_esc(s, "long", esc);
                        print::print_esc(s, "outer macro", esc);
                    }
                }
                s.push(':');
                self.show_macro(s, mac, esc)?;
            }
            Mean::Undefined => s.push_str("undefined"),
            Mean::Char(cat, ch) => {
                if let Some(n) = print::chr_cmd_name(*cat) {
                    s.push_str(n);
                    print::push_code(s, *ch);
                }
            }
            Mean::CharDef(n) => {
                print::print_esc(s, "char", esc);
                print::print_hex(s, *n);
            }
            Mean::MathCharDef(n) => {
                print::print_esc(s, "mathchar", esc);
                print::print_hex(s, *n);
            }
            Mean::Font(f) => {
                s.push_str("select font ");
                self.print_font_name(s, *f)?;
            }
            Mean::Reg(rc, loc) => {
                let (name, base) = match *rc {
                    cmd::ASSIGN_INT => ("count", crate::val::COUNT_BASE),
                    cmd::ASSIGN_DIMEN => ("dimen", crate::val::SCALED_BASE),
                    cmd::ASSIGN_GLUE => ("skip", crate::val::SKIP_BASE),
                    cmd::ASSIGN_MU_GLUE => ("muskip", crate::val::MU_SKIP_BASE),
                    _ => ("toks", crate::val::TOKS_BASE),
                };
                print::print_esc(s, name, esc);
                print::print_int(s, i64::from(loc - base));
            }
            Mean::NotExpanded => print::print_esc(s, "relax", esc),
            Mean::Prim(name, pc, _) => {
                if *pc == cmd::SET_FONT {
                    s.push_str("select font ");
                    self.print_font_name(s, 0)?;
                } else {
                    print::print_esc(s, name, esc);
                }
                if c == cmd::TOP_BOT_MARK && h < crate::etex::MARKS_CODE {
                    s.push(':');
                    let name = ["top", "first", "bot", "splitfirst", "splitbot"]
                        [usize::try_from(h).unwrap_or(0).min(4)];
                    let mk = crate::etex::mark_read(self, name, 0)?;
                    let toks = crate::val::toks_of(self.v, mk);
                    self.show_toks_into(s, &toks)?;
                }
            }
        }
        Ok(())
    }

    fn show_macro(&mut self, s: &mut String, m: &Macro, _esc: i64) -> R<()> {
        let mut all = m.params.clone();
        all.push(crate::val::end_match());
        all.extend(m.body.iter().copied());
        self.show_toks_into(s, &all)
    }

    /// `show_token_list` with the current `\escapechar` and category codes.
    pub(crate) fn show_toks_into(&mut self, s: &mut String, toks: &[Tok]) -> R<()> {
        let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
        // single-character names: their current category code (§262)
        let mut cats: crate::FxHashMap<u32, i64> = crate::FxHashMap::default();
        for t in toks {
            if let Some(n) = t.as_cs() {
                let mut cs = n.chars();
                if let (Some(c), None) = (cs.next(), cs.next()) {
                    let k = u32::from(c);
                    if let std::collections::hash_map::Entry::Vacant(e) = cats.entry(k) {
                        e.insert(i64::from(self.cat(k)?));
                    }
                }
            }
        }
        print::show_toks(s, toks, esc, &mut |c| cats.get(&c).copied().unwrap_or(12));
        Ok(())
    }

    /// `\endinput` (§362): the rest of the current file is skipped once
    /// its current line is done. The rest may be in the siblings that
    /// follow (a run's leftover input becomes children): they are pulled
    /// in until the file's end marker is reached.
    fn end_input(&mut self) -> R<()> {
        loop {
            if self.end_input_prune() || !self.pull()? {
                break;
            }
        }
        crate::etex::force_eof(self);
        Ok(())
    }

    /// `start_input` (§537): the file's lines are read next; a marker
    /// prints its `)` when it ends.
    fn start_input(&mut self) -> R<()> {
        let name = self.scan_file_name()?;
        let key = if name.contains('.') {
            name.clone()
        } else {
            format!("{name}.tex")
        };
        let x = self.read_file(&key)?;
        let x = match x {
            Some(x) => Some(x),
            None => {
                if key == name {
                    None
                } else {
                    self.read_file(&name)?
                }
            }
        };
        let Some(x) = x else {
            // §530: in the nonstop modes (a build's, [`Run::fatal`]) the
            // job ends
            self.log("error", "I can't find file")?;
            return self.fatal("*** (job aborted, file error in nonstop mode)");
        };
        // (a job-written file whose read-back only defines)
        self.read_back(&key)?;
        let xs = crate::val::list_of(self.v, x);
        if xs.len() < 2 {
            self.log("error", "I can't find file")?;
            return self.fatal("*** (job aborted, file error in nonstop mode)");
        }
        let path = str_of(self.v, xs[0]);
        self.log("open", &path)?;
        self.push_toks([Tok::cs("\u{10FFFE}eof")]);
        self.input
            .push_file(crate::run::FileRest::new(self.v, xs[1], 0, xs[0]));
        Ok(())
    }

    /// `scan_file_name` (§526).
    pub(crate) fn scan_file_name(&mut self) -> R<String> {
        self.skip_blanks_x()?;
        let mut name = String::new();
        let mut quoted = false;
        loop {
            if self.cur.cmd > cmd::OTHER_CHAR
                || self.cur.chr > 255
                || self.cur.is_cs() && !matches!(self.cur_mean()?, Mean::Char(..))
            {
                let t = self.cur.tok;
                self.back_input(t);
                break;
            }
            // web2c's more_name (tex.ch, §516): a `"` toggles quoting
            // and is not part of the name; a space ends it unquoted
            if self.cur.chr == 32 && !quoted {
                break;
            }
            if self.cur.chr == 34 {
                quoted = !quoted;
            } else {
                print::push_code(&mut name, u32::try_from(self.cur.chr).unwrap_or(0));
            }
            self.get_x_token()?;
        }
        Ok(name)
    }
}

/// The lines of a file's text, each with its end of line.
pub(crate) fn split_lines(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text.split_inclusive('\n').map(str::to_owned).collect();
    if out.last().is_some_and(std::string::String::is_empty) {
        out.pop();
    }
    out
}

/// `str_toks` (§464).
pub(crate) fn str_toks(s: &str) -> Vec<Tok> {
    let mut v = Vec::with_capacity(s.len());
    v.extend(s.chars().map(char_tok));
    v
}

fn char_tok(c: char) -> Tok {
    if c == ' ' {
        crate::val::space_tok()
    } else {
        Tok::ch(12, u32::from(c))
    }
}
