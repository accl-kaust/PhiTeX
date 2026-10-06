//! Part 28: Conditional processing (§487–§510).

use crate::host::Host;
use crate::input::ux;
use crate::scanner::NO_EXPAND_FLAG;
use crate::tex::{Jump, Tex};
use crate::toklists::CLOSED;
use crate::track::Tracker;
use crate::web::*;

/// §489: a condition stack entry: the state of the enclosing conditional.
#[derive(Clone, Copy, Debug, Hash)]
pub(crate) struct CondRecord {
    /// `if_limit`
    pub(crate) limit: i32,
    pub(crate) cur_if: i32,
    /// `if_line`
    pub(crate) line: i32,
}

partex_engine::persist_struct!(CondRecord {
    limit,
    cur_if,
    line
});

impl partex_ssa::Value for CondRecord {
    fn version(&self) -> partex_ssa::Version {
        partex_ssa::Version::of(self)
    }
}

/// §489: the condition stack as a value (DESIGN 7.17.12, `cond_stack`): a
/// persistent stack (`partex-ssa`'s `PStack`), each record versioned when
/// pushed, so the stack's version is made at the push and a copy is O(1).
#[derive(Clone, Default)]
pub(crate) struct CondStack(partex_ssa::PStack<CondRecord>);

impl CondStack {
    pub(crate) fn len(&self) -> usize {
        self.0.len()
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub(crate) fn push(&mut self, r: CondRecord) {
        self.0 = self.0.push(r);
    }
    pub(crate) fn pop(&mut self) -> Option<CondRecord> {
        let (r, rest) = self.0.pop()?;
        self.0 = rest;
        Some(r)
    }
    /// The stack's version (O(1)).
    pub(crate) fn version(&self) -> partex_ssa::Version {
        self.0.version()
    }
    /// The records from the bottom up.
    pub(crate) fn to_vec(&self) -> alloc::vec::Vec<CondRecord> {
        let mut v: alloc::vec::Vec<CondRecord> = self.0.iter().copied().collect();
        v.reverse();
        v
    }
    pub(crate) fn from_vec(v: &[CondRecord]) -> Self {
        let mut s = CondStack::default();
        for &r in v {
            s.push(r);
        }
        s
    }
    /// The records from the bottom up.
    pub(crate) fn iter(&self) -> impl Iterator<Item = CondRecord> {
        self.to_vec().into_iter()
    }
    /// Record `i` from the bottom.
    pub(crate) fn get(&self, i: usize) -> Option<CondRecord> {
        let n = self.len();
        if i >= n {
            return None;
        }
        self.0.iter().nth(n - 1 - i).copied()
    }
    /// Record `i` from the bottom with its limit changed: the records above
    /// it pushed again (a value is not changed in place).
    pub(crate) fn set_limit(&mut self, i: usize, limit: i32) -> bool {
        let mut above = alloc::vec::Vec::new();
        while self.len() > i + 1 {
            above.push(self.pop().expect("a record"));
        }
        let Some(mut r) = self.pop() else {
            return false;
        };
        r.limit = limit;
        self.push(r);
        while let Some(a) = above.pop() {
            self.push(a);
        }
        true
    }
}

impl core::ops::Index<usize> for CondStack {
    type Output = CondRecord;
    fn index(&self, i: usize) -> &CondRecord {
        let n = self.len();
        self.0.iter().nth(n - 1 - i).expect("a condition record")
    }
}

impl core::fmt::Debug for CondStack {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.to_vec()).finish()
    }
}

impl PartialEq for CondStack {
    fn eq(&self, o: &Self) -> bool {
        self.version() == o.version()
    }
}

impl core::hash::Hash for CondStack {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        self.version().hash(h);
    }
}

impl partex_engine::persist::Persist for CondStack {
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.to_vec().save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let v: alloc::vec::Vec<CondRecord> = partex_engine::persist::Persist::load(l)?;
        Some(CondStack::from_vec(&v))
    }
}

impl<H: Host, T: Tracker> Tex<H, T> {
    /// §494: skip text until `\fi`, `\else` or `\or` at level zero.
    pub(crate) fn pass_text(&mut self) -> Result<(), Jump> {
        let save_scanner_status = self.scanner_status;
        self.scanner_status = SKIPPING;
        let mut l = 0;
        self.skip_line = self.line;
        if self.skip_remembered() {
            self.scanner_status = save_scanner_status;
            if self.int_par(TRACING_IFS_CODE) > 0 {
                self.show_cur_cmd_chr();
            }
            return Ok(());
        }
        // (a skipped token's meaning matters by its class alone)
        self.tokens_only(|t| {
            loop {
                t.bulk_skip(); // (a run at once, `bulk.rs`)
                t.get_next()?;
                if t.cur_cmd == FI_OR_ELSE {
                    if l == 0 {
                        break;
                    }
                    if t.cur_chr == FI_CODE {
                        l -= 1;
                    }
                } else if t.cur_cmd == IF_TEST {
                    l += 1;
                }
            }
            Ok(())
        })?;
        self.scanner_status = save_scanner_status;
        if self.int_par(TRACING_IFS_CODE) > 0 {
            self.show_cur_cmd_chr();
        }
        Ok(())
    }

    /// `pass_text` from a macro body, remembered (`skipcache.rs`): true
    /// if the skip was done, false if it must run as tex.web's (nothing
    /// changed then).
    fn skip_remembered(&mut self) -> bool {
        // (with the SSA build's tracker, a skip remembered makes the
        // lookups the skip makes token by token: what `bulk_skip_run`
        // and `get_next` read when the skip runs in bulk, which it does
        // where `bulk_ready` holds; the lookups it repeats are no reads)
        let replay = T::CLASSES && self.tracker.ssa().is_some();
        if (T::READS && !self.skip_tracked && !replay)
            || (replay && !self.bulk_ready())
            || !self.params.skip_cache
            || self.memo.recording()
            || self.cur_input.state != TOKEN_LIST
            || self.cur_input.loc == NULL
            || self.token_type() != MACRO
        {
            return false;
        }
        let loc = self.cur_input.loc;
        // (looked up by the list in place; a miss holds it in the cache)
        let known = match self.cur_input.list.as_ref() {
            Some(l) => self.skip.get(l, loc),
            None => return false,
        };
        let skip = if let Some(s) = known {
            s
        } else {
            let Some(list) = self.cur_input.list.clone() else {
                return false;
            };
            let found = if replay {
                // (the meanings as they are, untracked: the replay below
                // reads them)
                let mut reads = alloc::vec::Vec::new();
                let mut seen = alloc::collections::BTreeSet::new();
                crate::skipcache::scan(&list, loc, |cs| {
                    if seen.insert(cs) {
                        reads.push(cs);
                    }
                    let w = self.peek_eqtb(cs);
                    (w.b0(), w.rh())
                })
                .map(|s| crate::skipcache::Skip {
                    reads: Some(reads.into()),
                    ..s
                })
            } else {
                crate::skipcache::scan(&list, loc, |cs| {
                    let w = self.eqtb(cs);
                    (w.b0(), w.rh())
                })
            };
            self.skip.put(&list, loc, found.clone());
            found
        };
        let Some(skip) = skip else {
            return false;
        };
        // (with a machine's tracker: the skip read every class)
        self.classes_read = true;
        let align = self.align_state();
        if let Some(reads) = &skip.reads {
            for &cs in reads.iter() {
                self.token_meaning(cs);
            }
        }
        self.cur_input.loc = skip.end;
        self.set_align_state(align + skip.align);
        self.cur_cs = skip.cs;
        self.cur_cmd = skip.cmd;
        self.cur_chr = skip.chr;
        true
    }

    /// The conditionals' state as a value (DESIGN 7.17.12): the stack of
    /// open conditionals with `if_limit`, `cur_if` and `if_line`, versioned
    /// by its contents at each write ([`Self::cond_wrote`]).
    pub(crate) fn cond_version(&self) -> u128 {
        // (the stack carries its version: combined, O(1))
        partex_ssa::Version::of(&(
            self.cond_stack.version(),
            self.if_limit,
            self.cur_if,
            self.if_line,
        ))
        .0
    }

    /// A read of the conditionals' state, for a tracker that keeps
    /// versions.
    #[inline]
    pub(crate) fn cond_read(&self) {
        if T::VALUES {
            self.tracker
                .value_read(crate::track::Row::Cond, || self.cond_version());
        }
    }

    /// The conditionals' state was written: its version, made now.
    #[inline]
    pub(crate) fn cond_wrote(&self) {
        if T::VALUES {
            self.tracker.value_wrote(crate::track::Row::Cond);
        }
    }

    /// §495: push the condition stack.
    fn push_cond(&mut self) {
        self.cond_read();
        self.cond_stack.push(CondRecord {
            limit: self.if_limit,
            cur_if: self.cur_if,
            line: self.if_line,
        });
        self.cur_if = self.cur_chr;
        self.if_limit = IF_CODE;
        self.if_line = self.line;
        self.cond_wrote();
    }

    /// §496: pop the condition stack.
    pub(crate) fn pop_cond(&mut self) {
        self.cond_read();
        if self.if_stack[self.in_open] == self.cond_stack.len() {
            // e-TeX: conditionals possibly not properly nested with files
            self.if_warning();
        }
        let r = self.cond_stack.pop().expect("condition stack");
        self.memo.cond_closed(self.cond_stack.len());
        self.if_line = r.line;
        self.cur_if = r.cur_if;
        self.if_limit = r.limit;
        self.cond_wrote();
    }

    /// §497: `p` is the depth of the condition stack when the conditional
    /// began (tex.web's `save_cond_ptr`).
    fn change_if_limit(&mut self, l: i32, p: usize) -> Result<(), Jump> {
        self.cond_read();
        if p == self.cond_stack.len() {
            self.if_limit = l; // that's the easy case
        } else {
            // The record pushed on top of level `p` holds its limit.
            if !self.cond_stack.set_limit(p, l) {
                return self.confusion(b"if");
            }
        }
        self.cond_wrote();
        Ok(())
    }

    /// §506: `get_x_token_or_active_char`.
    fn get_x_token_or_active_char(&mut self) -> Result<(), Jump> {
        self.get_x_token()?;
        if self.cur_cmd == RELAX && self.cur_chr == NO_EXPAND_FLAG {
            self.cur_cmd = ACTIVE_CHAR;
            self.cur_chr = match crate::wide::active_char(self.cur_tok - CS_TOKEN_FLAG) {
                Some(c) => c,
                // (`XeTeX`'s `cur_tok-cs_token_flag-active_base` of another
                // control sequence is past every scalar value)
                None if self.unicode => TOO_BIG_USV,
                None => self.cur_tok - CS_TOKEN_FLAG - ACTIVE_BASE,
            };
        }
        Ok(())
    }

    /// §498: begin a conditional.
    pub(crate) fn conditional(&mut self) -> Result<(), Jump> {
        if self.int_par(TRACING_IFS_CODE) > 0 && self.int_par(TRACING_COMMANDS_CODE) <= 1 {
            self.show_cur_cmd_chr();
        }
        self.push_cond();
        let save_cond_ptr = self.cond_stack.len();
        let is_unless = self.cur_chr >= UNLESS_CODE; // e-TeX's `\unless`
        let this_if = self.cur_chr % UNLESS_CODE;
        // §501: either process \ifcase or set `b` to the value of a boolean
        // condition.
        let b = match this_if {
            IF_CHAR_CODE | IF_CAT_CODE => {
                // §506: test if two characters match (`XeTeX` §541: any
                // scalar value is one).
                let biggest = if self.unicode { BIGGEST_USV } else { 255 };
                self.get_x_token_or_active_char()?;
                let (m, n) = if self.cur_cmd > ACTIVE_CHAR || self.cur_chr > biggest {
                    (RELAX, TOO_BIG_USV) // not a character
                } else {
                    (self.cur_cmd, self.cur_chr)
                };
                self.get_x_token_or_active_char()?;
                if self.cur_cmd > ACTIVE_CHAR || self.cur_chr > biggest {
                    self.cur_cmd = RELAX;
                    self.cur_chr = TOO_BIG_USV;
                }
                if this_if == IF_CHAR_CODE {
                    n == self.cur_chr
                } else {
                    m == self.cur_cmd
                }
            }
            IF_INT_CODE | IF_DIM_CODE => {
                // §503: test relation between integers or dimensions.
                let scan = |t: &mut Self| {
                    if this_if == IF_INT_CODE {
                        t.scan_int()
                    } else {
                        t.scan_normal_dimen()
                    }
                };
                scan(self)?;
                let n = self.cur_val;
                self.get_nonblank_noncall()?;
                let r = if self.cur_tok >= OTHER_TOKEN + i32::from(b'<')
                    && self.cur_tok <= OTHER_TOKEN + i32::from(b'>')
                {
                    self.cur_tok - OTHER_TOKEN
                } else {
                    self.print_err(b"Missing = inserted for ");
                    self.print_cmd_chr(IF_TEST, this_if);
                    self.help(&[b"I was expecting to see `<', `=', or `>'. Didn't."]);
                    self.back_error()?;
                    i32::from(b'=')
                };
                scan(self)?;
                match u8::try_from(r).unwrap_or(b'=') {
                    b'<' => n < self.cur_val,
                    b'=' => n == self.cur_val,
                    _ => n > self.cur_val,
                }
            }
            IF_ODD_CODE => {
                // §504: test if an integer is odd.
                self.scan_int()?;
                self.cur_val % 2 != 0
            }
            IF_VMODE_CODE => self.mode().abs() == VMODE,
            IF_HMODE_CODE => self.mode().abs() == HMODE,
            IF_MMODE_CODE => self.mode().abs() == MMODE,
            IF_INNER_CODE => self.mode() < 0,
            IF_VOID_CODE | IF_HBOX_CODE | IF_VBOX_CODE => {
                // §505: test box register status.
                self.scan_register_num()?;
                match self.box_reg(self.cur_val) {
                    None => this_if == IF_VOID_CODE,
                    Some(b) if this_if == IF_HBOX_CODE => !b.vertical,
                    Some(b) => this_if == IF_VBOX_CODE && b.vertical,
                }
            }
            IFX_CODE => {
                // §507: test if two tokens match.
                let save_scanner_status = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_next()?;
                let n = self.cur_cs;
                let p = self.cur_cmd;
                let q = self.cur_chr;
                self.get_next()?;
                let b = if self.cur_cmd != p {
                    false
                } else if self.cur_cmd < CALL {
                    self.cur_chr == q
                } else {
                    // §508: test if two macro texts match.
                    // (the lists by content: the same list, or equal
                    // tokens and `\protected` flags)
                    let (p, q) = (
                        self.equiv_toks(self.cur_cs).cloned(),
                        self.equiv_toks(n).cloned(),
                    );
                    p == q
                };
                self.scanner_status = save_scanner_status;
                b
            }
            IF_EOF_CODE => {
                self.scan_four_bit_int_or_18()?;
                if self.cur_val == 18 {
                    !self.params.shell_escape
                } else {
                    if !T::VALUES {
                        // (the machine's cell; with values, `read_open`'s
                        // slot is what `\ifeof` reads)
                        self.tracker.read(crate::track::Cell::Read(self.cur_val));
                    }
                    self.read_open(ux(self.cur_val)) == CLOSED
                }
            }
            IF_TRUE_CODE => true,
            IF_FALSE_CODE => false,
            IF_DEF_CODE => {
                // e-TeX: `\ifdefined` (`\outer` control sequences are fine)
                let save_scanner_status = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_next()?;
                self.scanner_status = save_scanner_status;
                self.cur_cmd != UNDEFINED_CS
            }
            IF_CS_CODE => {
                // e-TeX: `\ifcsname` (no new name enters the hash table)
                let e = self.is_in_csname;
                self.is_in_csname = true;
                let name = self.scan_cs_name(false)?;
                self.look_up_cs_name(&name, false)?;
                let b = self.eq_type(self.cur_cs) != UNDEFINED_CS;
                self.is_in_csname = e;
                b
            }
            IF_IN_CSNAME_CODE => self.is_in_csname, // pdfTeX
            IF_PDFPRIMITIVE_CODE => {
                // pdfTeX §527: is the next token's meaning its primitive
                // meaning?
                let save_scanner_status = self.scanner_status;
                self.scanner_status = NORMAL;
                self.get_next()?;
                self.scanner_status = save_scanner_status;
                let m = self.prim_of_cur_cs()?;
                self.cur_cmd != UNDEFINED_CS
                    && m != UNDEFINED_PRIMITIVE
                    && (self.cur_cmd, self.cur_chr) == self.prim_meaning(m)
            }
            IF_PDFABS_NUM_CODE | IF_PDFABS_DIM_CODE => {
                // pdfTeX: compare absolute values
                let scan = |t: &mut Self| {
                    if this_if == IF_PDFABS_NUM_CODE {
                        t.scan_int()
                    } else {
                        t.scan_normal_dimen()
                    }
                };
                scan(self)?;
                let n = self.cur_val.wrapping_abs();
                self.get_nonblank_noncall()?;
                let r = if self.cur_tok >= OTHER_TOKEN + i32::from(b'<')
                    && self.cur_tok <= OTHER_TOKEN + i32::from(b'>')
                {
                    self.cur_tok - OTHER_TOKEN
                } else {
                    self.print_err(b"Missing = inserted for ");
                    self.print_cmd_chr(IF_TEST, this_if);
                    self.help(&[b"I was expecting to see `<', `=', or `>'. Didn't."]);
                    self.back_error()?;
                    i32::from(b'=')
                };
                scan(self)?;
                let v = self.cur_val.wrapping_abs();
                match u8::try_from(r).unwrap_or(b'=') {
                    b'<' => n < v,
                    b'=' => n == v,
                    _ => n > v,
                }
            }
            IF_FONT_CHAR_CODE => {
                // e-TeX: `\iffontchar`
                self.scan_font_ident()?;
                let f = self.cur_val;
                if self.params.flavor == crate::params::Flavor::XeTeX {
                    self.scan_usv_num()?;
                } else {
                    self.scan_char_num()?;
                }
                if let Some(nf) = self.native_font(f) {
                    // `XeTeX`: whether the font maps it to a glyph
                    nf.font.map_char_to_glyph(self.cur_val) > 0
                } else {
                    self.font_read(f, crate::track::font::METRICS);
                    self.fonts.get(f).glyph(self.cur_val).is_some()
                }
            }
            _ => {
                // §509: select the appropriate case and return or `goto
                // common_ending`.
                self.scan_int()?;
                let mut n = self.cur_val; // `n` is the number of cases to pass
                if self.int_par(TRACING_COMMANDS_CODE) > 1 {
                    self.begin_diagnostic();
                    self.print_str(b"{case ");
                    self.print_int(n);
                    self.print_char(b'}');
                    self.end_diagnostic(false);
                }
                while n != 0 {
                    self.pass_text()?;
                    if self.cond_stack.len() == save_cond_ptr {
                        if self.cur_chr == OR_CODE {
                            n -= 1;
                        } else {
                            self.common_ending();
                            return Ok(());
                        }
                    } else if self.cur_chr == FI_CODE {
                        self.pop_cond();
                    }
                }
                // wait for \or, \else, or \fi
                return self.change_if_limit(OR_CODE, save_cond_ptr);
            }
        };
        let b = b != is_unless;
        if self.int_par(TRACING_COMMANDS_CODE) > 1 {
            // §502: display the value of `b`.
            self.begin_diagnostic();
            self.print_str(if b { b"{true}" } else { b"{false}" });
            self.end_diagnostic(false);
        }
        if b {
            // wait for \else or \fi
            return self.change_if_limit(ELSE_CODE, save_cond_ptr);
        }
        // §500: skip to \else or \fi, then `goto common_ending`.
        loop {
            self.pass_text()?;
            if self.cond_stack.len() == save_cond_ptr {
                if self.cur_chr != OR_CODE {
                    break;
                }
                self.print_err(b"Extra ");
                self.print_esc(b"or");
                self.help(&[b"I'm ignoring this; it doesn't match any \\if."]);
                self.error()?;
            } else if self.cur_chr == FI_CODE {
                self.pop_cond();
            }
        }
        self.common_ending();
        Ok(())
    }

    /// §498: `common_ending`.
    fn common_ending(&mut self) {
        if self.cur_chr == FI_CODE {
            self.pop_cond();
        } else {
            self.if_limit = FI_CODE; // wait for \fi
            self.cond_wrote();
        }
    }

    /// §510: terminate the current conditional and skip to `\fi`.
    pub(crate) fn terminate_conditional(&mut self) -> Result<(), Jump> {
        if self.int_par(TRACING_IFS_CODE) > 0 && self.int_par(TRACING_COMMANDS_CODE) <= 1 {
            self.show_cur_cmd_chr();
        }
        self.cond_read();
        if self.cur_chr > self.if_limit {
            if self.if_limit == IF_CODE {
                self.insert_relax()?; // condition not yet evaluated
            } else {
                self.print_err(b"Extra ");
                self.print_cmd_chr(FI_OR_ELSE, self.cur_chr);
                self.help(&[b"I'm ignoring this; it doesn't match any \\if."]);
                self.error()?;
            }
        } else {
            while self.cur_chr != FI_CODE {
                self.pass_text()?; // skip to \fi
            }
            self.pop_cond();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::mem::NULL;
    use crate::testing::{engine, feed, term_output};
    use crate::web::*;

    /// Oracle: `\tracingcommands=2 \tracingonline=1 \message{...}` with
    /// the conditionals below, in INITEX.
    #[test]
    fn conditionals_match_tex() {
        let mut t = engine();
        t.init_prim().unwrap();
        for (c, cat) in [(b'{', LEFT_BRACE), (b'}', RIGHT_BRACE)] {
            t.set_equiv(CAT_CODE_BASE + i32::from(c), cat);
        }
        t.set_int_par(TRACING_COMMANDS_CODE, 2);
        t.set_int_par(TRACING_ONLINE_CODE, 1);
        t.shown_mode = t.mode();
        feed(
            &mut t,
            b"\\ifnum1<2 a\\else b\\fi \\ifx\\relax\\relax c\\fi \\ifcase 2 x\\or y\\or z\\else w\\fi \
              \\if aa\\ifcat 1a no\\else yes\\fi\\fi \\ifodd-3 odd\\fi \\ifdim1pt=65536sp eq\\fi \
              \\iftrue\\iffalse\\else nested\\fi\\fi \\ifcase 5 q\\or r\\else s\\fi}\n",
        );
        let out = term_output(&mut t, |t| {
            let mut head = alloc::vec::Vec::new();
            loop {
                t.get_x_token().unwrap();
                if t.cur_cmd == RIGHT_BRACE {
                    break;
                }
                if t.cur_cmd != SPACER {
                    head.push(t.cur_tok);
                }
            }
            t.show_token_list(&head, NULL, 1000);
        });
        let trace = [
            "{\\ifnum}",
            "{true}",
            "{\\else}",
            "{\\ifx}",
            "{true}",
            "{\\fi}",
            "{\\ifcase}",
            "{case 2}",
            "{\\else}",
            "{\\if}",
            "{true}",
            "{\\ifcat}",
            "{false}",
            "{\\fi}",
            "{\\fi}",
            "{\\ifodd}",
            "{true}",
            "{\\fi}",
            "{\\ifdim}",
            "{true}",
            "{\\fi}",
            "{\\iftrue}",
            "{true}",
            "{\\iffalse}",
            "{false}",
            "{\\fi}",
            "{\\fi}",
            "{\\ifcase}",
            "{case 5}",
            "{\\fi}",
            "aczyesoddeqnesteds",
        ];
        assert_eq!(core::str::from_utf8(&out).unwrap(), trace.join("\n"));
        assert!(t.cond_stack.is_empty());
    }
}
