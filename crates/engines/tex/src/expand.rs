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

thread_local! {
    pub(crate) static NAME: std::cell::Cell<String> = std::cell::Cell::default();
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
cs_once!(frozen_fi_tok, "\u{10FFFE}fi");
cs_once!(notexpanded_tok, "\u{10FFFE}notexpanded");
cs_once!(halt_tok, "\u{10FFFE}halt");

impl Run<'_, '_> {
    /// `expand` (§366): `cur` is expandable. A halt is raised, never
    /// returned (spec-final §2.2).
    pub(crate) fn expand(&mut self) {
        let c = self.cur.cmd;
        if (cmd::CALL..cmd::END_TEMPLATE).contains(&c) {
            // (the common case: no other state to keep)
            let saved = (self.radix, self.cur_order);
            self.macro_call();
            (self.radix, self.cur_order) = saved;
            return;
        }
        if let Err(h) = self.expand_other() {
            self.raise(h);
        }
    }

    /// `expand` of what is not a macro call.
    fn expand_other(&mut self) -> R<()> {
        let saved = (self.radix, self.cur_order);
        let c = self.cur.cmd;
        if crate::etex::expand(self)? {
            (self.radix, self.cur_order) = saved;
            return Ok(());
        }
        {
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
                    // §368 (the first token is stepped over: of its
                    // meaning only whether it is outer is read, §336)
                    self.outer_reads = true;
                    self.get_token();
                    self.outer_reads = false;
                    self.ck()?;
                    let t = self.cur.tok;
                    self.get_token();
                    self.ck()?;
                    if self.cur.cmd > cmd::MAX_COMMAND {
                        self.expand();
                        self.ck()?;
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
                    // (only the token is used: of its meaning, the class a
                    // stored token's fetch reads)
                    let was = self.scanner.take();
                    self.get_token_stored();
                    self.scanner = was;
                    self.ck()?;
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
                    let toks = self.the_toks();
                    self.ck()?;
                    self.push_toks(toks);
                }
                cmd::IF_TEST => self.conditional()?,
                cmd::FI_OR_ELSE => self.fi_or_else()?,
                cmd::INPUT => {
                    if self.cur.chr > 0 {
                        self.end_input()?;
                    } else {
                        self.start_input()?;
                    }
                }
                _ => {
                    // §370: "Undefined control sequence" (dropped)
                    if crate::exec::obs_flag!("PHITEX_TEX_TRACE")
                        || crate::exec::obs_flag!("PHITEX_UNDEF_TRACE")
                    {
                        // (debug: what the engine has for the name where
                        // this node stands, and who defined it)
                        let t = format!("{:?}", self.cur.tok);
                        let at = t
                            .strip_prefix("S(\"")
                            .and_then(|x| x.strip_suffix("\")"))
                            .map(|n| {
                                let f = self.v.field(&format!("cs:{n}"));
                                {
                                    let l = self.loc_of(f);
                                    let here = self.cx.read(f);
                                    format!("here {here:?}, local {}", self.st.debug_slot(l))
                                }
                            })
                            .unwrap_or_default();
                        eprintln!(
                            "undefined {t} in node {} (op {:?}, page {}): engine {at}",
                            self.cx.node().raw(),
                            self.cx.op(),
                            self.page_nth()
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
        // (§372: of the name's meaning only whether it is defined)
        let undefined = match self.defined_lookup(t)? {
            Some(d) => !d,
            None => self.mean_of(t)? == Mean::Undefined,
        };
        if undefined {
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

    /// `overflow` (§94): TeX's capacity `what` (`n`) exceeded; the job
    /// ends there, the halt raised.
    #[cold]
    pub(crate) fn overflow_raise(&mut self, what: &str, n: usize) {
        if crate::exec::obs_flag!("PHITEX_OVERFLOW_TRACE") {
            let a: String = self.cx.args().first().map(|&x| self.v.render(x)).unwrap_or_default().chars().take(300).collect();
            let lv: Vec<(u8, u32, u32, u32)> = self.input.levels.iter().rev().take(6).map(|l| (l.kind, l.c, l.a, l.b)).collect();
            let mut top = Vec::new();
            for (k, c, _, b) in lv {
                let toks: Vec<Tok> = if k == crate::input::kind::TOKS && c < b { self.input.arena.get(crate::input::Range { start: c, end: b.min(c + 12) }).to_vec() } else { Vec::new() };
                let mut t = String::new();
                let _ = self.show_toks_into(&mut t, &toks);
                top.push(t);
            }
            eprintln!("overflow: node {} {what}: levels {} arena {} text {a} top {top:?}", self.cx.node().raw(), self.input.levels.len(), self.input.arena.len());
        }
        if let Err(h) = self.fatal::<()>(&format!("TeX capacity exceeded, sorry [{what}={n}]")) {
            self.raise(h);
        }
    }

    /// §372: the characters of a `\\csname` into `name`.
    fn cs_name_chars(&mut self, name: &mut String) -> R<()> {
        loop {
            self.get_x_token();
            self.ck()?;
            if self.cur.is_cs() {
                return Ok(());
            }
            print::push_code(name, self.cur.tok.chr() & 0xFF);
        }
    }

    /// `macro_call` (§389–§400) over the arena (spec-final §2.3): the
    /// arguments are matched straight into the arena's scratch region
    /// ([`crate::input::Arena::scratch`]), and the expansion is written
    /// in one pass (the body with the argument ranges copied in) and
    /// pushed as the macro's level. No list per argument, no buffer of
    /// the run's own. A halt is raised, never returned.
    pub(crate) fn macro_call(&mut self) {
        // (the engine bounds a run on a guess by its work: an expansion
        // that never ends, as `\edef` of a macro calling itself, makes
        // no command)
        self.exps += 1;
        if self.exps.trailing_zeros() >= 12
            && let Err(s) = self.cx.tick(0x1000)
        {
            self.raise(crate::run::Halt::Stop(s));
            return;
        }
        // (§321 `push_input`'s and §120 `get_avail`'s overflows: a
        // command whose expansion never ends fills them)
        if self.input.levels.len() >= crate::input::STACK_SIZE {
            return self.overflow_raise("input stack size", crate::input::STACK_SIZE);
        }
        if self.input.arena.len() >= crate::input::ARENA_MAX {
            return self.overflow_raise("main memory size", crate::input::MAIN_MEMORY);
        }
        let name = self.cur.tok;
        let Some(m) = self.cur_macro() else {
            return;
        };
        if m.params.is_empty() {
            // no arguments: the body is read in place (§390); a body
            // already in the arena is pushed by its range
            let r = self.body_range(&m.body);
            self.push_range(r, crate::input::ty::MACRO);
            return;
        }
        // §389: `scanner_status:=matching; warning_index:=warning_index`
        let was = self
            .scanner
            .replace((crate::run::SCAN_MATCHING, self.cur.tok));
        let base = self.input.arena.scratch_begin();
        let ok = self.macro_args(&m, base);
        self.scanner = was;
        if ok && self.halt.is_none() && m.params.len() == 2 && self.clist_call(name, &m, base) {
            self.input.arena.scratch_end(base);
            return;
        }
        if ok && self.halt.is_none() {
            // §400: the body with its arguments substituted, written
            // straight into the arena as the macro's level
            self.input.pop_spent();
            let range = self.input.arena.expand_body(&m.body, &m.segs, base);
            self.push_range(range, crate::input::ty::MACRO);
        }
        self.input.arena.scratch_end(base);
    }

    /// microtype's `\\MT@in@clist{x}\\list` (whether `x` is an item of the
    /// comma list `\\list` holds: `\\ifMT@inlist@`), its arguments
    /// matched since `base`: the list read as the item's membership
    /// ([`Run::clist_has`]), so a list grown by other items (a font
    /// another piece used first) leaves the run standing. Its body runs
    /// as written up to its `\\def\\MT@res@a…{…}`, then the switch its
    /// match would set (`\\MT@inlist@true` or `false`): the same
    /// definitions, the list's expansion and the match left out. False
    /// (the call runs whole) for any other macro, an item holding a comma,
    /// a brace or `\\@empty`/`\\@nnil`, a list that is not one
    /// ([`crate::val::clist_projection`]). (No macro is traced here:
    /// `\\tracingmacros` writes nothing in this engine.)
    fn clist_call(&mut self, name: Tok, m: &Macro, base: crate::input::Scratch) -> bool {
        const BODY: [&str; 14] = [
            "def",
            "MT@res@a",
            "@nnil",
            "ifx",
            "@empty",
            "MT@inlist@false",
            "else",
            "MT@inlist@true",
            "fi",
            "expandafter",
            "MT@res@a",
            "expandafter",
            "@empty",
            "@nnil",
        ];
        static CLIST: std::sync::OnceLock<Tok> = std::sync::OnceLock::new();
        if name != *CLIST.get_or_init(|| Tok::cs("MT@in@clist"))
            || !m.body.iter().filter_map(|t| t.as_cs().map(crate::val::CsId::name)).eq(BODY)
        {
            return false;
        }
        let (Some(item), Some(list)) = (self.input.arena.scratch_arg(base, 0), self.input.arena.scratch_arg(base, 1)) else {
            return false;
        };
        let (item, list) = (item.to_vec(), list.to_vec());
        let plain = |t: &Tok| match t.as_cs() {
            Some(c) => !matches!(c.name(), "@empty" | "@nnil"),
            None => !matches!(t.cat(), Some(1 | 2)) && (t.cat() != Some(12) || t.chr() != u32::from(b',')),
        };
        if item.is_empty() || !item.iter().all(plain) || list.len() != 1 || !list[0].is_cs() {
            return false;
        }
        let has = match self.lift(|r| r.clist_has(list[0], &item)) {
            None => return true,
            Some(None) => return false,
            Some(Some(h)) => h,
        };
        // the body with its arguments, to the end of its `\def`
        let mut toks: Vec<Tok> = Vec::with_capacity(m.body.len() + item.len());
        for sg in &m.segs {
            toks.extend_from_slice(&m.body[sg.lo as usize..sg.hi as usize]);
            match sg.param {
                0 => {}
                1 => toks.extend_from_slice(&item),
                _ => toks.extend_from_slice(&list),
            }
        }
        let Some(open) = toks.iter().position(|t| t.cat() == Some(1)) else {
            return false;
        };
        let mut depth = 0i32;
        let mut close = None;
        for (i, t) in toks.iter().enumerate().skip(open) {
            match t.cat() {
                Some(1) => depth += 1,
                Some(2) => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let want = if has { "MT@inlist@true" } else { "MT@inlist@false" };
        let (Some(close), Some(&sw)) = (close, toks.iter().find(|t| t.as_cs().is_some_and(|c| c.name() == want))) else {
            return false;
        };
        toks.truncate(close + 1);
        toks.push(sw);
        self.input.pop_spent();
        let r = self.input.arena.alloc(&toks);
        self.push_range(r, crate::input::ty::MACRO);
        true
    }

    /// §391–§400: the arguments of `m` into the arena's scratch region
    /// (from `base`: its tokens, each argument's bounds); false when the
    /// call was abandoned or a halt raised.
    #[allow(clippy::too_many_lines)]
    fn macro_args(&mut self, m: &Macro, base: crate::input::Scratch) -> bool {
        let name = self.cur.tok;
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
            let start = self.input.arena.scratch_len(base);
            let mut m_count = 0usize;
            let mut rbrace: Option<usize> = None;
            'cont: loop {
                let tok = self.get_token_tok();
                if self.halt.is_some() {
                    return false;
                }
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
                        self.lift(|r| r.log("error", "doesn't match its definition"));
                        return false;
                    };
                    let mut t = s0;
                    loop {
                        self.input.arena.scratch_push(p[t]);
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
                        return false;
                    }
                    // §396: a runaway argument; the \par is read again
                    self.lift(|r| r.runaway(name));
                    self.back_input(tok);
                    return false;
                }
                match tok {
                    x if x.raw() >> 21 == 1 => {
                        // §399
                        let mut unbalance = 1;
                        self.input.arena.scratch_push(tok);
                        loop {
                            let t2 = self.get_token_tok();
                            if self.halt.is_some() {
                                return false;
                            }
                            if t2 == par && (!long_state || self.outer_call) {
                                if std::mem::take(&mut self.outer_call) {
                                    return false;
                                }
                                self.lift(|r| r.runaway(name));
                                self.back_input(t2);
                                return false;
                            }
                            if let Some(y) = t2.as_char() {
                                match y >> 21 {
                                    1 => unbalance += 1,
                                    2 => {
                                        unbalance -= 1;
                                        if unbalance == 0 {
                                            rbrace =
                                                Some(self.input.arena.scratch_len(base) - start);
                                            self.input.arena.scratch_push(t2);
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            self.input.arena.scratch_push(t2);
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
                        self.input.arena.scratch_push(tok);
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
                let mut hi = self.input.arena.scratch_len(base);
                if m_count == 1
                    && let Some(rb) = rbrace
                    && hi > lo
                    && self.input.arena.scratch_at(base, hi - 1).raw() >> 21 == 2
                    && rb == hi - lo - 1
                {
                    lo += 1;
                    hi -= 1;
                }
                self.input.arena.scratch_bound(lo, hi);
            }
            if r >= len {
                break;
            }
        }
        true
    }

    /// A step that may stop, on the token path: its halt raised
    /// ([`Run::raise`]), never returned.
    #[inline]
    pub(crate) fn lift<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> Option<T> {
        match f(self) {
            Ok(x) => Some(x),
            Err(h) => {
                self.raise(h);
                None
            }
        }
    }

    /// `pass_text` (§494): to the `\or`, `\else` or `\fi` at this level.
    /// A halt is raised, never returned.
    pub(crate) fn pass_text(&mut self) {
        self.in_skip = true;
        // (§494: `skip_line:=line`, for §336's message)
        self.skip_from = self.line_mark();
        let mut l = 0;
        loop {
            if !self.get_next() || self.halt.is_some() {
                break;
            }
            if self.cur.cmd == cmd::FI_OR_ELSE {
                if l == 0 {
                    break;
                }
                if self.cur.chr == FI_CODE {
                    l -= 1;
                }
            } else if self.cur.cmd == cmd::IF_TEST {
                l += 1;
            }
        }
        self.in_skip = false;
    }

    /// `get_x_token_or_active_char` (§506): `(cmd, chr)` for `\if`.
    fn x_char_or_active(&mut self) -> R<(u8, i32)> {
        self.get_x_token();
        self.ck()?;
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
        if !self.next_ck()? {
            return Ok(false);
        }
        let (p, h1, m1) = self.ifx_side()?;
        if !self.next_ck()? {
            return Ok(false);
        }
        let (c2, h2, m2) = self.ifx_side()?;
        Ok(if c2 != p {
            false
        } else if c2 < cmd::CALL {
            h2 == h1
        } else {
            match (m1, m2) {
                (Some(a), Some(b)) => Rc::ptr_eq(&a, &b) || (a.params == b.params && a.body == b.body),
                _ => false,
            }
        })
    }

    /// One side of `\ifx` (§507): the current token's command code, its
    /// modifier (0 for a macro), and its macro if it is one. A decoded
    /// meaning of the run's table is read in place, no `Mean` made.
    fn ifx_side(&mut self) -> R<(u8, i32, Option<Rc<Macro>>)> {
        let (c, h) = (self.cur.cmd, self.cur.chr);
        if self.interp.is_none()
            && !(c == cmd::RELAX && h == crate::val::NO_EXPAND_FLAG)
            && !self.cur.tok.is_frozen_cs()
            && let Some(i) = self.cur.tok.cs_index()
        {
            let l = crate::runstate::Loc::try_from(crate::fields::MEAN_BASE + i).unwrap_or(u32::MAX);
            if self.st.meaning_entry(l).is_some() {
                return Ok(match self.st.macro_at(l) {
                    Some(m) => (c, 0, Some(m)),
                    None => (c, h, None),
                });
            }
        }
        let t = self.cur.tok;
        let m = self.cur_mean()?;
        let (c, h) = self.cur_cmd_chr_for_ifx(t, &m);
        Ok(match m {
            Mean::Macro(x) => (c, h, Some(x)),
            _ => (c, h, None),
        })
    }

    /// `\fi`, `\else`, `\or` (§510), a conditional begun before this
    /// node ended from the stack it left ([`Run::load_cond_stack`]).
    fn fi_or_else(&mut self) -> R<()> {
        if self.conds.is_empty() {
            self.load_cond_stack()?;
        }
        let limit = self.conds.last().copied().unwrap_or(OUTSIDE_LIMIT);
        if self.cur.chr > i32::from(limit) {
            if limit == IF_CODE {
                self.insert_relax();
            }
            // "Extra \or/\else/\fi": ignored
        } else {
            while self.cur.chr != FI_CODE {
                self.pass_text();
                self.ck()?;
            }
            self.muts += 1;
            self.conds.pop();
            self.if_codes.truncate(self.conds.len());
        }
        Ok(())
    }

    /// `conditional` (§498–§509).
    #[allow(clippy::too_many_lines)]
    pub(crate) fn conditional(&mut self) -> R<()> {
        self.muts += 1;
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
            2 => {
                // (`\ifnum`: an operand that is a macro's number is read as
                // the comparison's outcome alone, `crate::scan::Num`)
                use crate::scan::Num;
                let a = self.scan_int_num()?;
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
                let b = self.scan_int_num()?;
                // (the order of the first operand against the second)
                let o = match (a, b) {
                    (Num::Int(n), Num::Int(x)) => order(n, x),
                    (Num::Macro(t, neg), Num::Int(x)) => {
                        // (-m against x is m against -x, reversed)
                        if neg {
                            -self.num_order(t, -i64::from(x))?
                        } else {
                            self.num_order(t, i64::from(x))?
                        }
                    }
                    (Num::Int(n), Num::Macro(t, neg)) => {
                        if neg {
                            self.num_order(t, -i64::from(n))?
                        } else {
                            -self.num_order(t, i64::from(n))?
                        }
                    }
                    (a, b) => {
                        let (n, x) = (self.num_int(a)?, self.num_int(b)?);
                        order(n, x)
                    }
                };
                match rel {
                    60 => o < 0,
                    61 => o == 0,
                    _ => o > 0,
                }
            }
            3 => {
                let n = self.scan_normal_dimen()?;
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
                let x = self.scan_normal_dimen()?;
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
                st.is_none_or(|x| self.v.with(x, |y| matches!(y, Value::Unit)))
            }
            14 => true,
            15 => false,
            17.. => crate::etex::if_test(self, this_if)?,
            _ => {
                // §509
                let mut n = self.scan_int()?;
                while n != 0 {
                    self.pass_text();
                    self.ck()?;
                    if self.conds.len() == save {
                        if self.cur.chr == OR_CODE {
                            n -= 1;
                        } else {
                            self.common_ending(save);
                            return Ok(());
                        }
                    } else if self.cur.chr == FI_CODE {
                        self.muts += 1;
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
            self.pass_text();
            self.ck()?;
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
                self.muts += 1;
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
            self.muts += 1;
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

    /// `the_toks` (§465–§466): a halt is raised, never returned (the
    /// list is then empty).
    pub(crate) fn the_toks(&mut self) -> Vec<Tok> {
        self.lift(Self::the_toks_r).unwrap_or_default()
    }

    fn the_toks_r(&mut self) -> R<Vec<Tok>> {
        if self.cur.cmd == cmd::THE && self.cur.chr != 0 {
            return crate::etex::the_toks(self);
        }
        self.get_x_token();
        self.ck()?;
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
        let x = self.font_get(f, crate::fields::FontCell::Id)?;
        let name = x.map_or_else(|| "nullfont".to_owned(), |x| str_of(self.v, x));
        Ok(Tok::cs(&format!("\u{10FFFE}F{f}\u{2}{name}")))
    }

    /// `conv_toks` (§470–§472).
    fn conv_toks(&mut self) -> R<()> {
        let c = self.cur.chr;
        if c == 0 {
            // (`\number`: its digits onto the input, §465, no string made)
            let n = self.scan_int()?;
            let mut buf = [Tok::c(0); 11];
            let mut k = buf.len();
            let mut x = n.unsigned_abs();
            loop {
                k -= 1;
                buf[k] = Tok::ch(12, u32::from(b'0') + x % 10);
                x /= 10;
                if x == 0 {
                    break;
                }
            }
            if n < 0 {
                k -= 1;
                buf[k] = Tok::ch(12, u32::from(b'-'));
            }
            self.push_toks(&buf[k..]);
            return Ok(());
        }
        let mut s = String::new();
        match c {
            1 => {
                let n = self.scan_int()?;
                print::print_roman_int(&mut s, i64::from(n));
            }
            2 => {
                self.get_token();
                self.ck()?;
                let esc = i64::from(self.ipar(crate::fields::Par::escapechar)?);
                if self.cur.is_cs() {
                    print::sprint_cs(&mut s, self.cur.tok, esc);
                } else {
                    print::push_code(&mut s, self.cur.tok.chr());
                }
            }
            3 => {
                self.get_token();
                self.ck()?;
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
        self.push_toks(str_toks(&s));
        Ok(())
    }

    pub(crate) fn print_font_name(&mut self, s: &mut String, f: i64) -> R<()> {
        let info = self.font_get(f, crate::fields::FontCell::Info)?;
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
        // (a file the job writes whose text the client gave, not read
        // back yet: read back from that text here, its read-back point,
        // `Run::seed_written`)
        self.seed_written(&key)?;
        // (a document's `\input`: a file the client cut, its text not
        // read, `Run::read_cut`; else its text)
        let cut = if self.doc { self.read_cut(&key)? } else { None };
        if let Some(x) = cut {
            let path = crate::val::list_of(self.v, x)[0];
            self.log("open", &str_of(self.v, path))?;
            self.push_toks([Tok::cs("\u{10FFFE}eof")]);
            let f = crate::run::FileRest::cut(self.v, self.v.str(&key), path);
            self.input.push_file(f);
            return Ok(());
        }
        let x = self.read_file_lazy(&key)?;
        let x = match x {
            Some(x) => Some(x),
            None => {
                if key == name {
                    None
                } else {
                    self.read_file_lazy(&name)?
                }
            }
        };
        let Some(x) = x else {
            // §530: in the nonstop modes (a build's, [`Run::fatal`]) the
            // job ends
            self.log("error", "I can't find file")?;
            return self.fatal("*** (job aborted, file error in nonstop mode)");
        };
        let xs = crate::val::list_of(self.v, x);
        if xs.len() < 2 {
            self.log("error", "I can't find file")?;
            return self.fatal("*** (job aborted, file error in nonstop mode)");
        }
        // (a job-written file whose read-back only defines: its
        // definitions from the per-field loop-backs; the text, when this
        // run wrote it, is read as TeX reads it, `\enddocument`'s `.aux`
        // with its `\@writefile` lines and the errors its end gives where
        // it is read)
        self.read_back(&key)?;
        let text = xs[1];
        let path = str_of(self.v, xs[0]);
        self.log("open", &path)?;
        self.push_toks([Tok::cs("\u{10FFFE}eof")]);
        // (a file the job writes: its units, read where a run reads its
        // lines, a piece each where it slices, `Run::doc_file_regions`)
        let f = match xs.get(2) {
            Some(&order) if text == phitex_defuse::ValueId::UNIT => {
                // (a file the job writes read back as its text: its lines
                // are read under the state here too, [`Run::pre_value`])
                self.mark_read_point(&key)?;
                self.note_read_back(&key)?;
                crate::run::FileRest::written(self.v, order, xs[0])
            }
            // (a tool's output, `crate::tools::ToolRun`: read inline, what
            // the tool said reported here, where its output is read)
            Some(&m) if self.v.as_int(m).is_some() => {
                if let Some(&said) = xs.get(3) {
                    for s in crate::val::list_of(self.v, said) {
                        let e = crate::val::list_of(self.v, s);
                        // (a warning either way: the tool's errors are
                        // not TeX's, and pdflatex's run does not fail on
                        // them)
                        if let [_, msg] = e[..] {
                            self.diag(false, &str_of(self.v, msg));
                        }
                    }
                }
                crate::run::FileRest::inline(self.v, text, xs[0])
            }
            _ => crate::run::FileRest::new(self.v, text, 0, xs[0]),
        };
        if crate::exec::obs_flag!("PHITEX_WFILE_TRACE") && f.units.is_some() {
            // (whether the run stops at the file, its units the region
            // children, `Run::has_input`, or reads it inline: and why)
            eprintln!(
                "wfile: {key} opened in node {}: units {}, sliceable {}, document run {}, own groups open {}, levels {:?}",
                self.cx.node().raw(),
                f.units.as_ref().map_or(0, |u| u.len()),
                f.sliceable,
                self.doc,
                self.nest.len(),
                self.input.levels.iter().map(|l| l.kind).collect::<Vec<_>>()
            );
        }
        self.input.push_file(f);
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
            self.get_x_token();
            self.ck()?;
        }
        if crate::exec::obs_flag!("PHITEX_CMD_TRACE") && self.cmd_traced() {
            eprintln!(
                "cmd-trace {} file name {name:?} ended by {:?} cmd {} chr {}",
                self.cx.node().raw(),
                self.cur.tok,
                self.cur.cmd,
                self.cur.chr
            );
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

/// The order of `a` against `b`: -1, 0 or 1.
fn order(a: i32, b: i32) -> i64 {
    match a.cmp(&b) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
